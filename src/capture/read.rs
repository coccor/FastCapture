//! Reading a raw source back: what the review list shows for it, and the
//! attachments it links to. Pure: callers pass the text.

use crate::notebook::frontmatter::{self, Value};
use std::path::{Component, Path, PathBuf};

/// What a raw source says about itself.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RawInfo {
    pub capture_id: Option<String>,
    /// `YYYY-MM-DDTHH:MM`.
    pub captured: String,
    pub kinds: Vec<String>,
    /// The first line of your text (URL lines included), else the first link's name.
    pub title: Option<String>,
    /// The files it links to with relative Markdown links, relative to the notebook.
    pub attachments: Vec<PathBuf>,
}

/// Reads the raw source at `raw` (relative to the notebook) from its `text`.
pub fn read(raw: &Path, text: &str) -> RawInfo {
    let front = frontmatter::parse(text);
    let text_of = |key: &str| match front.as_ref().and_then(|front| front.get(key)) {
        Some(Value::Text(value)) => Some(value.clone()),
        _ => None,
    };
    let kinds = match front.as_ref().and_then(|front| front.get("kind")) {
        Some(Value::List(items)) => items.clone(),
        Some(Value::Text(item)) if !item.is_empty() => vec![item.clone()],
        _ => Vec::new(),
    };
    let body = &text[front.as_ref().map_or(0, |front| front.body_start)..];
    let folder = raw.parent().unwrap_or(Path::new(""));
    let mut title = None;
    let mut attachments = Vec::new();
    for line in body.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        match link_of(line) {
            Some((label, target)) => {
                if let Some(path) = resolve(folder, &target) {
                    if title.is_none() && !label.is_empty() {
                        title = Some(label);
                    }
                    attachments.push(path);
                }
            }
            None if title.is_none() => title = Some(shorten(line, 80)),
            None => {}
        }
    }
    RawInfo {
        capture_id: text_of("capture_id"),
        captured: text_of("captured").unwrap_or_default(),
        kinds,
        title,
        attachments,
    }
}

/// A line that is one Markdown link or image, `[label](target)` or `![](target)`: its label
/// (unescaped) and target.
pub(crate) fn link_of(line: &str) -> Option<(String, String)> {
    let line = line.strip_prefix('!').unwrap_or(line);
    let rest = line.strip_prefix('[')?;
    let mut label = String::new();
    let mut chars = rest.char_indices();
    let close = loop {
        let (at, c) = chars.next()?;
        match c {
            '\\' => label.push(chars.next()?.1),
            ']' => break at,
            c => label.push(c),
        }
    };
    let target = rest[close + 1..].strip_prefix('(')?.strip_suffix(')')?;
    (!target.is_empty() && !target.contains(' ')).then(|| (label, target.to_owned()))
}

/// A relative link target resolved against `folder` (relative to the notebook), with `%20` and
/// `%25` decoded. Links that leave the notebook, absolute paths and URLs give `None`.
fn resolve(folder: &Path, target: &str) -> Option<PathBuf> {
    if target.contains(':') || target.starts_with('/') || target.starts_with('#') {
        return None;
    }
    let decoded = target.replace("%20", " ").replace("%25", "%");
    let mut parts: Vec<String> = folder
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name.to_owned()),
        }
    }
    (!parts.is_empty()).then(|| parts.iter().collect())
}

/// `text` cut to `limit` characters with an ellipsis.
fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The attachments `raw` links to that no other raw source in `others` links to: what Shift+Del
/// recycles along with it.
pub fn own_attachments<'a>(
    raw: &RawInfo,
    others: impl IntoIterator<Item = &'a RawInfo>,
) -> Vec<PathBuf> {
    let shared: Vec<String> = others
        .into_iter()
        .flat_map(|other| other.attachments.iter())
        .map(|path| path.to_string_lossy().to_lowercase())
        .collect();
    raw.attachments
        .iter()
        .filter(|path| !shared.contains(&path.to_string_lossy().to_lowercase()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Kind;
    use crate::capture::raw::{RawSource, raw_text};
    use crate::notebook::dates::{Date, Time};

    fn raw(text: &str, links: &[&str]) -> String {
        let links: Vec<String> = links.iter().map(|link| (*link).to_owned()).collect();
        raw_text(&RawSource {
            date: Date::new(2026, 10, 19).unwrap(),
            time: Time {
                hour: 10,
                minute: 30,
            },
            capture_id: "ID1",
            kinds: &[Kind::Text, Kind::Screenshot],
            text,
            links: &links,
            source_window: None,
        })
    }

    #[test]
    fn a_raw_source_reads_back() {
        let text = raw(
            "  \nlag spikes during ledger batch??\nmore",
            &[
                "![](../attachments/2026-10-19%201030%20lag.png)",
                "[ledger \\[1\\].log](../attachments/ledger%20[1].log)",
            ],
        );
        let info = read(Path::new(r"raw\x.md"), &text);
        assert_eq!(info.capture_id.as_deref(), Some("ID1"));
        assert_eq!(info.captured, "2026-10-19T10:30");
        assert_eq!(info.kinds, ["text", "screenshot"]);
        assert_eq!(
            info.title.as_deref(),
            Some("lag spikes during ledger batch??")
        );
        assert_eq!(
            info.attachments,
            [
                PathBuf::from(r"attachments\2026-10-19 1030 lag.png"),
                PathBuf::from(r"attachments\ledger [1].log")
            ]
        );
    }

    #[test]
    fn a_capture_with_only_files_is_named_by_its_first_file() {
        let text = raw("", &["[report.pdf](../attachments/report.pdf)"]);
        let info = read(Path::new(r"raw\x.md"), &text);
        assert_eq!(info.title.as_deref(), Some("report.pdf"));
        let text = raw("", &["![](img/a.png)"]);
        let info = read(Path::new(r"raw\x.md"), &text);
        assert_eq!(info.title, None, "an image has no name to show");
        assert_eq!(info.attachments, [PathBuf::from(r"raw\img\a.png")]);
    }

    #[test]
    fn links_that_leave_the_notebook_or_are_urls_are_not_attachments() {
        let text = "---\ntype: raw\n---\n\n[a](../../outside.txt)\n[b](https://example.com)\n[c](C:/x.txt)\n[d](/x.txt)\nsee [e](e.txt) inline\n";
        let info = read(Path::new(r"raw\x.md"), text);
        assert!(info.attachments.is_empty(), "{:?}", info.attachments);
        assert_eq!(
            info.title.as_deref(),
            Some("see [e](e.txt) inline"),
            "link lines are never the title"
        );
    }

    #[test]
    fn long_first_lines_are_shortened() {
        let long = "word ".repeat(40);
        let info = read(Path::new(r"raw\x.md"), &raw(&long, &[]));
        let title = info.title.unwrap();
        assert_eq!(title.chars().count(), 80);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn only_attachments_no_other_raw_source_uses_are_its_own() {
        let mine = RawInfo {
            attachments: vec![
                PathBuf::from(r"attachments\a.png"),
                PathBuf::from(r"attachments\shared.log"),
            ],
            ..RawInfo::default()
        };
        let other = RawInfo {
            attachments: vec![PathBuf::from(r"Attachments\SHARED.log")],
            ..RawInfo::default()
        };
        assert_eq!(
            own_attachments(&mine, [&other]),
            [PathBuf::from(r"attachments\a.png")]
        );
    }
}
