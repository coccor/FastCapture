//! Console output for a GUI-subsystem program: `fastcapture add`, `--version` and `--help`
//! print to the console they were started from. Output already redirected to a file or pipe is
//! left as it is.

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};

/// Attaches to the parent's console, if it has one, and points stdout and stderr at it where
/// they aren't redirected already.
pub fn attach() {
    let unset = |which: STD_HANDLE| {
        let handle = unsafe { GetStdHandle(which) };
        handle.is_null() || handle == INVALID_HANDLE_VALUE
    };
    if !unset(STD_OUTPUT_HANDLE) && !unset(STD_ERROR_HANDLE) {
        return;
    }
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        if !unset(which) {
            continue;
        }
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            // The handle lives for the rest of the process, as a standard handle does.
            unsafe { SetStdHandle(which, handle) };
        }
    }
}
