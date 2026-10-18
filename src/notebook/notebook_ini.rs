//! Copied from FastPad `src/library/notebook_ini.rs` (read-only: without `starter_text`).
//!
//! `.fastpad\notebook.ini`: a notebook's conventions,
//! shared with whoever else works in it. A missing file means the defaults; a bad value falls
//! back to its default and is reported with its line. FastCapture reads it and never writes it.

use super::dates;
use std::path::{Component, Path, PathBuf};

/// FastPad's folder inside a notebook.
pub const FASTPAD_DIR: &str = ".fastpad";

/// The file's name inside `.fastpad`.
pub const NOTEBOOK_FILE: &str = "notebook.ini";

/// Where `notebook.ini` lives for the notebook at `folder`.
pub fn notebook_file(folder: &Path) -> PathBuf {
    folder.join(FASTPAD_DIR).join(NOTEBOOK_FILE)
}

/// Where a capture goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureTarget {
    Inbox,
    Daily,
    Folder,
}

/// A notebook's conventions. Paths are relative to the notebook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotebookSettings {
    pub daily_folder: PathBuf,
    pub daily_format: String,
    pub daily_template: PathBuf,
    pub templates_folder: PathBuf,
    pub inbox: PathBuf,
    pub capture_folder: PathBuf,
    pub capture_target: CaptureTarget,
    /// `None` until set: the default, or the Obsidian vault's own folder.
    pub attachments_folder: Option<PathBuf>,
}

impl Default for NotebookSettings {
    fn default() -> Self {
        Self {
            daily_folder: PathBuf::from("journal"),
            daily_format: dates::DEFAULT_DAILY_FORMAT.to_owned(),
            daily_template: PathBuf::from(r".fastpad\templates\daily.md"),
            templates_folder: PathBuf::from(r".fastpad\templates"),
            inbox: PathBuf::from("inbox.md"),
            capture_folder: PathBuf::from("inbox"),
            capture_target: CaptureTarget::Inbox,
            attachments_folder: None,
        }
    }
}

/// A line FastPad couldn't use, numbered from 1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Warning {
    pub line: usize,
    pub message: String,
}

/// A path inside the notebook written with `/` or `\`, or `None` for one that is empty,
/// absolute or climbs out of it.
pub(crate) fn inside_path(value: &str) -> Option<PathBuf> {
    let path = PathBuf::from(value.trim().replace('/', "\\"));
    let mut parts = 0;
    for component in path.components() {
        match component {
            Component::Normal(_) => parts += 1,
            Component::CurDir => {}
            _ => return None,
        }
    }
    (parts > 0).then(|| {
        path.components()
            .filter(|c| *c != Component::CurDir)
            .collect()
    })
}

/// Reads `text`, the file's contents: the settings with every bad value at its default, and
/// what was wrong with which line. Blank lines and `#` or `;` comments are skipped.
pub fn parse(text: &str) -> (NotebookSettings, Vec<Warning>) {
    let mut settings = NotebookSettings::default();
    let mut warnings = Vec::new();
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let mut warn = |message: String| {
            warnings.push(Warning {
                line: line_number,
                message,
            });
        };
        let Some((key, value)) = trimmed.split_once('=') else {
            warn(format!(
                "line is not a recognized \"key=value\" setting: {trimmed}"
            ));
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let invalid = || format!("invalid value for {key}: \"{value}\"");
        let path_into = |slot: &mut PathBuf| inside_path(value).map(|path| *slot = path);
        let applied = match key {
            "version" => Some(()),
            "daily_folder" => path_into(&mut settings.daily_folder),
            "daily_template" => path_into(&mut settings.daily_template),
            "templates_folder" => path_into(&mut settings.templates_folder),
            "inbox" => path_into(&mut settings.inbox),
            "capture_folder" => path_into(&mut settings.capture_folder),
            "attachments_folder" => inside_path(value).map(|path| {
                settings.attachments_folder = Some(path);
            }),
            "daily_format" => dates::is_daily_format(value).then(|| {
                settings.daily_format = value.to_owned();
            }),
            "capture_target" => match value.to_ascii_lowercase().as_str() {
                "inbox" => Some(CaptureTarget::Inbox),
                "daily" => Some(CaptureTarget::Daily),
                "folder" => Some(CaptureTarget::Folder),
                _ => None,
            }
            .map(|target| settings.capture_target = target),
            _ => {
                warn(format!("unknown setting key: {key}"));
                continue;
            }
        };
        if applied.is_none() {
            warn(invalid());
        }
    }
    (settings, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file FastPad writes when the user first edits a notebook's settings: every key
    /// commented out at its default.
    const FASTPAD_STARTER: &str = "# FastPad notebook settings. Paths are relative to the notebook.\r\n\
        # Remove the # in front of a line to change it.\r\nversion=1\r\n\
        # daily_folder=journal\r\n# daily_format=YYYY-MM-DD\r\n\
        # daily_template=.fastpad/templates/daily.md\r\n# templates_folder=.fastpad/templates\r\n\
        # inbox=inbox.md\r\n# capture_folder=inbox\r\n# capture_target=inbox\r\n\
        # attachments_folder=attachments\r\n";

    #[test]
    fn a_missing_or_empty_file_means_the_defaults() {
        assert_eq!(parse(""), (NotebookSettings::default(), Vec::new()));
        let (settings, warnings) = parse(FASTPAD_STARTER);
        assert_eq!(settings, NotebookSettings::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn values_are_read_with_either_slash() {
        let (settings, warnings) = parse(
            "\u{FEFF}version=1\r\ndaily_folder = notes/daily\r\ndaily_format=YYYY-MM-DD ddd\r\n\
             templates_folder=templates\r\ncapture_target=Daily\r\nattachments_folder=./files\r\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.daily_folder, PathBuf::from(r"notes\daily"));
        assert_eq!(settings.daily_format, "YYYY-MM-DD ddd");
        assert_eq!(settings.templates_folder, PathBuf::from("templates"));
        assert_eq!(settings.capture_target, CaptureTarget::Daily);
        assert_eq!(settings.attachments_folder, Some(PathBuf::from("files")));
    }

    #[test]
    fn a_bad_value_keeps_its_default_and_names_its_line() {
        let (settings, warnings) = parse(
            "# notes\ndaily_folder=../outside\ndaily_format=YYYY-MM\ncapture_target=email\ncolour=blue\nnonsense\n",
        );
        assert_eq!(settings, NotebookSettings::default());
        let lines: Vec<usize> = warnings.iter().map(|warning| warning.line).collect();
        assert_eq!(lines, [2, 3, 4, 5, 6]);
        assert_eq!(
            warnings[0].message,
            "invalid value for daily_folder: \"../outside\""
        );
        assert_eq!(warnings[3].message, "unknown setting key: colour");
    }

    #[test]
    fn paths_must_stay_inside_the_notebook() {
        assert_eq!(inside_path("journal"), Some(PathBuf::from("journal")));
        assert_eq!(inside_path("./a/b"), Some(PathBuf::from(r"a\b")));
        assert_eq!(inside_path(""), None);
        assert_eq!(inside_path("."), None);
        assert_eq!(inside_path("C:/notes"), None);
        assert_eq!(inside_path("/notes"), None);
        assert_eq!(inside_path("a/../../b"), None);
    }
}
