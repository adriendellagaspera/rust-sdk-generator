//! The methods under test come directly from the immutable upstream checkout.
//! Neither this consumer nor its runtime depends on the generator or adapter.
use call_shape_matrix_consumer::generated::client::HttpClient;
use call_shape_matrix_consumer::generated::types::{NewRecord, UploadRequest};
use futures_util::StreamExt;
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
    let listener = TcpListener::bind("127.0.0.1:0").expect("local HTTP listener");
    let url = format!("http://{}", listener.local_addr().expect("mock port"));
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("request");
        socket.set_read_timeout(Some(Duration::from_secs(10))).expect("read timeout");
        let mut data = Vec::new();
        let mut chunk = [0u8; 8192];
        let (head_end, length, chunked) = loop {
            let received = socket.read(&mut chunk).expect("read headers");
            assert!(received > 0, "connection closed before headers");
            data.extend_from_slice(&chunk[..received]);
            if let Some(index) = data.windows(4).position(|part| part == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&data[..index]).to_ascii_lowercase();
                let length = head.lines().find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().expect("Content-Length"))
                });
                let chunked = head.contains("transfer-encoding: chunked");
                break (index + 4, length, chunked);
            }
        };
        if let Some(length) = length {
            while data.len() < head_end + length {
                let received = socket.read(&mut chunk).expect("request body");
                assert!(received > 0, "request body truncated");
                data.extend_from_slice(&chunk[..received]);
            }
        } else if chunked {
            // Reqwest's streamed multipart form is chunked; wait for the final
            // zero-size chunk and terminating CRLF before replying.
            while !data[head_end..].ends_with(b"\r\n0\r\n\r\n")
                && !data[head_end..].ends_with(b"0\r\n\r\n")
            {
                let received = socket.read(&mut chunk).expect("chunked request body");
                assert!(received > 0, "chunked request ended prematurely");
                data.extend_from_slice(&chunk[..received]);
            }
        }
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len(),
        ).expect("write response headers");
        socket.write_all(payload).expect("write response body");
        data
    });
    (url, handle)
}

fn captured(handle: thread::JoinHandle<Vec<u8>>) -> (String, Vec<u8>) {
    let bytes = handle.join().expect("mock server");
    let index = bytes.windows(4).position(|part| part == b"\r\n\r\n").expect("HTTP framing");
    (
        String::from_utf8(bytes[..index].to_vec()).expect("HTTP header UTF-8"),
        bytes[index + 4..].to_vec(),
    )
}

fn header(headers: &str, name: &str) -> Option<String> {
    headers.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
    })
}

#[tokio::test]
async fn json_path_query_header_request_body_and_status() {
    let (url, server) = mock_once(
        "200 OK",
        "application/json",
        br#"{"id":"rec-1","title":"Evidence","note":null}"#,
    );
    let client = HttpClient::new().with_base_url(url).with_api_key("local-key");
    let result = client.fetch_record("rec-1", Some(true), "trace-1")
        .await.expect("get JSON response");
    assert_eq!(result.id, "rec-1");
    assert_eq!(result.title, "Evidence");
    let (headers, body) = captured(server);
    assert!(headers.starts_with("GET /records/rec-1?verbose=true HTTP/1.1"), "{headers}");
    assert_eq!(header(&headers, "X-Trace").as_deref(), Some("trace-1"));
    assert_eq!(header(&headers, "Accept").as_deref(), Some("application/json"));
    assert!(body.is_empty(), "GET should have no body");

    let (url, server) = mock_once(
        "201 Created",
        "application/json",
        br#"{"id":"rec-2","title":"Created"}"#,
    );
    let client = HttpClient::new().with_base_url(url);
    let request: NewRecord = serde_json::from_value(json!({"title":"Created","priority":3}))
        .expect("generated new-record model");
    let response = client.create_record(request).await.expect("create JSON response");
    assert_eq!(response.id, "rec-2");
    let (headers, body) = captured(server);
    assert!(headers.starts_with("POST /records HTTP/1.1"), "{headers}");
    assert_eq!(header(&headers, "Content-Type").as_deref(), Some("application/json"));
    assert_eq!(header(&headers, "Accept").as_deref(), Some("application/json"));
    let body: Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(body["title"], "Created");
    assert_eq!(body["priority"], 3);
    assert!(body.get("note").is_none(), "unset optional nullable field must be absent");
}

#[tokio::test]
async fn plain_text_and_buffered_non_utf8_binary() {
    let (url, server) = mock_once("200 OK", "text/plain", b"plain response");
    let text = HttpClient::new().with_base_url(url).fetch_text()
        .await.expect("text response");
    assert_eq!(text, "plain response");
    let (headers, body) = captured(server);
    assert!(headers.starts_with("GET /message HTTP/1.1"), "{headers}");
    assert_eq!(header(&headers, "Accept").as_deref(), Some("text/plain"));
    assert!(body.is_empty());

    let payload = b"\x00\xff\x80binary";
    let (url, server) = mock_once("200 OK", "application/octet-stream", payload);
    let bytes = HttpClient::new().with_base_url(url).fetch_binary()
        .await.expect("binary response");
    assert_eq!(bytes.as_ref(), payload);
    let (headers, body) = captured(server);
    assert!(headers.starts_with("GET /download HTTP/1.1"), "{headers}");
    assert_eq!(header(&headers, "Accept").as_deref(), Some("application/octet-stream"));
    assert!(body.is_empty());
}

#[tokio::test]
async fn empty_success_and_detailed_error() {
    let (url, server) = mock_once("204 No Content", "application/json", b"");
    let result = HttpClient::new().with_base_url(url).delete_record("gone")
        .await.expect("204 empty response");
    assert_eq!(result, ());
    let (headers, _) = captured(server);
    assert!(headers.starts_with("DELETE /records/gone HTTP/1.1"), "{headers}");

    let (url, server) = mock_once(
        "404 Not Found",
        "application/json",
        br#"{"code":"missing","message":"no such record"}"#,
    );
    let error = HttpClient::new().with_base_url(url).fetch_record("absent", None, "trace-2")
        .await.expect_err("404 must remain an API error");
    let diagnostic = format!("{error:?}");
    assert!(diagnostic.contains("404") || diagnostic.contains("missing"), "{diagnostic}");
    let (headers, _) = captured(server);
    assert!(headers.starts_with("GET /records/absent HTTP/1.1"), "{headers}");
    assert!(!headers.starts_with("GET /records/absent?"), "None query must be omitted");
}

#[tokio::test]
async fn multipart_fields_and_binary_part() {
    let (url, server) = mock_once("204 No Content", "application/json", b"");
    let upload: UploadRequest = serde_json::from_value(json!({
        "file": "Zml4dHVyZS1maWxl",
        "caption": "matrix fixture"
    })).expect("generated multipart model");
    HttpClient::new().with_base_url(url).upload_file(upload)
        .await.expect("multipart upload");
    let (headers, body) = captured(server);
    assert!(headers.starts_with("POST /upload HTTP/1.1"), "{headers}");
    assert!(
        header(&headers, "Content-Type").is_some_and(|value| value.starts_with("multipart/form-data; boundary=")),
        "{headers}"
    );
    let body = String::from_utf8(body).expect("ASCII multipart fixture");
    assert!(body.contains("name=\"file\""), "{body}");
    assert!(body.contains("fixture-file"), "{body}");
    assert!(body.contains("name=\"caption\""), "{body}");
    assert!(body.contains("matrix fixture"), "{body}");
}
