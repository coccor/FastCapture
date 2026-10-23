//! The front matter edits filing and placing make: `status:` set, `suggested_home:` set or
//! removed, and every other byte of the note left as it was (its line endings, a byte order
//! mark, other keys and their order). Pure.

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

/// `text` with `suggested_home: <home>` (`home` already quoted) in place of any earlier one,
/// just before `status:`, or the closing `---` when there is none; with no `home`, without
/// one. `None` when the note has no front matter.
pub fn set_suggested_home(text: &str, home: Option<&str>) -> Option<String> {
    let front = frontmatter::parse(text)?;
    let head = &text[..front.body_start];
    let mut output = String::with_capacity(text.len() + 40);
    let lines: Vec<&str> = head.split_inclusive('\n').collect();
    let newline = if lines.first().is_some_and(|line| line.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let line = home.map_or_else(String::new, |home| {
        format!("suggested_home: {home}{newline}")
    });
    let mut written = false;
    let mut skipping = false;
    for (index, current) in lines.iter().enumerate() {
        let content = current.trim_end_matches(['\r', '\n']);
        let last = index + 1 == lines.len();
        if index > 0 && !last {
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
                Some(key) if key.eq_ignore_ascii_case("status") && !written => {
                    output.push_str(&line);
                    written = true;
                }
                _ => {}
            }
        }
        if last && !written {
            output.push_str(&line);
        }
        output.push_str(current);
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

    #[test]
    fn a_suggested_home_goes_before_the_status_and_replaces_the_last_one() {
        let note = "---\r\ntype: finding\r\nstatus: inbox\r\n---\r\n\r\n# Lag\r\n";
        let placed = set_suggested_home(note, Some("\"[[Payments incidents]]\"")).unwrap();
        assert_eq!(
            placed,
            "---\r\ntype: finding\r\nsuggested_home: \"[[Payments incidents]]\"\r\nstatus: inbox\r\n---\r\n\r\n# Lag\r\n"
        );
        assert_eq!(
            set_suggested_home(&placed, Some("\"Payments/\"")).unwrap(),
            "---\r\ntype: finding\r\nsuggested_home: \"Payments/\"\r\nstatus: inbox\r\n---\r\n\r\n# Lag\r\n"
        );
        assert_eq!(set_suggested_home(&placed, None).unwrap(), note);
        assert_eq!(
            set_suggested_home("---\ntype: finding\n---\nBody", Some("\"Payments/\"")).unwrap(),
            "---\ntype: finding\nsuggested_home: \"Payments/\"\n---\nBody"
        );
        assert_eq!(
            set_suggested_home("# No front matter\n", Some("\"x\"")),
            None
        );
    }

    #[test]
    fn a_backslash_before_a_quote_reads_back_as_written() {
        let written = r#"Match \"quoted\" with a regex in C:\Temp"#;
        let note = format!(
            "---\nsummary: {}\n---\n",
            crate::capture::raw::quoted(written)
        );
        assert_eq!(summary(&note).as_deref(), Some(written));
    }
}
