//! The capture box's draft: what was in the box when it closed without saving, kept
//! in `%LocalAppData%\FastCapture\draft` and brought back the next time it opens. Text goes in
//! `draft.txt`; pasted images are saved beside it as PNGs; `attachments.txt` lists the
//! attachments in order, `png:<name>` or `file:<path>` (dropped files are kept as their paths),
//! and `source_url:<url>` for pasted rich text's page.

use super::model::{Attachment, Capture};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const TEXT_FILE: &str = "draft.txt";
const LIST_FILE: &str = "attachments.txt";

/// `%LocalAppData%\FastCapture\draft`.
pub fn default_dir() -> Option<PathBuf> {
    crate::paths::local().map(|local| local.join("draft"))
}

/// Saves `capture` as the draft in `dir`, replacing any earlier one. An empty capture clears it.
///
/// The new draft is written beside the old one and swapped in, so a failure part-way leaves the
/// earlier draft in place; a crash between the two moves leaves it as `<dir>.old`, which `load`
/// falls back to.
pub fn save(dir: &Path, capture: &Capture) -> std::io::Result<()> {
    if capture.is_empty() {
        clear(dir);
        return Ok(());
    }
    let new = sibling(dir, "new");
    let _ = fs::remove_dir_all(&new);
    let result = write_folder(&new, capture).and_then(|()| swap_in(dir, &new));
    if result.is_err() {
        let _ = fs::remove_dir_all(&new);
    }
    result
}

/// Moves the finished folder `new` into place as `dir`, keeping the draft it replaces as
/// `<dir>.old` until the move is done.
fn swap_in(dir: &Path, new: &Path) -> std::io::Result<()> {
    let old = sibling(dir, "old");
    if dir.exists() {
        let _ = fs::remove_dir_all(&old);
        fs::rename(dir, &old)?;
    }
    if let Err(error) = fs::rename(new, dir) {
        // Put the earlier draft back rather than leave only `<dir>.old`.
        let _ = fs::rename(&old, dir);
        return Err(error);
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// `dir` with `.{suffix}` added to its name.
fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes `capture` into the new folder `dir`, each file flushed to disk.
fn write_folder(dir: &Path, capture: &Capture) -> std::io::Result<()> {
    fs::create_dir(dir)?;
    let mut list = String::new();
    let mut image = 0;
    for attachment in &capture.attachments {
        match attachment {
            Attachment::Png(bytes) => {
                image += 1;
                let name = format!("image {image}.png");
                write_synced(&dir.join(&name), bytes)?;
                list.push_str(&format!("png:{name}\n"));
            }
            Attachment::File(path) => list.push_str(&format!("file:{}\n", path.display())),
        }
    }
    if let Some(url) = &capture.source_url {
        list.push_str(&format!("source_url:{}\n", url.replace(['\r', '\n'], "")));
    }
    write_synced(&dir.join(LIST_FILE), list.as_bytes())?;
    // The text last, so a draft is only there once it is whole.
    write_synced(&dir.join(TEXT_FILE), capture.text.as_bytes())
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The draft in `dir`, or `None` when there is none. Pasted images that went missing are left
/// out; dropped files are kept even if they've moved, so the box can say so.
pub fn load(dir: &Path) -> Option<Capture> {
    read_folder(dir).or_else(|| read_folder(&sibling(dir, "old")))
}

fn read_folder(dir: &Path) -> Option<Capture> {
    let text = fs::read_to_string(dir.join(TEXT_FILE)).ok()?;
    let list = fs::read_to_string(dir.join(LIST_FILE)).unwrap_or_default();
    let attachments = list
        .lines()
        .filter_map(|line| {
            if let Some(name) = line.strip_prefix("png:") {
                // Only names this module wrote: no paths out of the folder.
                if name.contains(['/', '\\']) {
                    return None;
                }
                fs::read(dir.join(name)).ok().map(Attachment::Png)
            } else {
                line.strip_prefix("file:")
                    .map(|path| Attachment::File(PathBuf::from(path)))
            }
        })
        .collect();
    let source_url = list
        .lines()
        .find_map(|line| line.strip_prefix("source_url:"))
        .map(str::to_owned);
    let capture = Capture {
        text,
        attachments,
        source_window: None,
        source_url,
    };
    (!capture.is_empty()).then_some(capture)
}

/// Removes the draft in `dir`.
pub fn clear(dir: &Path) {
    for folder in [dir.to_owned(), sibling(dir, "new"), sibling(dir, "old")] {
        let _ = fs::remove_dir_all(folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-draft-{name}-{}", std::process::id()));
        clear(&dir);
        dir
    }

    #[test]
    fn a_draft_comes_back_as_it_was() {
        let dir = dir("round-trip");
        let capture = Capture {
            text: "half a thought\r\nsecond line".to_owned(),
            attachments: vec![
                Attachment::File(PathBuf::from(r"C:\logs\ledger.log")),
                Attachment::Png(b"one".to_vec()),
                Attachment::Png(b"two".to_vec()),
            ],
            source_window: Some("not kept".to_owned()),
            source_url: Some("https://example.com/kept".to_owned()),
        };
        save(&dir, &capture).unwrap();
        assert_eq!(
            load(&dir),
            Some(Capture {
                source_window: None,
                ..capture
            })
        );
        clear(&dir);
        assert_eq!(load(&dir), None);
    }

    #[test]
    fn an_empty_capture_clears_the_draft() {
        let dir = dir("empty");
        save(
            &dir,
            &Capture {
                text: "x".to_owned(),
                ..Capture::default()
            },
        )
        .unwrap();
        save(&dir, &Capture::default()).unwrap();
        assert_eq!(load(&dir), None);
        assert!(!dir.exists());
    }

    fn text(s: &str) -> Capture {
        Capture {
            text: s.to_owned(),
            ..Capture::default()
        }
    }

    #[test]
    fn a_failed_save_keeps_the_previous_draft() {
        let dir = dir("keep");
        save(&dir, &text("first")).unwrap();
        // A file where the new draft's folder goes makes the save fail before it swaps.
        fs::write(sibling(&dir, "new"), b"in the way").unwrap();
        assert!(save(&dir, &text("second")).is_err());
        assert_eq!(load(&dir).unwrap().text, "first");
        let _ = fs::remove_file(sibling(&dir, "new"));
        clear(&dir);
    }

    #[test]
    fn a_save_interrupted_after_the_old_draft_moved_still_loads_it() {
        let dir = dir("moved");
        save(&dir, &text("first")).unwrap();
        fs::rename(&dir, sibling(&dir, "old")).unwrap();
        assert_eq!(load(&dir).unwrap().text, "first");
        clear(&dir);
        assert!(!sibling(&dir, "old").exists());
    }

    #[test]
    fn a_save_leaves_no_sibling_folders() {
        let dir = dir("tidy");
        save(&dir, &text("one")).unwrap();
        save(&dir, &text("two")).unwrap();
        assert!(!sibling(&dir, "new").exists() && !sibling(&dir, "old").exists());
        assert_eq!(load(&dir).unwrap().text, "two");
        clear(&dir);
    }
}
