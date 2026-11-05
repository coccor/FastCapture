//! For tests and the soak run: an HTTP/1.1 server on `127.0.0.1` that answers each connection
//! with the next scripted response, or always the same one, and records the requests it got.
//! No test reaches the internet.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Scripted {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// How long to wait before answering.
    pub delay: Duration,
    /// When set, the body is written one byte at a time, this far apart, after the headers.
    pub drip: Option<Duration>,
}

impl Scripted {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
            drip: None,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn dripping(mut self, every: Duration) -> Self {
        self.drip = Some(every);
        self
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("the request body is JSON")
    }
}

pub struct TestServer {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

/// How many requests `TestServer::answering` keeps, the latest, so a server answering for days
/// doesn't grow.
const KEPT_WHILE_ANSWERING: usize = 100;

impl TestServer {
    /// Starts answering with `script`, one response per connection, in order. After the script
    /// runs out, connections get a 500.
    pub fn start(script: Vec<Scripted>) -> Self {
        let mut script = script.into_iter();
        Self::serve(usize::MAX, move || {
            script
                .next()
                .unwrap_or_else(|| Scripted::json(500, r#"{"error":"script ran out"}"#))
        })
    }

    /// Starts answering every connection with `reply`, for as long as the process runs. Only
    /// the latest requests are kept.
    pub fn answering(reply: Scripted) -> Self {
        Self::serve(KEPT_WHILE_ANSWERING, move || reply.clone())
    }

    fn serve(keep: usize, mut next: impl FnMut() -> Scripted + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let reply = next();
                if let Some(request) = answer(stream, &reply) {
                    let mut seen = record.lock().unwrap();
                    seen.push(request);
                    let extra = seen.len().saturating_sub(keep);
                    seen.drain(..extra);
                }
            }
        });
        Self { port, seen }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn answer(stream: TcpStream, reply: &Scripted) -> Option<Seen> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;

    std::thread::sleep(reply.delay);
    let mut stream = stream;
    let mut head = format!(
        "HTTP/1.1 {} Scripted\r\ncontent-length: {}\r\nconnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    match reply.drip {
        None => {
            let _ = stream.write_all(&reply.body);
        }
        Some(every) => {
            let _ = stream.flush();
            for byte in &reply.body {
                std::thread::sleep(every);
                if stream.write_all(&[*byte]).is_err() || stream.flush().is_err() {
                    break;
                }
            }
        }
    }
    let _ = stream.flush();
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str) -> String {
        let address = url.strip_prefix("http://").unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(b"GET /x HTTP/1.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn answering_gives_every_request_the_same_reply_and_keeps_the_latest() {
        let server = TestServer::answering(Scripted::json(200, r#"{"ok":true}"#));
        for _ in 0..KEPT_WHILE_ANSWERING + 5 {
            let response = get(&server.url());
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            assert!(response.ends_with(r#"{"ok":true}"#), "{response}");
        }
        assert_eq!(server.requests().len(), KEPT_WHILE_ANSWERING);
    }
}
