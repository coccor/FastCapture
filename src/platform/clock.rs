//! Copied from FastPad `src/platform/clock.rs`.
//!
//! The local date and time, for capture names and `captured:` stamps.

use crate::notebook::dates::{Date, Time};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

/// Now, in the PC's time zone.
pub fn local_now() -> (Date, Time) {
    let mut now = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut now) };
    let date = Date::new(now.wYear, now.wMonth as u8, now.wDay as u8)
        .unwrap_or_else(|| Date::new(2000, 1, 1).expect("a real date"));
    let time = Time {
        hour: now.wHour as u8,
        minute: now.wMinute as u8,
    };
    (date, time)
}
