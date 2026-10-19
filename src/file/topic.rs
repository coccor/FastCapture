//! The line a topic note gets for each note filed under it:
//! `- 2026-10-19 [[Consumer lag spikes]]: Kafka consumer lag on payments jumps…`, appended with
//! FastPad's rules for capture entries (`append_entry` in `src/library/capture.rs`): in the list
//! when the note ends with one, after a blank line when it ends with prose, in the note's own
//! line endings. Pure.

/// A summary in a topic line is cut to this many characters.
pub const SUMMARY_CHARS: usize = 140;

/// The line for the note `name` (a wikilink target) filed on `date` (`YYYY-MM-DD`).
pub fn topic_line(date: &str, name: &str, summary: Option<&str>) -> String {
    match summary.map(str::trim).filter(|summary| !summary.is_empty()) {
        Some(summary) => format!("- {date} [[{name}]]: {}", cap(summary, SUMMARY_CHARS)),
        None => format!("- {date} [[{name}]]"),
    }
}

/// `existing` with `line` appended as a list item.
pub fn appended(existing: &str, line: &str) -> String {
    let newline = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = String::with_capacity(existing.len() + line.len() + 4);
    text.push_str(existing);
    let body = existing.trim_start_matches('\u{feff}');
    if !body.is_empty() && !body.ends_with('\n') {
        text.push_str(newline);
    }
    let last = body.trim_end_matches(['\r', '\n']).lines().last();
    let ends_blank = body.trim().is_empty() || body.ends_with("\n\n") || body.ends_with("\r\n\r\n");
    let ends_in_list = last.is_some_and(|line| {
        let trimmed = line.trim_start();
        line.starts_with("  ")
            || ["- ", "* ", "+ "]
                .iter()
                .any(|bullet| trimmed.starts_with(bullet))
    });
    if !ends_blank && !ends_in_list {
        text.push_str(newline);
    }
    text.push_str(line);
    text.push_str(newline);
    text
}

/// `text` cut to at most `limit` characters, at a word boundary when there is one, with `…`.
fn cap(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space > cut.len() / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_names_the_note_and_its_summary() {
        assert_eq!(
            topic_line("2026-10-19", "Lag spikes", Some(" Lag hit 40k. ")),
            "- 2026-10-19 [[Lag spikes]]: Lag hit 40k."
        );
        assert_eq!(topic_line("2026-10-19", "x", None), "- 2026-10-19 [[x]]");
        let long = "word ".repeat(60);
        let line = topic_line("d", "x", Some(&long));
        assert!(line.ends_with('…'));
        assert!(line.chars().count() < SUMMARY_CHARS + 20);
    }

    #[test]
    fn a_topic_note_ending_with_a_list_gets_one_more_item() {
        assert_eq!(
            appended("# Payments incidents\n\n- 2026-10-01 [[Old]]\n", "- new"),
            "# Payments incidents\n\n- 2026-10-01 [[Old]]\n- new\n"
        );
        assert_eq!(
            appended("- a\n  more of a", "- new"),
            "- a\n  more of a\n- new\n"
        );
    }

    #[test]
    fn prose_gets_a_blank_line_first_and_line_endings_are_kept() {
        assert_eq!(
            appended("# Incidents\r\nAll outages.", "- new"),
            "# Incidents\r\nAll outages.\r\n\r\n- new\r\n"
        );
        assert_eq!(
            appended("# Incidents\n\n", "- new"),
            "# Incidents\n\n- new\n"
        );
        assert_eq!(appended("", "- new"), "- new\n");
        assert_eq!(appended("\u{feff}", "- new"), "\u{feff}- new\n");
    }
}
