//! Thin wrappers over Windows: atomic saves, the local clock, random bytes. Most are copied or
//! adapted from FastPad, each naming its source on its first line.

pub mod clipboard;
pub mod clock;
pub mod console;
pub mod hotkey;
pub mod instance;
pub mod png;
pub mod random;
pub mod saver;
pub mod shell;
pub mod startup;
pub mod tray;
pub mod watch;
pub mod window;

use crate::Error;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::GetLastError;

/// `call` failed: the error with `GetLastError()`'s code.
pub fn last_error(call: &'static str) -> Error {
    Error::Win32 {
        call,
        code: unsafe { GetLastError() },
    }
}

/// `path` as a NUL-terminated UTF-16 string for Win32 calls.
pub fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
