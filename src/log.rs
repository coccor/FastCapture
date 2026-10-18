//! The diagnostic log, `%LocalAppData%\FastCapture\log\fastcapture.log`. It
//! records what the app did and what failed, never capture text, page content, prompts or keys.
//! Callers pass only their own wording and paths. Logging never fails the caller.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE: &str = "fastcapture.log";
const OLD_LOG_FILE: &str = "fastcapture.old.log";
/// Past this size the log is moved to `fastcapture.old.log` and started again.
const MAX_BYTES: u64 = 1024 * 1024;

/// Appends one timestamped line to the log.
pub fn line(message: &str) {
    if let Some(dir) = crate::paths::log_dir() {
        let _ = append(&dir, SystemTime::now(), message);
    }
}

fn append(dir: &Path, now: SystemTime, message: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = fs::rename(&path, dir.join(OLD_LOG_FILE));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let message = message.replace(['\r', '\n'], " ");
    writeln!(file, "{} {message}", utc_stamp(now))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for `time`.
fn utc_stamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap_day = UNIX_EPOCH + Duration::from_secs(1_709_210_096);
        assert_eq!(utc_stamp(leap_day), "2024-02-29T12:34:56Z");
        let today = UNIX_EPOCH + Duration::from_secs(1_792_281_600);
        assert_eq!(utc_stamp(today), "2026-10-18T00:00:00Z");
    }

    #[test]
    fn lines_are_appended_on_one_line_each() {
        let dir = temp_dir("append");
        append(&dir, UNIX_EPOCH, "first").unwrap();
        append(&dir, UNIX_EPOCH, "second\r\nline").unwrap();
        let text = fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(
            text,
            "1970-01-01T00:00:00Z first\n1970-01-01T00:00:00Z second  line\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_log_is_moved_aside() {
        let dir = temp_dir("rotate");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOG_FILE), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append(&dir, UNIX_EPOCH, "fresh").unwrap();
        assert_eq!(
            fs::metadata(dir.join(OLD_LOG_FILE)).unwrap().len(),
            MAX_BYTES + 1
        );
        assert_eq!(
            fs::read_to_string(dir.join(LOG_FILE)).unwrap(),
            "1970-01-01T00:00:00Z fresh\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
