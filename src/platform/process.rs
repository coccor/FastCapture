//! Whether a process is still running, by its id.

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Whether the process `pid` is running. This process always is, and so is one Windows refuses
/// to open (it exists, it just isn't ours to look at); a process that can't be opened for any
/// other reason, or can't be queried, counts as not running.
pub fn running(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
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

    #[test]
    fn the_system_process_runs_though_it_cant_be_looked_at() {
        // Pid 4 is the System process: opening it is refused to most callers, which must not
        // read as "not running".
        assert!(running(4));
    }
}
