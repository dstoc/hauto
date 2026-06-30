use crate::*;
use crate::{
    RestStateError, RestStateMethod, RestStateRequest, RestStateResponse, RestStateTransport,
    rest::ReqwestRestStateTransport,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, json};
use std::{
    collections::HashMap,
    future::Future,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use tokio::sync::watch;
use tokio_tungstenite::{accept_async, tungstenite::Message};

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
fn app_context_generation_posts_and_deletes_states_over_rest() {
    run_async(async {
        let created = sample_state("sensor.generated", "ready");
        let (base_url, requests, server) = TestHttpServer::spawn([
            TestHttpResponse::json(201, json!(created)),
            TestHttpResponse::empty(404),
        ]);
        let app = App::new(base_url, "secret-token");
        let ctx = app.new_context_generation().unwrap();
        let entity_id = EntityId::new("sensor.generated").unwrap();

        assert!(matches!(
            ctx.home_assistant()
                .set_state_raw(
                    &entity_id,
                    StateWrite::new("ready", json!({ "source": "hauto" })).unwrap(),
                )
                .await
                .unwrap(),
            SetStateResult::Created(state) if state.entity_id == entity_id && state.state == "ready"
        ));
        assert_eq!(
            ctx.home_assistant()
                .delete_state_raw(&entity_id)
                .await
                .unwrap(),
            DeleteStateResult::NotFound
        );

        server.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/api/states/sensor.generated");
        assert_eq!(
            requests[0].headers.get("authorization"),
            Some(&"Bearer secret-token".to_string())
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&requests[0].body).unwrap(),
            json!({ "state": "ready", "attributes": { "source": "hauto" } })
        );
        assert_eq!(requests[1].method, "DELETE");
        assert_eq!(requests[1].path, "/api/states/sensor.generated");
        assert_eq!(
            requests[1].headers.get("authorization"),
            Some(&"Bearer secret-token".to_string())
        );
        assert!(requests[1].body.is_empty());
    });
}

#[test]
fn app_one_generation_bootstraps_snapshot_then_runs_automation_with_websocket_context() {
    run_async(async {
        let initial = sample_state("sensor.temperature", "21");
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_for_server = observed.clone();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;

            let subscribe = recv_ws_json(&mut ws).await;
            let subscribe_id = subscribe
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            observed_for_server.lock().unwrap().push(subscribe.clone());
            assert_eq!(subscribe.get("type"), Some(&json!("subscribe_events")));
            assert_eq!(subscribe.get("event_type"), Some(&json!("state_changed")));
            ws.send(ws_json(json!({
                "id": subscribe_id,
                "type": "result",
                "success": true,
                "result": null
            })))
            .await
            .unwrap();

            let get_states = recv_ws_json(&mut ws).await;
            let get_states_id = get_states
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            observed_for_server.lock().unwrap().push(get_states.clone());
            assert_eq!(get_states.get("type"), Some(&json!("get_states")));
            ws.send(ws_json(json!({
                "id": get_states_id,
                "type": "result",
                "success": true,
                "result": [initial]
            })))
            .await
            .unwrap();

            let ping = recv_ws_json(&mut ws).await;
            let ping_id = ping.get("id").and_then(serde_json::Value::as_u64).unwrap();
            observed_for_server.lock().unwrap().push(ping.clone());
            assert_eq!(ping.get("type"), Some(&json!("ping")));
            ws.send(ws_json(json!({
                "id": ping_id,
                "type": "result",
                "success": true,
                "result": { "pong": true }
            })))
            .await
            .unwrap();
            ws.close(None).await.unwrap();
        })
        .await;

        let base_url = url.replacen("ws://", "http://", 1);
        let runs = Arc::new(AtomicUsize::new(0));
        let runs_for_automation = runs.clone();
        let app =
            App::new(base_url, "secret-token").automation_fn("snapshot then ping", move |ctx| {
                let runs = runs_for_automation.clone();
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    let state = ctx
                        .home_assistant()
                        .get_state_raw(&EntityId::new("sensor.temperature").unwrap())
                        .await?;
                    assert_eq!(state.state, "21");
                    assert_eq!(
                        ctx.home_assistant()
                            .command_raw(json!({ "type": "ping" }))
                            .await?,
                        json!({ "pong": true })
                    );
                    Ok(())
                }
            });

        assert_eq!(
            app.run_one_generation().await.unwrap(),
            crate::app::GenerationOutcome::ConnectionLost
        );
        server.await.unwrap();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 3);
        assert_eq!(observed[0].get("type"), Some(&json!("subscribe_events")));
        assert_eq!(observed[1].get("type"), Some(&json!("get_states")));
        assert_eq!(observed[2].get("type"), Some(&json!("ping")));
    });
}

