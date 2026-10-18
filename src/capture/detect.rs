//! Working out what a capture is: a line that is only an `http(s)://` URL is a link
//! to fetch; a URL inside a sentence stays text.

use super::model::{Attachment, Capture, Kind};
use std::path::Path;

/// Whether `line` (trimmed) is a single `http://` or `https://` URL and nothing else.
pub fn is_url_line(line: &str) -> bool {
    let line = line.trim();
    let rest = ["https://", "http://"].iter().find_map(|scheme| {
        line.get(..scheme.len())
            .filter(|head| head.eq_ignore_ascii_case(scheme))
            .map(|_| &line[scheme.len()..])
    });
    rest.is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty() && !rest.chars().any(char::is_whitespace)
    })
}

/// The URLs on lines of their own, in order, without repeats.
pub fn urls(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines().filter(|line| is_url_line(line)) {
        let url = line.trim().to_owned();
        if !found.contains(&url) {
            found.push(url);
        }
    }
    found
}

/// `text` without its URL lines: what you wrote about the capture.
pub fn prose(text: &str) -> String {
    text.lines()
        .filter(|line| !is_url_line(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// The host of an `http(s)://` URL, without `www.`.
pub fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = host.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then_some(host)
}

/// Whether a file attachment is an image (linked as `![](…)`). FastPad's list, so both apps link
/// the same files the same way.
pub fn is_image_path(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// What `capture` contains, in a fixed order, each kind once.
pub fn kinds(capture: &Capture) -> Vec<Kind> {
    let mut kinds = Vec::new();
    if !prose(&capture.text).is_empty() {
        kinds.push(Kind::Text);
    }
    if !urls(&capture.text).is_empty() {
        kinds.push(Kind::Link);
    }
    for attachment in &capture.attachments {
        kinds.push(match attachment {
            Attachment::Png(_) => Kind::Screenshot,
            Attachment::File(path) if is_image_path(path) => Kind::Image,
            Attachment::File(_) => Kind::File,
        });
    }
    kinds.sort();
    kinds.dedup();
    kinds
}

/// The box's footer text for `kinds`: "Text", "Link + screenshot", or "Empty".
pub fn label(kinds: &[Kind]) -> String {
    let mut words = kinds.iter().map(|kind| kind.label().to_owned());
    let Some(first) = words.next() else {
        return "Empty".to_owned();
    };
    words.fold(first, |text, word| {
        format!("{text} + {}", word.to_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_a_url_alone_on_its_line_is_a_link() {
        assert!(is_url_line("https://example.com/a?b=c#d"));
        assert!(is_url_line("  HTTP://example.com  "));
        assert!(!is_url_line("see https://example.com"));
        assert!(!is_url_line("https://"));
        assert!(!is_url_line("https:// example.com"));
        assert!(!is_url_line("ftp://example.com"));
        assert!(!is_url_line("example.com"));
    }

    #[test]
    fn urls_and_prose_are_separated() {
        let text = "Rust cancellation\nhttps://a.example/x\nsee https://b.example inline\nhttps://a.example/x\nhttps://c.example\n";
        assert_eq!(urls(text), ["https://a.example/x", "https://c.example"]);
        assert_eq!(
            prose(text),
            "Rust cancellation\nsee https://b.example inline"
        );
    }

    #[test]
    fn hosts_drop_www_ports_and_users() {
        assert_eq!(host("https://www.example.com/a"), Some("example.com"));
        assert_eq!(host("http://me@docs.rs:8080/x"), Some("docs.rs"));
        assert_eq!(host("https://"), None);
    }

    #[test]
    fn kinds_come_in_a_fixed_order_once_each() {
        let capture = Capture {
            text: "https://example.com\nwhy this matters".to_owned(),
            attachments: vec![
                Attachment::File(PathBuf::from(r"C:\x\ledger.log")),
                Attachment::Png(vec![1]),
                Attachment::File(PathBuf::from(r"C:\x\photo.JPG")),
                Attachment::Png(vec![2]),
            ],
            source_window: None,
        };
        assert_eq!(
            kinds(&capture),
            [
                Kind::Text,
                Kind::Link,
                Kind::Screenshot,
                Kind::Image,
                Kind::File
            ]
        );
        assert_eq!(
            label(&kinds(&capture)),
            "Text + link + screenshot + image + file"
        );
        assert_eq!(label(&[]), "Empty");
        assert_eq!(
            kinds(&Capture {
                text: "https://example.com".to_owned(),
                ..Capture::default()
            }),
            [Kind::Link]
        );
    }
}
