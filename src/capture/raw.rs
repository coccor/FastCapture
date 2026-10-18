//! A raw source's name and text. `first_words`, `capture_stem` and `link_lines` are
//! copied from FastPad `src/library/capture.rs`, so captures from both apps are named and linked
//! alike. Nothing here touches the disk.

use super::detect;
use super::model::Kind;
use crate::notebook::dates::{self, Date, Time};
use crate::notebook::{links, title};
use std::path::{Path, PathBuf};

/// A file written for a capture, relative to the notebook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAttachment {
    pub path: PathBuf,
    /// An image is linked as `![](…)`, anything else as `[name](…)`.
    pub image: bool,
}

/// The first up to five words of `text`'s first non-blank line, cleaned for a file name; `None`
/// when nothing is left.
pub fn first_words(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let words: Vec<&str> = line.split_whitespace().take(5).collect();
    let cleaned = title::sanitize_stem(&words.join(" "));
    (cleaned != "Untitled" || words.join(" ").eq_ignore_ascii_case("untitled")).then_some(cleaned)
}

/// `YYYY-MM-DD HHmm <name>`.
fn stem(date: Date, time: Time, name: &str) -> String {
    format!(
        "{} {:02}{:02} {name}",
        dates::format(date, "YYYY-MM-DD"),
        time.hour,
        time.minute,
    )
}

/// `YYYY-MM-DD HHmm <first words>`, or with `capture` when the text has no words. Names pasted
/// images, as FastPad does.
pub fn capture_stem(date: Date, time: Time, text: &str) -> String {
    stem(
        date,
        time,
        &first_words(text).unwrap_or_else(|| "capture".to_owned()),
    )
}

/// A raw source's stem: the first words of what you wrote (URL lines left out), else the first
/// link's host, else `capture`.
pub fn raw_stem(date: Date, time: Time, text: &str) -> String {
    let name = first_words(&detect::prose(text))
        .or_else(|| {
            detect::urls(text)
                .first()
                .and_then(|url| detect::host(url))
                .map(title::sanitize_stem)
        })
        .unwrap_or_else(|| "capture".to_owned());
    stem(date, time, &name)
}

/// One link line per attachment, relative to the note at `note` (both relative to the notebook).
pub fn link_lines(note: &Path, attachments: &[StoredAttachment]) -> Vec<String> {
    attachments
        .iter()
        .map(|attachment| {
            let target = links::relative_target(note, &attachment.path, "");
            if attachment.image {
                format!("![]({target})")
            } else {
                let name = attachment
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace('[', "\\[")
                    .replace(']', "\\]");
                format!("[{name}]({target})")
            }
        })
        .collect()
}

/// Everything a raw source's text is made from.
pub struct RawSource<'a> {
    pub date: Date,
    pub time: Time,
    pub capture_id: &'a str,
    pub kinds: &'a [Kind],
    /// Your text, URL lines included.
    pub text: &'a str,
    /// Lines from `link_lines`.
    pub links: &'a [String],
    pub source_window: Option<&'a str>,
}

/// `YYYY-MM-DDTHH:MM`, the `captured:` value.
pub fn captured_stamp(date: Date, time: Time) -> String {
    format!(
        "{}T{:02}:{:02}",
        dates::format(date, "YYYY-MM-DD"),
        time.hour,
        time.minute
    )
}

/// A YAML double-quoted string.
fn quoted(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n', '\t'], " ");
    format!("\"{escaped}\"")
}

