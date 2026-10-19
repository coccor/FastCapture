//! Filing notes: their homes, moving them there (into a folder, or beside a topic note that
//! lists them), keeping them in the inbox, and putting them back.

pub mod front;
pub mod home;
pub mod move_note;
pub mod topic;
pub mod undo;

use crate::note::write::{Stamp, Written};
use crate::platform::saver;
use std::path::{Path, PathBuf};
use undo::{TopicChange, Undo};

/// A file that changes while FastCapture writes it is read and written again this many times.
const TRIES: usize = 3;

/// Where a note is filed, relative to the notebook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Folder(PathBuf),
    /// A topic note: the note moves into its folder and gets a line in it.
    Topic(PathBuf),
}

/// Files the note at `note` (relative to the notebook at `root`) under `target` on `today`
/// (`YYYY-MM-DD`): `status: filed`, its suggestion removed, moved into the folder under a free
/// name with its links kept working, and listed in the topic note as `link_name` names it from
/// there. A topic note that can't be written leaves the note where it was.
pub fn file_note(
    root: &Path,
    note: &Path,
    target: &Target,
    today: &str,
    link_name: &dyn Fn(&Path) -> String,
) -> Result<(Written, Undo), String> {
    let folder = match target {
        Target::Folder(folder) => folder.clone(),
        Target::Topic(topic) => topic.parent().unwrap_or(Path::new("")).to_path_buf(),
    };
    let exists = |path: &Path| root.join(path).exists();
    for _ in 0..TRIES {
        let (text, stamp) = read(root, note)?;
        let in_place = note
            .parent()
            .is_some_and(|parent| undo::same(parent, &folder));
        let to = if in_place {
            note.to_path_buf()
        } else {
            move_note::destination(root, note, &folder)
        };
        let filed = front::set_status(&text, "filed")
            .ok_or_else(|| format!("{} has no front matter.", note.display()))?;
        let moved = move_note::moved_text(&filed, note, &to, &exists);

        let topic = match target {
            Target::Topic(topic) => {
                let line =
                    topic::topic_line(today, &link_name(&to), front::summary(&text).as_deref());
                let (before, after) =
                    update(root, topic, &|existing| topic::appended(existing, &line)).map_err(
                        |error| {
                            format!(
                                "Couldn't add the note to {}: {error}",
                                topic.file_stem().unwrap_or_default().to_string_lossy()
                            )
                        },
                    )?;
                Some(TopicChange {
                    path: topic.clone(),
                    before,
                    after,
                })
            }
            Target::Folder(_) => None,
        };
        let put_topic_back = |topic: &Option<TopicChange>| {
            if let Some(topic) = topic
                && Stamp::of(&root.join(&topic.path)) == Some(topic.after)
                && let Err(error) = undo::overwrite(root, &topic.path, &topic.before)
            {
                crate::log::line(&format!("file: {}: {error}", topic.path.display()));
            }
        };

        // The note must not change between reading and replacing it.
        if Stamp::of(&root.join(note)) != Some(stamp) {
            put_topic_back(&topic);
            continue;
        }
        let written = if in_place {
            undo::overwrite(root, note, &moved)
        } else {
            undo::write_new(root, &to, &moved)
        };
        let written = match written {
            Ok(written) => written,
            Err(error) => {
                put_topic_back(&topic);
                // Another file took the name meanwhile: try again under the next one.
                if root.join(&to).exists() && !in_place {
                    continue;
                }
                return Err(error);
            }
        };
        if !in_place {
            if Stamp::of(&root.join(note)) != Some(stamp) {
                let _ = std::fs::remove_file(root.join(&to));
                put_topic_back(&topic);
                continue;
            }
            std::fs::remove_file(root.join(note)).map_err(|error| {
                let _ = std::fs::remove_file(root.join(&to));
                put_topic_back(&topic);
                format!("FastCapture could not move {}: {error}", note.display())
            })?;
        }
        return Ok((
            written.clone(),
            Undo::Filed {
                from: note.to_path_buf(),
                text,
                to: written,
                topic,
            },
        ));
    }
    Err(format!(
        "{} kept changing while it was filed.",
        note.display()
    ))
}

/// Keeps the note at `note` in the inbox: `status: kept`, its suggestion removed.
pub fn keep_note(root: &Path, note: &Path) -> Result<(Written, Undo), String> {
    let (current, _) = read(root, note)?;
    if front::set_status(&current, "kept").is_none() {
        return Err(format!("{} has no front matter.", note.display()));
    }
    let (text, after) = update(root, note, &|text| {
        front::set_status(text, "kept").unwrap_or_else(|| text.to_owned())
    })?;
    let written = Written {
        path: note.to_path_buf(),
        stamp: after,
    };
    Ok((
        written.clone(),
        Undo::Kept {
            note: written,
            text,
        },
    ))
}

