//! Watching a folder for changes on a thread of its own (`FindFirstChangeNotificationW`), for
//! re-reading `fastcapture.ini` when it is saved (spec §11).

use super::wide_path;
use std::path::Path;
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
    FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

/// Calls `changed` on a background thread each time a file in `folder` (not its subfolders) is
/// written, created, renamed or deleted. Several changes close together may give one call. The
/// thread lives as long as the process. Fails when the folder can't be watched.
pub fn watch_folder(folder: &Path, changed: impl Fn() + Send + 'static) -> crate::Result<()> {
    let wide = wide_path(folder);
    let handle = unsafe {
        FindFirstChangeNotificationW(
            wide.as_ptr(),
            0,
            FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(super::last_error("FindFirstChangeNotificationW"));
    }
    // Handles are plain values; this thread is the only user of this one.
    let handle = handle as usize;
    std::thread::spawn(move || {
        let handle = handle as windows_sys::Win32::Foundation::HANDLE;
        loop {
            if unsafe { WaitForSingleObject(handle, INFINITE) } != WAIT_OBJECT_0 {
                break;
            }
            // Let the writer finish (editors save in several steps) before reading.
            std::thread::sleep(std::time::Duration::from_millis(150));
            changed();
            if unsafe { FindNextChangeNotification(handle) } == 0 {
                break;
            }
        }
        unsafe { FindCloseChangeNotification(handle) };
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_write_in_the_folder_is_reported() {
        let folder = std::env::temp_dir().join(format!("fastcapture-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let (sender, receiver) = mpsc::channel();
        super::watch_folder(&folder, move || {
            let _ = sender.send(());
        })
        .unwrap();
        std::fs::write(folder.join("fastcapture.ini"), b"version=1\r\n").unwrap();
        assert!(receiver.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
