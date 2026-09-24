use field_station_sdk::sdk::Client;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

#[tokio::test]
async fn newly_added_delete_operation_reaches_mock_http() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut bytes = [0_u8; 4096];
        let read = socket.read(&mut bytes).unwrap();
        write!(socket, "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        String::from_utf8(bytes[..read].to_vec()).unwrap()
    });
    Client::new("local-test").with_base_url(url)
        .stations().delete_station("s-1").await.expect("DELETE");
    let request = server.join().unwrap();
    assert!(request.starts_with("DELETE /stations/s-1 HTTP/1.1"), "{request}");
}
