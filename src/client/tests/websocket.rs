use crate::*;
use crate::{client::HomeAssistantClient, state::EntityState, test_support::*};
use futures_util::SinkExt;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

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
