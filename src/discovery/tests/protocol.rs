use crate::*;
use crate::{client::HomeAssistantClient, discovery::AreaInfo, test_support::*};
use serde_json::{Map, json};
use std::time::Duration;

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
