//! The usage ledger: the tokens cloud requests used, per day and model, as the APIs reported
//! them, in `%LocalAppData%\FastCapture\usage.txt`. One line per day and model:
//!
//! `2026-10-19 cloud/claude-haiku-5-5 input=900 cache_read=1605 cache_write=0 output=391`
//!
//! Settings adds the lines up for today and this month, and the token cap reads the month.

use crate::model::Usage;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const FILE: &str = "usage.txt";
/// Months kept, this one included.
const KEEP_MONTHS: i64 = 13;
const HEADER: &str =
    "# Tokens FastCapture's cloud requests used, per day and model, as the APIs reported them.";

/// `%LocalAppData%\FastCapture\usage.txt`.
pub fn path() -> Option<PathBuf> {
    crate::paths::local().map(|local| local.join(FILE))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Line {
    Day {
        /// `YYYY-MM-DD`.
        day: String,
        label: String,
        usage: Usage,
    },
    /// A comment, or a line that couldn't be read: kept as it is, counting nothing.
    Other(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ledger {
    lines: Vec<Line>,
}

/// The file couldn't be read once already, so it isn't logged again.
static UNREADABLE_LOGGED: AtomicBool = AtomicBool::new(false);

/// `YYYY-MM` of `day` as a month count, for comparing months.
fn month_number(day: &str) -> Option<i64> {
    let year: i64 = day.get(0..4)?.parse().ok()?;
    let month: i64 = day.get(5..7)?.parse().ok()?;
    (day.as_bytes().get(4) == Some(&b'-') && (1..=12).contains(&month))
        .then_some(year * 12 + month - 1)
}

fn parse_line(line: &str) -> Line {
    let other = || Line::Other(line.to_owned());
    let words: Vec<&str> = line.split_whitespace().collect();
    if line.trim_start().starts_with('#') || words.len() < 6 || month_number(words[0]).is_none() {
        return other();
    }
    let counts = &words[words.len() - 4..];
    let mut usage = Usage::default();
    for (word, (key, slot)) in counts.iter().zip([
        ("input", &mut usage.input),
        ("cache_read", &mut usage.cache_read),
        ("cache_write", &mut usage.cache_write),
        ("output", &mut usage.output),
    ]) {
        match word.split_once('=') {
            Some((name, value)) if name == key => match value.parse() {
                Ok(value) => *slot = value,
                Err(_) => return other(),
            },
            _ => return other(),
        }
    }
    Line::Day {
        day: words[0].to_owned(),
        label: words[1..words.len() - 4].join(" "),
        usage,
    }
}

impl Ledger {
    pub fn from_text(text: &str) -> Self {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        Self {
            lines: text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(parse_line)
                .collect(),
        }
    }

    /// The ledger at `path`: empty when there is none yet, or when it can't be read.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_text(&text),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound
                    && !UNREADABLE_LOGGED.swap(true, Ordering::Relaxed)
                {
                    crate::log::line(&format!(
                        "usage: {} couldn't be read: {error}",
                        path.display()
                    ));
                }
                Self::default()
            }
        }
    }

    pub fn text(&self) -> String {
        let mut text = String::new();
        if !matches!(self.lines.first(), Some(Line::Other(first)) if first == HEADER) {
            text.push_str(HEADER);
            text.push_str("\r\n");
        }
        for line in &self.lines {
            match line {
                Line::Day { day, label, usage } => text.push_str(&format!(
                    "{day} {label} input={} cache_read={} cache_write={} output={}",
                    usage.input, usage.cache_read, usage.cache_write, usage.output
                )),
                Line::Other(other) => text.push_str(other),
            }
            text.push_str("\r\n");
        }
        text
    }

    /// Adds `usage` to `day`'s line for `label`, and drops days older than the months kept.
    pub fn add(&mut self, day: &str, label: &str, usage: Usage) {
        let found = self.lines.iter_mut().find_map(|line| match line {
            Line::Day {
                day: line_day,
                label: line_label,
                usage,
            } if line_day == day && line_label == label => Some(usage),
            _ => None,
        });
        match found {
            Some(total) => total.add(usage),
            None => self.lines.push(Line::Day {
                day: day.to_owned(),
                label: label.to_owned(),
                usage,
            }),
        }
        if let Some(now) = month_number(day) {
            self.lines.retain(|line| match line {
                Line::Day { day, .. } => {
                    month_number(day).is_some_and(|month| now - month < KEEP_MONTHS)
                }
                Line::Other(_) => true,
            });
        }
    }

    fn sum(&self, keep: impl Fn(&str) -> bool) -> Usage {
        let mut total = Usage::default();
        for line in &self.lines {
            if let Line::Day { day, usage, .. } = line
                && keep(day)
            {
                total.add(*usage);
            }
        }
        total
    }

    /// The tokens used on `day` (`YYYY-MM-DD`).
    pub fn day(&self, day: &str) -> Usage {
        self.sum(|line_day| line_day == day)
    }

    /// The tokens used in `day`'s month.
    pub fn month(&self, day: &str) -> Usage {
        let month = month_number(day);
        self.sum(|line_day| month.is_some() && month_number(line_day) == month)
    }
}

