use crate::{
    BinarySensor, BinaryState, Context, Error, HoldResult, Light, Sensor, SensorValue, Switch,
    TimeoutResult, WaitResult,
    test_support::{run_async, sample_state, wait_for_predicate_evaluations},
};
use std::{
    future::IntoFuture,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[test]
fn binary_sensor_wait_satisfies_immediately() {
    run_async(async {
        let sensor = BinarySensor::new("binary_sensor.door").unwrap();
        let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "on")]);

        sensor.wait_until_on(&ctx).await.unwrap();
    });
}

#[test]
fn binary_sensor_wait_require_transition_leaves_and_reenters_target() {
    run_async(async {
        let sensor = BinarySensor::new("binary_sensor.door").unwrap();
        let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "on")]);

        assert_eq!(
            ctx.timeout(
                Duration::from_millis(5),
                sensor
                    .wait_until_on(&ctx)
                    .require_transition()
                    .into_future(),
            )
            .await
            .unwrap(),
            TimeoutResult::TimedOut
        );

        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let waiter = ctx.spawn(async move {
            waiter_sensor
                .wait_until_on(&waiter_ctx)
                .require_transition()
                .await
        });
        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "off"))
            .unwrap();
        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "on"))
            .unwrap();
        waiter.await.unwrap();
    });
}

#[test]
fn binary_sensor_wait_returns_entity_not_found_when_deleted() {
    run_async(async {
        let sensor = BinarySensor::new("binary_sensor.door").unwrap();
        let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let waiter = ctx.spawn(async move { waiter_sensor.wait_until_on(&waiter_ctx).await });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .remove_cached_state(sensor.entity_id())
            .unwrap();

        assert!(matches!(
            waiter.await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == *sensor.entity_id()
        ));

        let hold_sensor = BinarySensor::new("binary_sensor.window").unwrap();
        let hold_ctx = Context::with_seeded_states([sample_state("binary_sensor.window", "off")]);
        let hold_waiter_ctx = hold_ctx.clone();
        let hold_waiter_sensor = hold_sensor.clone();
        let hold_waiter = hold_ctx.spawn(async move {
            hold_waiter_sensor
                .wait_until_on(&hold_waiter_ctx)
                .for_at_least(Duration::from_millis(50))
                .await
        });

        hold_ctx
            .home_assistant()
            .cache_state(sample_state("binary_sensor.window", "on"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
        hold_ctx
            .home_assistant()
            .remove_cached_state(hold_sensor.entity_id())
            .unwrap();

        assert!(matches!(
            hold_waiter.await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == *hold_sensor.entity_id()
        ));
    });
}

#[test]
fn binary_sensor_wait_for_at_least_resets_on_other_state() {
    run_async(async {
        let sensor = BinarySensor::new("binary_sensor.door").unwrap();
        let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let mut waiter = ctx.spawn(async move {
            waiter_sensor
                .wait_until_on(&waiter_ctx)
                .for_at_least(Duration::from_millis(20))
                .await
        });

        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "on"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "off"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), &mut waiter)
                .await
                .unwrap(),
            TimeoutResult::TimedOut
        );
        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "on"))
            .unwrap();
        waiter.await.unwrap();
    });
}

#[test]
fn binary_sensor_wait_within_times_out() {
    run_async(async {
        let sensor = BinarySensor::new("binary_sensor.door").unwrap();
        let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);

        assert_eq!(
            sensor
                .wait_until_on(&ctx)
                .within(Duration::from_millis(1))
                .await
                .unwrap(),
            WaitResult::TimedOut
        );
    });
}

