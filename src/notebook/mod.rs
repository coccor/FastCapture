//! A notebook's conventions (spec §3, §11): FastPad's `.fastpad\notebook.ini`, FastCapture's own
//! `.fastcapture\capture.ini` and Obsidian's attachment folder, read together. Every file is
//! optional and none is ever written here. The pure modules are copied from FastPad, each naming
//! its source on its first line.

pub mod capture_ini;
pub mod dates;
pub mod frontmatter;
pub mod links;
pub mod notebook_ini;
pub mod obsidian;
pub mod title;

use capture_ini::CaptureSettings;
use notebook_ini::NotebookSettings;
use obsidian::ObsidianAttachments;
use std::path::{Path, PathBuf};

/// A line in one of the notebook's files that FastCapture couldn't use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileWarning {
    /// The file, relative to the notebook (`.fastpad\notebook.ini`, …).
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
}

/// What FastCapture needs to know about where things go in a notebook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conventions {
    pub root: PathBuf,
    pub notebook: NotebookSettings,
    pub capture: CaptureSettings,
    pub obsidian: Option<ObsidianAttachments>,
    pub warnings: Vec<FileWarning>,
}

impl Conventions {
    /// Reads the notebook at `root`. Missing or unreadable files mean their defaults.
    pub fn load(root: &Path) -> Self {
        let mut warnings = Vec::new();
        let mut note = |file: PathBuf, found: Vec<notebook_ini::Warning>| {
            let relative = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
            warnings.extend(found.into_iter().map(|warning| FileWarning {
                file: relative.clone(),
                line: warning.line,
                message: warning.message,
            }));
        };

        let notebook_path = notebook_ini::notebook_file(root);
        let notebook = match read_text(&notebook_path) {
            Some(text) => {
                let (settings, found) = notebook_ini::parse(&text);
                note(notebook_path, found);
                settings
            }
            None => NotebookSettings::default(),
        };
        let capture_path = capture_ini::capture_file(root);
        let capture = match read_text(&capture_path) {
            Some(text) => {
                let (settings, found) = capture_ini::parse(&text);
                note(capture_path, found);
                settings
            }
            None => CaptureSettings::default(),
        };
        let obsidian = read_text(&root.join(obsidian::OBSIDIAN_DIR).join(obsidian::APP_FILE))
            .and_then(|json| obsidian::obsidian_attachments(&json));

        Self {
            root: root.to_path_buf(),
            notebook,
            capture,
            obsidian,
            warnings,
        }
    }

    /// Where written notes land, relative to the notebook: `notebook.ini`'s `capture_folder`.
    pub fn capture_folder(&self) -> &Path {
        &self.notebook.capture_folder
    }

    /// Where raw sources go, relative to the notebook.
    pub fn raw_folder(&self) -> &Path {
        &self.capture.raw_folder
    }

    /// Where attachments for the note at `note` (relative to the notebook) go.
    pub fn attachments_folder_for(&self, note: &Path) -> PathBuf {
        obsidian::attachments_folder(
            self.notebook.attachments_folder.as_deref(),
            self.obsidian.as_ref(),
            note,
        )
    }

    pub fn daily_folder(&self) -> &Path {
        &self.notebook.daily_folder
    }

    pub fn daily_format(&self) -> &str {
        &self.notebook.daily_format
    }
}

/// A small settings file's text: UTF-8 (with or without a BOM) or UTF-16 with a BOM. `None` when
/// it is missing or can't be read.
fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let utf16 = |bytes: &[u8], from: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| from(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    Some(match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        other => String::from_utf8_lossy(other).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempNotebook(PathBuf);

    impl TempNotebook {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fastcapture-notebook-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for TempNotebook {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_notebook_with_no_files_uses_fastpads_defaults() {
        let notebook = TempNotebook::new("empty");
        let conventions = Conventions::load(&notebook.0);
        assert_eq!(conventions.capture_folder(), Path::new("inbox"));
        assert_eq!(conventions.raw_folder(), Path::new("raw"));
        assert_eq!(
            conventions.attachments_folder_for(Path::new(r"raw\x.md")),
            PathBuf::from("attachments")
        );
        assert_eq!(conventions.daily_folder(), Path::new("journal"));
        assert_eq!(conventions.daily_format(), "YYYY-MM-DD");
        assert!(conventions.warnings.is_empty());
    }

    #[test]
    fn fastpads_file_sets_the_folders_and_its_bad_lines_are_reported() {
        let notebook = TempNotebook::new("fastpad");
        notebook.write(
            r".fastpad\notebook.ini",
            b"\xEF\xBB\xBFcapture_folder=Inbox/new\r\nattachments_folder=files\r\ndaily_folder=../x\r\n",
        );
        let conventions = Conventions::load(&notebook.0);
        assert_eq!(conventions.capture_folder(), Path::new(r"Inbox\new"));
        assert_eq!(
            conventions.attachments_folder_for(Path::new(r"raw\x.md")),
            PathBuf::from("files")
        );
        assert_eq!(
            conventions.warnings,
            [FileWarning {
                file: PathBuf::from(r".fastpad\notebook.ini"),
                line: 3,
                message: "invalid value for daily_folder: \"../x\"".to_owned(),
            }]
        );
    }

    #[test]
    fn obsidians_folder_is_used_unless_notebook_ini_names_one() {
        let notebook = TempNotebook::new("obsidian");
        notebook.write(
            r".obsidian\app.json",
            br#"{"attachmentFolderPath": "./img"}"#,
        );
        let conventions = Conventions::load(&notebook.0);
        assert_eq!(
            conventions.attachments_folder_for(Path::new(r"raw\x.md")),
            PathBuf::from(r"raw\img")
        );
        notebook.write(r".fastpad\notebook.ini", b"attachments_folder=files\n");
        let conventions = Conventions::load(&notebook.0);
        assert_eq!(
            conventions.attachments_folder_for(Path::new(r"raw\x.md")),
            PathBuf::from("files")
        );
    }

    #[test]
    fn capture_ini_sets_the_raw_folder_and_utf16_files_are_read() {
        let notebook = TempNotebook::new("capture");
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "raw_folder=sources\r\n".encode_utf16() {
            utf16.extend(unit.to_le_bytes());
        }
        notebook.write(r".fastcapture\capture.ini", &utf16);
        let conventions = Conventions::load(&notebook.0);
        assert_eq!(conventions.raw_folder(), Path::new("sources"));
    }
}
