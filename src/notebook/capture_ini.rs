//! `.fastcapture\capture.ini`: FastCapture's own keys for a notebook, kept apart from
//! FastPad's `notebook.ini`. A missing file means the defaults; a bad value falls back to its
//! default and is reported with its line, as `notebook.ini` does. M1 only reads it.

use super::notebook_ini::{Warning, inside_path};
use std::path::{Path, PathBuf};

/// FastCapture's folder inside a notebook, and the file's name there.
pub const FASTCAPTURE_DIR: &str = ".fastcapture";
pub const CAPTURE_FILE: &str = "capture.ini";

/// Where `capture.ini` lives for the notebook at `folder`.
pub fn capture_file(folder: &Path) -> PathBuf {
    folder.join(FASTCAPTURE_DIR).join(CAPTURE_FILE)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureSettings {
    /// Folder for raw sources, relative to the notebook.
    pub raw_folder: PathBuf,
    /// Front matter `type:` values that mark a note as a possible home, lowercase.
    pub home_types: Vec<String>,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            raw_folder: PathBuf::from("raw"),
            home_types: ["topic", "project", "moc"].map(str::to_owned).to_vec(),
        }
    }
}

/// Reads `text`, the file's contents: the settings with every bad value at its default, and
/// what was wrong with which line. Blank lines and `#` or `;` comments are skipped.
pub fn parse(text: &str) -> (CaptureSettings, Vec<Warning>) {
    let mut settings = CaptureSettings::default();
    let mut warnings = Vec::new();
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let mut warn = |message: String| {
            warnings.push(Warning {
                line: index + 1,
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
        let applied = match key {
            "version" => Some(()),
            "raw_folder" => inside_path(value).map(|path| settings.raw_folder = path),
            "home_types" => {
                let types: Vec<String> = value
                    .split(',')
                    .map(|item| item.trim().to_lowercase())
                    .filter(|item| !item.is_empty())
                    .collect();
                (!types.is_empty()).then(|| settings.home_types = types)
            }
            _ => {
                warn(format!("unknown setting key: {key}"));
                continue;
            }
        };
        if applied.is_none() {
            warn(format!("invalid value for {key}: \"{value}\""));
        }
    }
    (settings, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_means_the_defaults() {
        let (settings, warnings) = parse("");
        assert_eq!(settings, CaptureSettings::default());
        assert!(warnings.is_empty());
        assert_eq!(settings.raw_folder, PathBuf::from("raw"));
        assert_eq!(settings.home_types, ["topic", "project", "moc"]);
    }

    #[test]
    fn values_are_read() {
        let (settings, warnings) = parse(
            "\u{FEFF}version=1\r\n# sources\r\nraw_folder = sources/raw\r\nhome_types=Topic, area ,\r\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.raw_folder, PathBuf::from(r"sources\raw"));
        assert_eq!(settings.home_types, ["topic", "area"]);
    }

    #[test]
    fn a_bad_value_keeps_its_default_and_names_its_line() {
        let (settings, warnings) =
            parse("raw_folder=../outside\nhome_types= , \nmodel=claude\nnonsense\n");
        assert_eq!(settings, CaptureSettings::default());
        let lines: Vec<usize> = warnings.iter().map(|warning| warning.line).collect();
        assert_eq!(lines, [1, 2, 3, 4]);
        assert_eq!(
            warnings[0].message,
            "invalid value for raw_folder: \"../outside\""
        );
        assert_eq!(warnings[2].message, "unknown setting key: model");
    }
}
