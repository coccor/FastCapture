//! What filing, keeping or discarding a note did, so it can be put back while the files are
//! still as FastCapture left them. A file changed since is never overwritten.

use crate::note::write::{Stamp, Written};
use crate::platform::saver;
use std::path::{Path, PathBuf};

/// A topic note's text before a line was appended, and its stamp after.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopicChange {
    /// Relative to the notebook.
    pub path: PathBuf,
    pub before: String,
    pub after: Stamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Undo {
    /// The note was filed: moved from `from`, whose text was `text`, to `to`, and maybe listed
    /// in a topic note.
    Filed {
        from: PathBuf,
        text: String,
        to: Written,
        topic: Option<TopicChange>,
    },
    /// The note was kept in the inbox: its text before.
    Kept { note: Written, text: String },
    /// The note went to the Recycle Bin: its path and text.
    Discarded { path: PathBuf, text: String },
}

impl Undo {
    /// Where the note is now, if anywhere.
    pub fn current(&self) -> Option<&Path> {
        match self {
            Self::Filed { to, .. } => Some(&to.path),
            Self::Kept { note, .. } => Some(&note.path),
            Self::Discarded { .. } => None,
        }
    }
}

/// What to keep to undo a discard of the note at `note` (relative to `root`): read before it goes.
pub fn before_discard(root: &Path, note: &Path) -> Result<Undo, String> {
    let text = std::fs::read_to_string(root.join(note))
        .map_err(|error| format!("FastCapture could not read {}: {error}", note.display()))?;
    Ok(Undo::Discarded {
        path: note.to_path_buf(),
        text,
    })
}

/// Puts back what `undo` records, in the notebook at `root`. Returns the note where it is again.
pub fn undo(root: &Path, undo: &Undo) -> Result<Written, String> {
    let unchanged = |written: &Written| Stamp::of(&root.join(&written.path)) == Some(written.stamp);
    let changed = |path: &Path| {
        format!(
            "{} changed since, so it wasn't put back.",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    };
    match undo {
        Undo::Filed {
            from,
            text,
            to,
            topic,
        } => {
            if !unchanged(to) {
                return Err(changed(&to.path));
            }
            if let Some(topic) = topic
                && Stamp::of(&root.join(&topic.path)) != Some(topic.after)
            {
                return Err(changed(&topic.path));
            }
            let back = if same(from, &to.path) {
                overwrite(root, from, text)?
            } else {
                let back = write_new(root, from, text)?;
                if let Err(error) = std::fs::remove_file(root.join(&to.path)) {
                    crate::log::line(&format!("undo: {}: {error}", to.path.display()));
                }
                back
            };
            if let Some(topic) = topic {
                overwrite(root, &topic.path, &topic.before)?;
            }
            Ok(back)
        }
        Undo::Kept { note, text } => {
            if !unchanged(note) {
                return Err(changed(&note.path));
            }
            overwrite(root, &note.path, text)
        }
        Undo::Discarded { path, text } => write_new(root, path, text),
    }
}

pub(crate) fn same(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// `text` saved over the file at `path` (relative to `root`).
pub(crate) fn overwrite(root: &Path, path: &Path, text: &str) -> Result<Written, String> {
    let absolute = root.join(path);
    saver::save_atomic(&absolute, text.as_bytes())
        .map_err(|error| format!("FastCapture could not save {}: {error}", absolute.display()))?;
    stamped(root, path)
}

/// `text` saved as a new file at `path` (relative to `root`), which must not exist.
pub(crate) fn write_new(root: &Path, path: &Path, text: &str) -> Result<Written, String> {
    let absolute = root.join(path);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("FastCapture could not create {}: {error}", parent.display())
        })?;
    }
    saver::save_atomic_new(&absolute, text.as_bytes()).map_err(|error| {
        if saver::is_already_exists(&error) {
            format!("{} is taken by another file.", path.display())
        } else {
            format!("FastCapture could not save {}: {error}", absolute.display())
        }
    })?;
    stamped(root, path)
}

fn stamped(root: &Path, path: &Path) -> Result<Written, String> {
    let stamp = Stamp::of(&root.join(path))
        .ok_or_else(|| format!("{} vanished after writing", path.display()))?;
    Ok(Written {
        path: path.to_path_buf(),
        stamp,
    })
}
