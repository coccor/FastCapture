//! Copied from FastPad `src/library/watch.rs`.
//!
//! A notebook's watcher: a thread that asks Windows for changes under the notebook folder
//! (`ReadDirectoryChangesW`) and hands them to a callback in batches, 250 ms after the last one.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

/// How long the watcher keeps collecting after the last change before it hands over the batch.
pub const BATCH_DELAY: Duration = Duration::from_millis(250);
/// The notification buffer, in bytes: room for several hundred changes between reads.
const BUFFER_BYTES: usize = 64 * 1024;

/// One batch of changes under the notebook.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileChanges {
    /// Absolute paths that were written, created, deleted or renamed (both old and new names),
    /// once each, in the order they first changed.
    pub paths: Vec<PathBuf>,
    /// A file or folder was added, removed or renamed.
    pub names_changed: bool,
    /// Windows dropped changes (its buffer overflowed): everything must be checked again.
    pub overflow: bool,
    /// Watching failed and stopped.
    pub failed: bool,
}

/// `FILE_ACTION_*` values from `FILE_NOTIFY_INFORMATION`.
const FILE_ACTION_ADDED: u32 = 1;
const FILE_ACTION_REMOVED: u32 = 2;
const FILE_ACTION_RENAMED_OLD_NAME: u32 = 4;
const FILE_ACTION_RENAMED_NEW_NAME: u32 = 5;

/// Whether a change to `relative` (a path under the notebook) is of no interest: anything inside
/// a folder the scan skips (`.git`, `.obsidian`, `node_modules`, …), except the notebook's own
/// settings (`.fastpad\notebook.ini`, `.fastcapture\capture.ini`), which say where things go.
pub fn skip_by_name(relative: &Path) -> bool {
    let components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some((_, folders)) = components.split_last() else {
        return true;
    };
    match components.as_slice() {
        [".fastpad", file] => return !file.eq_ignore_ascii_case("notebook.ini"),
        [".fastcapture", file] => return !file.eq_ignore_ascii_case("capture.ini"),
        _ => {}
    }
    // Only the folders on the way count: a file such as `.env` is a file like any other.
    folders.iter().any(|name| super::scan::skip_directory(name))
}

/// One change read from a `FILE_NOTIFY_INFORMATION` buffer.
#[derive(Debug, Eq, PartialEq)]
pub struct Notification {
    pub action: u32,
    /// Relative to the watched folder.
    pub path: PathBuf,
}

/// Reads the `FILE_NOTIFY_INFORMATION` records in `buffer` (the bytes Windows filled). A record
/// that runs past the end of the buffer ends the list.
pub fn parse_notifications(buffer: &[u8]) -> Vec<Notification> {
    use std::os::windows::ffi::OsStringExt;
    let field = |at: usize| -> Option<u32> {
        let bytes = buffer.get(at..at + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };
    let mut notifications = Vec::new();
    let mut offset = 0usize;
    while let (Some(next), Some(action), Some(length)) =
        (field(offset), field(offset + 4), field(offset + 8))
    {
        let start = offset + 12;
        let Some(name) = buffer.get(start..start + length as usize) else {
            break;
        };
        let units = name
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect::<Vec<_>>();
        notifications.push(Notification {
            action,
            path: PathBuf::from(std::ffi::OsString::from_wide(&units)),
        });
        if next == 0 {
            break;
        }
        offset += next as usize;
    }
    notifications
}

/// Adds `notifications` to `batch`, dropping the ones `skip_by_name` refuses.
fn add_to_batch(batch: &mut FileChanges, folder: &Path, notifications: Vec<Notification>) {
    for notification in notifications {
        if skip_by_name(&notification.path) {
            continue;
        }
        if matches!(
            notification.action,
            FILE_ACTION_ADDED
                | FILE_ACTION_REMOVED
                | FILE_ACTION_RENAMED_OLD_NAME
                | FILE_ACTION_RENAMED_NEW_NAME
        ) {
            batch.names_changed = true;
        }
        let path = folder.join(&notification.path);
        if !batch.paths.contains(&path) {
            batch.paths.push(path);
        }
    }
}

/// A kernel handle the watcher thread and its owner share, closed on drop. Event and directory
/// handles may be used from any thread.
#[derive(Debug)]
struct SharedHandle(HANDLE);
unsafe impl Send for SharedHandle {}
unsafe impl Sync for SharedHandle {}

impl SharedHandle {
    fn new(handle: HANDLE, call: &'static str) -> crate::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(crate::platform::last_error(call));
        }
        Ok(Self(handle))
    }
}

