use crate::{EntityId, state::EntityState};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, json};
use std::{
    collections::HashMap,
    future::Future,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

pub(crate) fn run_async(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future);
}

pub(crate) async fn wait_for_predicate_evaluations(evaluations: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_millis(50), async {
        while evaluations.load(Ordering::Acquire) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "expected at least {expected} predicate evaluations, got {}",
            evaluations.load(Ordering::Acquire)
        )
    });
}

pub(crate) async fn spawn_test_ws_server<F, Fut>(
    handler: F,
) -> (String, tokio::task::JoinHandle<()>)
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

pub(crate) async fn authenticate_test_ws(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) {
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

pub(crate) async fn recv_ws_json(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> serde_json::Value {
    let message = ws.next().await.unwrap().unwrap();
    match message {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text WebSocket message, got {other:?}"),
    }
}

pub(crate) async fn send_ws_result(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    id: u64,
    result: serde_json::Value,
) {
    ws.send(ws_json(json!({
        "id": id,
        "type": "result",
        "success": true,
        "result": result,
    })))
    .await
    .unwrap();
}

pub(crate) fn ws_json(value: serde_json::Value) -> Message {
    Message::Text(value.to_string().into())
}

#[derive(Debug)]
pub(crate) struct CapturedHttpRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: String,
}

pub(crate) struct TestHttpResponse {
    status: u16,
    reason: &'static str,
    body: String,
    content_type: Option<&'static str>,
}

impl TestHttpResponse {
    pub(crate) fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            reason: status_reason(status),
            body: body.to_string(),
            content_type: Some("application/json"),
        }
    }

    pub(crate) fn empty(status: u16) -> Self {
        Self {
            status,
            reason: status_reason(status),
            body: String::new(),
            content_type: None,
        }
    }
}

pub(crate) struct TestHttpServer;

impl TestHttpServer {
    pub(crate) fn spawn(
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

pub(crate) fn sample_state(entity_id: &str, state: &str) -> EntityState {
    EntityState {
        entity_id: EntityId::new(entity_id).unwrap(),
        state: state.to_string(),
        attributes: Map::new(),
        last_changed: "2026-06-30T00:00:00Z".to_string(),
        last_updated: "2026-06-30T00:00:00Z".to_string(),
    }
}
