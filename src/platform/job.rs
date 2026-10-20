//! Starting a helper program the way FastCapture's backends do: no console window, and inside a
//! job object, so ending the job (or dropping it) ends the program and anything it started.

use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// `Command::creation_flags` for a program that mustn't open a console window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A job object that ends every process in it when it is closed.
pub struct Job(HANDLE);

// The handle is only used through calls that are safe from any thread.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> Option<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return None;
        }
        let job = Self(handle);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        (ok != 0).then_some(job)
    }

    /// Puts `child` in the job; false when it couldn't be.
    pub fn add(&self, child: &std::process::Child) -> bool {
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
    }

    /// Ends every process in the job now.
    pub fn end(&self) {
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
