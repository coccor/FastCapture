//! A written note's Markdown: front matter, then the title, the summary as a quote, the body,
//! key points, tasks, related notes, the links captured and the capture's attachments. Empty
//! sections are left out. Pure: the caller writes the text.

use super::reply::NoteJson;
use crate::capture::detect;
use crate::notebook::{links, title};
use std::path::Path;

/// A link the capture holds, with what its page said about itself.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceLink {
    pub url: String,
    pub title: Option<String>,
    pub site: Option<String>,
}

/// Everything a note's text is made from besides the model's reply.
pub struct NoteFile<'a> {
    pub note: &'a NoteJson,
    /// `YYYY-MM-DDTHH:MM`, from the raw source.
    pub captured: &'a str,
    pub capture_id: &'a str,
    /// The raw source, relative to the notebook.
    pub raw: &'a Path,
    pub kinds: &'a [String],
    /// `cloud/claude-sonnet-5-5`, `local/llama3.2-vision`.
    pub model: &'a str,
    /// The raw source's attachments, relative to the notebook.
    pub attachments: &'a [std::path::PathBuf],
    /// The capture's links.
    pub links: &'a [SourceLink],
    /// Sources cut to fit the request, by name.
    pub truncated: &'a [String],
    /// Where the note goes, relative to the notebook.
    pub path: &'a Path,
}

/// A YAML double-quoted string.
fn quoted(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n', '\t'], " ");
    format!("\"{escaped}\"")
}

/// A path as a wikilink target: `/` separators and no `.md`.
fn wiki_target(path: &Path) -> String {
    path.with_extension("").to_string_lossy().replace('\\', "/")
}

/// The note file's stem for `title`: the title cleaned for a file name, at most 80 characters.
pub fn file_stem(title_text: &str) -> String {
    let short: String = title_text.chars().take(80).collect();
    title::sanitize_stem(&short)
}

