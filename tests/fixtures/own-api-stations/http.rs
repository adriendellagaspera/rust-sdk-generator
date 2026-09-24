//! Independent own-API proof, deliberately unrelated to notebook/Mistral.
use field_station_sdk::sdk::Client;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

fn once(status: &'static str, body: &'static [u8]) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener");
    let url = format!("http://{}", listener.local_addr().expect("local address"));
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("incoming SDK HTTP request");
        socket.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let n = socket.read(&mut chunk).expect("HTTP bytes");
            assert!(n > 0, "request ended before headers");
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.windows(4).any(|part| part == b"\r\n\r\n") { break; }
        }
        write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("HTTP headers");
        socket.write_all(body).expect("HTTP body");
        String::from_utf8(bytes).expect("ASCII HTTP request")
    });
    (url, handle)
}

#[tokio::test]
async fn reads_an_independent_field_station_over_real_http() {
    let (url, server) = once("200 OK", br#"{"id":"s-1","label":"North"}"#);
    let station = Client::new("local-test").with_base_url(url)
        .stations().read_station("s-1").await.expect("generated HTTP call");
    assert_eq!(station.raw().id, "s-1");
    assert_eq!(station.raw().label, "North");
    let request = server.join().expect("HTTP server");
    assert!(request.starts_with("GET /stations/s-1 HTTP/1.1"), "{request}");
}
