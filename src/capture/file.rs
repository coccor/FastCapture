//! Writing a capture into the notebook: the attachments are copied into the
//! attachments folder first, then the raw source is written into `raw/`. All or nothing: a
//! failure removes the files already written. `store` and `save_attachments` follow FastPad
//! `src/library/capture_file.rs`. Works without a window, for the box and `fastcapture add`.

use super::detect;
use super::model::{Attachment, Capture};
use super::raw::{self, RawSource, StoredAttachment};
use crate::notebook::Conventions;
use crate::notebook::dates::{Date, Time};
use crate::notebook::title;
use crate::platform::saver;
use std::path::{Path, PathBuf};

/// A file this large is asked about before it is copied into the notebook, and `add` refuses
/// it (as FastPad does).
pub const LARGE_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// One capture to write.
pub struct Request<'a> {
    pub conventions: &'a Conventions,
    pub capture: &'a Capture,
    pub capture_id: &'a str,
    pub date: Date,
    pub time: Time,
    /// The user agreed to copy files over `LARGE_FILE_BYTES`.
    pub allow_large: bool,
}

/// Where a capture went, relative to the notebook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Filed {
    pub raw: PathBuf,
    pub attachments: Vec<StoredAttachment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileError {
    /// Nothing was written: `path` is `bytes` long, over `LARGE_FILE_BYTES`, and `allow_large`
    /// was off. The box asks; `add` refuses.
    TooLarge { path: PathBuf, bytes: u64 },
    /// Nothing is left on disk; the message is for the box or stderr.
    Failed(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { path, bytes } => write!(
                f,
                "{} is {} MB, over the {} MB limit for copying into the notebook",
                path.file_name().unwrap_or_default().to_string_lossy(),
                bytes.div_ceil(1024 * 1024),
                LARGE_FILE_BYTES / (1024 * 1024)
            ),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Writes the capture: attachments, then the raw source. Nothing is ever overwritten.
pub fn file_capture(request: &Request<'_>) -> Result<Filed, FileError> {
    let capture = request.capture;
    if capture.is_empty() {
        return Err(FileError::Failed("There is nothing to capture.".to_owned()));
    }
    if !request.allow_large {
        for attachment in &capture.attachments {
            if let Attachment::File(path) = attachment {
                let bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
                if bytes > LARGE_FILE_BYTES {
                    return Err(FileError::TooLarge {
                        path: path.clone(),
                        bytes,
                    });
                }
            }
        }
    }

    let root = &request.conventions.root;
    let raw_folder = request.conventions.raw_folder().to_path_buf();
    let stem = raw::raw_stem(request.date, request.time, &capture.text);
    // Attachment folders and links depend only on the raw source's folder, so a name picked
    // now that is taken by the time it is written only needs the next free name.
    let placeholder = raw_folder.join(title::file_name(&stem, "md"));
    let attachments = save_attachments(request, &placeholder).map_err(FileError::Failed)?;
    let links = raw::link_lines(&placeholder, &attachments);
    let text = raw::raw_text(&RawSource {
        date: request.date,
        time: request.time,
        capture_id: request.capture_id,
        kinds: &detect::kinds(capture),
        text: &capture.text,
        links: &links,
        source_window: capture.source_window.as_deref(),
        source_url: capture.source_url.as_deref(),
    });

    let written = write_raw(root, &raw_folder, &stem, text.as_bytes());
    match written {
        Ok(raw) => Ok(Filed { raw, attachments }),
        Err(message) => {
            remove_attachments(root, &attachments);
            Err(FileError::Failed(message))
        }
    }
}

/// Writes the raw source under the first free `stem`, `stem 2`, … in `raw_folder`.
fn write_raw(root: &Path, raw_folder: &Path, stem: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let directory = root.join(raw_folder);
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "FastCapture could not create the folder {}: {error}",
            directory.display()
        )
    })?;
    loop {
        let name = title::free_name(stem, "md", |name| directory.join(name).exists());
        match saver::save_atomic_new(&directory.join(&name), bytes) {
            Ok(()) => return Ok(raw_folder.join(name)),
            // Something took the name between the check and the write: try the next.
            Err(error) if saver::is_already_exists(&error) => {}
            Err(error) => {
                return Err(format!(
                    "FastCapture could not save {}: {error}",
                    directory.join(name).display()
                ));
            }
        }
    }
}

/// Removes attachments written for a capture that then failed.
pub fn remove_attachments(root: &Path, attachments: &[StoredAttachment]) {
    for attachment in attachments {
        let _ = std::fs::remove_file(root.join(&attachment.path));
    }
}

