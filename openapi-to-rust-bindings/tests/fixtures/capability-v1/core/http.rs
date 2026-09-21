//! Behavioral proof over actual methods emitted by the pinned unmodified upstream.
use capability_matrix_consumer::generated::client::{ApiOpError, HttpClient};
use capability_matrix_consumer::generated::types::{UpdateWidgetRequest, UploadRequest};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

fn mock_once(
    status: &'static str,
    content_type: &'static str,
    payload: &'static [u8],
) -> (String, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("local mock");
    let url = format!("http://{}", listener.local_addr().expect("listener port"));
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("request");
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        let (header_end, length, chunked) = loop {
            let size = socket.read(&mut chunk).expect("read headers");
            assert!(size > 0, "request ended before headers");
            bytes.extend_from_slice(&chunk[..size]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                let length = header.lines().find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().expect("content length"))
                });
                break (index + 4, length, header.contains("transfer-encoding: chunked"));
            }
        };
        if let Some(length) = length {
            while bytes.len() < header_end + length {
                let size = socket.read(&mut chunk).expect("read request body");
                assert!(size > 0, "truncated body");
                bytes.extend_from_slice(&chunk[..size]);
            }
        } else if chunked {
            while !bytes[header_end..].ends_with(b"0\r\n\r\n") {
                let size = socket.read(&mut chunk).expect("read chunked request");
                assert!(size > 0, "truncated chunked body");
                bytes.extend_from_slice(&chunk[..size]);
            }
        }
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        ).expect("response header");
        socket.write_all(payload).expect("response body");
        bytes
    });
    (url, server)
}

fn captured(server: thread::JoinHandle<Vec<u8>>) -> (String, Vec<u8>) {
    let request = server.join().expect("mock response");
    let index = request
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .expect("HTTP header separator");
    (
        String::from_utf8(request[..index].to_vec()).expect("HTTP header UTF-8"),
        request[index + 4..].to_vec(),
    )
}

fn header(headers: &str, key: &str) -> Option<String> {
    headers.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(key)
            .then(|| value.trim().to_owned())
    })
}

fn request(tri: Option<Option<&str>>) -> UpdateWidgetRequest {
    let mut request: UpdateWidgetRequest = serde_json::from_value(json!({
        "title": "Updated",
        "nullable-name": null,
        "note": "optional"
    }))
    .expect("generated request model");
    request.tri = tri.map(|value| value.map(str::to_owned));
    request
}

#[tokio::test]
async fn json_path_query_header_and_optional_nullable_tristate_body() {
    for (tri, expected) in [
        (None, None),
        (Some(None), Some(Value::Null)),
        (Some(Some("present")), Some(json!("present"))),
    ] {
        let (url, server) = mock_once(
            "200 OK",
            "application/json",
            br#"{"id":"w-1","title":"Updated"}"#,
        );
        let client = HttpClient::new()
            .with_base_url(url)
            .with_api_key("fixture-api-key");
        let result = client
            .update_widget("w-1", Some(true), "trace-123", request(tri))
            .await
            .expect("JSON success");
        assert_eq!(result.id, "w-1");
        assert_eq!(result.title, "Updated");
        let (headers, body) = captured(server);
        assert!(
            headers.starts_with("POST /widgets/w-1?verbose=true HTTP/1.1"),
            "{headers}"
        );
        assert_eq!(header(&headers, "X-Trace-Id").as_deref(), Some("trace-123"));
        assert_eq!(header(&headers, "Accept").as_deref(), Some("application/json"));
        assert_eq!(header(&headers, "Content-Type").as_deref(), Some("application/json"));
        let body: Value = serde_json::from_slice(&body).expect("JSON body");
        assert_eq!(body["title"], "Updated");
        assert_eq!(body["nullable-name"], Value::Null);
        assert_eq!(body["note"], "optional");
        assert_eq!(body.get("tri").cloned(), expected);
    }
}

#[tokio::test]
async fn text_and_binary_response_decoding() {
    let (url, server) = mock_once("200 OK", "text/plain", b"plain content");
    let result = HttpClient::new()
        .with_base_url(url)
        .read_text()
        .await
        .expect("text response");
    assert_eq!(result, "plain content");
    let (headers, _) = captured(server);
    assert!(headers.starts_with("GET /text HTTP/1.1"), "{headers}");
    assert_eq!(header(&headers, "Accept").as_deref(), Some("text/plain"));

    let payload = b"\xff\x00\x80binary";
    let (url, server) = mock_once("200 OK", "application/octet-stream", payload);
    let result = HttpClient::new()
        .with_base_url(url)
        .download_blob()
        .await
        .expect("buffered binary response");
    assert_eq!(result.to_vec(), payload);
    let (headers, _) = captured(server);
    assert!(headers.starts_with("GET /blob HTTP/1.1"), "{headers}");
    assert_eq!(
        header(&headers, "Accept").as_deref(),
        Some("application/octet-stream")
    );
}

#[tokio::test]
async fn empty_status_and_typed_http_error() {
    let (url, server) = mock_once("204 No Content", "application/json", b"");
    let result = HttpClient::new()
        .with_base_url(url)
        .delete_widget("w-2")
        .await
        .expect("empty success");
    assert_eq!(result, ());
    let (headers, _) = captured(server);
    assert!(headers.starts_with("DELETE /widgets/w-2 HTTP/1.1"), "{headers}");

    let (url, server) = mock_once(
        "400 Bad Request",
        "application/json",
        br#"{"code":"invalid","message":"rejected"}"#,
    );
    let error = HttpClient::new()
        .with_base_url(url)
        .update_widget("w-3", None, "trace-error", request(None))
        .await
        .expect_err("400 must not decode as success");
    match error {
        ApiOpError::Api(api) => {
            assert_eq!(api.status, 400);
            let parsed: Value = serde_json::from_str(&api.body).expect("error body");
            assert_eq!(parsed["code"], "invalid");
        }
        other => panic!("expected API status error: {other:?}"),
    }
    let (headers, _) = captured(server);
    assert!(headers.starts_with("POST /widgets/w-3 HTTP/1.1"), "{headers}");
    assert!(!headers.starts_with("POST /widgets/w-3?"), "None query is omitted");
}

#[tokio::test]
async fn multipart_binary_field_and_scalar_field() {
    let (url, server) = mock_once(
        "201 Created",
        "application/json",
        br#"{"id":"w-upload","title":"Uploaded"}"#,
    );
    let upload: UploadRequest = serde_json::from_value(json!({
        "file": "fixture-file",
        "caption": "metadata"
    }))
    .expect("generated typed upload request");
    let result = HttpClient::new()
        .with_base_url(url)
        .upload_file(upload)
        .await
        .expect("multipart upload");
    assert_eq!(result.id, "w-upload");
    let (headers, body) = captured(server);
    assert!(headers.starts_with("POST /upload HTTP/1.1"), "{headers}");
    assert!(
        header(&headers, "Content-Type")
            .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")),
        "{headers}"
    );
    assert_eq!(header(&headers, "Accept").as_deref(), Some("application/json"));
    let multipart = String::from_utf8(body).expect("ASCII upload body");
    assert!(multipart.contains("name=\"file\""), "{multipart}");
    assert!(multipart.contains("fixture-file"), "{multipart}");
    assert!(multipart.contains("name=\"caption\""), "{multipart}");
    assert!(multipart.contains("metadata"), "{multipart}");
}
