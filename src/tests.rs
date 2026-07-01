use crate::*;
use crate::{
    RestStateError, RestStateMethod, RestStateRequest, RestStateResponse, RestStateTransport,
    client::HomeAssistantClient,
    discovery::AreaInfo,
    rest::ReqwestRestStateTransport,
    runtime::BoxFuture,
    state::{DeleteStateResult, EntityState, SetStateResult, StateWrite},
    test_support::*,
};
use futures_util::SinkExt;
use serde_json::{Map, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

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
fn discovery_protocol_coalesces_catalog_and_area_requests_and_joins_cached_metadata() {
    run_async(async {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;

            for _ in 0..2 {
                let command = recv_ws_json(&mut ws).await;
                let id = command["id"].as_u64().unwrap();
                match command["type"].as_str().unwrap() {
                    "config/area_registry/list" => {
                        assert_eq!(
                            command,
                            json!({ "id": id, "type": "config/area_registry/list" })
                        );
                        send_ws_result(
                            &mut ws,
                            id,
                            json!([
                                { "area_id": "main_bathroom", "name": "Main Bathroom" },
                                { "area_id": "hall", "name": "Hall" }
                            ]),
                        )
                        .await;
                    }
                    "config/entity_registry/list_for_display" => {
                        assert_eq!(
                            command,
                            json!({
                                "id": id,
                                "type": "config/entity_registry/list_for_display"
                            })
                        );
                        send_ws_result(
                            &mut ws,
                            id,
                            json!({
                                "entities": [
                                    { "ei": "sensor.bathroom_temperature", "en": "Registry Temp" },
                                    { "ei": "sensor.bathroom_humidity", "en": "Registry Humidity" },
                                    { "ei": "sensor.unclassified", "en": "Unclassified" },
                                    { "ei": "switch.fan", "en": "Bathroom Fan" }
                                ]
                            }),
                        )
                        .await;
                    }
                    other => panic!("unexpected discovery command: {other}"),
                }
            }

            let extract = recv_ws_json(&mut ws).await;
            let id = extract["id"].as_u64().unwrap();
            assert_eq!(
                extract,
                json!({
                    "id": id,
                    "type": "extract_from_target",
                    "target": { "area_id": ["main_bathroom"] }
                })
            );
            send_ws_result(
                &mut ws,
                id,
                json!({
                    "referenced_entities": [
                        "sensor.bathroom_temperature",
                        "sensor.bathroom_humidity",
                        "sensor.unclassified"
                    ]
                }),
            )
            .await;

            done_rx.await.unwrap();
        })
        .await;

        let ha = HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
            .await
            .unwrap();
        let mut temperature = sample_state("sensor.bathroom_temperature", "unavailable");
        temperature.attributes = Map::from_iter([
            ("friendly_name".to_string(), json!("Live Temperature")),
            ("device_class".to_string(), json!("temperature")),
        ]);
        let mut humidity = sample_state("sensor.bathroom_humidity", "unknown");
        humidity.attributes = Map::from_iter([("device_class".to_string(), json!("humidity"))]);
        let mut unclassified = sample_state("sensor.unclassified", "unavailable");
        unclassified.attributes =
            Map::from_iter([("friendly_name".to_string(), json!("Live Unclassified"))]);
        ha.cache_state(temperature).unwrap();
        ha.cache_state(humidity).unwrap();
        ha.cache_state(unclassified).unwrap();
        let ctx = Context { home_assistant: ha };

        let cloned_ctx = ctx.clone();
        let (first, second) = tokio::join!(ctx.entity_catalog(), cloned_ctx.entity_catalog());
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(
            first.area_named(" main bathroom ").unwrap().name(),
            "Main Bathroom"
        );
        assert_eq!(second.area_named("HALL").unwrap().id().as_str(), "hall");

        let all = first.entities();
        let temperature = all
            .query()
            .device_class("temperature")
            .exactly_one()
            .unwrap();
        assert_eq!(temperature.name(), Some("Live Temperature"));
        assert_eq!(temperature.device_class(), Some("temperature"));
        let humidity = all.query().device_class("humidity").exactly_one().unwrap();
        assert_eq!(humidity.name(), Some("Registry Humidity"));
        assert_eq!(humidity.device_class(), Some("humidity"));
        assert!(all.query().device_class("unavailable").collect().is_empty());
        assert!(
            all.query()
                .named("live unclassified")
                .exactly_one()
                .unwrap()
                .device_class()
                .is_none()
        );
        assert_eq!(
            all.query()
                .domain("switch")
                .named("bathroom fan")
                .exactly_one()
                .unwrap()
                .entity_id()
                .as_str(),
            "switch.fan"
        );

        let area = first.area_named("Main Bathroom").unwrap();
        let (area_entities, same_area_entities) =
            tokio::join!(first.entities_in(&area), second.entities_in(&area));
        assert_eq!(area_entities.unwrap().query().collect().len(), 3);
        assert_eq!(same_area_entities.unwrap().query().collect().len(), 3);
        assert_eq!(
            first
                .entities_in(&area)
                .await
                .unwrap()
                .query()
                .collect()
                .len(),
            3
        );

        done_tx.send(()).unwrap();
        server.await.unwrap();
    });
}

