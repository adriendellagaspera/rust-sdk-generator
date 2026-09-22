use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

pub fn respond(status: &'static str, media_type: &'static str, response: &'static [u8])
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