impl Drop for SharedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// What a batch is handed to.
pub type OnChanges = Box<dyn Fn(FileChanges) + Send>;

/// The running watcher. Dropping it stops the thread and waits for it.
#[derive(Debug)]
pub struct Watcher {
    stop: Arc<SharedHandle>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching `folder`, calling `on_changes` on the watcher's thread with each batch.
    /// Fails when the folder can't be opened for watching.
    pub fn start(folder: PathBuf, on_changes: OnChanges) -> crate::Result<Self> {
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Threading::CreateEventW;
        let wide = crate::platform::wide_path(&folder);
        let directory = SharedHandle::new(
            unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    std::ptr::null_mut(),
                )
            },
            "CreateFileW",
        )?;
        let event = || {
            SharedHandle::new(
                unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) },
                "CreateEventW",
            )
        };
        let io = event()?;
        let stop = Arc::new(event()?);
        let thread_stop = Arc::clone(&stop);
        let (armed, ready) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("fastcapture-watch".to_owned())
            .spawn(move || run(&folder, &directory, &io, &thread_stop, &armed, &*on_changes))
            .map_err(|error| crate::Error::Message(error.to_string()))?;
        // Changes are only recorded once the first read is issued: wait for it, so none made
        // after `start` returns are missed.
        let _ = ready.recv();
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.stop.0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What one wait on the watcher's handles ended with.
enum Wake {
    Changes(Vec<Notification>),
    Overflow,
    Timeout,
    Stop,
    Failed,
}

