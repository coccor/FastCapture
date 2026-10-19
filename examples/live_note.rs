//! One real note from a sample capture, with the backend your settings choose, to see what a
//! model writes and what it costs. In Cloud mode this makes a real API request, billed to your
//! Anthropic account; it never runs in `cargo test` or CI.
//!
//! ```text
//! cargo run --release --example live_note -- [--effort low|medium|high] [--image PATH] ["text"]
//! ```
//!
//! The capture goes to a scratch notebook in the temp folder; your notebook isn't touched.

use fastcapture::capture::submit::submit;
use fastcapture::capture::{Attachment, Capture};
use fastcapture::config::settings::{self, Mode, SettingsFile};
use fastcapture::model::Backend;
use fastcapture::model::claude::Claude;
use fastcapture::model::request;
use fastcapture::note::render;
use fastcapture::note::reply::{self, Allowed};
use fastcapture::queue::Store;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SAMPLE: &str = "lag spikes during ledger batch?? seen on the payments board around 2am, \
only the ledger-events consumer group. need to check the batch size";

fn main() {
    let mut effort: Option<&'static str> = None;
    let mut image: Option<PathBuf> = None;
    let mut text = SAMPLE.to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--effort" => {
                effort = match args.next().as_deref() {
                    Some("low") => Some("low"),
                    Some("medium") => Some("medium"),
                    Some("high") => Some("high"),
                    other => panic!("unknown effort {other:?}"),
                }
            }
            "--image" => image = args.next().map(PathBuf::from),
            other => text = other.to_owned(),
        }
    }

    let path = settings::settings_path().expect("APPDATA is set");
    let settings = SettingsFile::load(&path).settings().0;
    let backend: Box<dyn Backend> = match (settings.mode, effort) {
        (Some(Mode::Cloud), Some(effort)) => {
            let key = fastcapture::secrets::load(&fastcapture::secrets::target("claude"))
                .expect("a Claude API key saved in Settings");
            Box::new(
                Claude::new(&settings.claude_model, key)
                    .unwrap()
                    .with_effort(effort),
            )
        }
        _ => fastcapture::runner::from_settings::build(&settings, fastcapture::secrets::load)
            .unwrap_or_else(|problem| panic!("{problem}")),
    };

    let scratch = std::env::temp_dir().join(format!("fastcapture-live-{}", std::process::id()));
    let notebook = scratch.join("Live test");
    std::fs::create_dir_all(&notebook).unwrap();
    let attachments = image
        .map(|path| vec![Attachment::File(path)])
        .unwrap_or_default();
    submit(
        &notebook,
        &Capture {
            text,
            attachments,
            source_window: None,
        },
        Some(&scratch.join("queue")),
        true,
    )
    .unwrap();
    let job = Store::new(scratch.join("queue")).list().remove(0);
    let request = request::build(&job, "2026-10-19").unwrap();

    let started = Instant::now();
    let result = backend.complete(&request);
    let elapsed = started.elapsed();
    match result {
        Ok(note) => {
            let note = reply::check(note, &request.capture.fallback_title, &Allowed::default());
            let facts = &request.capture;
            let text = render::render(&render::NoteFile {
                note: &note,
                captured: &facts.captured,
                capture_id: &facts.capture_id,
                raw: &facts.raw,
                kinds: &facts.kinds,
                model: &backend.label(),
                attachments: &facts.attachments,
                path: Path::new(r"inbox\note.md"),
            });
            println!("{text}");
        }
        Err(error) => println!("No note: {error}"),
    }
    println!("--- {} in {:.1} s", backend.label(), elapsed.as_secs_f64());
    // The usage line the Claude backend logs, when it is the backend.
    if let Some(log) = fastcapture::paths::log_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("fastcapture.log")).ok())
        && let Some(line) = log.lines().rev().find(|line| line.contains("claude: "))
    {
        println!("--- {line}");
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
