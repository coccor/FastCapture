//! Placing a finding an agent wrote: its suggested home goes in its front matter and the notes
//! it relates to under `## Related`; nothing else in it changes. A note that no longer waits in
//! the inbox (you filed or kept it meanwhile) is left as it is.

use super::reply::{HOME_FOLDER, NoteJson};
use super::write::{ReplaceError, Stamp, Written};
use crate::file::front;
use crate::platform::saver;
use crate::queue::findings::{self, Finding};
use std::path::Path;

/// `text` with a `- [[name]]` line under `## Related` for each of `names` it doesn't link yet:
/// after the section's last line when it has one, else in a new section at the end.
pub fn add_related(text: &str, names: &[String]) -> String {
    let lower = text.to_lowercase();
    let mut missing: Vec<&String> = Vec::new();
    for name in names {
        let target = format!("[[{}", name.to_lowercase());
        let linked = [']', '|', '#']
            .iter()
            .any(|end| lower.contains(&format!("{target}{end}")));
        if !linked && !missing.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            missing.push(name);
        }
    }
    if missing.is_empty() {
        return text.to_owned();
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let items: String = missing
        .iter()
        .map(|name| format!("- [[{name}]]{newline}"))
        .collect();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let heading = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("## related"));
    let Some(heading) = heading else {
        let mut output = text.to_owned();
        if !output.is_empty() && !output.ends_with('\n') {
            output.push_str(newline);
        }
        output.push_str(&format!("{newline}## Related{newline}{newline}{items}"));
        return output;
    };
    let end = lines[heading + 1..]
        .iter()
        .position(|line| line.starts_with("# ") || line.starts_with("## "))
        .map_or(lines.len(), |at| heading + 1 + at);
    let last = (heading + 1..end)
        .rev()
        .find(|&at| !lines[at].trim().is_empty());
    let mut output = String::with_capacity(text.len() + items.len() + 4);
    for (at, line) in lines.iter().enumerate() {
        output.push_str(line);
        let after = last.unwrap_or(heading);
        if at == after {
            if !line.ends_with('\n') {
                output.push_str(newline);
            }
            if last.is_none() {
                output.push_str(newline);
            }
            output.push_str(&items);
        }
    }
    output
}

