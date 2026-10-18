//! Copied from FastPad `src/file/saver.rs`.
//!
//! Atomic same-directory file save.

use super::{last_error, wide_path};
use crate::{Error, Result};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_LOCK_VIOLATION,
    ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_REMOVE_REPLACED,
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH, ReplaceFileW,
};

/// Writes `bytes` to `path` without ever exposing partial content.
///
/// The bytes are first written to a collision-resistant sibling temp file in the same directory,
/// `fsync`ed and closed, then atomically swapped into place: `ReplaceFileW` when `path` already
/// exists, `MoveFileExW` when it does not. A swap that another program blocks for a moment is
/// retried (`SWAP_RETRY_DELAYS_MS`). On any failure the original file (if any) is left untouched
/// and the sibling temp file is removed.
pub fn save_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_then_swap(path, bytes, false)
}

/// Like `save_atomic`, but never replaces a file: when `path` exists by the time the bytes are
/// swapped in, it fails with `ERROR_ALREADY_EXISTS` (see `is_already_exists`) and leaves that
/// file untouched. For a first save under a name the user just picked.
pub fn save_atomic_new(path: &Path, bytes: &[u8]) -> Result<()> {
    write_then_swap(path, bytes, true)
}

/// Whether `error` is `save_atomic_new` refusing an existing destination.
pub fn is_already_exists(error: &Error) -> bool {
    matches!(
        error,
        Error::Win32 { code, .. } if *code == ERROR_ALREADY_EXISTS || *code == ERROR_FILE_EXISTS
    )
}

/// The pauses before each retry of a swap that failed because another program had the file open
/// for a moment: antivirus, the search indexer and sync clients all open a file just written.
/// About 0.3 s in all, spent only when a swap fails.
const SWAP_RETRY_DELAYS_MS: [u64; 5] = [10, 20, 40, 80, 160];

/// Whether a failed swap may succeed if tried again shortly: the file was open elsewhere.
/// `ERROR_UNABLE_TO_MOVE_REPLACEMENT` left the destination deleted and the temp file in place, so
/// the retry moves the temp file in (`replace_or_move` sees no destination).
fn is_transient(error: &Error) -> bool {
    matches!(
        error,
        Error::Win32 { code, .. } if [
            ERROR_SHARING_VIOLATION,
            ERROR_LOCK_VIOLATION,
            ERROR_ACCESS_DENIED,
            ERROR_UNABLE_TO_REMOVE_REPLACED,
            ERROR_UNABLE_TO_MOVE_REPLACEMENT,
        ]
        .contains(code)
    )
}

fn with_retries(swap: impl Fn() -> Result<()>) -> Result<()> {
    let mut result = swap();
    for delay in SWAP_RETRY_DELAYS_MS {
        match &result {
            Err(error) if is_transient(error) => {}
            _ => break,
        }
        std::thread::sleep(std::time::Duration::from_millis(delay));
        result = swap();
    }
    result
}

fn write_then_swap(path: &Path, bytes: &[u8], create_new: bool) -> Result<()> {
    let temp = next_sibling_temp(path)?;
    let existed = path.exists();
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| Error::io(&temp, error))?;
        file.write_all(bytes)
            .map_err(|error| Error::io(&temp, error))?;
        file.sync_all().map_err(|error| Error::io(&temp, error))?;
        drop(file);
        if create_new {
            with_retries(|| move_without_replacing(&temp, path))
        } else {
            with_retries(|| replace_or_move(&temp, path))
        }
    })();
    // A replace that deleted the destination but could not move the temp file in leaves the
    // temp file as the only copy of the content: it stays rather than losing both.
    if result.is_err() && !(existed && !create_new && !path.exists()) {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn next_sibling_temp(path: &Path) -> Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| Error::Message(format!("{} has no file name", path.display())))?
        .to_string_lossy();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".{file_name}.{}.{counter}.fastcapture-tmp",
        std::process::id()
    )))
}

