//! What the review window lists: one row per queued capture, newest
//! first, from the queue and the raw sources. Pure: the caller reads the files.

use crate::capture::read::{self, RawInfo};
use crate::notebook::dates::{self, Date};
use crate::queue::{Job, State};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub capture_id: String,
    pub notebook: PathBuf,
    /// Relative to the notebook.
    pub raw: PathBuf,
    pub title: String,
    pub kinds: String,
    pub created: String,
    pub state: State,
    pub error: Option<String>,
    /// The raw source's text, for the preview.
    pub text: String,
    pub info: RawInfo,
}

impl Row {
    pub fn raw_path(&self) -> PathBuf {
        self.notebook.join(&self.raw)
    }
}

/// The rows for `jobs`, newest first, reading each raw source with `read_file` (absolute path).
/// The second list holds the jobs whose raw source is gone.
pub fn rows(jobs: Vec<Job>, read_file: impl Fn(&Path) -> Option<String>) -> (Vec<Row>, Vec<Job>) {
    let mut rows = Vec::new();
    let mut gone = Vec::new();
    for job in jobs {
        let Some(text) = read_file(&job.notebook.join(&job.raw)) else {
            gone.push(job);
            continue;
        };
        let info = read::read(&job.raw, &text);
        let title = info.title.clone().unwrap_or_else(|| {
            job.raw
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        rows.push(Row {
            kinds: kind_label(&info.kinds),
            capture_id: job.capture_id,
            notebook: job.notebook,
            raw: job.raw,
            title,
            created: job.created,
            state: job.state,
            error: job.last_error,
            text,
            info,
        });
    }
    rows.sort_by(|a, b| {
        (b.created.as_str(), b.capture_id.as_str())
            .cmp(&(a.created.as_str(), a.capture_id.as_str()))
    });
    (rows, gone)
}

/// `text + screenshot` from front matter kinds: "Text + screenshot".
pub fn kind_label(kinds: &[String]) -> String {
    let joined = kinds.join(" + ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `2026-10-19T10:30` as "10:30" on `today`, else "19 Oct 10:30", else as written.
pub fn time_label(created: &str, today: Date) -> String {
    let Some((day, time)) = created.split_once('T') else {
        return created.to_owned();
    };
    match dates::parse(day, "YYYY-MM-DD") {
        Some(date) if date == today => time.to_owned(),
        Some(date) if date.year == today.year => format!("{} {time}", dates::format(date, "D MMM")),
        Some(date) => format!("{} {time}", dates::format(date, "D MMM YYYY")),
        None => created.to_owned(),
    }
}

pub fn state_label(state: State) -> &'static str {
    match state {
        State::Queued => "Queued",
        State::Working => "Working",
        State::Done => "Done",
        State::Failed => "Failed",
    }
}

/// The preview pane's text: a failed job's error first, then the raw source, with CRLF line
/// ends for the edit control.
pub fn preview_text(row: &Row) -> String {
    let mut text = String::new();
    if let Some(error) = &row.error {
        text.push_str(&format!("Last error: {error}\n\n"));
    }
    text.push_str(&row.text);
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn job(id: &str, created: &str, raw: &str) -> Job {
        Job::queued(
            id,
            PathBuf::from(r"D:\Notes"),
            PathBuf::from(raw),
            created.to_owned(),
        )
    }

    #[test]
    fn rows_are_newest_first_and_jobs_without_a_raw_source_are_set_aside() {
        let files: HashMap<PathBuf, String> = [
            (
                PathBuf::from(r"D:\Notes\raw\a.md"),
                "---\ntype: raw\nkind: [text, link]\n---\n\nRust cancellation\nhttps://x.example\n"
                    .to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\raw\b.md"),
                "---\ntype: raw\nkind: [file]\n---\n\n[report.pdf](../attachments/report.pdf)\n"
                    .to_owned(),
            ),
        ]
        .into();
        let mut failed = job("B", "2026-10-19T10:31", r"raw\b.md");
        failed.state = State::Failed;
        failed.last_error = Some("timeout".to_owned());
        let (rows, gone) = rows(
            vec![
                job("A", "2026-10-19T09:00", r"raw\a.md"),
                failed,
                job("C", "2026-10-19T11:00", r"raw\gone.md"),
            ],
            |path| files.get(path).cloned(),
        );
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].capture_id, "C");
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["report.pdf", "Rust cancellation"]);
        assert_eq!(rows[0].kinds, "File");
        assert_eq!(rows[1].kinds, "Text + link");
        assert_eq!(rows[0].raw_path(), PathBuf::from(r"D:\Notes\raw\b.md"));
        assert!(preview_text(&rows[0]).starts_with("Last error: timeout\r\n\r\n---\r\ntype: raw"));
    }

    #[test]
    fn times_are_short_for_today_and_dated_otherwise() {
        let today = Date::new(2026, 10, 19).unwrap();
        assert_eq!(time_label("2026-10-19T10:30", today), "10:30");
        assert_eq!(time_label("2026-10-18T09:05", today), "18 Oct 09:05");
        assert_eq!(time_label("2025-12-31T23:59", today), "31 Dec 2025 23:59");
        assert_eq!(time_label("soon", today), "soon");
        assert_eq!(state_label(State::Queued), "Queued");
        assert_eq!(kind_label(&[]), "");
    }
}
