//! JSON-RPC 2.0 over a pair of pipes, each message framed by a `Content-Length` header as the
//! Language Server Protocol frames it. Replies are matched to their calls by id; notifications
//! are handed to the caller in order; requests from the other side are refused, since FastCapture
//! offers it nothing to call. Everything is read on a thread of its own, so the other side never
//! blocks on a full pipe.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The JSON-RPC code for a method the other side doesn't have.
pub const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Clone, Debug, PartialEq)]
pub enum RpcError {
    /// The other side ended, or its output can't be read any more.
    Closed,
    /// No answer in time.
    TimedOut,
    /// It answered with an error.
    Remote { code: i64, message: String },
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("the connection closed"),
            Self::TimedOut => f.write_str("no answer in time"),
            Self::Remote { code, message } => write!(f, "{message} ({code})"),
        }
    }
}

/// One message, framed.
pub fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// Splits a stream of bytes into messages, however the bytes arrive.
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole message, if one has arrived. A message that isn't framed or isn't JSON is
    /// an error, after which the stream can't be trusted.
    pub fn message(&mut self) -> Option<Result<Value, String>> {
        let end = self.buffer.windows(4).position(|w| w == b"\r\n\r\n")?;
        let header = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
        let length = header.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        });
        let Some(length) = length else {
            return Some(Err(format!("a message without a length: {header:?}")));
        };
        let start = end + 4;
        if self.buffer.len() < start + length {
            return None;
        }
        let body: Vec<u8> = self.buffer.drain(..start + length).skip(start).collect();
        Some(
            serde_json::from_slice(&body)
                .map_err(|error| format!("a message that isn't JSON: {error}")),
        )
    }
}

type Waiting = Arc<Mutex<HashMap<u64, Sender<Result<Value, RpcError>>>>>;

/// A connection to the other side of `reader` and `writer`.
pub struct Connection {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    waiting: Waiting,
    /// Set once the other side has ended.
    closed: Arc<AtomicBool>,
    notifications: Receiver<Value>,
    next_id: AtomicU64,
}

impl Connection {
    pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(writer)));
        let waiting: Waiting = Arc::default();
        let (notify, notifications) = mpsc::channel();
        let reply_writer = Arc::clone(&writer);
        let replies = Arc::clone(&waiting);
        let closed = Arc::new(AtomicBool::new(false));
        let ended = Arc::clone(&closed);
        std::thread::spawn(move || {
            read(reader, &replies, &notify, &reply_writer);
            // Set before the calls still waiting are let go, so a call made after this sees it.
            ended.store(true, Ordering::SeqCst);
            if let Ok(mut waiting) = replies.lock() {
                waiting.clear();
            }
        });
        Self {
            writer,
            waiting,
            closed,
            notifications,
            next_id: AtomicU64::new(1),
        }
    }

    /// Calls `method` and waits up to `timeout` for its result.
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = mpsc::channel();
        self.waiting
            .lock()
            .map_err(|_| RpcError::Closed)?
            .insert(id, answer);
        if self.closed.load(Ordering::SeqCst) {
            self.forget(id);
            return Err(RpcError::Closed);
        }
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if !self.send(&message) {
            self.forget(id);
            return Err(RpcError::Closed);
        }
        match answered.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.forget(id);
                Err(RpcError::TimedOut)
            }
            Err(RecvTimeoutError::Disconnected) => Err(RpcError::Closed),
        }
    }

    /// The next notification (`{"method", "params"}`), waiting up to `timeout` for one.
    pub fn notification(&self, timeout: Duration) -> Result<Value, RpcError> {
        self.notifications
            .recv_timeout(timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => RpcError::TimedOut,
                RecvTimeoutError::Disconnected => RpcError::Closed,
            })
    }

    fn send(&self, message: &Value) -> bool {
        write_message(&self.writer, message)
    }

    fn forget(&self, id: u64) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(&id);
        }
    }
}

