//! The front matter edits filing makes: `status:` set, `suggested_home:` removed, and every other
//! byte of the note left as it was (its line endings, a byte order mark, other keys and their
//! order). Pure.

use crate::notebook::frontmatter;

/// `text` with `status: <status>` (added before the closing `---` when missing) and without
/// `suggested_home:`. `None` when the note has no front matter.
pub fn set_status(text: &str, status: &str) -> Option<String> {
    let front = frontmatter::parse(text)?;
    let head = &text[..front.body_start];
    let mut output = String::with_capacity(text.len() + 16);
    // The front matter's lines with their line ends; the first is the opening `---`.
    let lines: Vec<&str> = head.split_inclusive('\n').collect();
    let newline = if lines.first().is_some_and(|line| line.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let mut status_written = false;
    let mut skipping = false;
    for (index, line) in lines.iter().enumerate() {
        let content = line.trim_end_matches(['\r', '\n']);
        let ending = &line[content.len()..];
        let last = index + 1 == lines.len();
        if index > 0 && !last {
            // A key's continuation lines (a block list, an indented value) go with it.
            let continues = content.starts_with([' ', '\t']) || content.starts_with("- ");
            if skipping && continues {
                continue;
            }
            skipping = false;
            match key_of(content) {
                Some(key) if key.eq_ignore_ascii_case("suggested_home") => {
                    skipping = true;
                    continue;
                }
                Some(key) if key.eq_ignore_ascii_case("status") && !status_written => {
                    output.push_str(&format!("status: {status}{ending}"));
                    status_written = true;
                    skipping = true;
                    continue;
                }
                _ => {}
            }
        }
        if last && !status_written {
            output.push_str(&format!("status: {status}{newline}"));
        }
        output.push_str(line);
    }
    output.push_str(&text[front.body_start..]);
    Some(output)
}

/// The key of a `key: value` line.
fn key_of(line: &str) -> Option<&str> {
    if line.starts_with([' ', '\t', '-', '#']) {
        return None;
    }
    let (key, rest) = line.split_once(':')?;
    (!key.contains(' ') && (rest.is_empty() || rest.starts_with([' ', '\t']))).then_some(key)
}

/// The note's `summary:`, as one line.
pub fn summary(text: &str) -> Option<String> {
    let front = frontmatter::parse(text)?;
    match front.get("summary")? {
        frontmatter::Value::Text(summary) => {
            // The reader takes the quotes off but leaves the escapes in.
            let summary = summary.replace("\\\"", "\"").replace("\\\\", "\\");
            let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
            (!summary.is_empty()).then_some(summary)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_changes_and_the_suggestion_goes_with_nothing_else_touched() {
        let note = "\u{feff}---\r\ntype: capture\r\nunknown: [x,  y]\r\nsuggested_home: \"[[Payments incidents]]\"\r\nstatus: inbox\r\nmodel: \"m\"\r\n---\r\n\r\n# Title\r\nstatus: not front matter\r\n";
        assert_eq!(
            set_status(note, "filed").unwrap(),
            "\u{feff}---\r\ntype: capture\r\nunknown: [x,  y]\r\nstatus: filed\r\nmodel: \"m\"\r\n---\r\n\r\n# Title\r\nstatus: not front matter\r\n"
        );
    }

    #[test]
    fn a_missing_status_is_added_and_list_values_go_with_their_key() {
        let note =
            "---\ntags: [a]\nsuggested_home:\n  - Payments/\n  - other\naliases:\n- A\n---\nBody";
        assert_eq!(
            set_status(note, "kept").unwrap(),
            "---\ntags: [a]\naliases:\n- A\nstatus: kept\n---\nBody"
        );
        assert_eq!(set_status("# No front matter\n", "filed"), None);
    }

    #[test]
    fn the_summary_is_read_without_its_escapes() {
        let note = "---\nsummary: \"Lag on \\\"ledger\\\"  hit 40k\"\n---\n";
        assert_eq!(summary(note).as_deref(), Some("Lag on \"ledger\" hit 40k"));
        assert_eq!(summary("---\ntitle: x\n---\n"), None);
    }
}
