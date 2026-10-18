//! `fastcapture add`: a capture with no window. When the tray app is running the
//! capture is forwarded to it over the pipe; otherwise this process files it and queues it, and
//! the tray app processes it when it next runs. `handle` is shared: the tray app answers
//! forwarded captures with it.

use crate::capture::file::FileError;
use crate::capture::submit::submit;
use crate::capture::{Attachment, Capture};
use crate::config::settings::{self, SettingsFile};
use crate::ipc::pipe::{self, SendError};
use crate::ipc::{Reply, Request};
use crate::launch::{AddArgs, AddText, MAX_TEXT_BYTES, too_long};
use std::io::{Read, Write};
use std::path::PathBuf;

/// Runs `add` and returns the exit code: 0 when the raw source is written, 1 when filing
/// failed, 2 for bad arguments, text that is too long or a file that is too large.
pub fn run(args: AddArgs) -> i32 {
    crate::platform::console::attach();
    let text = match &args.text {
        AddText::None => String::new(),
        AddText::Given(text) => text.clone(),
        AddText::Stdin => {
            let mut bytes = Vec::new();
            let read = std::io::stdin()
                .take(MAX_TEXT_BYTES as u64 + 1)
                .read_to_end(&mut bytes);
            if let Err(error) = read {
                return fail(2, &format!("Couldn't read stdin: {error}"));
            }
            if bytes.len() > MAX_TEXT_BYTES {
                return fail(2, &too_long());
            }
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            match String::from_utf8(bytes.to_vec()) {
                Ok(text) => text,
                Err(_) => return fail(2, "The text on stdin must be UTF-8."),
            }
        }
    };
    let request = Request::Add {
        text: args.capture_text(&text),
        attachments: args.attachments.clone(),
        notebook: args.notebook.clone(),
    };
    let reply = match pipe::send(&request) {
        Ok(reply) => reply,
        Err(SendError::NotRunning) => handle(request, &Context::from_environment()),
        Err(SendError::Failed(message)) => return fail(1, &message),
    };
    match reply {
        Reply::Filed(path) => {
            let _ = writeln!(std::io::stdout(), "{}", path.display());
            0
        }
        Reply::Failed { exit_code, message } => fail(i32::from(exit_code), &message),
    }
}

fn fail(code: i32, message: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "fastcapture: {message}");
    code
}

/// Where `handle` reads and writes its own state.
pub struct Context {
    /// `fastcapture.ini`.
    pub settings: Option<PathBuf>,
    pub queue: Option<PathBuf>,
    /// FastPad's data folder and Documents, for choosing the first notebook.
    pub fastpad_data: Option<PathBuf>,
    pub documents: Option<PathBuf>,
}

impl Context {
    pub fn from_environment() -> Self {
        Self {
            settings: settings::settings_path(),
            queue: crate::queue::store::default_dir(),
            fastpad_data: settings::fastpad_data_dir(),
            documents: crate::platform::shell::documents_folder(),
        }
    }

    /// The capture notebook from the settings, or the first-run choice, which is then saved.
    pub fn notebook(&self) -> Option<PathBuf> {
        let path = self.settings.as_deref();
        let mut file = path.map(SettingsFile::load).unwrap_or_default();
        if let Some(notebook) = file.settings().0.notebook {
            return Some(notebook);
        }
        let chosen =
            settings::default_notebook(self.fastpad_data.as_deref(), self.documents.as_deref())?;
        if let Some(path) = path {
            file.choose_notebook(&chosen);
            if let Err(error) = file.save(path) {
                crate::log::line(&format!("settings: {error}"));
            }
        }
        Some(chosen)
    }
}

/// Files a forwarded or local `add` capture and queues it.
pub fn handle(request: Request, context: &Context) -> Reply {
    let Request::Add {
        text,
        attachments,
        notebook,
    } = request;
    let failed = |exit_code: u8, message: String| Reply::Failed { exit_code, message };
    for path in &attachments {
        if !path.is_file() {
            return failed(2, format!("No such file: {}", path.display()));
        }
    }
    let Some(root) = notebook.or_else(|| context.notebook()) else {
        return failed(
            1,
            "No notebook is set. Run fastcapture --settings.".to_owned(),
        );
    };
    let capture = Capture {
        text,
        attachments: attachments.into_iter().map(Attachment::File).collect(),
        source_window: None,
    };
    match submit(&root, &capture, context.queue.as_deref(), false) {
        Ok(raw) => Reply::Filed(raw),
        Err(error @ FileError::TooLarge { .. }) => failed(2, format!("{error}.")),
        Err(FileError::Failed(message)) => failed(1, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::file;
    use crate::queue::{State, Store};
    use std::fs;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("fastcapture-add-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn context(&self) -> Context {
            Context {
                settings: Some(self.0.join(r"AppData\FastCapture\fastcapture.ini")),
                queue: Some(self.0.join("queue")),
                fastpad_data: Some(self.0.join("FastPad")),
                documents: Some(self.0.join("Documents")),
            }
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn add(text: &str, attachments: Vec<PathBuf>, notebook: Option<PathBuf>) -> Request {
        Request::Add {
            text: text.to_owned(),
            attachments,
            notebook,
        }
    }

    #[test]
    fn a_capture_is_filed_queued_and_the_first_notebook_remembered() {
        let temp = Temp::new("filed");
        let context = temp.context();
        let log = temp.0.join("ledger.log");
        fs::write(&log, b"x").unwrap();
        let reply = handle(
            add("lag spikes\nhttps://grafana.example/d", vec![log], None),
            &context,
        );
        let Reply::Filed(raw) = reply else {
            panic!("{reply:?}")
        };
        let notebook = temp.0.join(r"Documents\FastPad");
        assert!(raw.starts_with(notebook.join("raw")), "{}", raw.display());
        assert!(notebook.join(r"attachments\ledger.log").is_file());
        let text = fs::read_to_string(&raw).unwrap();
        assert!(text.contains("kind: [text, link, file]"), "{text}");

        let jobs = Store::new(temp.0.join("queue")).list();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].state, State::Queued);
        assert_eq!(jobs[0].notebook, notebook);
        assert_eq!(notebook.join(&jobs[0].raw), raw);
        assert!(text.contains(&format!("capture_id: {}", jobs[0].capture_id)));

        let saved = SettingsFile::load(context.settings.as_ref().unwrap())
            .settings()
            .0;
        assert_eq!(saved.notebook, Some(notebook));
    }

    #[test]
    fn bad_requests_get_exit_code_2_and_failures_1() {
        let temp = Temp::new("errors");
        let context = temp.context();
        let reply = handle(add("x", vec![temp.0.join("missing.log")], None), &context);
        assert!(
            matches!(&reply, Reply::Failed { exit_code: 2, message } if message.starts_with("No such file")),
            "{reply:?}"
        );
        let big = temp.0.join("big.bin");
        fs::File::create(&big)
            .unwrap()
            .set_len(file::LARGE_FILE_BYTES + 1)
            .unwrap();
        let reply = handle(add("", vec![big], None), &context);
        assert!(
            matches!(reply, Reply::Failed { exit_code: 2, .. }),
            "{reply:?}"
        );

        // A file where the notebook's raw folder should be.
        let notebook = temp.0.join("Blocked");
        fs::create_dir_all(&notebook).unwrap();
        fs::write(notebook.join("raw"), b"in the way").unwrap();
        let reply = handle(add("x", Vec::new(), Some(notebook)), &context);
        assert!(
            matches!(reply, Reply::Failed { exit_code: 1, .. }),
            "{reply:?}"
        );
    }
}
