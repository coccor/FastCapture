//! A web page for a captured link: one GET as a desktop browser would make it, without cookies,
//! with a short timeout and a size limit, decoded with the page's own character set.

use super::http::{Client, HttpError, Request};
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
/// A desktop browser's, since some sites send others a stripped-down page or none.
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Fetched {
    /// The page's HTML, and where it came from after redirects.
    Page { url: String, html: String },
    /// The server answered with something other than a web page.
    NotPage { content_type: String },
    /// The server answered with an error status.
    Status(u16),
}

/// Fetches `url`. Errors that may pass (offline, timeout) are `Transient`; a page over 5 MB
/// or a bad address is `Permanent`.
pub fn fetch(url: &str) -> Result<Fetched, HttpError> {
    let client = Client::new(USER_AGENT)?;
    let response = client.send_limited(
        &Request {
            method: "GET",
            url,
            headers: vec![
                (
                    "accept",
                    "text/html,application/xhtml+xml;q=0.9,*/*;q=0.5".to_owned(),
                ),
                ("accept-language", "en;q=0.9,*;q=0.5".to_owned()),
            ],
            body: &[],
            timeout: TIMEOUT,
        },
        MAX_PAGE_BYTES,
    )?;
    if !(200..300).contains(&response.status) {
        return Ok(Fetched::Status(response.status));
    }
    let content_type = response.header("content-type").unwrap_or("").to_owned();
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let html = match mime.as_str() {
        "text/html" | "application/xhtml+xml" => true,
        // No type given: a body that starts like HTML is taken as HTML.
        "" => looks_like_html(&response.body),
        _ => false,
    };
    if !html {
        return Ok(Fetched::NotPage {
            content_type: if mime.is_empty() {
                "unknown".to_owned()
            } else {
                mime
            },
        });
    }
    Ok(Fetched::Page {
        url: response.url,
        html: decode(&response.body, &content_type),
    })
}

fn looks_like_html(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<!doctype html") || head.starts_with("<html") || head.contains("<body")
}

/// The page's bytes as text: by BOM, then the `Content-Type` charset, then a `<meta>` charset
/// in the first 1,024 bytes, then UTF-8 when it is valid, else Windows-1252 as browsers do.
pub fn decode(body: &[u8], content_type: &str) -> String {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(body) {
        return encoding
            .decode_without_bom_handling(&body[bom..])
            .0
            .into_owned();
    }
    let label = charset_of(content_type).or_else(|| meta_charset(body));
    if let Some(encoding) =
        label.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
    {
        return encoding.decode_without_bom_handling(body).0.into_owned();
    }
    match std::str::from_utf8(body) {
        Ok(text) => text.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(body)
            .0
            .into_owned(),
    }
}

/// `charset=` in a `Content-Type` value.
fn charset_of(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// `<meta charset="…">` or `<meta http-equiv="Content-Type" content="…; charset=…">`.
fn meta_charset(body: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
    let mut rest = head.as_str();
    while let Some(at) = rest.find("<meta") {
        rest = &rest[at + 5..];
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        if let Some(at) = tag.find("charset=") {
            let value = tag[at + "charset=".len()..].trim_start_matches(['"', '\'']);
            let end = value
                .find(|c: char| matches!(c, '"' | '\'' | ';' | '/' | '>') || c.is_whitespace())
                .unwrap_or(value.len());
            if end > 0 {
                return Some(value[..end].to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::test_server::{Scripted, TestServer};

    fn page(content_type: &str, body: &[u8]) -> Scripted {
        Scripted {
            status: 200,
            headers: vec![("content-type".to_owned(), content_type.to_owned())],
            body: body.to_vec(),
            delay: Duration::ZERO,
        }
    }

    #[test]
    fn a_page_is_fetched_as_a_browser_would_and_decoded() {
        let server = TestServer::start(vec![
            page(
                "text/html; charset=windows-1252",
                b"<p>caf\xe9 \x93ok\x94</p>",
            ),
            page(
                "text/html",
                b"<meta charset=\"iso-8859-2\"><p>\xb3\xf3d\xbc</p>",
            ),
            page("text/html", "<p>zăpadă</p>".as_bytes()),
            page("", b"<!DOCTYPE html><p>untyped</p>"),
        ]);
        let html = |fetched: Fetched| match fetched {
            Fetched::Page { html, .. } => html,
            other => panic!("{other:?}"),
        };
        assert_eq!(html(fetch(&server.url()).unwrap()), "<p>café “ok”</p>");
        assert!(html(fetch(&server.url()).unwrap()).ends_with("<p>łódź</p>"));
        assert_eq!(html(fetch(&server.url()).unwrap()), "<p>zăpadă</p>");
        assert_eq!(
            html(fetch(&server.url()).unwrap()),
            "<!DOCTYPE html><p>untyped</p>"
        );
        let seen = &server.requests()[0];
        assert!(
            seen.header("user-agent")
                .unwrap()
                .contains("Windows NT 10.0")
        );
        assert!(seen.header("accept").unwrap().starts_with("text/html"));
        assert!(seen.header("cookie").is_none());
    }

    #[test]
    fn errors_other_types_and_large_pages() {
        let server = TestServer::start(vec![
            Scripted::json(403, "forbidden"),
            page("application/zip", b"PK\x03\x04"),
            page("text/html", &vec![b'x'; MAX_PAGE_BYTES + 1]),
        ]);
        assert_eq!(fetch(&server.url()), Ok(Fetched::Status(403)));
        assert_eq!(
            fetch(&server.url()),
            Ok(Fetched::NotPage {
                content_type: "application/zip".to_owned()
            })
        );
        assert!(matches!(fetch(&server.url()), Err(HttpError::Permanent(_))));
    }

    #[test]
    fn charsets_are_found_where_browsers_look() {
        assert_eq!(
            charset_of("text/html; charset=\"UTF-8\""),
            Some("UTF-8".to_owned())
        );
        assert_eq!(charset_of("text/html"), None);
        assert_eq!(
            meta_charset(
                b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=Shift_JIS\">"
            ),
            Some("shift_jis".to_owned())
        );
        assert_eq!(
            meta_charset(b"<meta charset=utf-8/>"),
            Some("utf-8".to_owned())
        );
        assert_eq!(
            decode(b"\xEF\xBB\xBFhi", "text/html; charset=windows-1252"),
            "hi"
        );
        assert_eq!(decode(b"\xe9t\xe9", ""), "été");
    }
}
