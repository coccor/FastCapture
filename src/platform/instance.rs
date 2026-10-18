//! One tray app per Windows session (spec §5.1), guarded by a named mutex, as FastPad does.

use std::os::windows::io::{FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// The mutex while this process is the session's tray app; dropping it lets another start.
pub struct Instance(#[allow(dead_code)] OwnedHandle);

/// Claims `Local\<name>`, or `None` when another process holds it.
pub fn claim(name: &str) -> Option<Instance> {
    let wide: Vec<u16> = format!("Local\\{name}\0").encode_utf16().collect();
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if raw.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return None;
    }
    Some(Instance(handle))
}

/// The tray app's mutex name.
pub const TRAY: &str = "FastCapture";

#[cfg(test)]
mod tests {
    #[test]
    fn a_second_claim_fails_until_the_first_is_dropped() {
        let name = format!("FastCaptureTest-{}", std::process::id());
        let first = super::claim(&name).unwrap();
        assert!(super::claim(&name).is_none());
        drop(first);
        assert!(super::claim(&name).is_some());
    }
}
