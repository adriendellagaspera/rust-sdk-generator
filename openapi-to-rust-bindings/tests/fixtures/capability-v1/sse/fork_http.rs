//! Only the fork emits the supported owned stream ABI for this SSE operation.
use capability_matrix_consumer::generated::client::{ApiOpError, HttpClient};
use futures_util::StreamExt;
mod fork_http_support;
use fork_http_support::respond;

#[tokio::test]
async fn emitted_sse_stream_delivers_live_bytes_and_sets_accept() {
    let payload: &'static [u8] = b"event: tick\ndata: first\n\nevent: tick\ndata: second\n\n";
    let (url, server) = respond("200 OK", "text/event-stream", payload);
    let mut stream = HttpClient::new().with_base_url(url)
        .stream_events().await.expect("owned SSE stream");
    let mut received = Vec::new();
    while let Some(chunk) = stream.next().await {
        received.extend_from_slice(&chunk.expect("SSE chunk"));
    }
    assert_eq!(received, payload);
    let (headers, body) = server.join().expect("mock server");
    assert!(headers.starts_with("GET /events HTTP/1.1"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("accept: text/event-stream"), "{headers}");
    assert!(body.is_empty());
}

#[tokio::test]
async fn emitted_sse_stream_preserves_non_success_error() {
    let (url, server) = respond("401 Unauthorized", "application/json", br#"{"code":"auth","message":"denied"}"#);
    let err = HttpClient::new().with_base_url(url)
        .stream_events().await.expect_err("401 stream must fail");
    match err {
        ApiOpError::Api(api) => {
            assert_eq!(api.status, 401);
            assert!(api.body.contains("auth"), "error body: {}", api.body);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    let (headers, _) = server.join().expect("mock server");
    assert!(headers.starts_with("GET /events HTTP/1.1"), "{headers}");
}
