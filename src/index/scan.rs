//! Copied from FastPad `src/library/scan.rs`.
//!
//! Walks a notebook for its Markdown notes, and lists every folder it walks. Each directory is
//! listed with `FileIdBothDirectoryInfo` queries, which return names, sizes and write times in
//! bulk without opening any file. FastCapture's own folders (raw sources, attachments,
//! templates) are passed in to be left out.

use crate::platform::{last_error, wide_path};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_SYSTEM, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdBothDirectoryInfo,
    GetFileInformationByHandleEx, OPEN_EXISTING,
};

pub const NOTE_LIMIT: usize = 10_000;
/// At most this many folders are listed in `Scan::folders`; past it folders are still walked.
pub const FOLDER_LIMIT: usize = 10_000;
const SKIPPED: [&str; 4] = ["node_modules", "target", "bin", "obj"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanEntry {
    /// Relative to the notebook.
    pub path: PathBuf,
    pub size: u64,
    /// Last write time in FILETIME ticks.
    pub mtime: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Scan {
    pub entries: Vec<ScanEntry>,
    /// Every folder walked, relative to the notebook and spelled as on disk, the root not
    /// included: at most `FOLDER_LIMIT` of them, in walk order.
    pub folders: Vec<PathBuf>,
    /// The note limit was reached before the walk ended.
    pub truncated: bool,
}

pub fn skip_directory(name: &str) -> bool {
    name.starts_with('.')
        || SKIPPED
            .iter()
            .any(|skipped| skipped.eq_ignore_ascii_case(name))
}

/// A Markdown note, by its extension.
pub fn is_note(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

/// A directory handle, closed on drop.
struct Directory(HANDLE);

impl Drop for Directory {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open_directory(path: &Path) -> crate::Result<Directory> {
    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW"));
    }
    Ok(Directory(handle))
}

/// The notes and folders under `notebook`, leaving out the folders in `left_out` (relative to the
/// notebook, matched without regard to case) and at most `limit` notes.
pub fn scan(notebook: &Path, left_out: &[PathBuf], limit: usize) -> crate::Result<Scan> {
    scan_limited(notebook, left_out, limit, FOLDER_LIMIT)
}

/// `scan` with its own folder cap.
pub fn scan_limited(
    notebook: &Path,
    left_out: &[PathBuf],
    limit: usize,
    folder_limit: usize,
) -> crate::Result<Scan> {
    let root = open_directory(notebook)?;
    let left_out: Vec<String> = left_out
        .iter()
        .map(|folder| folder.to_string_lossy().to_lowercase())
        .collect();
    let mut scan = Scan::default();
    let mut pending = vec![(PathBuf::new(), Some(root))];
    // 64 KiB, 8-byte aligned as FILE_ID_BOTH_DIR_INFO requires.
    let mut buffer = vec![0_u64; 8 * 1024];
    while let Some((relative, handle)) = pending.pop() {
        let handle = match handle {
            Some(handle) => handle,
            // A subfolder that cannot be opened is skipped, not fatal.
            None => match open_directory(&notebook.join(&relative)) {
                Ok(handle) => handle,
                Err(_) => continue,
            },
        };
        loop {
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    handle.0,
                    FileIdBothDirectoryInfo,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            };
            if ok == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_NO_MORE_FILES && relative.as_os_str().is_empty() {
                    return Err(crate::Error::Win32 {
                        call: "GetFileInformationByHandleEx",
                        code: error,
                    });
                }
                break;
            }
            let mut offset = 0_usize;
            loop {
                // Entries are packed back to back; NextEntryOffset is 8-byte aligned. FileName runs
                // past the end of the struct, so it is reached through a raw pointer into the
                // buffer, never through a reference to the struct (whose bounds end at FileName[1]).
                let entry = unsafe {
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>()
                };
                let name = unsafe {
                    std::slice::from_raw_parts(
                        std::ptr::addr_of!((*entry).FileName).cast::<u16>(),
                        ((*entry).FileNameLength / 2) as usize,
                    )
                };
                let name = std::ffi::OsString::from_wide(name)
                    .to_string_lossy()
                    .into_owned();
                // SAFETY: the fixed-size part of the entry lies inside the buffer.
                let entry = unsafe { &*entry };
                let attributes = entry.FileAttributes;
                let skipped_attributes = FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM;
                if name != "." && name != ".." && attributes & skipped_attributes == 0 {
                    let path = relative.join(&name);
                    if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                        let ours = left_out.contains(&path.to_string_lossy().to_lowercase());
                        if attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
                            && !skip_directory(&name)
                            && !ours
                        {
                            if scan.folders.len() < folder_limit {
                                scan.folders.push(path.clone());
                            }
                            pending.push((path, None));
                        }
                    } else if is_note(Path::new(&name)) {
                        if scan.entries.len() >= limit {
                            scan.truncated = true;
                            return Ok(scan);
                        }
                        scan.entries.push(ScanEntry {
                            path,
                            size: entry.EndOfFile as u64,
                            mtime: entry.LastWriteTime as u64,
                        });
                    }
                }
                if entry.NextEntryOffset == 0 {
                    break;
                }
                offset += entry.NextEntryOffset as usize;
            }
        }
    }
    Ok(scan)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scratch notebook in the temp folder, removed on drop.
    pub(crate) struct Scratch(pub(crate) PathBuf);

    impl Scratch {
        pub(crate) fn new(label: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("fastcapture-scan-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        pub(crate) fn file(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        pub(crate) fn dir(&self, relative: &str) {
            std::fs::create_dir_all(self.0.join(relative)).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn paths(scan: &Scan) -> Vec<String> {
        let mut paths: Vec<_> = scan
            .entries
            .iter()
            .map(|e| e.path.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        paths
    }

    fn folders(scan: &Scan) -> Vec<String> {
        let mut folders: Vec<_> = scan
            .folders
            .iter()
            .map(|folder| folder.to_string_lossy().into_owned())
            .collect();
        folders.sort();
        folders
    }

    fn hide(path: &Path) {
        let wide = wide_path(path);
        unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
                wide.as_ptr(),
                FILE_ATTRIBUTE_HIDDEN,
            );
        }
    }

    #[test]
    fn notes_and_folders_are_found_and_the_rest_is_skipped() {
        let scratch = Scratch::new("rules");
        scratch.file("a.md", "a");
        scratch.file(r"Payments\Incidents\b.MD", "bb");
        scratch.file(r"Payments\c.markdown", "c");
        scratch.file("picture.png", "x");
        scratch.file("notes.txt", "x");
        scratch.file(r".obsidian\d.md", "x");
        scratch.file(r"node_modules\e.md", "x");
        scratch.file(r"Bin\f.md", "x");
        scratch.file(r"raw\2026-10-19 1030 lag.md", "x");
        scratch.file(r"Attachments\g.md", "x");
        scratch.file(r"hidden\h.md", "x");
        scratch.dir("empty");
        hide(&scratch.0.join("hidden"));
        let scan = scan(
            &scratch.0,
            &[PathBuf::from("raw"), PathBuf::from("attachments")],
            NOTE_LIMIT,
        )
        .unwrap();
        assert_eq!(
            paths(&scan),
            ["Payments\\Incidents\\b.MD", "Payments\\c.markdown", "a.md"]
        );
        assert_eq!(folders(&scan), ["Payments", r"Payments\Incidents", "empty"]);
        let b = scan
            .entries
            .iter()
            .find(|e| e.path.ends_with("b.MD"))
            .unwrap();
        assert_eq!(b.size, 2);
        assert_ne!(b.mtime, 0);
        assert!(!scan.truncated);
    }

    #[test]
    fn a_nested_folder_of_ours_is_left_out_too() {
        let scratch = Scratch::new("nested");
        scratch.file(r"Work\raw\x.md", "x");
        scratch.file(r"Work\y.md", "y");
        let scan = scan(&scratch.0, &[PathBuf::from(r"Work\Raw")], NOTE_LIMIT).unwrap();
        assert_eq!(paths(&scan), [r"Work\y.md"]);
    }

    #[test]
    fn the_scan_stops_at_the_limits() {
        let scratch = Scratch::new("limit");
        for index in 0..5 {
            scratch.file(&format!("{index}.md"), "x");
        }
        let scan = scan(&scratch.0, &[], 3).unwrap();
        assert_eq!(scan.entries.len(), 3);
        assert!(scan.truncated);

        let scratch = Scratch::new("folder-limit");
        for name in ["a", "b", "c"] {
            scratch.file(&format!(r"{name}\n.md"), "x");
        }
        let scan = scan_limited(&scratch.0, &[], NOTE_LIMIT, 2).unwrap();
        assert_eq!(scan.folders.len(), 2);
        assert_eq!(
            scan.entries.len(),
            3,
            "folders past the cap are still walked"
        );
    }

    #[test]
    fn reparse_points_are_not_followed() {
        let scratch = Scratch::new("junction");
        scratch.file("a.md", "a");
        let link = scratch.0.join("loop");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&scratch.0)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /J failed: {made:?}");
        let scan = scan(&scratch.0, &[], NOTE_LIMIT).unwrap();
        assert_eq!(paths(&scan), ["a.md"]);
        assert!(scan.folders.is_empty());
    }

    #[test]
    fn a_missing_folder_is_an_error_and_skip_rules_ignore_case() {
        assert!(
            scan(
                Path::new(r"Z:\fastcapture-does-not-exist\x"),
                &[],
                NOTE_LIMIT
            )
            .is_err()
        );
        assert!(skip_directory("Node_Modules"));
        assert!(skip_directory(".obsidian"));
        assert!(!skip_directory("notes"));
        assert!(is_note(Path::new("a.MD")) && !is_note(Path::new("a.txt")));
    }
}
