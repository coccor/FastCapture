//! Copied from FastPad `src/library/capture.rs` (`obsidian_attachments`, `attachments_folder`).
//!
//! Where attachments go: `notebook.ini`'s `attachments_folder`, else Obsidian's
//! `attachmentFolderPath` from `.obsidian\app.json`, else `attachments`.
//! FastCapture only reads `app.json`, never writes it.

use std::path::{Path, PathBuf};

/// Obsidian's folder inside a vault, and its settings file there.
pub const OBSIDIAN_DIR: &str = ".obsidian";
pub const APP_FILE: &str = "app.json";

/// Where Obsidian keeps attachments, from `.obsidian\app.json`'s `attachmentFolderPath`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObsidianAttachments {
    /// `/`: the notebook root.
    Root,
    /// A folder inside the notebook.
    Folder(PathBuf),
    /// `./`: next to the note.
    NextToNote,
    /// `./<folder>`: in that folder under the note's folder.
    UnderNote(PathBuf),
}

/// Reads `attachmentFolderPath` from the text of `.obsidian\app.json`. A path that would leave
/// the notebook gives `None`, as does a file without the key.
pub fn obsidian_attachments(json: &str) -> Option<ObsidianAttachments> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let path = value.get("attachmentFolderPath")?.as_str()?.trim();
    let inside = |rest: &str| -> Option<PathBuf> {
        let mut out = PathBuf::new();
        for part in rest
            .split(['/', '\\'])
            .filter(|part| !part.is_empty() && *part != ".")
        {
            if part == ".." || part.contains(':') {
                return None;
            }
            out.push(part);
        }
        Some(out)
    };
    if path.is_empty() || path == "/" {
        return Some(ObsidianAttachments::Root);
    }
    if path == "./" || path == "." {
        return Some(ObsidianAttachments::NextToNote);
    }
    if let Some(rest) = path.strip_prefix("./") {
        let folder = inside(rest)?;
        return Some(if folder.as_os_str().is_empty() {
            ObsidianAttachments::NextToNote
        } else {
            ObsidianAttachments::UnderNote(folder)
        });
    }
    let folder = inside(path)?;
    Some(if folder.as_os_str().is_empty() {
        ObsidianAttachments::Root
    } else {
        ObsidianAttachments::Folder(folder)
    })
}

/// The folder, relative to the notebook, that attachments for the note at `note` (relative) go
/// in: `notebook.ini`'s `attachments_folder`, else Obsidian's setting, else `attachments`.
pub fn attachments_folder(
    configured: Option<&Path>,
    obsidian: Option<&ObsidianAttachments>,
    note: &Path,
) -> PathBuf {
    if let Some(configured) = configured {
        return configured.to_path_buf();
    }
    let note_folder = note.parent().unwrap_or(Path::new(""));
    match obsidian {
        Some(ObsidianAttachments::Root) => PathBuf::new(),
        Some(ObsidianAttachments::Folder(folder)) => folder.clone(),
        Some(ObsidianAttachments::NextToNote) => note_folder.to_path_buf(),
        Some(ObsidianAttachments::UnderNote(folder)) => note_folder.join(folder),
        None => PathBuf::from("attachments"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidians_attachment_folder_is_honoured_inside_the_vault() {
        let read =
            |path: &str| obsidian_attachments(&format!("{{\"attachmentFolderPath\": \"{path}\"}}"));
        assert_eq!(read("/"), Some(ObsidianAttachments::Root));
        assert_eq!(read("./"), Some(ObsidianAttachments::NextToNote));
        assert_eq!(
            read("./img"),
            Some(ObsidianAttachments::UnderNote(PathBuf::from("img")))
        );
        assert_eq!(
            read("assets/img"),
            Some(ObsidianAttachments::Folder(PathBuf::from(r"assets\img")))
        );
        assert_eq!(read("../outside"), None);
        assert_eq!(read("C:/elsewhere"), None);
        assert_eq!(obsidian_attachments("{}"), None);

        let note = Path::new(r"work\plan.md");
        assert_eq!(
            attachments_folder(None, None, note),
            PathBuf::from("attachments")
        );
        assert_eq!(
            attachments_folder(Some(Path::new("files")), read("/").as_ref(), note),
            PathBuf::from("files")
        );
        assert_eq!(
            attachments_folder(None, read("./img").as_ref(), note),
            PathBuf::from(r"work\img")
        );
    }
}