/// The raw source's text, with `\n` line endings:
///
/// ```text
/// ---
/// type: raw
/// captured: 2026-10-19T10:30
/// kind: [text, screenshot]
/// capture_id: 01JAB3K9X7Q2M8…
/// urls:
///   - "https://example.com"
/// source_window: "Grafana – Google Chrome"
/// ---
///
/// your text
///
/// ![](../attachments/…png)
/// ```
pub fn raw_text(source: &RawSource<'_>) -> String {
    let kinds: Vec<&str> = source.kinds.iter().map(|kind| kind.key()).collect();
    let mut text = format!(
        "---
type: raw
captured: {}
kind: [{}]
capture_id: {}
",
        captured_stamp(source.date, source.time),
        kinds.join(", "),
        source.capture_id,
    );
    let urls = detect::urls(source.text);
    if !urls.is_empty() {
        text.push_str("urls:\n");
        for url in &urls {
            text.push_str(&format!("  - {}\n", quoted(url)));
        }
    }
    if let Some(window) = source.source_window {
        text.push_str(&format!("source_window: {}\n", quoted(window)));
    }
    text.push_str("---\n");
    let body: Vec<&str> = source.text.trim().lines().map(str::trim_end).collect();
    if !body.is_empty() {
        text.push('\n');
        text.push_str(&body.join("\n"));
        text.push('\n');
    }
    if !source.links.is_empty() {
        text.push('\n');
        for link in source.links {
            text.push_str(link);
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::frontmatter::{self, Value};

    fn day() -> Date {
        Date::new(2026, 10, 19).unwrap()
    }

    fn at(hour: u8, minute: u8) -> Time {
        Time { hour, minute }
    }

    #[test]
    fn first_words_name_the_capture() {
        assert_eq!(
            first_words("\n  consumer lag spikes when ledger batch runs??\nmore").as_deref(),
            Some("consumer lag spikes when ledger")
        );
        assert_eq!(
            first_words("what? <script>").as_deref(),
            Some("what script")
        );
        assert_eq!(first_words("  \n "), None);
        assert_eq!(
            capture_stem(day(), at(10, 30), ""),
            "2026-10-19 1030 capture"
        );
    }

    #[test]
    fn raw_names_skip_url_lines_then_use_the_host() {
        assert_eq!(
            raw_stem(
                day(),
                at(9, 5),
                "https://example.com/x\nRust cancellation notes"
            ),
            "2026-10-19 0905 Rust cancellation notes"
        );
        assert_eq!(
            raw_stem(day(), at(9, 5), "https://www.without.boats/blog/x\n"),
            "2026-10-19 0905 without.boats"
        );
        assert_eq!(raw_stem(day(), at(9, 5), ""), "2026-10-19 0905 capture");
    }

    #[test]
    fn links_are_relative_to_the_raw_folder() {
        let attachments = [
            StoredAttachment {
                path: PathBuf::from(r"attachments\2026-10-19 1030 lag.png"),
                image: true,
            },
            StoredAttachment {
                path: PathBuf::from(r"attachments\ledger [1].log"),
                image: false,
            },
        ];
        assert_eq!(
            link_lines(Path::new(r"raw\x.md"), &attachments),
            [
                "![](../attachments/2026-10-19%201030%20lag.png)",
                "[ledger \\[1\\].log](../attachments/ledger%20[1].log)"
            ]
        );
    }

    #[test]
    fn a_raw_source_has_its_facts_in_front_matter_and_your_text_below() {
        let text = raw_text(&RawSource {
            date: day(),
            time: at(10, 30),
            capture_id: "01JAB3K9X7Q2M8ZZZZZZZZZZZZ",
            kinds: &[Kind::Text, Kind::Link, Kind::Screenshot],
            text: "lag spikes during ledger batch??\r\nhttps://grafana.example/d/x?from=1#panel\r\n\r\n",
            links: &["![](../attachments/a.png)".to_owned()],
            source_window: Some("Grafana \"payments\" \\ Chrome"),
        });
        assert_eq!(
            text,
            "---\ntype: raw\ncaptured: 2026-10-19T10:30\nkind: [text, link, screenshot]\n\
             capture_id: 01JAB3K9X7Q2M8ZZZZZZZZZZZZ\nurls:\n  - \"https://grafana.example/d/x?from=1#panel\"\n\
             source_window: \"Grafana \\\"payments\\\" \\\\ Chrome\"\n---\n\n\
             lag spikes during ledger batch??\nhttps://grafana.example/d/x?from=1#panel\n\n\
             ![](../attachments/a.png)\n"
        );
        let front = frontmatter::parse(&text).unwrap();
        assert_eq!(front.get("type"), Some(&Value::Text("raw".into())));
        assert_eq!(
            front.get("kind"),
            Some(&Value::List(vec![
                "text".into(),
                "link".into(),
                "screenshot".into()
            ]))
        );
        assert_eq!(
            front.get("urls"),
            Some(&Value::List(vec![
                "https://grafana.example/d/x?from=1#panel".into()
            ]))
        );
    }

    #[test]
    fn an_attachment_only_capture_has_no_text_block() {
        let text = raw_text(&RawSource {
            date: day(),
            time: at(8, 0),
            capture_id: "ID",
            kinds: &[Kind::File],
            text: "  ",
            links: &["[a.log](../attachments/a.log)".to_owned()],
            source_window: None,
        });
        assert_eq!(
            text,
            "---\ntype: raw\ncaptured: 2026-10-19T08:00\nkind: [file]\ncapture_id: ID\n---\n\n\
             [a.log](../attachments/a.log)\n"
        );
    }
}
