use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::{net::TcpStream, sync::oneshot};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as TungsteniteError, Message},
};
use url::Url;

use crate::{
    AreaId, EntityState, Error, Result, StateChangedEvent,
    client::GenerationState,
    discovery::{AreaRegistryEntry, EntityRegistryDisplayResponse, ExtractTargetResponse},
};

type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type ClientWriter = SplitSink<ClientSocket, Message>;

#[derive(Debug)]
pub(crate) struct WsTransport {
    writer: tokio::sync::Mutex<ClientWriter>,
    next_request_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, WsRequestFailure>>>>,
    closed: AtomicBool,
}

impl WsTransport {
    pub(crate) async fn connect(
        websocket_url: impl AsRef<str>,
        access_token: impl Into<String>,
        generation: Arc<GenerationState>,
    ) -> Result<Arc<Self>> {
        let websocket_url = Url::parse(websocket_url.as_ref())
            .map_err(|error| Error::Connection(format!("invalid WebSocket URL: {error}")))?;
        match websocket_url.scheme() {
            "ws" | "wss" => {}
            scheme => {
                return Err(Error::Connection(format!(
                    "WebSocket URL must use ws or wss, got `{scheme}`"
                )));
            }
        }

        let (socket, _) = connect_async(websocket_url.as_str())
            .await
            .map_err(|error| Error::Connection(format!("WebSocket connect failed: {error}")))?;
        let (mut writer, mut reader) = socket.split();

        let auth_required = next_json_message(&mut reader).await?;
        match auth_required.get("type").and_then(Value::as_str) {
            Some("auth_required") => {}
            Some(other) => {
                return Err(Error::Authentication(format!(
                    "expected auth_required, got `{other}`"
                )));
            }
            None => {
                return Err(Error::Authentication(
                    "expected auth_required message".to_string(),
                ));
            }
        }

        writer
            .send(Message::Text(
                json!({
                    "type": "auth",
                    "access_token": access_token.into(),
                })
                .to_string()
                .into(),
            ))
            .await
            .map_err(|error| Error::Connection(format!("WebSocket auth send failed: {error}")))?;

        let auth_response = next_json_message(&mut reader).await?;
        match auth_response.get("type").and_then(Value::as_str) {
            Some("auth_ok") => {}
            Some("auth_invalid") => {
                let message = auth_response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Home Assistant rejected the access token");
                return Err(Error::Authentication(message.to_string()));
            }
            Some(other) => {
                return Err(Error::Authentication(format!(
                    "expected auth_ok, got `{other}`"
                )));
            }
            None => {
                return Err(Error::Authentication(
                    "expected auth_ok message".to_string(),
                ));
            }
        }

        let transport = Arc::new(Self {
            writer: tokio::sync::Mutex::new(writer),
            next_request_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        tokio::spawn(read_loop(transport.clone(), reader, generation));
        Ok(transport)
    }

    pub(crate) async fn command_raw(&self, command: Value) -> Result<Value> {
        let mut object = command.as_object().cloned().ok_or_else(|| {
            Error::InvalidServiceOptions("raw commands must be JSON objects".to_string())
        })?;
        if object.contains_key("id") {
            return Err(Error::InvalidServiceOptions(
                "raw commands must not include caller-supplied `id`".to_string(),
            ));
        }
        match object.get("type") {
            Some(Value::String(value)) if !value.is_empty() => {}
            Some(Value::String(_)) => {
                return Err(Error::InvalidServiceOptions(
                    "raw commands require a non-empty string `type`".to_string(),
                ));
            }
            _ => {
                return Err(Error::InvalidServiceOptions(
                    "raw commands require a string `type`".to_string(),
                ));
            }
        }

        if self.closed.load(Ordering::Acquire) {
            return Err(Error::NotSent(
                "WebSocket connection is already closed".to_string(),
            ));
        }

        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        object.insert("id".to_string(), Value::from(id));
        let message = Value::Object(object).to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending WebSocket command lock poisoned")
            .insert(id, sender);

        // `closed` gives a deterministic NotSent boundary before any write is
        // attempted. Once this task starts writing the frame, failure is
        // conservative OutcomeUnknown because the service side may have seen it.
        let send_result = self
            .writer
            .lock()
            .await
            .send(Message::Text(message.into()))
            .await;
        if let Err(error) = send_result {
            self.pending
                .lock()
                .expect("pending WebSocket command lock poisoned")
                .remove(&id);
            self.closed.store(true, Ordering::Release);
            return Err(Error::OutcomeUnknown(format!(
                "WebSocket command write failed after send was attempted: {error}"
            )));
        }

        match receiver.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(WsRequestFailure::Rejected(message))) => Err(Error::ServiceRejected(message)),
            Ok(Err(WsRequestFailure::Connection(message))) => Err(Error::Connection(message)),
            Ok(Err(WsRequestFailure::OutcomeUnknown(message))) => {
                Err(Error::OutcomeUnknown(message))
            }
            Err(_) => Err(Error::OutcomeUnknown(
                "WebSocket response channel closed before result".to_string(),
            )),
        }
    }

