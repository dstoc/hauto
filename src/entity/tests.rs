use super::*;
use crate::test_support::{run_async, sample_state};

#[test]
fn entity_id_accepts_basic_home_assistant_shape() {
    let id = EntityId::new("binary_sensor.office_occupancy").unwrap();
    assert_eq!(id.domain(), "binary_sensor");
    assert_eq!(id.object_id(), "office_occupancy");
}

#[test]
fn entity_id_rejects_invalid_syntax() {
    for value in [
        "",
        "light",
        ".office",
        "light.",
        "Light.office",
        "light.office-1",
        "light.office.extra",
    ] {
        assert!(EntityId::new(value).is_err(), "{value} should be invalid");
    }
}

#[test]
fn typed_handles_validate_domain() {
    assert!(Light::new("light.office").is_ok());
    assert!(BinarySensor::new("binary_sensor.office_occupancy").is_ok());
    assert!(Switch::new("switch.fan").is_ok());
    assert!(Sensor::<f64>::new("sensor.temperature").is_ok());
    assert!(Light::new("switch.office").is_err());
}

#[test]
fn entity_handle_state_reads_from_cache() {
    run_async(async {
        let light = Light::new("light.office").unwrap();
        let state = sample_state("light.office", "off");
        let ctx = Context::with_seeded_states([state.clone()]);

        assert_eq!(light.state(&ctx).await.unwrap(), state);
    });
}

#[test]
fn numeric_sensor_read_decodes_hit_miss_and_invalid_state_from_cache() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let missing = Sensor::<f64>::new("sensor.missing").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "21.5")]);
        let cache = StateCache::new(&ctx.home_assistant.generation);

        assert_eq!(sensor.read(&cache).unwrap(), Some(21.5));
        assert_eq!(missing.read(&cache).unwrap(), None);

        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "unknown"))
            .unwrap();
        let cache = StateCache::new(&ctx.home_assistant.generation);
        assert!(matches!(
            sensor.read(&cache),
            Err(Error::InvalidState { entity_id, .. }) if entity_id == *sensor.entity_id()
        ));
    });
}

#[test]
fn typed_entity_get_fetches_current_state_and_decodes_it() {
    run_async(async {
        let light = Light::new("light.office").unwrap();
        let temperature = Sensor::<f64>::new("sensor.temperature").unwrap();
        let unavailable_temperature =
            Sensor::<SensorValue<f64>>::new("sensor.unavailable_temperature").unwrap();
        let missing = Sensor::<SensorValue<f64>>::new("sensor.missing").unwrap();
        let ctx = Context::with_seeded_states([
            sample_state("light.office", "on"),
            sample_state("sensor.temperature", "21.5"),
            sample_state("sensor.unavailable_temperature", "unavailable"),
        ]);

        assert_eq!(light.get(&ctx).await.unwrap(), BinaryState::On);
        assert_eq!(temperature.get(&ctx).await.unwrap(), 21.5);
        assert_eq!(
            unavailable_temperature.get(&ctx).await.unwrap(),
            SensorValue::Unavailable
        );
        assert!(matches!(
            missing.get(&ctx).await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == *missing.entity_id()
        ));
    });
}

#[test]
fn typed_entity_next_change_waits_for_change_and_decodes_new_state() {
    run_async(async {
        let light = Light::new("light.office").unwrap();
        let temperature = Sensor::<SensorValue<f64>>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([
            sample_state("light.office", "off"),
            sample_state("sensor.temperature", "20.0"),
        ]);

        let light_ctx = ctx.clone();
        let light_waiter = light.clone();
        let light_change = ctx.spawn(async move { light_waiter.next_change(&light_ctx).await });
        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("light.office", "on"))
            .unwrap();
        assert_eq!(light_change.await.unwrap(), BinaryState::On);

        let temperature_ctx = ctx.clone();
        let temperature_waiter = temperature.clone();
        let temperature_change =
            ctx.spawn(async move { temperature_waiter.next_change(&temperature_ctx).await });
        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "unavailable"))
            .unwrap();
        assert_eq!(temperature_change.await.unwrap(), SensorValue::Unavailable);
    });
}

#[test]
fn typed_entity_next_change_reports_deleted_entity_and_cancellation() {
    run_async(async {
        let sensor = Sensor::<SensorValue<f64>>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "20.0")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let waiter = ctx.spawn(async move { waiter_sensor.next_change(&waiter_ctx).await });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .remove_cached_state(sensor.entity_id())
            .unwrap();
        assert!(matches!(
            waiter.await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == *sensor.entity_id()
        ));

        let cancelled_ctx =
            Context::with_seeded_states([sample_state("sensor.temperature", "20.0")]);
        let cancelled_waiter_ctx = cancelled_ctx.clone();
        let cancelled_sensor = sensor.clone();
        let cancelled_waiter = cancelled_ctx
            .spawn(async move { cancelled_sensor.next_change(&cancelled_waiter_ctx).await });
        tokio::task::yield_now().await;
        cancelled_ctx.cancel_generation();
        assert!(matches!(cancelled_waiter.await, Err(Error::Cancelled)));
    });
}

#[test]
fn sensor_value_numeric_sensor_read_decodes_values_sentinels_miss_and_invalid_state() {
    run_async(async {
        let sensor = Sensor::<SensorValue<f64>>::new("sensor.temperature").unwrap();
        let missing = Sensor::<SensorValue<f64>>::new("sensor.missing").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "21.5")]);
        let cache = StateCache::new(&ctx.home_assistant.generation);

        assert_eq!(sensor.read(&cache).unwrap(), Some(SensorValue::Value(21.5)));
        assert_eq!(missing.read(&cache).unwrap(), None);

        for (raw, expected) in [
            ("unknown", SensorValue::Unknown),
            ("unavailable", SensorValue::Unavailable),
            ("", SensorValue::Unknown),
        ] {
            ctx.home_assistant()
                .cache_state(sample_state("sensor.temperature", raw))
                .unwrap();
            let cache = StateCache::new(&ctx.home_assistant.generation);
            assert_eq!(sensor.read(&cache).unwrap(), Some(expected));
        }

        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "not-a-number"))
            .unwrap();
        let cache = StateCache::new(&ctx.home_assistant.generation);
        assert!(matches!(
            sensor.read(&cache),
            Err(Error::InvalidState { entity_id, .. }) if entity_id == *sensor.entity_id()
        ));
    });
}
