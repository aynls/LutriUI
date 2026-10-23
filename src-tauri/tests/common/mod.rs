//! Shared minimal HTTP responder for integration tests. Binds a loopback
//! listener, replays the given (status, body) responses in order, records
//! every request head and JSON payload, and closes each connection.
//!
//! Callers pass response headers themselves. Empty means none, so a suite
//! cannot inherit another provider's billing or retry dialect.
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

pub type Captured = Vec<(String, Value)>;

/// `headers` returns extra header lines for that status: empty, or ending in `\r\n`.
pub fn server(
    responses: Vec<(u16, Value)>,
    headers: impl Fn(u16) -> String + Send + 'static,
) -> (String, thread::JoinHandle<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        for (status, body) in responses {
            let extra = headers(status);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "expected HTTP request did not arrive"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head, length, offset) = loop {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    break (head, length, end + 4);
                }
            };
            while bytes.len() < offset + length {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let payload = if length > 0 {
                serde_json::from_slice(&bytes[offset..offset + length]).unwrap()
            } else {
                Value::Null
            };
            requests.push((head, payload));
            let text = serde_json::to_string(&body).unwrap();
            let len = text.len();
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {len}\r\n{extra}Connection: close\r\n\r\n{text}"
            )
            .unwrap();
        }
        requests
    });
    (url, handle)
}
