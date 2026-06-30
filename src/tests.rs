use crate::*;
use crate::{
    RestStateError, RestStateMethod, RestStateRequest, RestStateResponse, RestStateTransport,
};
use serde_json::{Map, json};
use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

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
fn state_write_requires_object_attributes() {
    assert!(StateWrite::new("ok", json!({ "friendly_name": "Status" })).is_ok());
    assert!(StateWrite::new("bad", json!(["not", "object"])).is_err());
}

#[test]
fn light_turn_on_validates_brightness_pct() {
    assert!(
        LightTurnOn {
            brightness_pct: Some(100),
            ..Default::default()
        }
        .validate()
        .is_ok()
    );

    assert!(
        LightTurnOn {
            brightness_pct: Some(101),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn light_service_payloads_include_entity_transition_rgb_and_brightness() {
    let entity_id = EntityId::new("light.office").unwrap();
    let payload = LightTurnOn {
        brightness_pct: Some(75),
        brightness: Some(128),
        transition: Some(Duration::from_millis(1500)),
        color_temp_kelvin: Some(2700),
        rgb_color: Some((1, 2, 3)),
        effect: Some("pulse".to_string()),
    }
    .into_service_data(&entity_id);

    assert_eq!(
        payload,
        json!({
            "entity_id": "light.office",
            "brightness_pct": 75,
            "brightness": 128,
            "transition": 1.5,
            "color_temp_kelvin": 2700,
            "rgb_color": [1, 2, 3],
            "effect": "pulse",
        })
    );

    let payload = LightTurnOff {
        transition: Some(Duration::from_secs(2)),
    }
    .into_service_data(&entity_id);
    assert_eq!(
        payload,
        json!({
            "entity_id": "light.office",
            "transition": 2.0,
        })
    );
}

#[test]
fn call_service_raw_validates_domain_and_service_before_placeholder() {
    run_async(async {
        let ha = HomeAssistantClient::new_generation();

        assert!(matches!(
            ha.call_service_raw("", "turn_on", json!({})).await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(matches!(
            ha.call_service_raw("light", " ", json!({})).await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(matches!(
            ha.call_service_raw("light", "turn_on", json!({})).await,
            Err(Error::NotImplemented(
                "HomeAssistantClient::call_service_raw"
            ))
        ));
    });
}

#[test]
fn command_raw_validates_shape_id_and_type_before_placeholder() {
    run_async(async {
        let ha = HomeAssistantClient::new_generation();

        assert!(matches!(
            ha.command_raw(json!("not an object")).await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(matches!(
            ha.command_raw(json!({ "id": 7, "type": "ping" })).await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(matches!(
            ha.command_raw(json!({ "payload": true })).await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(matches!(
            ha.command_raw(json!({ "type": 7 })).await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(matches!(
            ha.command_raw(json!({ "type": "ping" })).await,
            Err(Error::NotImplemented("HomeAssistantClient::command_raw"))
        ));
    });
}

#[test]
fn set_state_raw_posts_to_rest_path_and_maps_created_updated_without_cache_write() {
    run_async(async {
        let state = sample_state("sensor.generated", "ready");
        let transport = RecordingRestTransport::new([
            Ok(RestStateResponse {
                status: 201,
                state: Some(state.clone()),
            }),
            Ok(RestStateResponse {
                status: 200,
                state: Some(EntityState {
                    state: "updated".to_string(),
                    ..state.clone()
                }),
            }),
        ]);
        let requests = transport.requests.clone();
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = state.entity_id.clone();
        let write = StateWrite::new("ready", json!({ "source": "hauto" })).unwrap();

        assert_eq!(
            ha.set_state_raw(&entity_id, write.clone()).await.unwrap(),
            SetStateResult::Created(state.clone())
        );
        assert!(matches!(
            ha.get_state_raw(&entity_id).await,
            Err(Error::EntityNotFound(missing)) if missing == entity_id
        ));

        assert!(matches!(
            ha.set_state_raw(&entity_id, write.clone()).await.unwrap(),
            SetStateResult::Updated(returned) if returned.state == "updated"
        ));

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, RestStateMethod::Post);
        assert_eq!(requests[0].path, "/api/states/sensor.generated");
        assert_eq!(requests[0].entity_id, entity_id);
        assert_eq!(requests[0].body, Some(write));
    });
}

#[test]
fn set_state_raw_validates_attributes_before_transport() {
    run_async(async {
        let transport = RecordingRestTransport::new([Ok(RestStateResponse {
            status: 201,
            state: Some(sample_state("sensor.generated", "ready")),
        })]);
        let requests = transport.requests.clone();
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert!(matches!(
            ha.set_state_raw(
                &entity_id,
                StateWrite {
                    state: "bad".to_string(),
                    attributes: json!(["not", "object"]),
                },
            )
            .await,
            Err(Error::InvalidServiceOptions(_))
        ));
        assert!(requests.lock().unwrap().is_empty());
    });
}

#[test]
fn delete_state_raw_deletes_or_reports_not_found() {
    run_async(async {
        let transport = RecordingRestTransport::new([
            Ok(RestStateResponse {
                status: 200,
                state: None,
            }),
            Ok(RestStateResponse {
                status: 404,
                state: None,
            }),
        ]);
        let requests = transport.requests.clone();
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert_eq!(
            ha.delete_state_raw(&entity_id).await.unwrap(),
            DeleteStateResult::Deleted
        );
        assert_eq!(
            ha.delete_state_raw(&entity_id).await.unwrap(),
            DeleteStateResult::NotFound
        );

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, RestStateMethod::Delete);
        assert_eq!(requests[0].path, "/api/states/sensor.generated");
        assert_eq!(requests[0].body, None);
    });
}

#[test]
fn rest_state_connection_loss_can_map_to_outcome_unknown() {
    run_async(async {
        let transport = RecordingRestTransport::new([
            Err(RestStateError::outcome_unknown(
                "connection closed after write",
            )),
            Err(RestStateError::outcome_unknown(
                "connection closed after delete",
            )),
        ]);
        let ha = HomeAssistantClient::with_rest_states_transport(transport);
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert!(matches!(
            ha.set_state_raw(
                &entity_id,
                StateWrite::new("ready", json!({ "source": "hauto" })).unwrap(),
            )
            .await,
            Err(Error::OutcomeUnknown(message))
                if message == "connection closed after write"
        ));
        assert!(matches!(
            ha.delete_state_raw(&entity_id).await,
            Err(Error::OutcomeUnknown(message))
                if message == "connection closed after delete"
        ));
    });
}

#[test]
fn app_registration_keeps_names() {
    let app = App::new("http://homeassistant.local:8123", "token")
        .automation_fn("noop", |_ctx| async { Ok(()) });

    assert_eq!(app.automation_names(), vec!["noop"]);
}

#[test]
fn app_derives_home_assistant_endpoints() {
    let app = App::new("https://homeassistant.local:8123/", "token");

    assert_eq!(app.home_assistant_url, "https://homeassistant.local:8123");
    assert_eq!(
        app.websocket_url,
        "wss://homeassistant.local:8123/api/websocket"
    );
    assert_eq!(
        app.rest_states_url,
        "https://homeassistant.local:8123/api/states"
    );

    let app = App::new("http://localhost:8123?ignored=true#fragment", "token");
    assert_eq!(app.home_assistant_url, "http://localhost:8123");
    assert_eq!(app.websocket_url, "ws://localhost:8123/api/websocket");
    assert_eq!(app.rest_states_url, "http://localhost:8123/api/states");
}

#[test]
fn state_cache_get_state_raw_hits_and_misses() {
    run_async(async {
        let state = sample_state("light.office", "on");
        let ctx = Context::with_seeded_states([state.clone()]);
        let ha = ctx.home_assistant();

        assert_eq!(ha.get_state_raw(&state.entity_id).await.unwrap(), state);

        let missing = EntityId::new("light.missing").unwrap();
        assert!(matches!(
            ha.get_state_raw(&missing).await,
            Err(Error::EntityNotFound(entity_id)) if entity_id == missing
        ));
    });
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
fn cache_state_and_remove_cached_state_update_generation_cache() {
    run_async(async {
        let ctx = Context::new_generation();
        let ha = ctx.home_assistant();
        let state = sample_state("sensor.temperature", "21.5");
        let entity_id = state.entity_id.clone();

        ha.cache_state(state.clone()).unwrap();
        assert_eq!(ha.get_state_raw(&entity_id).await.unwrap(), state);
        assert!(ha.remove_cached_state(&entity_id).unwrap().is_some());
        assert!(matches!(
            ha.get_state_raw(&entity_id).await,
            Err(Error::EntityNotFound(missing)) if missing == entity_id
        ));
    });
}

#[test]
fn cancellation_notifies_context_and_stales_client_handles() {
    run_async(async {
        let state = sample_state("switch.fan", "on");
        let ctx = Context::with_seeded_states([state.clone()]);
        let ha = ctx.home_assistant();
        let cancelled = ctx.cancelled();

        ctx.cancel_generation();

        tokio::time::timeout(Duration::from_millis(50), cancelled)
            .await
            .expect("cancellation future should become ready");

        assert!(matches!(
            ha.get_state_raw(&state.entity_id).await,
            Err(Error::Cancelled)
        ));
    });
}

#[test]
fn sleep_returns_cancelled_when_generation_is_cancelled() {
    run_async(async {
        let ctx = Context::new_generation();
        ctx.cancel_generation();

        assert!(matches!(
            ctx.sleep(Duration::from_secs(60)).await,
            Err(Error::Cancelled)
        ));
    });
}

#[test]
fn timeout_reports_completed_and_timed_out() {
    run_async(async {
        let ctx = Context::new_generation();

        assert_eq!(
            ctx.timeout(Duration::from_secs(1), async { Ok(5) })
                .await
                .unwrap(),
            TimeoutResult::Completed(5)
        );
        assert_eq!(
            ctx.timeout(Duration::from_millis(1), async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(5)
            })
            .await
            .unwrap(),
            TimeoutResult::TimedOut
        );
    });
}

#[test]
fn spawn_handle_awaits_task_result() {
    run_async(async {
        let ctx = Context::new_generation();
        let handle = ctx.spawn(async { Ok(42) });

        assert_eq!(handle.await.unwrap(), 42);
    });
}

#[test]
fn run_after_can_complete_or_be_cancelled_without_dropping_task() {
    run_async(async {
        let ctx = Context::new_generation();
        let handle = ctx.run_after(Duration::from_millis(1), async { Ok("done") });
        assert_eq!(handle.await.unwrap(), "done");

        let mut handle = ctx.run_after(Duration::from_secs(60), async { Ok("late") });
        handle.cancel().await.unwrap();
        handle.cancel().await.unwrap();
        assert!(matches!(handle.await, Err(Error::Cancelled)));
    });
}

#[test]
fn run_after_cancel_waits_for_started_future_to_stop() {
    run_async(async {
        struct StopFlag(Arc<AtomicBool>);

        impl Drop for StopFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let ctx = Context::new_generation();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_for_task = stopped.clone();
        let (started_tx, mut started_rx) = watch::channel(false);
        let mut handle = ctx.run_after(Duration::from_millis(1), async move {
            let _stop = StopFlag(stopped_for_task);
            let _ = started_tx.send(true);
            std::future::pending::<()>().await;
            Ok(())
        });

        while !*started_rx.borrow() {
            started_rx.changed().await.unwrap();
        }

        handle.cancel().await.unwrap();
        assert!(stopped.load(Ordering::Acquire));
        assert!(matches!(handle.await, Err(Error::Cancelled)));
    });
}

#[test]
fn state_change_stream_filters_after_cache_update() {
    run_async(async {
        let ctx = Context::new_generation();
        let target = EntityId::new("binary_sensor.door").unwrap();
        let other = sample_state("binary_sensor.window", "on");
        let state = sample_state("binary_sensor.door", "on");
        let mut changes = ctx.state_changes(&target);

        ctx.home_assistant().cache_state(other).unwrap();
        ctx.home_assistant().cache_state(state.clone()).unwrap();

        let event = changes.next().await.unwrap().unwrap();
        assert_eq!(event.entity_id, target);
        assert_eq!(
            ctx.home_assistant().get_state_raw(&target).await.unwrap(),
            state
        );
    });
}

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

struct RecordingRestTransport {
    requests: Arc<Mutex<Vec<RestStateRequest>>>,
    responses: Arc<Mutex<Vec<Result<RestStateResponse, RestStateError>>>>,
}

impl RecordingRestTransport {
    fn new(responses: impl IntoIterator<Item = Result<RestStateResponse, RestStateError>>) -> Self {
        let mut responses = responses.into_iter().collect::<Vec<_>>();
        responses.reverse();
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses)),
        }
    }
}

impl RestStateTransport for RecordingRestTransport {
    fn send(
        &self,
        request: RestStateRequest,
    ) -> BoxFuture<Result<RestStateResponse, RestStateError>> {
        self.requests.lock().unwrap().push(request);
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Err(RestStateError::connection("no queued response")));
        Box::pin(async move { response })
    }
}

fn run_async(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future);
}

fn sample_state(entity_id: &str, state: &str) -> EntityState {
    EntityState {
        entity_id: EntityId::new(entity_id).unwrap(),
        state: state.to_string(),
        attributes: Map::new(),
        last_changed: "2026-06-30T00:00:00Z".to_string(),
        last_updated: "2026-06-30T00:00:00Z".to_string(),
    }
}
