//! Copied from FastPad `src/platform/clock.rs`.
//!
//! The local date and time, for capture names and `captured:` stamps.

use crate::notebook::dates::{Date, Time};
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

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

/// The PC's local date and time at `unix_seconds` (seconds since 1970, UTC).
pub fn local_at(unix_seconds: u64) -> Option<(Date, Time)> {
    // FILETIME counts 100 ns steps since 1601.
    let ticks = (unix_seconds + 11_644_473_600).checked_mul(10_000_000)?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { FileTimeToSystemTime(&file_time, &mut utc) } == 0
        || unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } == 0
    {
        return None;
    }
    let date = Date::new(local.wYear, local.wMonth as u8, local.wDay as u8)?;
    Some((
        date,
        Time {
            hour: local.wHour as u8,
            minute: local.wMinute as u8,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_time_is_read_in_local_time() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (date, time) = local_at(now).unwrap();
        let (today, clock) = local_now();
        assert_eq!(date, today);
        assert!(
            (i32::from(time.hour) * 60 + i32::from(time.minute)
                - (i32::from(clock.hour) * 60 + i32::from(clock.minute)))
            .abs()
                <= 1
        );
        // 1970-01-01 00:00 UTC is still 1969 west of Greenwich.
        assert!(matches!(local_at(0), Some((date, _)) if date.year == 1970 || date.year == 1969));
    }
}