#[test]
fn malformed_discovery_payload_is_a_connection_error() {
    run_async(async {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            for _ in 0..2 {
                let command = recv_ws_json(&mut ws).await;
                let id = command["id"].as_u64().unwrap();
                let result = match command["type"].as_str().unwrap() {
                    "config/area_registry/list" => json!({ "not": "an area list" }),
                    "config/entity_registry/list_for_display" => json!({ "entities": [] }),
                    other => panic!("unexpected discovery command: {other}"),
                };
                send_ws_result(&mut ws, id, result).await;
            }
            done_rx.await.unwrap();
        })
        .await;
        let ctx = Context {
            home_assistant: HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
                .await
                .unwrap(),
        };

        assert!(matches!(
            ctx.entity_catalog().await,
            Err(Error::Connection(message))
                if message.contains("config/area_registry/list")
                    && message.contains("could not be decoded")
        ));
        done_tx.send(()).unwrap();
        server.await.unwrap();
    });
}

#[test]
fn cancelling_generation_wakes_pending_discovery_catalog_load() {
    run_async(async {
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            let _pending_request = recv_ws_json(&mut ws).await;
            request_tx.send(()).unwrap();
            done_rx.await.unwrap();
        })
        .await;
        let ctx = Context {
            home_assistant: HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
                .await
                .unwrap(),
        };
        let pending_ctx = ctx.clone();
        let pending = tokio::spawn(async move { pending_ctx.entity_catalog().await });
        request_rx.await.unwrap();
        ctx.cancel_generation();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .expect("catalog load hung after generation cancellation")
                .unwrap(),
            Err(Error::Cancelled)
        ));
        done_tx.send(()).unwrap();
        server.await.unwrap();
    });
}

#[test]
fn cancelling_generation_wakes_pending_area_extraction() {
    run_async(async {
        let (extract_tx, extract_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            for _ in 0..2 {
                let command = recv_ws_json(&mut ws).await;
                let id = command["id"].as_u64().unwrap();
                let result = match command["type"].as_str().unwrap() {
                    "config/area_registry/list" => {
                        json!([{ "area_id": "bathroom", "name": "Bathroom" }])
                    }
                    "config/entity_registry/list_for_display" => {
                        json!({ "entities": [] })
                    }
                    other => panic!("unexpected discovery command: {other}"),
                };
                send_ws_result(&mut ws, id, result).await;
            }
            let _extract = recv_ws_json(&mut ws).await;
            extract_tx.send(()).unwrap();
            done_rx.await.unwrap();
        })
        .await;
        let ctx = Context {
            home_assistant: HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
                .await
                .unwrap(),
        };
        let catalog = ctx.entity_catalog().await.unwrap();
        let area = catalog.area_named("Bathroom").unwrap();
        let pending = tokio::spawn(async move { catalog.entities_in(&area).await });
        extract_rx.await.unwrap();
        ctx.cancel_generation();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .expect("area extraction hung after generation cancellation")
                .unwrap(),
            Err(Error::Cancelled)
        ));
        done_tx.send(()).unwrap();
        server.await.unwrap();
    });
}

#[test]
fn connection_loss_during_catalog_load_returns_without_hanging() {
    run_async(async {
        let (url, server) = spawn_test_ws_server(move |mut ws| async move {
            authenticate_test_ws(&mut ws).await;
            let _pending_request = recv_ws_json(&mut ws).await;
            ws.close(None).await.unwrap();
        })
        .await;
        let ctx = Context {
            home_assistant: HomeAssistantClient::connect_websocket_generation(&url, "secret-token")
                .await
                .unwrap(),
        };

        let result = tokio::time::timeout(Duration::from_secs(1), ctx.entity_catalog())
            .await
            .expect("catalog load hung after WebSocket connection loss");
        assert!(matches!(
            result,
            Err(Error::Cancelled | Error::Connection(_) | Error::OutcomeUnknown(_))
        ));
        server.await.unwrap();
    });
}

#[test]
fn fresh_websocket_generation_loads_a_fresh_catalog() {
    run_async(async {
        async fn load_catalog_area(area_id: &'static str, area_name: &'static str) -> AreaInfo {
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            let (url, server) = spawn_test_ws_server(move |mut ws| async move {
                authenticate_test_ws(&mut ws).await;
                for _ in 0..2 {
                    let command = recv_ws_json(&mut ws).await;
                    let id = command["id"].as_u64().unwrap();
                    let result = match command["type"].as_str().unwrap() {
                        "config/area_registry/list" => {
                            json!([{ "area_id": area_id, "name": area_name }])
                        }
                        "config/entity_registry/list_for_display" => {
                            json!({ "entities": [] })
                        }
                        other => panic!("unexpected discovery command: {other}"),
                    };
                    send_ws_result(&mut ws, id, result).await;
                }
                done_rx.await.unwrap();
            })
            .await;
            let ctx = Context {
                home_assistant: HomeAssistantClient::connect_websocket_generation(
                    &url,
                    "secret-token",
                )
                .await
                .unwrap(),
            };
            let area = ctx
                .entity_catalog()
                .await
                .unwrap()
                .area_named(area_name)
                .unwrap();
            done_tx.send(()).unwrap();
            server.await.unwrap();
            area
        }

        let first = load_catalog_area("old_bathroom", "Old Bathroom").await;
        let second = load_catalog_area("new_bathroom", "New Bathroom").await;
        assert_eq!(first.id().as_str(), "old_bathroom");
        assert_eq!(second.id().as_str(), "new_bathroom");
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
