//! Local model servers: finding Ollama, LM Studio and Foundry Local on this PC, making sure a
//! Local-mode endpoint really is on this PC, and finding out whether a model reads images.

use super::openai::{self, normalize};
use crate::net::http::{Client, Request, parse_url};
use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A server found on this PC, with the models it lists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Found {
    pub name: &'static str,
    pub endpoint: String,
    pub models: Vec<String>,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Whether `endpoint` is on this PC: `localhost`, a loopback address, or a name that resolves
/// only to loopback addresses. Local mode refuses anything else, so a remote server can't be
/// used by mistake while the setting says Local.
pub fn is_on_this_pc(endpoint: &str) -> bool {
    let Ok(target) = parse_url(&normalize(endpoint)) else {
        return false;
    };
    if target.host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(address) = target.host.parse::<IpAddr>() {
        return address.is_loopback();
    }
    match (target.host.as_str(), target.port).to_socket_addrs() {
        Ok(addresses) => {
            let addresses: Vec<_> = addresses.collect();
            !addresses.is_empty() && addresses.iter().all(|address| address.ip().is_loopback())
        }
        Err(_) => false,
    }
}

/// The servers answering on this PC, in a fixed order, each with its models. Takes a few
/// seconds at most; run it off the UI thread.
pub fn find_servers() -> Vec<Found> {
    let mut candidates = vec![
        ("Ollama", "http://localhost:11434".to_owned()),
        ("LM Studio", "http://localhost:1234".to_owned()),
    ];
    if let Some(endpoint) = foundry_endpoint() {
        candidates.push(("Foundry Local", endpoint));
    }
    candidates
        .into_iter()
        .filter_map(|(name, endpoint)| {
            let models = openai::list_models(&endpoint, PROBE_TIMEOUT).ok()?;
            Some(Found {
                name,
                endpoint,
                models,
            })
        })
        .collect()
}

/// Foundry Local's address, from `foundry service status`, when the `foundry` command exists.
fn foundry_endpoint() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = std::process::Command::new("foundry")
        .args(["service", "status"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let mut output = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut output).ok()?;
    first_http_origin(&output)
}

/// The first `http://host:port` in `text`, without its path.
fn first_http_origin(text: &str) -> Option<String> {
    let start = text.find("http://")?;
    let url: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
        .collect();
    let target = parse_url(&url).ok()?;
    Some(format!("http://{}:{}", target.host, target.port))
}

/// What the vision probe found, per endpoint and model, for this session.
static VISION: Mutex<Option<HashMap<(String, String), bool>>> = Mutex::new(None);

/// Whether `model` at `endpoint` reads images: a one-pixel image is sent once and the answer
/// kept for the session. `None` when the server couldn't be asked.
pub fn reads_images(endpoint: &str, model: &str) -> Option<bool> {
    let key = (normalize(endpoint), model.to_owned());
    if let Some(known) = VISION
        .lock()
        .ok()?
        .as_ref()
        .and_then(|cache| cache.get(&key).copied())
    {
        return Some(known);
    }
    let pixel = crate::platform::png::encode_png(&crate::platform::png::Pixels {
        width: 1,
        height: 1,
        bgra: vec![255, 255, 255, 255],
    })
    .ok()?;
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 5,
        "stream": false,
        "messages": [{"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", super::images::base64(&pixel))}},
            {"type": "text", "text": "Reply with one word: the image's colour."}
        ]}]
    })
    .to_string();
    let client = Client::new("FastCapture").ok()?;
    let response = client
        .send(&Request {
            method: "POST",
            url: &format!("{}/v1/chat/completions", key.0),
            headers: vec![("content-type", "application/json".to_owned())],
            body: body.as_bytes(),
            timeout: Duration::from_secs(60),
        })
        .ok()?;
    let answer = match response.status {
        200 => true,
        400 | 404 | 422 | 500 => {
            let message = super::backend::error_message(&response).to_lowercase();
            if ["image", "vision", "multimodal", "multi-modal"]
                .iter()
                .any(|word| message.contains(word))
            {
                false
            } else {
                return None;
            }
        }
        _ => return None,
    };
    VISION
        .lock()
        .ok()?
        .get_or_insert_with(HashMap::new)
        .insert(key, answer);
    Some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::test_server::{Scripted, TestServer};

    #[test]
    fn only_this_pc_counts_as_local() {
        assert!(is_on_this_pc("http://localhost:11434"));
        assert!(is_on_this_pc("http://LOCALHOST:1234/v1"));
        assert!(is_on_this_pc("http://127.0.0.1:1234"));
        assert!(is_on_this_pc("http://127.8.9.10:1234"));
        assert!(is_on_this_pc("http://[::1]:1234"));
        assert!(!is_on_this_pc("http://192.168.1.5:11434"));
        assert!(!is_on_this_pc("https://api.openai.com"));
        assert!(!is_on_this_pc("http://no-such-host.invalid:1234"));
        assert!(!is_on_this_pc("localhost:11434"), "not a URL");
    }

    #[test]
    fn foundrys_address_is_read_from_its_status_text() {
        assert_eq!(
            first_http_origin(
                "🟢 Model management service is running on http://127.0.0.1:5273/openai/status\n"
            ),
            Some("http://127.0.0.1:5273".to_owned())
        );
        assert_eq!(first_http_origin("service is not running"), None);
    }

    #[test]
    fn the_vision_probe_reads_the_answer_and_remembers_it() {
        let server = TestServer::start(vec![
            Scripted::json(
                200,
                r#"{"choices":[{"message":{"content":"white"},"finish_reason":"stop"}]}"#,
            ),
            Scripted::json(
                400,
                r#"{"error":{"message":"this model does not support image input"}}"#,
            ),
            Scripted::json(400, r#"{"error":{"message":"bad request"}}"#),
        ]);
        let endpoint = server.url();
        assert_eq!(reads_images(&endpoint, "vision-model"), Some(true));
        assert_eq!(
            reads_images(&endpoint, "vision-model"),
            Some(true),
            "from the cache"
        );
        assert_eq!(reads_images(&endpoint, "text-model"), Some(false));
        assert_eq!(reads_images(&endpoint, "odd-model"), None, "can't tell");
        assert_eq!(server.requests().len(), 3);
        let probe = server.requests()[0].json();
        assert_eq!(probe["model"], "vision-model");
        assert!(
            probe["messages"][0]["content"][0]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
}