#[test]
fn app_one_generation_surfaces_automation_failures() {
    run_async(async {
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;

            let subscribe = recv_ws_json(&mut ws).await;
            let subscribe_id = subscribe
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            ws.send(ws_json(json!({
                "id": subscribe_id,
                "type": "result",
                "success": true,
                "result": null
            })))
            .await
            .unwrap();

            let get_states = recv_ws_json(&mut ws).await;
            let get_states_id = get_states
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            ws.send(ws_json(json!({
                "id": get_states_id,
                "type": "result",
                "success": true,
                "result": []
            })))
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            ws.close(None).await.unwrap();
        })
        .await;

        let base_url = url.replacen("ws://", "http://", 1);
        let app = App::new(base_url, "secret-token").automation_fn("broken", |_ctx| async {
            Err(Error::InvalidServiceOptions("boom".to_string()))
        });

        assert!(matches!(
            app.run_one_generation().await,
            Err(Error::AutomationTask(message))
                if message.contains("broken") && message.contains("boom")
        ));
        server.await.unwrap();
    });
}

#[test]
fn reqwest_rest_transport_rejects_non_http_base_urls() {
    assert!(matches!(
        ReqwestRestStateTransport::new("ws://homeassistant.local/api", "token"),
        Err(Error::Connection(_))
    ));
}

#[test]
fn websocket_command_raw_authenticates_inserts_id_and_returns_result() {
    run_async(async {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_server = requests.clone();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            ws.send(ws_json(json!({ "type": "auth_required" })))
                .await
                .unwrap();
            let auth = recv_ws_json(&mut ws).await;
            requests_for_server.lock().unwrap().push(auth);
            ws.send(ws_json(json!({ "type": "auth_ok" })))
                .await
                .unwrap();

            let command = recv_ws_json(&mut ws).await;
            let id = command
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            requests_for_server.lock().unwrap().push(command);
            ws.send(ws_json(json!({
                "id": id,
                "type": "result",
                "success": true,
                "result": { "pong": true }
            })))
            .await
            .unwrap();
        })
        .await;

        let ha = HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
            .await
            .unwrap();
        assert_eq!(
            ha.command_raw(json!({ "type": "ping" })).await.unwrap(),
            json!({ "pong": true })
        );
        server.await.unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests[0],
            json!({ "type": "auth", "access_token": "secret-token" })
        );
        assert_eq!(requests[1].get("id"), Some(&json!(1)));
        assert_eq!(requests[1].get("type"), Some(&json!("ping")));
    });
}

#[test]
fn websocket_call_service_sends_domain_service_data_and_returns_payload() {
    run_async(async {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_server = requests.clone();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            let command = recv_ws_json(&mut ws).await;
            let id = command
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            requests_for_server.lock().unwrap().push(command);
            ws.send(ws_json(json!({
                "id": id,
                "type": "result",
                "success": true,
                "result": { "context": { "id": "abc" } }
            })))
            .await
            .unwrap();
        })
        .await;

        let ha = HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
            .await
            .unwrap();
        assert_eq!(
            ha.call_service_raw("light", "turn_on", json!({ "entity_id": "light.office" }))
                .await
                .unwrap(),
            json!({ "context": { "id": "abc" } })
        );
        server.await.unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests[0],
            json!({
                "id": 1,
                "type": "call_service",
                "domain": "light",
                "service": "turn_on",
                "service_data": { "entity_id": "light.office" }
            })
        );
    });
}

#[test]
fn websocket_error_response_maps_to_service_rejected() {
    run_async(async {
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            let command = recv_ws_json(&mut ws).await;
            let id = command
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            ws.send(ws_json(json!({
                "id": id,
                "type": "result",
                "success": false,
                "error": {
                    "code": "invalid_format",
                    "message": "bad command"
                }
            })))
            .await
            .unwrap();
        })
        .await;

        let ha = HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
            .await
            .unwrap();
        assert!(matches!(
            ha.command_raw(json!({ "type": "bad" })).await,
            Err(Error::ServiceRejected(message))
                if message == "invalid_format: bad command"
        ));
        server.await.unwrap();
    });
}