    pub(crate) async fn call_service_raw(
        &self,
        domain: &str,
        service: &str,
        data: Value,
    ) -> Result<Value> {
        self.command_raw(json!({
            "type": "call_service",
            "domain": domain,
            "service": service,
            "service_data": data,
        }))
        .await
    }

    pub(crate) async fn get_states(&self) -> Result<Value> {
        self.command_raw(json!({ "type": "get_states" })).await
    }

    pub(crate) async fn subscribe_events(&self, event_type: Option<&str>) -> Result<Value> {
        let mut command = Map::new();
        command.insert(
            "type".to_string(),
            Value::String("subscribe_events".to_string()),
        );
        if let Some(event_type) = event_type {
            command.insert(
                "event_type".to_string(),
                Value::String(event_type.to_string()),
            );
        }
        self.command_raw(Value::Object(command)).await
    }

    pub(crate) async fn list_areas(&self) -> Result<Vec<AreaRegistryEntry>> {
        self.typed_command(
            json!({ "type": "config/area_registry/list" }),
            "config/area_registry/list",
        )
        .await
    }

    pub(crate) async fn list_entities_for_display(&self) -> Result<EntityRegistryDisplayResponse> {
        self.typed_command(
            json!({ "type": "config/entity_registry/list_for_display" }),
            "config/entity_registry/list_for_display",
        )
        .await
    }

    pub(crate) async fn extract_area(&self, area_id: &AreaId) -> Result<ExtractTargetResponse> {
        self.typed_command(
            json!({
                "type": "extract_from_target",
                "target": {
                    "area_id": [area_id.as_str()],
                },
            }),
            "extract_from_target",
        )
        .await
    }

    async fn typed_command<T>(&self, command: Value, command_type: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let value = self.command_raw(command).await?;
        serde_json::from_value(value).map_err(|error| {
            Error::Connection(format!(
                "{command_type} response could not be decoded: {error}"
            ))
        })
    }

    fn fail_pending(&self, failure: WsRequestFailure) {
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .expect("pending WebSocket command lock poisoned"),
        );
        for sender in pending.into_values() {
            let _ = sender.send(Err(failure.clone()));
        }
    }
}

#[derive(Clone, Debug)]
enum WsRequestFailure {
    Rejected(String),
    Connection(String),
    OutcomeUnknown(String),
}