/// The note's text, with `\n` line endings.
pub fn render(file: &NoteFile<'_>) -> String {
    let note = file.note;
    let mut text = String::from("---\ntype: capture\n");
    text.push_str(&format!("captured: {}\n", file.captured));
    text.push_str(&format!("capture_id: {}\n", file.capture_id));
    text.push_str(&format!(
        "source: {}\n",
        quoted(&format!("[[{}]]", wiki_target(file.raw)))
    ));
    if !file.kinds.is_empty() {
        text.push_str(&format!("kind: [{}]\n", file.kinds.join(", ")));
    }
    // A capture of one link is about its page.
    if let [link] = file.links {
        text.push_str(&format!("url: {}\n", quoted(&link.url)));
        if let Some(site) = &link.site {
            text.push_str(&format!("site: {}\n", quoted(site)));
        }
    }
    if !note.tags.is_empty() {
        text.push_str(&format!("tags: [{}]\n", note.tags.join(", ")));
    }
    if !note.summary.is_empty() {
        text.push_str(&format!("summary: {}\n", quoted(&note.summary)));
    }
    if let Some(home) = &note.home {
        text.push_str(&format!(
            "suggested_home: {}\n",
            quoted(&format!("[[{}]]", home.note))
        ));
    }
    if !file.truncated.is_empty() {
        let names: Vec<String> = file.truncated.iter().map(|name| quoted(name)).collect();
        text.push_str(&format!("truncated: [{}]\n", names.join(", ")));
    }
    text.push_str("status: inbox\n");
    text.push_str(&format!("model: {}\n", quoted(file.model)));
    text.push_str("---\n\n");

    text.push_str(&format!("# {}\n", note.title));
    if !note.summary.is_empty() {
        text.push_str(&format!("\n> {}\n", note.summary));
    }
    if !note.body.is_empty() {
        let body = note.body.replace("\r\n", "\n");
        text.push_str(&format!("\n{}\n", body.trim_end()));
    }
    let section = |text: &mut String, heading: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            text.push_str(&format!("\n## {heading}\n\n"));
            for line in lines {
                text.push_str(&line);
                text.push('\n');
            }
        }
    };
    section(
        &mut text,
        "Key points",
        note.key_points
            .iter()
            .map(|point| format!("- {point}"))
            .collect(),
    );
    section(
        &mut text,
        "Tasks",
        note.tasks
            .iter()
            .map(|task| format!("- [ ] {task}"))
            .collect(),
    );
    section(
        &mut text,
        "Related",
        note.links
            .iter()
            .map(|link| format!("- [[{}]]", link.note))
            .collect(),
    );
    section(
        &mut text,
        "Source",
        file.links
            .iter()
            .map(|link| {
                let url = link.url.replace(' ', "%20").replace(')', "%29");
                match &link.title {
                    Some(title) => format!(
                        "- [{}]({url})",
                        title.replace('[', "\\[").replace(']', "\\]")
                    ),
                    None => format!("- <{url}>"),
                }
            })
            .collect(),
    );
    section(
        &mut text,
        "Attachments",
        file.attachments
            .iter()
            .map(|attachment| {
                let target = links::relative_target(file.path, attachment, "");
                if detect::is_image_path(attachment) {
                    format!("![]({target})")
                } else {
                    let name = attachment
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .replace('[', "\\[")
                        .replace(']', "\\]");
                    format!("[{name}]({target})")
                }
            })
            .collect(),
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::reply::{Home, Link};
    use std::path::PathBuf;

    fn note() -> NoteJson {
        NoteJson {
            title: "Consumer lag spikes during ledger batch".into(),
            summary: "Kafka consumer lag on payments jumps to ~40k when the nightly ledger batch runs.".into(),
            body: "Consumer lag spikes when the ledger batch runs. Seen on the Grafana payments board.".into(),
            key_points: vec![
                "Lag peaks at ~40k around 02:10, recovers by 02:35".into(),
                "Only the `ledger-events` consumer group".into(),
            ],
            tasks: vec!["Check batch size config in ledger job".into()],
            tags: vec!["investigate".into(), "payments".into()],
            links: Vec::new(),
            home: None,
        }
    }

    #[test]
    fn a_screenshot_capture_renders_in_full() {
        let note = note();
        let text = render(&NoteFile {
            note: &note,
            captured: "2026-10-19T10:30",
            capture_id: "01JAB3K9X7Q2M8ZZZZZZZZZZZZ",
            raw: Path::new(r"raw\2026-10-19 1030 lag spikes during ledger batch.md"),
            kinds: &["text".into(), "screenshot".into()],
            model: "cloud/claude-sonnet-5-5",
            attachments: &[PathBuf::from(r"attachments\2026-10-19 1030 lag spikes.png")],
            links: &[],
            truncated: &[],
            path: Path::new(r"inbox\Consumer lag spikes during ledger batch.md"),
        });
        assert_eq!(
            text,
            "---\n\
             type: capture\n\
             captured: 2026-10-19T10:30\n\
             capture_id: 01JAB3K9X7Q2M8ZZZZZZZZZZZZ\n\
             source: \"[[raw/2026-10-19 1030 lag spikes during ledger batch]]\"\n\
             kind: [text, screenshot]\n\
             tags: [investigate, payments]\n\
             summary: \"Kafka consumer lag on payments jumps to ~40k when the nightly ledger batch runs.\"\n\
             status: inbox\n\
             model: \"cloud/claude-sonnet-5-5\"\n\
             ---\n\
             \n\
             # Consumer lag spikes during ledger batch\n\
             \n\
             > Kafka consumer lag on payments jumps to ~40k when the nightly ledger batch runs.\n\
             \n\
             Consumer lag spikes when the ledger batch runs. Seen on the Grafana payments board.\n\
             \n\
             ## Key points\n\
             \n\
             - Lag peaks at ~40k around 02:10, recovers by 02:35\n\
             - Only the `ledger-events` consumer group\n\
             \n\
             ## Tasks\n\
             \n\
             - [ ] Check batch size config in ledger job\n\
             \n\
             ## Attachments\n\
             \n\
             ![](../attachments/2026-10-19%201030%20lag%20spikes.png)\n"
        );
        let front = crate::notebook::frontmatter::parse(&text).unwrap();
        assert_eq!(front.tags(), ["investigate", "payments"]);
    }

    #[test]
    fn empty_sections_are_left_out_and_links_and_homes_show_when_present() {
        let mut note = NoteJson {
            title: "A \"quoted\" thought".into(),
            ..NoteJson::default()
        };
        let plain = render(&NoteFile {
            note: &note,
            captured: "2026-10-19T08:00",
            capture_id: "ID",
            raw: Path::new(r"raw\x.md"),
            kinds: &[],
            model: "local/llama",
            attachments: &[PathBuf::from(r"attachments\report [v2].pdf")],
            links: &[],
            truncated: &[],
            path: Path::new("inbox.md"),
        });
        assert!(
            !plain.contains("## Key points") && !plain.contains("tags:") && !plain.contains("> ")
        );
        assert!(plain.contains("# A \"quoted\" thought\n"));
        assert!(
            plain.ends_with(
                "## Attachments\n\n[report \\[v2\\].pdf](attachments/report%20[v2].pdf)\n"
            )
        );

        note.links = vec![Link {
            note: "Payments incidents".into(),
            new: false,
        }];
        note.home = Some(Home {
            kind: "topic".into(),
            note: "Payments incidents".into(),
            reason: String::new(),
        });
        let linked = render(&NoteFile {
            note: &note,
            captured: "2026-10-19T08:00",
            capture_id: "ID",
            raw: Path::new(r"raw\x.md"),
            kinds: &[],
            model: "local/llama",
            attachments: &[],
            links: &[],
            truncated: &[],
            path: Path::new(r"inbox\x.md"),
        });
        assert!(linked.contains("suggested_home: \"[[Payments incidents]]\"\n"));
        assert!(linked.ends_with("## Related\n\n- [[Payments incidents]]\n"));
    }

    #[test]
    fn note_names_come_from_titles() {
        assert_eq!(file_stem("Plan: phase 1?"), "Plan phase 1");
        assert_eq!(file_stem(&"x".repeat(200)).chars().count(), 80);
        assert_eq!(file_stem("..."), "Untitled");
    }

    fn with_links(links: &[SourceLink], truncated: &[String]) -> String {
        let note = NoteJson {
            title: "Lag, explained".into(),
            ..NoteJson::default()
        };
        render(&NoteFile {
            note: &note,
            captured: "2026-10-19T08:00",
            capture_id: "ID",
            raw: Path::new(r"raw\x.md"),
            kinds: &["link".into()],
            model: "local/llama",
            attachments: &[],
            links,
            truncated,
            path: Path::new(r"inbox\x.md"),
        })
    }

    #[test]
    fn a_link_capture_names_its_page() {
        let page = SourceLink {
            url: "https://example.com/a b".into(),
            title: Some("Lag [explained]".into()),
            site: Some("Example".into()),
        };
        let one = with_links(std::slice::from_ref(&page), &["review.pdf".into()]);
        assert!(
            one.contains(
                "kind: [link]\nurl: \"https://example.com/a b\"\nsite: \"Example\"\n\
                 truncated: [\"review.pdf\"]\nstatus: inbox\n"
            ),
            "{one}"
        );
        assert!(one.ends_with("## Source\n\n- [Lag \\[explained\\]](https://example.com/a%20b)\n"));
        let front = crate::notebook::frontmatter::parse(&one).unwrap();
        assert_eq!(
            front.get("truncated"),
            Some(&crate::notebook::frontmatter::Value::List(vec![
                "review.pdf".into()
            ]))
        );

        // Two links: no single url in front matter, both under Source.
        let other = SourceLink {
            url: "https://other.example/".into(),
            ..SourceLink::default()
        };
        let two = with_links(&[page, other], &[]);
        assert!(!two.contains("url:") && !two.contains("site:") && !two.contains("truncated:"));
        assert!(two.ends_with(
            "## Source\n\n- [Lag \\[explained\\]](https://example.com/a%20b)\n- <https://other.example/>\n"
        ));
    }
}
