//! Writing notes into the notebook's capture folder. A new note never replaces a file; a
//! regenerated one replaces only the note FastCapture wrote, and not once you've edited it.

use crate::notebook::title;
use crate::platform::saver;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What a file looked like when FastCapture last wrote it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stamp {
    pub len: u64,
    /// Last write time, milliseconds since 1970.
    pub modified_ms: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let modified_ms = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        Some(Self {
            len: meta.len(),
            modified_ms,
        })
    }
}

/// A note on disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Written {
    /// Relative to the notebook.
    pub path: PathBuf,
    pub stamp: Stamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplaceError {
    /// The note changed since FastCapture wrote it: you edited it.
    Edited,
    Failed(String),
}

impl std::fmt::Display for ReplaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Edited => f.write_str("the note was edited since FastCapture wrote it"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Where a note named after `stem` would go in `folder` (relative to the notebook at `root`):
/// the first free `stem.md`, `stem 2.md`, …
pub fn note_path(root: &Path, folder: &Path, stem: &str) -> PathBuf {
    let directory = root.join(folder);
    folder.join(title::free_name(stem, "md", |name| {
        directory.join(name).exists()
    }))
}

/// Writes `text` as a new note at `path` (relative), or at the next free name if something took
/// it meanwhile. Returns where it went.
pub fn write_new(root: &Path, path: &Path, text: &str) -> Result<Written, String> {
    let folder = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned());
    let directory = root.join(folder);
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "FastCapture could not create the folder {}: {error}",
            directory.display()
        )
    })?;
    let mut target = path.to_path_buf();
    loop {
        let absolute = root.join(&target);
        match saver::save_atomic_new(&absolute, text.as_bytes()) {
            Ok(()) => {
                let stamp = Stamp::of(&absolute)
                    .ok_or_else(|| format!("{} vanished after writing", absolute.display()))?;
                return Ok(Written {
                    path: target,
                    stamp,
                });
            }
            Err(error) if saver::is_already_exists(&error) => {
                target = note_path(root, folder, &stem);
            }
            Err(error) => {
                return Err(format!(
                    "FastCapture could not save {}: {error}",
                    absolute.display()
                ));
            }
        }
    }
}

/// Replaces the note `old` with `text`, keeping its file name, unless it was edited. A note that
/// is gone is written again.
pub fn replace(root: &Path, old: &Written, text: &str) -> Result<Written, ReplaceError> {
    let absolute = root.join(&old.path);
    match Stamp::of(&absolute) {
        None => return write_new(root, &old.path, text).map_err(ReplaceError::Failed),
        Some(stamp) if stamp != old.stamp => return Err(ReplaceError::Edited),
        Some(_) => {}
    }
    saver::save_atomic(&absolute, text.as_bytes()).map_err(|error| {
        ReplaceError::Failed(format!(
            "FastCapture could not save {}: {error}",
            absolute.display()
        ))
    })?;
    let stamp = Stamp::of(&absolute).ok_or_else(|| {
        ReplaceError::Failed(format!("{} vanished after writing", absolute.display()))
    })?;
    Ok(Written {
        path: old.path.clone(),
        stamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("fastcapture-note-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn new_notes_take_the_next_free_name() {
        let temp = Temp::new("new");
        let path = note_path(&temp.0, Path::new("inbox"), "Plan");
        assert_eq!(path, PathBuf::from(r"inbox\Plan.md"));
        let first = write_new(&temp.0, &path, "one").unwrap();
        assert_eq!(first.path, PathBuf::from(r"inbox\Plan.md"));
        // The same name again, as when two notes are written from one stale path.
        let second = write_new(&temp.0, &path, "two").unwrap();
        assert_eq!(second.path, PathBuf::from(r"inbox\Plan 2.md"));
        assert_eq!(fs::read_to_string(temp.0.join(&first.path)).unwrap(), "one");
        assert_eq!(
            note_path(&temp.0, Path::new("inbox"), "Plan"),
            PathBuf::from(r"inbox\Plan 3.md")
        );
    }

    #[test]
    fn regenerating_replaces_an_unedited_note_and_refuses_an_edited_one() {
        let temp = Temp::new("replace");
        let written = write_new(&temp.0, Path::new(r"inbox\Plan.md"), "first draft").unwrap();
        let again = replace(&temp.0, &written, "second draft").unwrap();
        assert_eq!(again.path, written.path);
        assert_eq!(
            fs::read_to_string(temp.0.join(&again.path)).unwrap(),
            "second draft"
        );

        fs::write(temp.0.join(&again.path), "my own edits, a little longer").unwrap();
        assert_eq!(replace(&temp.0, &again, "third"), Err(ReplaceError::Edited));
        assert_eq!(
            fs::read_to_string(temp.0.join(&again.path)).unwrap(),
            "my own edits, a little longer"
        );

        fs::remove_file(temp.0.join(&again.path)).unwrap();
        let back = replace(&temp.0, &again, "rewritten").unwrap();
        assert_eq!(
            fs::read_to_string(temp.0.join(&back.path)).unwrap(),
            "rewritten"
        );
    }
}
