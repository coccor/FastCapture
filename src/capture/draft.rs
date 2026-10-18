//! The capture box's draft (spec §5.2): what was in the box when it closed without saving, kept
//! in `%LocalAppData%\FastCapture\draft` and brought back the next time it opens. Text goes in
//! `draft.txt`; pasted images are saved beside it as PNGs; `attachments.txt` lists the
//! attachments in order, `png:<name>` or `file:<path>` (dropped files are kept as their paths).

use super::model::{Attachment, Capture};
use std::fs;
use std::path::{Path, PathBuf};

const TEXT_FILE: &str = "draft.txt";
const LIST_FILE: &str = "attachments.txt";

/// `%LocalAppData%\FastCapture\draft`.
pub fn default_dir() -> Option<PathBuf> {
    crate::paths::local().map(|local| local.join("draft"))
}

/// Saves `capture` as the draft in `dir`, replacing any earlier one. An empty capture clears it.
pub fn save(dir: &Path, capture: &Capture) -> std::io::Result<()> {
    clear(dir);
    if capture.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let mut list = String::new();
    let mut image = 0;
    for attachment in &capture.attachments {
        match attachment {
            Attachment::Png(bytes) => {
                image += 1;
                let name = format!("image {image}.png");
                fs::write(dir.join(&name), bytes)?;
                list.push_str(&format!("png:{name}\n"));
            }
            Attachment::File(path) => list.push_str(&format!("file:{}\n", path.display())),
        }
    }
    fs::write(dir.join(LIST_FILE), list)?;
    // The text last, so a draft is only there once it is whole.
    fs::write(dir.join(TEXT_FILE), &capture.text)
}

/// The draft in `dir`, or `None` when there is none. Pasted images that went missing are left
/// out; dropped files are kept even if they've moved, so the box can say so.
pub fn load(dir: &Path) -> Option<Capture> {
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
    let capture = Capture {
        text,
        attachments,
        source_window: None,
    };
    (!capture.is_empty()).then_some(capture)
}

/// Removes the draft in `dir`.
pub fn clear(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
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
}