/// The watcher thread: keeps one read outstanding, collects what it reports, and hands over a
/// batch once nothing more arrives for `BATCH_DELAY`.
fn run(
    folder: &Path,
    directory: &SharedHandle,
    io: &SharedHandle,
    stop: &SharedHandle,
    armed: &std::sync::mpsc::Sender<()>,
    on_changes: &(dyn Fn(FileChanges) + Send),
) {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{INFINITE, ResetEvent, WaitForMultipleObjects};

    // DWORD-aligned, as ReadDirectoryChangesW requires.
    let mut buffer = vec![0u32; BUFFER_BYTES / 4];
    let mut overlapped = Box::new(OVERLAPPED::default());
    overlapped.hEvent = io.0;
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_SIZE;
    let handles = [io.0, stop.0];
    let mut batch = FileChanges::default();
    let failed = || FileChanges {
        failed: true,
        ..FileChanges::default()
    };

    let issue = |buffer: &mut Vec<u32>, overlapped: &mut OVERLAPPED| -> bool {
        unsafe {
            ResetEvent(io.0);
            ReadDirectoryChangesW(
                directory.0,
                buffer.as_mut_ptr().cast(),
                BUFFER_BYTES as u32,
                1,
                filter,
                std::ptr::null_mut(),
                overlapped,
                None,
            ) != 0
        }
    };
    let issued = issue(&mut buffer, &mut overlapped);
    let _ = armed.send(());
    if !issued {
        on_changes(failed());
        return;
    }
    loop {
        let timeout = if batch.paths.is_empty() && !batch.overflow {
            INFINITE
        } else {
            BATCH_DELAY.as_millis() as u32
        };
        let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) };
        let wake = match wait {
            w if w == WAIT_OBJECT_0 => {
                let mut bytes = 0u32;
                if unsafe { GetOverlappedResult(directory.0, &*overlapped, &mut bytes, 0) } == 0 {
                    Wake::Failed
                } else if bytes == 0 {
                    Wake::Overflow
                } else {
                    let filled = unsafe {
                        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes as usize)
                    };
                    Wake::Changes(parse_notifications(filled))
                }
            }
            w if w == WAIT_OBJECT_0 + 1 => Wake::Stop,
            WAIT_TIMEOUT => Wake::Timeout,
            _ => Wake::Failed,
        };
        match wake {
            Wake::Changes(notifications) => {
                add_to_batch(&mut batch, folder, notifications);
                if !issue(&mut buffer, &mut overlapped) {
                    on_changes(failed());
                    return;
                }
            }
            Wake::Overflow => {
                batch.overflow = true;
                if !issue(&mut buffer, &mut overlapped) {
                    on_changes(failed());
                    return;
                }
            }
            Wake::Timeout => on_changes(std::mem::take(&mut batch)),
            Wake::Stop | Wake::Failed => {
                unsafe {
                    CancelIoEx(directory.0, &*overlapped);
                    let mut bytes = 0u32;
                    // Waits for the cancelled read, so Windows is done with the buffer.
                    GetOverlappedResult(directory.0, &*overlapped, &mut bytes, 1);
                }
                if matches!(wake, Wake::Failed) {
                    on_changes(failed());
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::scan::tests::Scratch;
    use std::sync::mpsc;

    #[test]
    fn changes_inside_skipped_folders_are_dropped_but_the_notebooks_settings_are_kept() {
        for path in [
            r".git\index",
            r".obsidian\workspace.json",
            r"node_modules\x\index.md",
            r"sub\.git\HEAD",
            r"target\debug\out.md",
            r".fastpad\library.ini",
            r".fastcapture\other",
        ] {
            assert!(skip_by_name(Path::new(path)), "{path}");
        }
        for path in [
            "note.md",
            r"journal\2026-10-16.md",
            r".fastpad\NOTEBOOK.INI",
            r".fastcapture\capture.ini",
        ] {
            assert!(!skip_by_name(Path::new(path)), "{path}");
        }
    }

    fn record(next: u32, action: u32, name: &str) -> Vec<u8> {
        let units = name.encode_utf16().collect::<Vec<_>>();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&next.to_le_bytes());
        bytes.extend_from_slice(&action.to_le_bytes());
        bytes.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
        for unit in units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn notification_records_are_read_in_order_with_their_names() {
        let first = record(0, FILE_ACTION_RENAMED_OLD_NAME, "Résumé.md");
        let mut buffer = first.clone();
        let next = buffer.len() as u32;
        buffer[..4].copy_from_slice(&next.to_le_bytes());
        buffer.extend(record(0, FILE_ACTION_RENAMED_NEW_NAME, r"journal\été.md"));
        assert_eq!(
            parse_notifications(&buffer),
            [
                Notification {
                    action: FILE_ACTION_RENAMED_OLD_NAME,
                    path: PathBuf::from("Résumé.md"),
                },
                Notification {
                    action: FILE_ACTION_RENAMED_NEW_NAME,
                    path: PathBuf::from(r"journal\été.md"),
                },
            ]
        );
        let mut broken = record(0, FILE_ACTION_ADDED, "a.md");
        broken[8..12].copy_from_slice(&1000u32.to_le_bytes());
        assert!(parse_notifications(&broken).is_empty());
    }

    /// The next batch, waiting up to 5 s.
    fn next(batches: &mpsc::Receiver<FileChanges>) -> FileChanges {
        batches
            .recv_timeout(Duration::from_secs(5))
            .expect("a batch")
    }

    #[test]
    fn writes_renames_and_deletes_are_reported_in_batches() {
        let scratch = Scratch::new("watch");
        scratch.dir("Payments");
        let (send, batches) = mpsc::channel();
        let send = std::sync::Mutex::new(send);
        let watcher = Watcher::start(
            scratch.0.clone(),
            Box::new(move |changes| {
                let _ = send.lock().unwrap().send(changes);
            }),
        )
        .unwrap();

        scratch.file(r"Payments\lag.md", "one");
        scratch.file(r"Payments\lag.md", "two");
        scratch.file(r".git\index", "ignored");
        // Folders whose contents changed are reported too; what is inside `.git` never is.
        let batch = next(&batches);
        assert_eq!(batch.paths[0], scratch.0.join(r"Payments\lag.md"));
        assert!(!batch.paths.contains(&scratch.0.join(r".git\index")));
        assert!(batch.names_changed);

        std::fs::rename(
            scratch.0.join(r"Payments\lag.md"),
            scratch.0.join(r"Payments\lag 2.md"),
        )
        .unwrap();
        let batch = next(&batches);
        assert!(batch.paths.contains(&scratch.0.join(r"Payments\lag.md")));
        assert!(batch.paths.contains(&scratch.0.join(r"Payments\lag 2.md")));

        std::fs::remove_file(scratch.0.join(r"Payments\lag 2.md")).unwrap();
        assert!(
            next(&batches)
                .paths
                .contains(&scratch.0.join(r"Payments\lag 2.md"))
        );
        drop(watcher);
    }
}
