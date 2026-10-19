//! HTTP over WinHTTP: one blocking request at a time, with the system proxy and Windows' own TLS
//! and certificate store, so FastCapture needs no TLS crate. For the model backends and the
//! pages of captured links; the worker thread owns the clients.

use std::time::Duration;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_CONNECTION_ERROR, ERROR_WINHTTP_NAME_NOT_RESOLVED,
    ERROR_WINHTTP_TIMEOUT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_COOKIES,
    WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS, WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT,
    WINHTTP_OPTION_URL, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RAW_HEADERS_CRLF,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};

/// The largest response body read; past it the request fails rather than filling memory.
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
/// Redirects followed before a request gives up.
pub const MAX_REDIRECTS: u32 = 5;

/// Errors that may pass if the request is tried again later: the network or the server was
/// unreachable, slow or cut off.
const TRANSIENT: [u32; 4] = [
    ERROR_WINHTTP_TIMEOUT,
    ERROR_WINHTTP_CANNOT_CONNECT,
    ERROR_WINHTTP_NAME_NOT_RESOLVED,
    ERROR_WINHTTP_CONNECTION_ERROR,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpError {
    /// The request may succeed later (offline, server not running, timeout).
    Transient(String),
    /// Trying again won't help (a bad URL, a TLS failure, a response too large).
    Permanent(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(message) | Self::Permanent(message) => f.write_str(message),
        }
    }
}

pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    /// Header names and values. Never logged: they may hold keys.
    pub headers: Vec<(&'a str, String)>,
    pub body: &'a [u8],
    /// For connecting, sending and receiving, each. WinHTTP waits a few seconds at least (about
    /// 4 s on loopback, whatever is asked), which only matters to tests.
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    /// Where the response came from, after any redirects.
    pub url: String,
    /// Names as the server wrote them, values trimmed.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// The first header named `name`, matched without regard to case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A URL split for WinHTTP.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

/// Splits an `http://` or `https://` URL. IPv6 hosts are written in brackets.
pub fn parse_url(url: &str) -> Result<Target, HttpError> {
    let bad = || HttpError::Permanent(format!("not an http or https address: {url}"));
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(bad());
    };
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(split);
    if authority.is_empty() || authority.contains('@') {
        return Err(bad());
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(inner) => {
            let (host, after) = inner.split_once(']').ok_or_else(bad)?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let port = match port {
        Some(port) => port.parse::<u16>().map_err(|_| bad())?,
        None if secure => 443,
        None => 80,
    };
    if host.is_empty() {
        return Err(bad());
    }
    let path = if path.is_empty() {
        "/".to_owned()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_owned()
    };
    Ok(Target {
        secure,
        host: host.to_owned(),
        port,
        path,
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A WinHTTP handle, closed on drop.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn failed(what: &str) -> HttpError {
    let code = unsafe { GetLastError() };
    let message = format!("{what} failed (WinHTTP error {code})");
    if TRANSIENT.contains(&code) {
        HttpError::Transient(message)
    } else {
        HttpError::Permanent(message)
    }
}

/// A WinHTTP session. Handles are safe to use from any thread, one request at a time here.
pub struct Client {
    session: Handle,
}

// SAFETY: a WinHTTP session handle may be used from any thread.
unsafe impl Send for Client {}

impl Client {
    pub fn new(user_agent: &str) -> Result<Self, HttpError> {
        let agent = wide(user_agent);
        let session = unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        };
        if session.is_null() {
            return Err(failed("WinHttpOpen"));
        }
        Ok(Self {
            session: Handle(session),
        })
    }

    /// Sends `request` and reads the whole response.
    pub fn send(&self, request: &Request<'_>) -> Result<Response, HttpError> {
        self.send_limited(request, MAX_RESPONSE_BYTES)
    }

    /// Sends `request` and reads the response, failing when its body is over `max_body` bytes.
    /// Redirects are followed, `MAX_REDIRECTS` at most; cookies are never stored or sent.
    pub fn send_limited(
        &self,
        request: &Request<'_>,
        max_body: usize,
    ) -> Result<Response, HttpError> {
        let target = parse_url(request.url)?;
        let host = wide(&target.host);
        let connection =
            Handle(unsafe { WinHttpConnect(self.session.0, host.as_ptr(), target.port, 0) });
        if connection.0.is_null() {
            return Err(failed("WinHttpConnect"));
        }
        let method = wide(request.method);
        let path = wide(&target.path);
        let handle = Handle(unsafe {
            WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                if target.secure {
                    WINHTTP_FLAG_SECURE
                } else {
                    0
                },
            )
        });
        if handle.0.is_null() {
            return Err(failed("WinHttpOpenRequest"));
        }
        let millis = request.timeout.as_millis().min(i32::MAX as u128) as i32;
        unsafe { WinHttpSetTimeouts(handle.0, millis, millis, millis, millis) };
        // Waiting for the response headers has a timeout of its own, 90 s by default.
        let wait = millis as u32;
        unsafe {
            WinHttpSetOption(
                handle.0,
                WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT,
                (&raw const wait).cast(),
                size_of::<u32>() as u32,
            )
        };
        let set = |option: u32, value: u32| unsafe {
            WinHttpSetOption(
                handle.0,
                option,
                (&raw const value).cast(),
                size_of::<u32>() as u32,
            )
        };
        set(WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES);
        set(WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS, MAX_REDIRECTS);

        let headers: String = request
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect();
        let headers: Vec<u16> = headers.encode_utf16().collect();
        let sent = unsafe {
            WinHttpSendRequest(
                handle.0,
                if headers.is_empty() {
                    std::ptr::null()
                } else {
                    headers.as_ptr()
                },
                headers.len() as u32,
                request.body.as_ptr().cast(),
                request.body.len() as u32,
                request.body.len() as u32,
                0,
            )
        };
        if sent == 0 {
            return Err(failed("Sending the request"));
        }
        if unsafe { WinHttpReceiveResponse(handle.0, std::ptr::null_mut()) } == 0 {
            return Err(failed("Receiving the response"));
        }

        let mut status: u32 = 0;
        let mut size = size_of::<u32>() as u32;
        let ok = unsafe {
            WinHttpQueryHeaders(
                handle.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&raw mut status).cast(),
                &mut size,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(failed("Reading the status"));
        }
        let headers = raw_headers(handle.0);
        let url = final_url(handle.0).unwrap_or_else(|| request.url.to_owned());

        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if unsafe { WinHttpQueryDataAvailable(handle.0, &mut available) } == 0 {
                return Err(failed("Reading the response"));
            }
            if available == 0 {
                break;
            }
            if body.len() + available as usize > max_body {
                return Err(HttpError::Permanent("the response is too large".to_owned()));
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            let ok = unsafe {
                WinHttpReadData(
                    handle.0,
                    body[start..].as_mut_ptr().cast(),
                    available,
                    &mut read,
                )
            };
            if ok == 0 {
                return Err(failed("Reading the response"));
            }
            body.truncate(start + read as usize);
        }
        Ok(Response {
            status: status as u16,
            url,
            headers,
            body,
        })
    }
}

/// The request's URL as it is now, after redirects.
fn final_url(handle: *mut core::ffi::c_void) -> Option<String> {
    let mut size = 0u32;
    unsafe { WinHttpQueryOption(handle, WINHTTP_OPTION_URL, std::ptr::null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let ok = unsafe {
        WinHttpQueryOption(
            handle,
            WINHTTP_OPTION_URL,
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if ok == 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..size as usize / 2]);
    Some(text.trim_end_matches('\0').to_owned())
}

/// The response's headers as name and value pairs.
fn raw_headers(handle: *mut core::ffi::c_void) -> Vec<(String, String)> {
    let mut size = 0u32;
    unsafe {
        WinHttpQueryHeaders(
            handle,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
        )
    };
    if size == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let ok = unsafe {
        WinHttpQueryHeaders(
            handle,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            std::ptr::null(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    buffer.truncate(size as usize / 2);
    parse_headers(&String::from_utf16_lossy(&buffer))
}

/// `Name: value` lines after the status line.
fn parse_headers(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::test_server::{Scripted, TestServer};

    #[test]
    fn urls_split_into_host_port_and_path() {
        assert_eq!(
            parse_url("https://api.anthropic.com/v1/messages"),
            Ok(Target {
                secure: true,
                host: "api.anthropic.com".into(),
                port: 443,
                path: "/v1/messages".into()
            })
        );
        assert_eq!(
            parse_url("http://localhost:11434"),
            Ok(Target {
                secure: false,
                host: "localhost".into(),
                port: 11434,
                path: "/".into()
            })
        );
        assert_eq!(
            parse_url("http://[::1]:1234/v1/models?x=1").unwrap().host,
            "::1"
        );
        assert_eq!(parse_url("http://h?q").unwrap().path, "/?q");
        for bad in [
            "ftp://x",
            "https://",
            "http://:80",
            "http://a:b",
            "http://u@h/",
            "http://[::1",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn raw_headers_become_pairs() {
        assert_eq!(
            parse_headers(
                "HTTP/1.1 429 Too Many\r\nRetry-After: 7\r\ncontent-type:  application/json \r\n\r\n"
            ),
            [
                ("Retry-After".to_owned(), "7".to_owned()),
                ("content-type".to_owned(), "application/json".to_owned())
            ]
        );
    }

    #[test]
    fn a_post_sends_its_headers_and_body_and_reads_the_reply() {
        let server = TestServer::start(vec![
            Scripted::json(200, r#"{"ok":true}"#).with_header("x-test", "yes"),
        ]);
        let client = Client::new("FastCapture-test").unwrap();
        let response = client
            .send(&Request {
                method: "POST",
                url: &format!("{}/v1/messages", server.url()),
                headers: vec![
                    ("content-type", "application/json".to_owned()),
                    ("x-api-key", "secret".to_owned()),
                ],
                body: br#"{"hello":1}"#,
                timeout: Duration::from_secs(5),
            })
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"ok":true}"#);
        assert_eq!(response.header("X-TEST"), Some("yes"));
        let seen = server.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, "/v1/messages");
        assert_eq!(seen[0].header("x-api-key"), Some("secret"));
        assert_eq!(seen[0].json(), serde_json::json!({"hello": 1}));
    }

    fn redirect(to: &str) -> Scripted {
        Scripted::json(302, "")
            .with_header("location", to)
            .with_header("set-cookie", "session=abc; Path=/")
    }

    fn get(client: &Client, url: &str, max_body: usize) -> Result<Response, HttpError> {
        client.send_limited(
            &Request {
                method: "GET",
                url,
                headers: Vec::new(),
                body: &[],
                timeout: Duration::from_secs(5),
            },
            max_body,
        )
    }

    #[test]
    fn redirects_are_followed_five_times_without_cookies() {
        let client = Client::new("FastCapture-test").unwrap();
        let mut script: Vec<Scripted> = (1..=5).map(|n| redirect(&format!("/r{n}"))).collect();
        script.push(Scripted::json(200, "landed"));
        let server = TestServer::start(script);
        let response = get(
            &client,
            &format!("{}/start", server.url()),
            MAX_RESPONSE_BYTES,
        )
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"landed");
        assert_eq!(response.url, format!("{}/r5", server.url()));
        let seen = server.requests();
        assert_eq!(seen.len(), 6);
        assert!(
            seen.iter()
                .all(|request| request.header("cookie").is_none())
        );

        let server = TestServer::start((1..=6).map(|n| redirect(&format!("/r{n}"))).collect());
        let sixth = get(&client, &server.url(), MAX_RESPONSE_BYTES);
        assert!(
            !matches!(&sixth, Ok(response) if response.status == 200),
            "{sixth:?}"
        );
        assert!(server.requests().len() <= 6);
    }

    #[test]
    fn a_body_over_the_limit_fails() {
        let client = Client::new("FastCapture-test").unwrap();
        let server = TestServer::start(vec![
            Scripted::json(200, &"x".repeat(2000)),
            Scripted::json(200, &"x".repeat(2000)),
        ]);
        assert!(matches!(
            get(&client, &server.url(), 1000),
            Err(HttpError::Permanent(message)) if message.contains("too large")
        ));
        assert_eq!(get(&client, &server.url(), 2000).unwrap().body.len(), 2000);
    }

    #[test]
    fn error_statuses_are_responses_not_errors() {
        let server = TestServer::start(vec![Scripted::json(529, r#"{"type":"error"}"#)]);
        let client = Client::new("FastCapture-test").unwrap();
        let response = client
            .send(&Request {
                method: "GET",
                url: &server.url(),
                headers: Vec::new(),
                body: &[],
                timeout: Duration::from_secs(5),
            })
            .unwrap();
        assert_eq!(response.status, 529);
    }

    #[test]
    fn a_slow_server_times_out_and_a_closed_port_may_pass() {
        let server = TestServer::start(vec![
            Scripted::json(200, "{}").after(Duration::from_secs(8)),
        ]);
        let started = std::time::Instant::now();
        let client = Client::new("FastCapture-test").unwrap();
        let slow = client.send(&Request {
            method: "GET",
            url: &server.url(),
            headers: Vec::new(),
            body: &[],
            timeout: Duration::from_millis(1500),
        });
        assert!(
            started.elapsed() < Duration::from_secs(7),
            "gave up before the reply"
        );
        assert!(matches!(slow, Err(HttpError::Transient(_))), "{slow:?}");

        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let closed = client.send(&Request {
            method: "GET",
            url: &format!("http://127.0.0.1:{port}/"),
            headers: Vec::new(),
            body: &[],
            timeout: Duration::from_secs(2),
        });
        assert!(matches!(closed, Err(HttpError::Transient(_))), "{closed:?}");
    }
}
