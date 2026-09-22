//! HTTP behavioral checks for *fork-only* shapes. Ordinary pinned upstream
//! produces neither the stream variant nor the multipart filename helper.
use capability_matrix_consumer::generated::client::{ApiOpError, HttpClient};
use capability_matrix_consumer::generated::types::UploadRequest;
use futures_util::StreamExt;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn respond(status: &'static str, media_type: &'static str, response: &'static [u8])
    -> (String, thread::JoinHandle<(String, Vec<u8>)>)
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener");
    let url = format!("http://{}", listener.local_addr().expect("listener address"));
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept request");
        socket.set_read_timeout(Some(std::time::Duration::from_secs(10))).expect("read timeout");
        let mut request = Vec::new();
        let mut buf = [0u8; 8192];
        let (header_end, length, chunked) = loop {
            let n = socket.read(&mut buf).expect("read request");
            assert!(n > 0, "request headers truncated");
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..pos]).to_ascii_lowercase();
                let length = headers.lines().find_map(|line| line.strip_prefix("content-length:"))
                    .map(|number| number.trim().parse::<usize>().expect("content length"));
                break (pos + 4, length, headers.contains("transfer-encoding: chunked"));
            }
        };
        if let Some(length) = length {
            while request.len() < header_end + length {
                let n = socket.read(&mut buf).expect("read body");
                assert!(n > 0, "request body truncated");
                request.extend_from_slice(&buf[..n]);
            }
        } else if chunked {
            while !request[header_end..].ends_with(b"0\r\n\r\n") {
                let n = socket.read(&mut buf).expect("read chunked body");
                assert!(n > 0, "chunked body truncated");
                request.extend_from_slice(&buf[..n]);
            }
        }
        write!(socket, "HTTP/1.1 {status}\r\nContent-Type: {media_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).expect("response header");
        socket.write_all(response).expect("response body");
        let headers = String::from_utf8(request[..header_end].to_vec()).expect("request headers UTF-8");
        (headers, request[header_end..].to_vec())
    });
    (url, server)
}

#[tokio::test]
async fn emitted_binary_stream_transfers_non_utf8_bytes_and_handles_errors() {
    let binary: &'static [u8] = b"\xff\x00\x80stream-data";
    let (url, server) = respond("200 OK", "application/octet-stream", binary);
    let mut stream = HttpClient::new().with_base_url(url)
        .download_blob_stream().await.expect("fork stream variant");
    let mut result = Vec::new();
    while let Some(chunk) = stream.next().await {
        result.extend_from_slice(&chunk.expect("binary stream chunk"));
    }
    assert_eq!(result, binary);
    let (headers, body) = server.join().expect("mock server");
    assert!(headers.starts_with("GET /blob HTTP/1.1"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("accept: application/octet-stream"), "{headers}");
    assert!(body.is_empty());

    let (url, server) = respond("404 Not Found", "application/json", br#"{"code":"absent","message":"missing"}"#);
    let error = HttpClient::new().with_base_url(url)
        .download_blob_stream().await.expect_err("non-2xx binary stream is error");
    match error {
        ApiOpError::Api(api) => {
            assert_eq!(api.status, 404);
            assert!(api.body.contains("absent"), "error body: {}", api.body);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    let (headers, _) = server.join().expect("mock server");
    assert!(headers.starts_with("GET /blob HTTP/1.1"), "{headers}");
}

#[tokio::test]
async fn emitted_filename_helper_sets_exact_file_name_and_multipart_fields() {
    let (url, server) = respond("201 Created", "application/json", br#"{"id":"uploaded","title":"OK"}"#);
    let upload: UploadRequest = serde_json::from_value(json!({
        "file": "filename-content",
        "caption": "caption-value"
    })).expect("typed multipart request");
    let result = HttpClient::new().with_base_url(url)
        .upload_file_with_multipart_filenames(upload, &[("file", "evidence.bin")])
        .await.expect("emitted filename helper");
    assert_eq!(result.id, "uploaded");
    let (headers, body) = server.join().expect("mock server");
    assert!(headers.starts_with("POST /upload HTTP/1.1"), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("content-type: multipart/form-data; boundary="), "{headers}");
    assert!(headers.to_ascii_lowercase().contains("accept: application/json"), "{headers}");
    let body = String::from_utf8(body).expect("ASCII multipart");
    assert!(body.contains("name=\"file\"; filename=\"evidence.bin\""), "{body}");
    assert!(body.contains("filename-content"), "{body}");
    assert!(body.contains("name=\"caption\""), "{body}");
    assert!(body.contains("caption-value"), "{body}");
}