/// Adds `usage` to the ledger at `path` and saves it.
pub fn record(path: &Path, day: &str, label: &str, usage: Usage) -> crate::Result<()> {
    let mut ledger = Ledger::load(path);
    ledger.add(day, label, usage);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| crate::Error::io(dir, error))?;
    }
    crate::platform::saver::save_atomic(path, ledger.text().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input,
            cache_read: 0,
            cache_write: 0,
            output,
        }
    }

    #[test]
    fn days_and_models_add_up_and_a_new_month_starts_at_zero() {
        let mut ledger = Ledger::default();
        ledger.add("2026-10-19", "cloud/claude-haiku-5-5", usage(900, 391));
        ledger.add(
            "2026-10-19",
            "cloud/claude-haiku-5-5",
            Usage {
                input: 100,
                cache_read: 1605,
                cache_write: 0,
                output: 9,
            },
        );
        ledger.add("2026-10-19", "cloud/gpt-5-mini", usage(50, 50));
        ledger.add("2026-10-02", "cloud/gpt-5-mini", usage(1000, 0));
        assert_eq!(
            ledger.day("2026-10-19").total(),
            900 + 391 + 100 + 1605 + 9 + 100
        );
        assert_eq!(ledger.month("2026-10-31").total(), 3105 + 1000);
        assert_eq!(ledger.month("2026-11-01").total(), 0);
        assert_eq!(ledger.day("2026-10-20").total(), 0);

        let text = ledger.text();
        assert!(text.starts_with("# Tokens"), "{text}");
        assert!(text.contains(
            "2026-10-19 cloud/claude-haiku-5-5 input=1000 cache_read=1605 cache_write=0 output=400\r\n"
        ));
        assert_eq!(
            Ledger::from_text(&text).text(),
            text,
            "read back as written"
        );
    }

    #[test]
    fn days_older_than_thirteen_months_are_dropped() {
        let mut ledger = Ledger::default();
        ledger.add("2025-09-30", "cloud/m", usage(1, 1));
        ledger.add("2025-10-01", "cloud/m", usage(2, 2));
        ledger.add("2026-10-19", "cloud/m", usage(3, 3));
        let text = ledger.text();
        assert!(!text.contains("2025-09-30"), "{text}");
        assert!(text.contains("2025-10-01"), "{text}");
    }

    #[test]
    fn a_line_it_cant_read_is_kept_and_counts_nothing() {
        let text = "2026-10-19 cloud/m input=10 cache_read=0 cache_write=0 output=5\r\n\
                    2026-10-19 cloud/m input=lots\r\n\
                    2026-10-19 cloud/name with spaces input=1 cache_read=0 cache_write=0 output=1\r\n";
        let ledger = Ledger::from_text(text);
        assert_eq!(ledger.day("2026-10-19").total(), 17);
        assert!(ledger.text().contains("2026-10-19 cloud/m input=lots\r\n"));
        assert!(ledger.text().contains("cloud/name with spaces input=1"));
    }

    #[test]
    fn recording_saves_the_file_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("fastcapture-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FILE);
        assert_eq!(Ledger::load(&path), Ledger::default());
        record(&path, "2026-10-19", "cloud/m", usage(10, 2)).unwrap();
        record(&path, "2026-10-19", "cloud/m", usage(5, 1)).unwrap();
        assert_eq!(Ledger::load(&path).day("2026-10-19"), usage(15, 3));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
