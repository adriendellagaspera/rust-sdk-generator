use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use independent_notebook_consumer::{sdk::{self, NotebookClient, SdkError}};
use serde_json::Value;

fn mock_once(status: &str, media_type: &str, payload: &[u8]) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock address"));
    let response = payload.to_vec();
    let status = status.to_owned();
    let media_type = media_type.to_owned();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept request");
        socket.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        let (header_end, length) = loop {
            let read = socket.read(&mut chunk).expect("read request");
            assert!(read > 0, "request ended before HTTP headers");
            bytes.extend_from_slice(&chunk[..read]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                let length = head.lines().find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().expect("Content-Length"))
                }).unwrap_or(0);
                break (index + 4, length);
            }
        };
        while bytes.len() < header_end + length {
            let read = socket.read(&mut chunk).expect("read request body");
            assert!(read > 0, "request body truncated");
            bytes.extend_from_slice(&chunk[..read]);
        }
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Type: {media_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        ).expect("write response header");
        socket.write_all(&response).expect("write response body");
        String::from_utf8(bytes).expect("UTF-8 request")
    });
    (endpoint, server)
}

fn request_parts(request: &str) -> (&str, Value) {
    let (headers, payload) = request.split_once("\r\n\r\n").expect("HTTP framing");
    (
        headers,
        serde_json::from_str(payload).expect("JSON request body"),
    )
}

#[tokio::test]
async fn serializes_json_request_and_deserializes_success_response() {
    let (url, server) = mock_once(
        "201 Created", "application/json",
        br#"{"id":"n-1","title":"First note","subtitle":null,"priority":3}"#,
    );
    let client = NotebookClient::new("local-test").with_base_url(url);
    let response = client.notes().create(sdk::CreateNotesRequest::new("First note"))
        .await.expect("create note");
    assert_eq!(response.raw().id, "n-1");
    assert_eq!(response.raw().title, "First note");
    let (headers, body) = request_parts(&server.join().expect("mock request"));
    assert!(headers.starts_with("POST /notes HTTP/1.1"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("content-type: application/json"));
    assert_eq!(body["title"], "First note");
    assert!(body.get("subtitle").is_none() || body["subtitle"].is_null());
    assert!(body.get("priority").is_none(), "optional field should be absent");
}

#[tokio::test]
async fn serializes_path_and_query_and_preserves_null_response() {
    let (url, server) = mock_once(
        "200 OK", "application/json",
        br#"{"id":"n-2","title":"Second","subtitle":null}"#,
    );
    let client = NotebookClient::new("local-test").with_base_url(url);
    let request = sdk::notes::ReadNotesRequest::new("n-2").verbose(true);
    let note = client.notes().read(request).await.expect("read note");
    assert_eq!(note.raw().id, "n-2");
    assert_eq!(note.raw().title, "Second");
    let request = server.join().expect("mock request");
    assert!(request.starts_with("GET /notes/n-2?verbose=true HTTP/1.1"), "{request}");
}

#[tokio::test]
async fn exposes_http_error_status_and_body() {
    let problem = br#"{"code":"not_found","message":"note absent"}"#;
    let (url, server) = mock_once("404 Not Found", "application/json", problem);
    let client = NotebookClient::new("local-test").with_base_url(url);
    let error = client.notes().delete("missing").await.expect_err("404 must remain an error");
    match error {
        SdkError::Api { status, body, .. } => {
            assert_eq!(status, 404);
            assert_eq!(serde_json::from_str::<Value>(&body).expect("error JSON")["code"], "not_found");
        }
        other => panic!("expected API error, got {other:?}"),
    }
    let request = server.join().expect("mock request");
    assert!(request.starts_with("DELETE /notes/missing HTTP/1.1"), "{request}");
}

#[tokio::test]
async fn exercises_buffered_and_streaming_binary_contracts() {
    let payload = b"independent binary export";
    let (url, server) = mock_once("200 OK", "application/octet-stream", payload);
    let client = NotebookClient::new("local-test").with_base_url(url);
    let body = client.notes().export("n-1").await.expect("buffered download");
    assert_eq!(body.as_ref(), payload);
    let request = server.join().expect("buffered request");
    assert!(request.starts_with("GET /notes/n-1/export HTTP/1.1"), "{request}");

    let (url, server) = mock_once("200 OK", "application/octet-stream", payload);
    let client = NotebookClient::new("local-test").with_base_url(url);
    let mut stream = client.notes().export_stream("n-1").await.expect("stream download");
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.expect("binary chunk"));
    }
    assert_eq!(bytes, payload);
    let request = server.join().expect("stream request");
    assert!(request.starts_with("GET /notes/n-1/export HTTP/1.1"), "{request}");
}