#[test]
fn binary_sensor_expectation_not_satisfied_interrupted_held_and_deleted() {
    run_async(async {
        let sensor = BinarySensor::new("binary_sensor.door").unwrap();
        let ctx = Context::with_seeded_states([sample_state("binary_sensor.door", "off")]);

        assert_eq!(
            sensor.expect_on(&ctx).await.unwrap(),
            HoldResult::NotSatisfied {
                actual: BinaryState::Off
            }
        );

        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "on"))
            .unwrap();
        assert_eq!(
            sensor
                .expect_on(&ctx)
                .for_at_least(Duration::from_millis(1))
                .await
                .unwrap(),
            HoldResult::Held
        );

        let interrupted_ctx = ctx.clone();
        let interrupted_sensor = sensor.clone();
        let interrupted = ctx.spawn(async move {
            interrupted_sensor
                .expect_on(&interrupted_ctx)
                .for_at_least(Duration::from_millis(50))
                .await
        });
        tokio::time::sleep(Duration::from_millis(1)).await;
        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "off"))
            .unwrap();
        assert_eq!(
            interrupted.await.unwrap(),
            HoldResult::Interrupted {
                actual: BinaryState::Off
            }
        );

        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.door", "on"))
            .unwrap();
        let deleted_ctx = ctx.clone();
        let deleted_sensor = sensor.clone();
        let deleted = ctx.spawn(async move {
            deleted_sensor
                .expect_on(&deleted_ctx)
                .for_at_least(Duration::from_millis(50))
                .await
        });
        tokio::time::sleep(Duration::from_millis(1)).await;
        ctx.home_assistant()
            .remove_cached_state(sensor.entity_id())
            .unwrap();
        assert!(matches!(
            deleted.await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == *sensor.entity_id()
        ));
    });
}

#[test]
fn light_wait_until_on_completes_from_cached_on_state() {
    run_async(async {
        let light = Light::new("light.office").unwrap();
        let ctx = Context::with_seeded_states([sample_state("light.office", "on")]);

        light.wait_until_on(&ctx).await.unwrap();
    });
}

#[test]
fn switch_wait_until_off_completes_after_matching_state_change() {
    run_async(async {
        let switch = Switch::new("switch.fan").unwrap();
        let ctx = Context::with_seeded_states([sample_state("switch.fan", "on")]);
        let waiter_ctx = ctx.clone();
        let waiter_switch = switch.clone();
        let waiter = ctx.spawn(async move { waiter_switch.wait_until_off(&waiter_ctx).await });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("switch.fan", "off"))
            .unwrap();

        waiter.await.unwrap();
    });
}

#[test]
fn light_expect_on_for_at_least_returns_held_when_state_stays_on() {
    run_async(async {
        let light = Light::new("light.office").unwrap();
        let ctx = Context::with_seeded_states([sample_state("light.office", "on")]);

        assert_eq!(
            light
                .expect_on(&ctx)
                .for_at_least(Duration::from_millis(1))
                .await
                .unwrap(),
            HoldResult::Held
        );
    });
}

#[test]
fn switch_expect_off_returns_not_satisfied_when_currently_on() {
    run_async(async {
        let switch = Switch::new("switch.fan").unwrap();
        let ctx = Context::with_seeded_states([sample_state("switch.fan", "on")]);

        assert_eq!(
            switch.expect_off(&ctx).await.unwrap(),
            HoldResult::NotSatisfied {
                actual: BinaryState::On
            }
        );
    });
}

#[test]
fn numeric_sensor_wait_until_matching_completes_after_state_change() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "29.5")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let waiter = ctx.spawn(async move {
            waiter_sensor
                .wait_until_matching(&waiter_ctx, |value| *value > 30.0)
                .await
        });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "30.5"))
            .unwrap();

        waiter.await.unwrap();
    });
}

#[test]
fn numeric_sensor_expect_matching_returns_held_from_cached_match() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "31.25")]);

        assert_eq!(
            sensor
                .expect_matching(&ctx, |value| *value > 30.0)
                .await
                .unwrap(),
            HoldResult::Held
        );
    });
}

