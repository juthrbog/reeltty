//! A tiny HTTP server for tests.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub target: String,
    /// Names are lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

pub struct Reply {
    status: u16,
    body: String,
}

pub fn reply(status: u16, body: impl Into<String>) -> Reply {
    Reply { status, body: body.into() }
}

pub type Log = Arc<Mutex<Vec<Request>>>;

/// A tiny HTTP/1.1 server. It records every request and answers with
/// `handler`. The connection closes after each reply.
pub async fn serve(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let handler = Arc::new(handler);
    let server_log = log.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let handler = handler.clone();
            let log = server_log.clone();
            tokio::spawn(async move {
                let request = read_request(&mut stream).await;
                let reply = handler(&request);
                log.lock().unwrap().push(request);
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reply.body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(reply.body.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            });
        }
    });
    (base_url, log)
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Request {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before the headers ended");
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap().split(' ');
    let method = start.next().unwrap().to_string();
    let target = start.next().unwrap().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers.iter().find(|(n, _)| n == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before the body ended");
        body.extend_from_slice(&chunk[..n]);
    }
    Request { method, target, headers, body: String::from_utf8(body).unwrap() }
}