/// Writes `source` into `directory` (made when needed) and returns the name it got, never over a
/// file: a pasted image as `image_stem.png`, a file as a copy under its own cleaned name, each
/// with ` 2`, ` 3`… when the name is taken.
pub fn store(directory: &Path, source: &Attachment, image_stem: &str) -> Result<String, String> {
    let (stem, extension, bytes) = match source {
        Attachment::Png(bytes) => (image_stem.to_owned(), "png".to_owned(), bytes.clone()),
        Attachment::File(path) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let (stem, extension) = match name.rsplit_once('.') {
                Some((stem, extension)) if !stem.is_empty() => {
                    (stem.to_owned(), extension.to_owned())
                }
                _ => (name.clone().into_owned(), String::new()),
            };
            let bytes = std::fs::read(path).map_err(|error| {
                format!("FastCapture could not read {}: {error}", path.display())
            })?;
            (title::sanitize_stem(&stem), extension, bytes)
        }
    };
    std::fs::create_dir_all(directory).map_err(|error| {
        format!(
            "FastCapture could not create the folder {}: {error}",
            directory.display()
        )
    })?;
    loop {
        let name = title::free_name(&stem, &extension, |name| directory.join(name).exists());
        match saver::save_atomic_new(&directory.join(&name), &bytes) {
            Ok(()) => return Ok(name),
            Err(error) if saver::is_already_exists(&error) => {}
            Err(error) => return Err(format!("FastCapture could not save {name}: {error}")),
        }
    }
}