/// A file's text and stamp, read together.
fn read(root: &Path, path: &Path) -> Result<(String, Stamp), String> {
    let absolute = root.join(path);
    for _ in 0..TRIES {
        let before = Stamp::of(&absolute)
            .ok_or_else(|| format!("{} isn't there any more.", path.display()))?;
        let text = std::fs::read_to_string(&absolute)
            .map_err(|error| format!("FastCapture could not read {}: {error}", path.display()))?;
        if Stamp::of(&absolute) == Some(before) {
            return Ok((text, before));
        }
    }
    Err(format!(
        "{} kept changing while it was read.",
        path.display()
    ))
}

/// Writes `change` of the file at `path` over it, unless it changes between reading and
/// writing, then tries again. Returns its text before and its stamp after.
fn update(
    root: &Path,
    path: &Path,
    change: &dyn Fn(&str) -> String,
) -> Result<(String, Stamp), String> {
    let absolute = root.join(path);
    for _ in 0..TRIES {
        let (text, stamp) = read(root, path)?;
        let changed = change(&text);
        if Stamp::of(&absolute) != Some(stamp) {
            continue;
        }
        saver::save_atomic(&absolute, changed.as_bytes())
            .map_err(|error| format!("FastCapture could not save {}: {error}", path.display()))?;
        let after = Stamp::of(&absolute)
            .ok_or_else(|| format!("{} vanished after writing", path.display()))?;
        return Ok((text, after));
    }
    Err(format!(
        "{} kept changing while it was written.",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::scan::tests::Scratch;
    use std::cell::Cell;
    use std::fs;

    const NOTE: &str = "---\r\ntype: capture\r\nsummary: \"Lag hit 40k during the batch.\"\r\nsuggested_home: \"[[Payments incidents]]\"\r\nstatus: inbox\r\n---\r\n\r\n# Lag spikes\r\n\r\n![shot](../attachments/lag%20shot.png)\r\n";

    fn notebook(label: &str) -> Scratch {
        let scratch = Scratch::new(label);
        scratch.file(r"inbox\Lag spikes.md", NOTE);
        scratch.file(r"attachments\lag shot.png", "png");
        scratch
    }

    fn text(scratch: &Scratch, path: &str) -> String {
        fs::read_to_string(scratch.0.join(path)).unwrap()
    }

    fn stem(path: &Path) -> String {
        path.file_stem().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn filing_into_a_folder_moves_the_note_and_keeps_its_links() {
        let scratch = notebook("file-folder");
        let note = Path::new(r"inbox\Lag spikes.md");
        let (written, undo) = file_note(
            &scratch.0,
            note,
            &Target::Folder(PathBuf::from(r"Payments\Incidents")),
            "2026-10-19",
            &stem,
        )
        .unwrap();
        assert_eq!(
            written.path,
            PathBuf::from(r"Payments\Incidents\Lag spikes.md")
        );
        assert!(!scratch.0.join(note).exists());
        let moved = text(&scratch, r"Payments\Incidents\Lag spikes.md");
        assert_eq!(
            moved,
            "---\r\ntype: capture\r\nsummary: \"Lag hit 40k during the batch.\"\r\nstatus: filed\r\n---\r\n\r\n# Lag spikes\r\n\r\n![shot](../../attachments/lag%20shot.png)\r\n"
        );

        // Undo puts it back as it was.
        let back = undo::undo(&scratch.0, &undo).unwrap();
        assert_eq!(back.path, note);
        assert_eq!(text(&scratch, r"inbox\Lag spikes.md"), NOTE);
        assert!(!scratch.0.join(&written.path).exists());
    }

    #[test]
    fn a_name_taken_in_the_folder_gets_a_number() {
        let scratch = notebook("file-clash");
        scratch.file(r"Payments\Lag spikes.md", "mine");
        let (written, _) = file_note(
            &scratch.0,
            Path::new(r"inbox\Lag spikes.md"),
            &Target::Folder(PathBuf::from("Payments")),
            "2026-10-19",
            &stem,
        )
        .unwrap();
        assert_eq!(written.path, PathBuf::from(r"Payments\Lag spikes 2.md"));
        assert_eq!(text(&scratch, r"Payments\Lag spikes.md"), "mine");
    }

    #[test]
    fn filing_under_a_topic_lists_the_note_there_and_undo_takes_it_out() {
        let scratch = notebook("file-topic");
        let topic = "# Payments incidents\r\n\r\n- 2026-10-01 [[Old outage]]";
        scratch.file(r"Payments\Payments incidents.md", topic);
        let (written, undo) = file_note(
            &scratch.0,
            Path::new(r"inbox\Lag spikes.md"),
            &Target::Topic(PathBuf::from(r"Payments\Payments incidents.md")),
            "2026-10-19",
            &stem,
        )
        .unwrap();
        assert_eq!(written.path, PathBuf::from(r"Payments\Lag spikes.md"));
        assert_eq!(
            text(&scratch, r"Payments\Payments incidents.md"),
            "# Payments incidents\r\n\r\n- 2026-10-01 [[Old outage]]\r\n- 2026-10-19 [[Lag spikes]]: Lag hit 40k during the batch.\r\n"
        );
        assert!(
            text(&scratch, r"Payments\Lag spikes.md").contains("](../attachments/lag%20shot.png)")
        );

        undo::undo(&scratch.0, &undo).unwrap();
        assert_eq!(text(&scratch, r"Payments\Payments incidents.md"), topic);
        assert_eq!(text(&scratch, r"inbox\Lag spikes.md"), NOTE);
        assert!(!scratch.0.join(r"Payments\Lag spikes.md").exists());
    }

    #[test]
    fn a_topic_note_that_cant_be_written_leaves_the_note_in_the_inbox() {
        let scratch = notebook("file-no-topic");
        let error = file_note(
            &scratch.0,
            Path::new(r"inbox\Lag spikes.md"),
            &Target::Topic(PathBuf::from(r"Payments\Gone.md")),
            "2026-10-19",
            &stem,
        )
        .unwrap_err();
        assert!(
            error.starts_with("Couldn't add the note to Gone"),
            "{error}"
        );
        assert_eq!(text(&scratch, r"inbox\Lag spikes.md"), NOTE);
    }

    #[test]
    fn filing_in_its_own_folder_only_changes_its_status() {
        let scratch = notebook("file-in-place");
        let (written, undo) = file_note(
            &scratch.0,
            Path::new(r"inbox\Lag spikes.md"),
            &Target::Folder(PathBuf::from("Inbox")),
            "2026-10-19",
            &stem,
        )
        .unwrap();
        assert_eq!(written.path, PathBuf::from(r"inbox\Lag spikes.md"));
        assert!(text(&scratch, r"inbox\Lag spikes.md").contains("status: filed\r\n"));
        undo::undo(&scratch.0, &undo).unwrap();
        assert_eq!(text(&scratch, r"inbox\Lag spikes.md"), NOTE);
    }

    #[test]
    fn undo_is_refused_when_a_file_changed_since() {
        let scratch = notebook("file-undo-changed");
        let (written, undo) = file_note(
            &scratch.0,
            Path::new(r"inbox\Lag spikes.md"),
            &Target::Folder(PathBuf::from("Payments")),
            "2026-10-19",
            &stem,
        )
        .unwrap();
        fs::write(scratch.0.join(&written.path), "edited after filing, longer").unwrap();
        let error = undo::undo(&scratch.0, &undo).unwrap_err();
        assert!(error.contains("changed since"), "{error}");
        assert!(!scratch.0.join(r"inbox\Lag spikes.md").exists());
        assert_eq!(
            text(&scratch, r"Payments\Lag spikes.md"),
            "edited after filing, longer"
        );
    }

    #[test]
    fn keeping_and_discarding_can_be_undone() {
        let scratch = notebook("file-keep");
        let note = Path::new(r"inbox\Lag spikes.md");
        let (_, kept) = keep_note(&scratch.0, note).unwrap();
        let after = text(&scratch, r"inbox\Lag spikes.md");
        assert!(after.contains("status: kept\r\n") && !after.contains("suggested_home"));
        undo::undo(&scratch.0, &kept).unwrap();
        assert_eq!(text(&scratch, r"inbox\Lag spikes.md"), NOTE);

        let discarded = undo::before_discard(&scratch.0, note).unwrap();
        fs::remove_file(scratch.0.join(note)).unwrap();
        let back = undo::undo(&scratch.0, &discarded).unwrap();
        assert_eq!(back.path, note);
        assert_eq!(text(&scratch, r"inbox\Lag spikes.md"), NOTE);
        // Something new in its place: the discard isn't undone over it.
        assert!(undo::undo(&scratch.0, &discarded).is_err());
    }

    #[test]
    fn a_file_changed_between_read_and_write_is_read_again() {
        let scratch = notebook("file-update");
        scratch.file("topic.md", "- one\n");
        let calls = Cell::new(0);
        let path = scratch.0.join("topic.md");
        let (before, _) = update(&scratch.0, Path::new("topic.md"), &|text| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                // Someone saves the note meanwhile.
                std::thread::sleep(std::time::Duration::from_millis(20));
                fs::write(&path, "- one\n- two, added elsewhere\n").unwrap();
            }
            topic::appended(text, "- three")
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(before, "- one\n- two, added elsewhere\n");
        assert_eq!(
            text(&scratch, "topic.md"),
            "- one\n- two, added elsewhere\n- three\n"
        );
    }
}
