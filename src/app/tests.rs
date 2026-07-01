use super::App;
use crate::{
    EntityId, Error,
    state::{DeleteStateResult, SetStateResult, StateWrite},
    test_support::*,
};
use futures_util::SinkExt;
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

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