/// Copies or writes the attachments into the attachments folder for the note at `note`, never
/// over a file. A failure removes the ones already written.
fn save_attachments(request: &Request<'_>, note: &Path) -> Result<Vec<StoredAttachment>, String> {
    let root = &request.conventions.root;
    let folder = request.conventions.attachments_folder_for(note);
    let directory = root.join(&folder);
    let image_stem = raw::capture_stem(
        request.date,
        request.time,
        &detect::prose(&request.capture.text),
    );
    let mut written: Vec<StoredAttachment> = Vec::new();
    for source in &request.capture.attachments {
        match store(&directory, source, &image_stem) {
            Ok(name) => written.push(StoredAttachment {
                image: matches!(source, Attachment::Png(_))
                    || detect::is_image_path(Path::new(&name)),
                path: folder.join(name),
            }),
            Err(error) => {
                remove_attachments(root, &written);
                return Err(error);
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempNotebook(PathBuf);

    impl TempNotebook {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("fastcapture-file-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn conventions(&self) -> Conventions {
            Conventions::load(&self.0)
        }

        fn files(&self, relative: &str) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.0.join(relative))
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    impl Drop for TempNotebook {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn request<'a>(conventions: &'a Conventions, capture: &'a Capture) -> Request<'a> {
        Request {
            conventions,
            capture,
            capture_id: "01TESTTESTTESTTESTTESTTEST",
            date: Date::new(2026, 10, 19).unwrap(),
            time: Time {
                hour: 10,
                minute: 30,
            },
            allow_large: false,
        }
    }

    #[test]
    fn a_capture_writes_its_attachments_then_its_raw_source() {
        let notebook = TempNotebook::new("write");
        let outside = notebook.0.join("ledger.log");
        fs::write(&outside, b"log line").unwrap();
        let capture = Capture {
            text: "lag spikes during ledger batch??".to_owned(),
            attachments: vec![
                Attachment::Png(b"png bytes".to_vec()),
                Attachment::File(outside.clone()),
            ],
            source_window: None,
            source_url: None,
        };
        let conventions = notebook.conventions();
        let filed = file_capture(&request(&conventions, &capture)).unwrap();

        assert_eq!(
            filed.raw,
            PathBuf::from(r"raw\2026-10-19 1030 lag spikes during ledger batch.md")
        );
        assert_eq!(
            notebook.files("attachments"),
            [
                "2026-10-19 1030 lag spikes during ledger batch.png",
                "ledger.log"
            ]
        );
        assert_eq!(
            fs::read(&outside).unwrap(),
            b"log line",
            "the original stays"
        );
        let text = fs::read_to_string(notebook.0.join(&filed.raw)).unwrap();
        assert!(text.contains("kind: [text, screenshot, file]\n"), "{text}");
        assert!(text.ends_with(
            "lag spikes during ledger batch??\n\n\
             ![](../attachments/2026-10-19%201030%20lag%20spikes%20during%20ledger%20batch.png)\n\
             [ledger.log](../attachments/ledger.log)\n"
        ));
    }

    #[test]
    fn a_taken_name_gets_a_number_and_nothing_is_overwritten() {
        let notebook = TempNotebook::new("clash");
        let capture = Capture {
            text: "same words".to_owned(),
            ..Capture::default()
        };
        let conventions = notebook.conventions();
        let first = file_capture(&request(&conventions, &capture)).unwrap();
        let second = file_capture(&request(&conventions, &capture)).unwrap();
        assert_eq!(
            first.raw,
            PathBuf::from(r"raw\2026-10-19 1030 same words.md")
        );
        assert_eq!(
            second.raw,
            PathBuf::from(r"raw\2026-10-19 1030 same words 2.md")
        );
    }

    #[test]
    fn conventions_choose_the_folders() {
        let notebook = TempNotebook::new("conventions");
        fs::create_dir_all(notebook.0.join(".obsidian")).unwrap();
        fs::write(
            notebook.0.join(r".obsidian\app.json"),
            br#"{"attachmentFolderPath": "./img"}"#,
        )
        .unwrap();
        fs::create_dir_all(notebook.0.join(".fastcapture")).unwrap();
        fs::write(
            notebook.0.join(r".fastcapture\capture.ini"),
            b"raw_folder=sources\n",
        )
        .unwrap();
        let capture = Capture {
            text: String::new(),
            attachments: vec![Attachment::Png(b"x".to_vec())],
            source_window: None,
            source_url: None,
        };
        let conventions = notebook.conventions();
        let filed = file_capture(&request(&conventions, &capture)).unwrap();
        assert_eq!(
            filed.raw,
            PathBuf::from(r"sources\2026-10-19 1030 capture.md")
        );
        assert_eq!(
            notebook.files(r"sources\img"),
            ["2026-10-19 1030 capture.png"]
        );
        let text = fs::read_to_string(notebook.0.join(&filed.raw)).unwrap();
        assert!(
            text.ends_with("\n![](img/2026-10-19%201030%20capture.png)\n"),
            "{text}"
        );
    }

    #[test]
    fn a_failure_leaves_nothing_behind() {
        let notebook = TempNotebook::new("failure");
        let missing = notebook.0.join("gone.log");
        let capture = Capture {
            text: "x".to_owned(),
            attachments: vec![Attachment::Png(b"x".to_vec()), Attachment::File(missing)],
            source_window: None,
            source_url: None,
        };
        let conventions = notebook.conventions();
        let error = file_capture(&request(&conventions, &capture)).unwrap_err();
        assert!(
            matches!(error, FileError::Failed(ref m) if m.contains("gone.log")),
            "{error}"
        );
        assert!(notebook.files("attachments").is_empty());
        assert!(notebook.files("raw").is_empty());

        // The raw source can't be written: a file stands where the raw folder should be.
        fs::write(notebook.0.join("raw"), b"in the way").unwrap();
        let capture = Capture {
            text: "y".to_owned(),
            attachments: vec![Attachment::Png(b"y".to_vec())],
            source_window: None,
            source_url: None,
        };
        assert!(file_capture(&request(&conventions, &capture)).is_err());
        assert!(notebook.files("attachments").is_empty());
    }

    #[test]
    fn large_files_are_refused_unless_allowed_and_empty_captures_always() {
        let notebook = TempNotebook::new("large");
        let big = notebook.0.join("big.bin");
        let file = fs::File::create(&big).unwrap();
        file.set_len(LARGE_FILE_BYTES + 1).unwrap();
        drop(file);
        let capture = Capture {
            text: String::new(),
            attachments: vec![Attachment::File(big.clone())],
            source_window: None,
            source_url: None,
        };
        let conventions = notebook.conventions();
        let error = file_capture(&request(&conventions, &capture)).unwrap_err();
        assert_eq!(
            error,
            FileError::TooLarge {
                path: big,
                bytes: LARGE_FILE_BYTES + 1
            }
        );
        assert_eq!(
            error.to_string(),
            "big.bin is 21 MB, over the 20 MB limit for copying into the notebook"
        );
        assert!(notebook.files("raw").is_empty());
        let allowed = Request {
            allow_large: true,
            ..request(&conventions, &capture)
        };
        assert!(file_capture(&allowed).is_ok());

        let empty = Capture::default();
        assert!(matches!(
            file_capture(&request(&conventions, &empty)),
            Err(FileError::Failed(_))
        ));
    }
}