fn replace_or_move(temp: &Path, destination: &Path) -> Result<()> {
    let temp_wide = wide_path(temp);
    let destination_wide = wide_path(destination);
    let ok = if destination.exists() {
        unsafe {
            ReplaceFileW(
                destination_wide.as_ptr(),
                temp_wide.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null(),
                std::ptr::null(),
            )
        }
    } else {
        unsafe {
            MoveFileExW(
                temp_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if ok == 0 {
        Err(last_error("ReplaceFileW or MoveFileExW"))
    } else {
        Ok(())
    }
}

/// `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` fails atomically when `destination` exists.
fn move_without_replacing(temp: &Path, destination: &Path) -> Result<()> {
    let temp_wide = wide_path(temp);
    let destination_wide = wide_path(destination);
    let ok = unsafe {
        MoveFileExW(
            temp_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(last_error("MoveFileExW"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    struct ExistingFile {
        directory: PathBuf,
        path: PathBuf,
        deny_handle: Option<std::fs::File>,
    }

    impl ExistingFile {
        fn new(contents: &[u8]) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "fastcapture-saver-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("document.txt");
            std::fs::write(&path, contents).unwrap();
            Self {
                directory,
                path,
                deny_handle: None,
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn sibling_temp_files(&self) -> Vec<PathBuf> {
            std::fs::read_dir(&self.directory)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|candidate| candidate != &self.path)
                .collect()
        }

        fn deny_replacement(&mut self) {
            // Holding the destination open without FILE_SHARE_DELETE denies ReplaceFileW and
            // MoveFileExW the delete-share they need to swap in a new file.
            self.deny_handle = Some(
                OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .open(&self.path)
                    .unwrap(),
            );
        }
    }

    impl Drop for ExistingFile {
        fn drop(&mut self) {
            self.deny_handle = None;
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn replacing_existing_file_never_exposes_partial_content() {
        let fixture = ExistingFile::new(b"old");
        super::save_atomic(fixture.path(), b"new content").unwrap();
        assert_eq!(std::fs::read(fixture.path()).unwrap(), b"new content");
        assert!(fixture.sibling_temp_files().is_empty());
    }

    #[test]
    fn a_replace_blocked_for_a_moment_by_another_program_is_retried() {
        // Break caught: an autosave, library.ini or session write lost because antivirus or a
        // sync client held the file for a few milliseconds after it was written.
        let mut fixture = ExistingFile::new(b"original");
        fixture.deny_replacement();
        let handle = fixture.deny_handle.take().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(60));
            drop(handle);
        });
        super::save_atomic(fixture.path(), b"new").unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(fixture.path()).unwrap(), b"new");
        assert!(fixture.sibling_temp_files().is_empty());
    }

    #[test]
    fn failed_replace_preserves_original() {
        let mut fixture = ExistingFile::new(b"original");
        fixture.deny_replacement();
        assert!(super::save_atomic(fixture.path(), b"replacement").is_err());
        assert_eq!(std::fs::read(fixture.path()).unwrap(), b"original");
    }

    #[test]
    fn an_exclusive_save_onto_an_existing_file_fails_and_leaves_it_untouched() {
        // Break caught: a first save under a freshly picked name replacing a file that appeared
        // after the name was checked.
        let fixture = ExistingFile::new(b"someone else's");
        let error = super::save_atomic_new(fixture.path(), b"mine").unwrap_err();
        assert!(super::is_already_exists(&error), "{error:?}");
        assert_eq!(std::fs::read(fixture.path()).unwrap(), b"someone else's");
        assert!(fixture.sibling_temp_files().is_empty());
    }

    #[test]
    fn an_exclusive_save_to_a_free_name_creates_it() {
        let fixture = ExistingFile::new(b"x");
        let fresh = fixture.path().with_file_name("fresh.md");
        super::save_atomic_new(&fresh, b"new").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"new");
    }

    #[test]
    fn saving_to_a_new_destination_creates_it_without_leaving_a_temp_sibling() {
        // Break caught: the MoveFileExW branch for a destination that does not yet exist can leave
        // the sibling temp file behind, or fail to create the destination at all.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "fastcapture-saver-new-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("brand-new.txt");

        super::save_atomic(&path, b"fresh").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        let siblings: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|candidate| candidate != &path)
            .collect();
        assert!(siblings.is_empty());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