#[test]
fn websocket_get_states_and_state_changed_subscription_update_cache_and_streams() {
    run_async(async {
        let initial = sample_state("sensor.temperature", "21");
        let updated = EntityState {
            state: "22".to_string(),
            ..initial.clone()
        };
        let updated_for_server = updated.clone();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;

            let get_states = recv_ws_json(&mut ws).await;
            let get_states_id = get_states
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            ws.send(ws_json(json!({
                "id": get_states_id,
                "type": "result",
                "success": true,
                "result": [initial]
            })))
            .await
            .unwrap();

            let subscribe = recv_ws_json(&mut ws).await;
            let subscribe_id = subscribe
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap();
            assert_eq!(subscribe.get("event_type"), Some(&json!("state_changed")));
            ws.send(ws_json(json!({
                "id": subscribe_id,
                "type": "result",
                "success": true,
                "result": null
            })))
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            ws.send(ws_json(json!({
                "id": subscribe_id,
                "type": "event",
                "event": {
                    "event_type": "state_changed",
                    "data": {
                        "entity_id": "sensor.temperature",
                        "old_state": null,
                        "new_state": updated_for_server
                    }
                }
            })))
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;

        let ha = HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
            .await
            .unwrap();
        let states = ha.refresh_states_from_websocket().await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(
            ha.get_state_raw(&EntityId::new("sensor.temperature").unwrap())
                .await
                .unwrap()
                .state,
            "21"
        );

        let mut changes = ha.subscribe_state_changes().await.unwrap();
        let mut raw = ha.subscribe_state_changed_events().await.unwrap();
        let raw_event = raw.next().await.unwrap().unwrap();
        assert_eq!(raw_event.get("event_type"), Some(&json!("state_changed")));
        let change = changes.next().await.unwrap().unwrap();
        assert_eq!(
            change.entity_id,
            EntityId::new("sensor.temperature").unwrap()
        );
        assert_eq!(change.new_state, Some(updated.clone()));
        assert_eq!(
            ha.get_state_raw(&EntityId::new("sensor.temperature").unwrap())
                .await
                .unwrap(),
            updated
        );
        server.await.unwrap();
    });
}

#[test]
fn websocket_connection_loss_fails_pending_then_rejects_new_commands_as_not_sent() {
    run_async(async {
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            let _command = recv_ws_json(&mut ws).await;
            ws.close(None).await.unwrap();
        })
        .await;

        let ha = HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
            .await
            .unwrap();
        assert!(matches!(
            ha.command_raw(json!({ "type": "maybe_sent" })).await,
            Err(Error::OutcomeUnknown(_))
        ));
        assert!(matches!(
            ha.command_raw(json!({ "type": "definitely_not_sent" }))
                .await,
            Err(Error::Cancelled)
        ));
        server.await.unwrap();
    });
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
        .enable_io()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future);
}

async fn spawn_test_ws_server<F, Fut>(handler: F) -> (String, tokio::task::JoinHandle<()>)
where
    F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        handler(ws).await;
    });
    (url, handle)
}

async fn authenticate_test_ws(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    ws.send(ws_json(json!({ "type": "auth_required" })))
        .await
        .unwrap();
    assert_eq!(
        recv_ws_json(ws).await,
        json!({ "type": "auth", "access_token": "secret-token" })
    );
    ws.send(ws_json(json!({ "type": "auth_ok" })))
        .await
        .unwrap();
}

async fn recv_ws_json(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> serde_json::Value {
    let message = ws.next().await.unwrap().unwrap();
    match message {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text WebSocket message, got {other:?}"),
    }
}

fn ws_json(value: serde_json::Value) -> Message {
    Message::Text(value.to_string().into())
}

#[derive(Debug)]
struct CapturedHttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct TestHttpResponse {
    status: u16,
    reason: &'static str,
    body: String,
    content_type: Option<&'static str>,
}

impl TestHttpResponse {
    fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            reason: status_reason(status),
            body: body.to_string(),
            content_type: Some("application/json"),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            reason: status_reason(status),
            body: String::new(),
            content_type: None,
        }
    }
}

struct TestHttpServer;

impl TestHttpServer {
    fn spawn(
        responses: impl IntoIterator<Item = TestHttpResponse>,
    ) -> (
        String,
        Arc<Mutex<Vec<CapturedHttpRequest>>>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_thread = requests.clone();
        let responses = responses.into_iter().collect::<Vec<_>>();
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                requests_for_thread.lock().unwrap().push(request);
                write_http_response(&mut stream, response);
            }
        });

        (base_url, requests, handle)
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> CapturedHttpRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "client closed connection before full request");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some((header_end, content_length)) = http_header_end_and_length(&bytes) {
            let expected_len = header_end + 4 + content_length;
            if bytes.len() >= expected_len {
                break;
            }
        }
    }

    let (header_end, content_length) = http_header_end_and_length(&bytes).unwrap();
    let headers_text = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let mut lines = headers_text.lines();
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap().to_string();
    let path = request_parts.next().unwrap().to_string();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    let body_start = header_end + 4;
    let body = String::from_utf8(bytes[body_start..body_start + content_length].to_vec()).unwrap();

    CapturedHttpRequest {
        method,
        path,
        headers,
        body,
    }
}

fn http_header_end_and_length(bytes: &[u8]) -> Option<(usize, usize)> {
    let header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    Some((header_end, content_length))
}

fn write_http_response(stream: &mut std::net::TcpStream, response: TestHttpResponse) {
    let content_type = response
        .content_type
        .map(|value| format!("content-type: {value}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "HTTP/1.1 {} {}\r\n{content_type}content-length: {}\r\nconnection: close\r\n\r\n{}",
        response.status,
        response.reason,
        response.body.len(),
        response.body
    )
    .unwrap();
}

fn status_reason(status: u16) -> &'static str {
    match status {
        201 => "Created",
        404 => "Not Found",
        _ => "OK",
    }
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