#[test]
fn numeric_sensor_expect_matching_for_at_least_returns_interrupted_on_later_miss() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "31.0")]);
        let expectation_ctx = ctx.clone();
        let expectation_sensor = sensor.clone();
        let expectation = ctx.spawn(async move {
            expectation_sensor
                .expect_matching(&expectation_ctx, |value| *value > 30.0)
                .for_at_least(Duration::from_millis(50))
                .await
        });

        tokio::time::sleep(Duration::from_millis(1)).await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "29.0"))
            .unwrap();

        assert_eq!(
            expectation.await.unwrap(),
            HoldResult::Interrupted { actual: 29.0 }
        );
    });
}

#[test]
fn numeric_sensor_predicate_require_transition_requires_true_false_true() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "31.0")]);

        assert_eq!(
            sensor
                .wait_until_matching(&ctx, |value| *value > 30.0)
                .require_transition()
                .within(Duration::from_millis(1))
                .await
                .unwrap(),
            WaitResult::TimedOut
        );

        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let waiter = ctx.spawn(async move {
            waiter_sensor
                .wait_until_matching(&waiter_ctx, |value| *value > 30.0)
                .require_transition()
                .await
        });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "29.0"))
            .unwrap();
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "31.5"))
            .unwrap();

        waiter.await.unwrap();
    });
}

#[test]
fn numeric_sensor_predicate_for_at_least_resets_when_predicate_becomes_false() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "29.0")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let mut waiter = ctx.spawn(async move {
            waiter_sensor
                .wait_until_matching(&waiter_ctx, |value| *value > 30.0)
                .for_at_least(Duration::from_millis(20))
                .await
        });

        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "31.0"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "29.0"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), &mut waiter)
                .await
                .unwrap(),
            TimeoutResult::TimedOut
        );

        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "31.0"))
            .unwrap();

        waiter.await.unwrap();
    });
}

#[test]
fn numeric_sensor_predicate_within_returns_timed_out() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "29.0")]);

        assert_eq!(
            sensor
                .wait_until_matching(&ctx, |value| *value > 30.0)
                .within(Duration::from_millis(1))
                .await
                .unwrap(),
            WaitResult::TimedOut
        );
    });
}

#[test]
fn numeric_sensor_non_numeric_state_returns_invalid_state() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "unknown")]);

        assert!(matches!(
            sensor
                .expect_matching(&ctx, |value| *value > 30.0)
                .await,
            Err(Error::InvalidState { entity_id, .. }) if entity_id == *sensor.entity_id()
        ));

        assert!(matches!(
            sensor
                .wait_until_matching(&ctx, |value| *value > 30.0)
                .await,
            Err(Error::InvalidState { entity_id, .. }) if entity_id == *sensor.entity_id()
        ));
    });
}

#[test]
fn sensor_value_numeric_sensor_wait_until_matching_completes_after_sentinel_state_change() {
    run_async(async {
        let sensor = Sensor::<SensorValue<f64>>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "29.5")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let waiter = ctx.spawn(async move {
            waiter_sensor
                .wait_until_matching(&waiter_ctx, |value| matches!(value, SensorValue::Unknown))
                .await
        });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "unknown"))
            .unwrap();

        waiter.await.unwrap();
    });
}

#[test]
fn sensor_value_numeric_sensor_expect_matching_for_at_least_interrupts_on_sentinel_states() {
    run_async(async {
        for (raw, expected) in [
            ("unknown", SensorValue::Unknown),
            ("unavailable", SensorValue::Unavailable),
        ] {
            let sensor = Sensor::<SensorValue<f64>>::new("sensor.temperature").unwrap();
            let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "31.0")]);
            let expectation_ctx = ctx.clone();
            let expectation_sensor = sensor.clone();
            let expectation = ctx.spawn(async move {
                expectation_sensor
                    .expect_matching(&expectation_ctx, |value| {
                        value.as_value().is_some_and(|value| *value > 30.0)
                    })
                    .for_at_least(Duration::from_millis(50))
                    .await
            });

            tokio::time::sleep(Duration::from_millis(1)).await;
            ctx.home_assistant()
                .cache_state(sample_state("sensor.temperature", raw))
                .unwrap();

            assert_eq!(
                expectation.await.unwrap(),
                HoldResult::Interrupted { actual: expected }
            );
        }
    });
}