/// Places the finding at `path` (relative to `root`) with `note`'s home and links (new notes
/// aren't linked). Returns the note as it is after. `Edited` when it changed while being
/// placed, so it can be tried again.
pub fn place(root: &Path, path: &Path, note: &NoteJson) -> Result<Written, ReplaceError> {
    let absolute = root.join(path);
    let read = || {
        let stamp = Stamp::of(&absolute);
        let text = std::fs::read_to_string(&absolute);
        match (stamp, text) {
            (Some(stamp), Ok(text)) => Ok((stamp, text)),
            (_, Err(error)) => Err(ReplaceError::Failed(format!(
                "FastCapture could not read {}: {error}",
                absolute.display()
            ))),
            (None, Ok(_)) => Err(ReplaceError::Failed(format!(
                "{} vanished while being read",
                absolute.display()
            ))),
        }
    };
    let (stamp, text) = read()?;
    let unchanged = Written {
        path: path.to_path_buf(),
        stamp,
    };
    if !matches!(findings::classify(&text), Some(Finding::Ready { .. })) {
        return Ok(unchanged);
    }
    let mut placed = text.clone();
    if let Some(home) = &note.home {
        let value = if home.kind == HOME_FOLDER {
            home.note.clone()
        } else {
            format!("[[{}]]", home.note)
        };
        if let Some(with_home) =
            front::set_suggested_home(&placed, &crate::capture::raw::quoted(&value))
        {
            placed = with_home;
        }
    }
    let links: Vec<String> = note
        .links
        .iter()
        .filter(|link| !link.new)
        .map(|link| link.note.clone())
        .collect();
    placed = add_related(&placed, &links);
    if placed == text {
        return Ok(unchanged);
    }
    if Stamp::of(&absolute) != Some(stamp) {
        return Err(ReplaceError::Edited);
    }
    saver::save_atomic(&absolute, placed.as_bytes()).map_err(|error| {
        ReplaceError::Failed(format!(
            "FastCapture could not save {}: {error}",
            absolute.display()
        ))
    })?;
    let stamp = Stamp::of(&absolute).ok_or_else(|| {
        ReplaceError::Failed(format!("{} vanished after writing", absolute.display()))
    })?;
    Ok(Written {
        path: path.to_path_buf(),
        stamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::reply::{HOME_TOPIC, Home, Link};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn links_go_at_the_end_of_the_related_section_once() {
        let note = "# Lag\n\nBody.\n\n## Related\n\n- [[Ledger batch]]\n\n## Later\n\nMore.\n";
        assert_eq!(
            add_related(note, &names(&["ledger batch", "Kafka consumer settings"])),
            "# Lag\n\nBody.\n\n## Related\n\n- [[Ledger batch]]\n- [[Kafka consumer settings]]\n\n## Later\n\nMore.\n"
        );
        assert_eq!(add_related(note, &names(&["Ledger batch"])), note);
    }

    #[test]
    fn without_a_related_section_one_is_added_at_the_end() {
        assert_eq!(
            add_related(
                "# Lag\r\n\r\nBody.",
                &names(&["Ledger batch", "Ledger batch"])
            ),
            "# Lag\r\n\r\nBody.\r\n\r\n## Related\r\n\r\n- [[Ledger batch]]\r\n"
        );
        assert_eq!(
            add_related("# Lag\n\n## Related\n", &names(&["A"])),
            "# Lag\n\n## Related\n\n- [[A]]\n"
        );
        assert_eq!(
            add_related("See [[A|the batch]].\n", &names(&["A"])),
            "See [[A|the batch]].\n"
        );
    }

    struct Temp(std::path::PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp(name: &str, text: &str) -> Temp {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-place-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inbox")).unwrap();
        std::fs::write(dir.join(r"inbox\Lag.md"), text).unwrap();
        Temp(dir)
    }

    fn reply() -> NoteJson {
        NoteJson {
            links: vec![
                Link {
                    note: "Ledger batch".to_owned(),
                    new: false,
                },
                Link {
                    note: "Brand new idea".to_owned(),
                    new: true,
                },
            ],
            home: Some(Home {
                kind: HOME_TOPIC.to_owned(),
                note: "Payments incidents".to_owned(),
                reason: "about payments".to_owned(),
            }),
            ..NoteJson::default()
        }
    }

    #[test]
    fn a_finding_gets_its_home_and_links_and_nothing_else_changes() {
        let text = "---\ntype: finding\nauthor: claude-code\nstatus: inbox\n---\n\n# Lag\n\nWhat we tried.\n";
        let temp = temp("finding", text);
        let written = place(&temp.0, Path::new(r"inbox\Lag.md"), &reply()).unwrap();
        assert_eq!(written.path, Path::new(r"inbox\Lag.md"));
        assert_eq!(
            std::fs::read_to_string(temp.0.join(r"inbox\Lag.md")).unwrap(),
            "---\ntype: finding\nauthor: claude-code\nsuggested_home: \"[[Payments incidents]]\"\nstatus: inbox\n---\n\n# Lag\n\nWhat we tried.\n\n## Related\n\n- [[Ledger batch]]\n"
        );
    }

    #[test]
    fn a_note_no_longer_in_the_inbox_is_left_alone() {
        let text = "---\ntype: finding\nstatus: kept\n---\n\n# Lag\n";
        let temp = temp("kept", text);
        place(&temp.0, Path::new(r"inbox\Lag.md"), &reply()).unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.0.join(r"inbox\Lag.md")).unwrap(),
            text
        );
    }
}
