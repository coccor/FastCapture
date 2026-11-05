//! Whether a process is still running, by its id.

use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Whether the process `pid` is running. This process always is; a process that can't be opened
/// or queried counts as not running.
pub fn running(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0_u32;
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    ok != 0 && code == STILL_ACTIVE as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_runs_and_an_impossible_id_does_not() {
        assert!(running(std::process::id()));
        // Process ids are multiples of 4, so 3 never names a process.
        assert!(!running(3));
    }
}