async fn next_json_message(
    reader: &mut futures_util::stream::SplitStream<ClientSocket>,
) -> Result<Value> {
    loop {
        let message = reader
            .next()
            .await
            .ok_or_else(|| Error::Connection("WebSocket closed before auth completed".to_string()))?
            .map_err(|error| Error::Connection(format!("WebSocket read failed: {error}")))?;
        match message {
            Message::Text(text) => {
                return serde_json::from_str(&text).map_err(|error| {
                    Error::Connection(format!("WebSocket message could not be decoded: {error}"))
                });
            }
            Message::Binary(bytes) => {
                return serde_json::from_slice(&bytes).map_err(|error| {
                    Error::Connection(format!(
                        "WebSocket binary message could not be decoded: {error}"
                    ))
                });
            }
            Message::Close(_) => {
                return Err(Error::Connection(
                    "WebSocket closed before auth completed".to_string(),
                ));
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
}

async fn read_loop(
    transport: Arc<WsTransport>,
    mut reader: futures_util::stream::SplitStream<ClientSocket>,
    generation: Arc<GenerationState>,
) {
    let failure = loop {
        match reader.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => route_server_message(&transport, &generation, value),
                Err(error) => {
                    break WsRequestFailure::Connection(format!(
                        "WebSocket message could not be decoded: {error}"
                    ));
                }
            },
            Some(Ok(Message::Binary(bytes))) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) => route_server_message(&transport, &generation, value),
                Err(error) => {
                    break WsRequestFailure::Connection(format!(
                        "WebSocket binary message could not be decoded: {error}"
                    ));
                }
            },
            Some(Ok(Message::Close(_))) | None => {
                break WsRequestFailure::OutcomeUnknown(
                    "WebSocket closed before pending command response".to_string(),
                );
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Err(TungsteniteError::ConnectionClosed)) => {
                break WsRequestFailure::OutcomeUnknown(
                    "WebSocket closed before pending command response".to_string(),
                );
            }
            Some(Err(error)) => {
                break WsRequestFailure::OutcomeUnknown(format!(
                    "WebSocket read failed before pending command response: {error}"
                ));
            }
        }
    };
    transport.closed.store(true, Ordering::Release);
    generation.cancel();
    transport.fail_pending(failure);
}

fn route_server_message(transport: &WsTransport, generation: &GenerationState, value: Value) {
    if value.get("type").and_then(Value::as_str) == Some("result") {
        route_result_message(transport, &value);
    } else if value.get("type").and_then(Value::as_str) == Some("event") {
        route_event_message(generation, value);
    }
}

fn route_result_message(transport: &WsTransport, value: &Value) {
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return;
    };
    let Some(sender) = transport
        .pending
        .lock()
        .expect("pending WebSocket command lock poisoned")
        .remove(&id)
    else {
        return;
    };

    let response = match value.get("success").and_then(Value::as_bool) {
        Some(true) => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
        Some(false) => Err(WsRequestFailure::Rejected(home_assistant_error_message(
            value,
        ))),
        None => Err(WsRequestFailure::Connection(
            "WebSocket result response missing success flag".to_string(),
        )),
    };
    let _ = sender.send(response);
}

fn home_assistant_error_message(value: &Value) -> String {
    let error = value.get("error").unwrap_or(&Value::Null);
    let code = error.get("code").and_then(Value::as_str);
    let message = error.get("message").and_then(Value::as_str);
    match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "Home Assistant rejected the command".to_string(),
    }
}

fn route_event_message(generation: &GenerationState, value: Value) {
    let Some(event) = value.get("event").cloned() else {
        return;
    };
    let _ = generation.raw_events.send(event.clone());
    if event.get("event_type").and_then(Value::as_str) == Some("state_changed") {
        apply_state_changed_event(generation, &event);
    }
}

fn apply_state_changed_event(generation: &GenerationState, event: &Value) {
    let Some(data) = event.get("data") else {
        return;
    };
    let Some(entity_id) = data.get("entity_id").and_then(Value::as_str) else {
        return;
    };
    let Ok(entity_id) = entity_id.parse() else {
        return;
    };
    let old_state = data
        .get("old_state")
        .cloned()
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<EntityState>(value).ok());
    let new_state = data
        .get("new_state")
        .cloned()
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<EntityState>(value).ok());

    generation.apply_state_changed_event(StateChangedEvent {
        entity_id,
        old_state,
        new_state,
    });
}
