//! Transport-specific request discriminators are fork-only emitted behavior.
use capability_matrix_consumer::generated::client::HttpClient;
use capability_matrix_consumer::generated::types::RenderRequest;
use futures_util::StreamExt;
use serde_json::{Value, json};
mod fork_http_support;
use fork_http_support::respond;

fn request(live: bool, payload: &str) -> RenderRequest {
    serde_json::from_value(json!({
        "live-output": live,
        "payload": payload,
        "mode": "initial",
        "nullable-mode": null
    })).expect("generated request model")
}

fn body_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("generated method emitted JSON")
}

#[tokio::test]
async fn buffered_transport_forces_exact_discriminator_values_before_serialization() {
    let (url, server) = respond("200 OK", "application/json", br#"{"id":"result-1"}"#);
    let result = HttpClient::new().with_base_url(url)
        .render(request(true, "buffered-input"))
        .await.expect("buffered render");
    assert_eq!(result.id, "result-1");
    let (headers, body) = server.join().expect("mock server");
    assert!(headers.starts_with("POST /render HTTP/1.1"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("content-type: application/json"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("accept: application/json"), "{headers}");
    let body = body_json(&body);
    assert_eq!(body["payload"], "buffered-input");
    assert_eq!(body["live-output"], false, "transport discriminator overrides caller's true");
    assert_eq!(body["mode"], "buffered");
    assert_eq!(body["nullable-mode"], "nullable");
    assert_eq!(body["tri"], "tri");
}

#[tokio::test]
async fn sse_transport_forces_stream_discriminator_before_request_send() {
    let bytes: &'static [u8] = b"data: streaming\n\n";
    let (url, server) = respond("200 OK", "text/event-stream", bytes);
    let mut stream = HttpClient::new().with_base_url(url)
        .render_stream(request(false, "stream-input"))
        .await.expect("stream render");
    let mut received = Vec::new();
    while let Some(chunk) = stream.next().await {
        received.extend_from_slice(&chunk.expect("SSE stream chunk"));
    }
    assert_eq!(received, bytes);
    let (headers, body) = server.join().expect("mock server");
    assert!(headers.starts_with("POST /render HTTP/1.1"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("content-type: application/json"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("accept: text/event-stream"), "{headers}");
    let body = body_json(&body);
    assert_eq!(body["payload"], "stream-input");
    assert_eq!(body["live-output"], true, "stream discriminator overrides caller's false");
    assert_eq!(body["mode"], "initial");
}