#[test]
fn global_state_wait_completes_from_initial_cache_state() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "31.0")]);

        ctx.wait_until_state(move |state| {
            Ok(sensor
                .read(state)?
                .is_some_and(|temperature| temperature > 30.0))
        })
        .await
        .unwrap();
    });
}

#[test]
fn global_state_wait_wakes_on_unrelated_change_and_finishes_when_entities_match() {
    run_async(async {
        let temperature = Sensor::<f64>::new("sensor.temperature").unwrap();
        let humidity = Sensor::<f64>::new("sensor.humidity").unwrap();
        let ctx = Context::with_seeded_states([
            sample_state("sensor.temperature", "19.0"),
            sample_state("sensor.humidity", "60.0"),
            sample_state("binary_sensor.window", "off"),
        ]);
        let evaluations = Arc::new(AtomicUsize::new(0));
        let waiter_ctx = ctx.clone();
        let waiter_temperature = temperature.clone();
        let waiter_humidity = humidity.clone();
        let waiter_evaluations = evaluations.clone();
        let mut waiter = ctx.spawn(async move {
            waiter_ctx
                .wait_until_state(move |state| {
                    waiter_evaluations.fetch_add(1, Ordering::AcqRel);
                    let temperature_matches = waiter_temperature
                        .read(state)?
                        .is_some_and(|temperature| temperature >= 20.0);
                    let humidity_matches = waiter_humidity
                        .read(state)?
                        .is_some_and(|humidity| humidity <= 50.0);
                    Ok(temperature_matches && humidity_matches)
                })
                .await
        });

        wait_for_predicate_evaluations(&evaluations, 1).await;
        ctx.home_assistant()
            .cache_state(sample_state("binary_sensor.window", "on"))
            .unwrap();
        wait_for_predicate_evaluations(&evaluations, 2).await;
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), &mut waiter)
                .await
                .unwrap(),
            TimeoutResult::TimedOut
        );

        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "21.0"))
            .unwrap();
        wait_for_predicate_evaluations(&evaluations, 3).await;
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), &mut waiter)
                .await
                .unwrap(),
            TimeoutResult::TimedOut
        );

        ctx.home_assistant()
            .cache_state(sample_state("sensor.humidity", "45.0"))
            .unwrap();
        waiter.await.unwrap();
    });
}

#[test]
fn global_state_wait_for_at_least_resets_when_predicate_becomes_false() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "29.0")]);
        let waiter_ctx = ctx.clone();
        let waiter_sensor = sensor.clone();
        let mut waiter = ctx.spawn(async move {
            waiter_ctx
                .wait_until_state(move |state| {
                    Ok(waiter_sensor
                        .read(state)?
                        .is_some_and(|temperature| temperature > 30.0))
                })
                .for_at_least(Duration::from_millis(20))
                .await
        });

        tokio::task::yield_now().await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "31.0"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "29.0"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), &mut waiter)
                .await
                .unwrap(),
            TimeoutResult::TimedOut
        );

        ctx.home_assistant()
            .cache_state(sample_state("sensor.temperature", "31.0"))
            .unwrap();
        waiter.await.unwrap();
    });
}

#[test]
fn global_state_wait_within_returns_timed_out() {
    run_async(async {
        let sensor = Sensor::<f64>::new("sensor.temperature").unwrap();
        let ctx = Context::with_seeded_states([sample_state("sensor.temperature", "29.0")]);

        assert_eq!(
            ctx.wait_until_state(move |state| {
                Ok(sensor
                    .read(state)?
                    .is_some_and(|temperature| temperature > 30.0))
            })
            .within(Duration::from_millis(1))
            .await
            .unwrap(),
            WaitResult::TimedOut
        );
    });
}