fn write_message(writer: &Mutex<Box<dyn Write + Send>>, message: &Value) -> bool {
    let Ok(mut writer) = writer.lock() else {
        return false;
    };
    writer
        .write_all(&frame(message))
        .and_then(|()| writer.flush())
        .is_ok()
}

/// Reads until the other side ends, handing each message where it goes.
fn read(
    mut reader: impl Read,
    waiting: &Waiting,
    notify: &Sender<Value>,
    writer: &Mutex<Box<dyn Write + Send>>,
) {
    let mut decoder = Decoder::default();
    let mut chunk = vec![0; 64 * 1024];
    'reading: loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        decoder.push(&chunk[..read]);
        while let Some(message) = decoder.message() {
            let Ok(message) = message else {
                break 'reading;
            };
            let method = message.get("method").and_then(Value::as_str);
            match (method, message.get("id")) {
                (Some(method), Some(id)) => {
                    let refusal = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": METHOD_NOT_FOUND, "message": format!("FastCapture doesn't handle {method}")},
                    });
                    write_message(writer, &refusal);
                }
                (Some(_), None) => {
                    let _ = notify.send(message);
                }
                (None, Some(id)) => {
                    let Some(id) = id.as_u64() else { continue };
                    let answer = waiting
                        .lock()
                        .ok()
                        .and_then(|mut waiting| waiting.remove(&id));
                    if let Some(answer) = answer {
                        let _ = answer.send(match message.get("error") {
                            Some(error) => Err(RpcError::Remote {
                                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                                message: error
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("an error")
                                    .to_owned(),
                            }),
                            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        });
                    }
                }
                (None, None) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{PipeReader, PipeWriter};

    #[test]
    fn messages_are_framed_and_read_back_however_they_arrive() {
        let first = json!({"jsonrpc": "2.0", "id": 1, "result": {"text": "café ☕"}});
        let second = json!({"jsonrpc": "2.0", "method": "session.event", "params": {}});
        let mut bytes = frame(&first);
        assert!(bytes.starts_with(b"Content-Length: "));
        bytes.extend(frame(&second));

        // Both at once.
        let mut decoder = Decoder::default();
        decoder.push(&bytes);
        assert_eq!(decoder.message(), Some(Ok(first.clone())));
        assert_eq!(decoder.message(), Some(Ok(second.clone())));
        assert_eq!(decoder.message(), None);

        // A byte at a time, splitting the header, the body and the multi-byte characters.
        let mut decoder = Decoder::default();
        let mut got = Vec::new();
        for byte in &bytes {
            decoder.push(std::slice::from_ref(byte));
            while let Some(message) = decoder.message() {
                got.push(message.unwrap());
            }
        }
        assert_eq!(got, [first, second]);
    }

    #[test]
    fn a_header_is_read_in_any_case_and_a_message_without_one_is_an_error() {
        let mut decoder = Decoder::default();
        decoder.push(b"content-length: 2\r\nContent-Type: application/json\r\n\r\n{}");
        assert_eq!(decoder.message(), Some(Ok(json!({}))));
        decoder.push(b"Hello\r\n\r\n{}");
        assert!(
            decoder
                .message()
                .unwrap()
                .unwrap_err()
                .contains("without a length")
        );
        let mut decoder = Decoder::default();
        decoder.push(b"Content-Length: 3\r\n\r\nnot");
        assert!(
            decoder
                .message()
                .unwrap()
                .unwrap_err()
                .contains("isn't JSON")
        );
    }

    /// The other side of a connection, played by the test.
    struct Peer {
        from_us: PipeReader,
        to_us: Option<PipeWriter>,
        decoder: Decoder,
    }

    impl Peer {
        fn connect() -> (Connection, Self) {
            let (from_us, our_writer) = std::io::pipe().unwrap();
            let (our_reader, to_us) = std::io::pipe().unwrap();
            let connection = Connection::new(our_reader, our_writer);
            let peer = Self {
                from_us,
                to_us: Some(to_us),
                decoder: Decoder::default(),
            };
            (connection, peer)
        }

        fn receive(&mut self) -> Value {
            let mut chunk = [0; 4096];
            loop {
                if let Some(message) = self.decoder.message() {
                    return message.unwrap();
                }
                let read = self.from_us.read(&mut chunk).unwrap();
                assert!(read > 0, "the connection ended");
                self.decoder.push(&chunk[..read]);
            }
        }

        fn send(&mut self, message: Value) {
            let to_us = self.to_us.as_mut().unwrap();
            to_us.write_all(&frame(&message)).unwrap();
        }
    }

    const SECOND: Duration = Duration::from_secs(5);

    #[test]
    fn a_call_gets_its_own_reply_and_notifications_come_in_order() {
        let (connection, mut peer) = Peer::connect();
        let server = std::thread::spawn(move || {
            let request = peer.receive();
            assert_eq!(request["jsonrpc"], "2.0");
            assert_eq!(request["method"], "ping");
            assert_eq!(request["params"], json!({"message": "hi"}));
            peer.send(json!({"jsonrpc": "2.0", "method": "session.event", "params": {"n": 1}}));
            // A reply to something nobody asked is ignored.
            peer.send(json!({"jsonrpc": "2.0", "id": 999, "result": {}}));
            peer.send(
                json!({"jsonrpc": "2.0", "id": request["id"], "result": {"message": "pong"}}),
            );
            peer.send(json!({"jsonrpc": "2.0", "method": "session.event", "params": {"n": 2}}));
            let next = peer.receive();
            assert_eq!(next["method"], "nope");
            assert_ne!(next["id"], request["id"], "each call has its own id");
            peer.send(json!({"jsonrpc": "2.0", "id": next["id"], "error": {"code": METHOD_NOT_FOUND, "message": "Unhandled method nope"}}));
            peer
        });
        assert_eq!(
            connection.call("ping", json!({"message": "hi"}), SECOND),
            Ok(json!({"message": "pong"}))
        );
        assert_eq!(connection.notification(SECOND).unwrap()["params"]["n"], 1);
        assert_eq!(connection.notification(SECOND).unwrap()["params"]["n"], 2);
        assert_eq!(
            connection.call("nope", json!({}), SECOND),
            Err(RpcError::Remote {
                code: METHOD_NOT_FOUND,
                message: "Unhandled method nope".to_owned()
            })
        );
        assert_eq!(
            connection.notification(Duration::from_millis(50)),
            Err(RpcError::TimedOut)
        );
        server.join().unwrap();
    }

    #[test]
    fn a_request_from_the_other_side_is_refused() {
        let (_connection, mut peer) = Peer::connect();
        peer.send(
            json!({"jsonrpc": "2.0", "id": "q1", "method": "userInput.request", "params": {}}),
        );
        let refusal = peer.receive();
        assert_eq!(refusal["id"], "q1");
        assert_eq!(refusal["error"]["code"], METHOD_NOT_FOUND);
        assert!(
            refusal["error"]["message"]
                .as_str()
                .unwrap()
                .contains("userInput.request")
        );
    }

    #[test]
    fn a_call_times_out_and_the_end_of_the_other_side_closes_everything() {
        let (connection, mut peer) = Peer::connect();
        assert_eq!(
            connection.call("slow", json!({}), Duration::from_millis(100)),
            Err(RpcError::TimedOut)
        );
        let _ = peer.receive();
        let waiting = std::thread::spawn(move || {
            let result = connection.call("never", json!({}), SECOND);
            (result, connection.notification(SECOND))
        });
        let _ = peer.receive();
        peer.to_us = None;
        let (result, notification) = waiting.join().unwrap();
        assert_eq!(result, Err(RpcError::Closed));
        assert_eq!(notification, Err(RpcError::Closed));
    }
}
