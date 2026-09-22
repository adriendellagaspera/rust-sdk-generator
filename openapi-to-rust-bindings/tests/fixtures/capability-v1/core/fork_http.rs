//! HTTP behavioral checks for *fork-only* shapes. Ordinary pinned upstream
//! produces neither the stream variant nor the multipart filename helper.
use capability_matrix_consumer::generated::client::{ApiOpError, HttpClient};
use capability_matrix_consumer::generated::types::UploadRequest;
use futures_util::StreamExt;
use serde_json::json;
mod fork_http_support;
use fork_http_support::respond;

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
    let error = match HttpClient::new().with_base_url(url)
        .download_blob_stream().await {
        Ok(_) => panic!("non-2xx binary stream is error"),
        Err(error) => error,
    };
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
