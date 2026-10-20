//! One real note from a sample capture, with the backend your settings choose, to see what a
//! model writes and what it costs. In Cloud mode this makes a real API request, billed to your
//! account with the provider Settings chose (Anthropic, or the OpenAI-compatible API); it never
//! runs in `cargo test` or CI. `--effort` applies to the Claude API only.
//!
//! ```text
//! cargo run --release --example live_note -- [--dry-run] [--effort low|medium|high] [--claude-code MODEL] [--copilot MODEL] [--notebook PATH] [--attach PATH]… ["text"]
//! ```
//!
//! `--claude-code` uses your installed Claude Code with that model (`haiku`, `sonnet`…),
//! whatever the settings say: the note counts against your Claude plan, with no API bill.
//! `--copilot` uses your installed Copilot CLI with that model (`auto`…), whatever the settings
//! say: the note uses your Copilot plan's allowance.
//!
//! `--dry-run` fetches pages and prints the request's text without asking the model: free.
//!
//! `--notebook` offers that notebook's related notes as candidates, so the note can link them
//! and suggest a home. The notebook is only read, into an index kept in memory: nothing is
//! written there, and no index cache is kept.
//!
//! `--attach` takes any file (an image, a PDF, a Word file, a log) and can be given more than
//! once; links in the text are fetched as the worker fetches them. The capture goes to a scratch
//! notebook in the temp folder; your notebook isn't touched.

use fastcapture::capture::submit::submit;
use fastcapture::capture::{Attachment, Capture};
use fastcapture::config::settings::{self, CloudBackend, Mode, SettingsFile};
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
    let mut dry_run = false;
    let mut attach: Vec<PathBuf> = Vec::new();
    let mut related: Option<PathBuf> = None;
    let mut claude_code: Option<String> = None;
    let mut copilot: Option<String> = None;
    let mut text = SAMPLE.to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--effort" => {
                effort = match args.next().as_deref() {
                    Some("low") => Some("low"),
                    Some("medium") => Some("medium"),
                    Some("high") => Some("high"),
                    other => panic!("unknown effort {other:?}"),
                }
            }
            "--attach" | "--image" => attach.extend(args.next().map(PathBuf::from)),
            "--notebook" => related = args.next().map(PathBuf::from),
            "--claude-code" => claude_code = args.next(),
            "--copilot" => copilot = args.next(),
            other => text = other.to_owned(),
        }
    }

    let path = settings::settings_path().expect("APPDATA is set");
    let settings = SettingsFile::load(&path).settings().0;
    let backend: Box<dyn Backend> = match (settings.mode, settings.backend, effort) {
        _ if claude_code.is_some() => Box::new(
            fastcapture::model::claude_code::ClaudeCode::new(
                settings.claude_code_path.as_deref(),
                claude_code.as_deref().unwrap_or_default(),
            )
            .unwrap_or_else(|problem| panic!("{problem}")),
        ),
        _ if copilot.is_some() => Box::new(
            fastcapture::model::copilot::Copilot::new(
                settings.copilot_path.as_deref(),
                copilot.as_deref().unwrap_or_default(),
            )
            .unwrap_or_else(|problem| panic!("{problem}")),
        ),
        (Some(Mode::Cloud), CloudBackend::Claude, Some(effort)) => {
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
    let attachments = attach.into_iter().map(Attachment::File).collect();
    submit(
        &notebook,
        &Capture {
            text,
            attachments,
            source_window: None,
            source_url: None,
        },
        Some(&scratch.join("queue")),
        true,
    )
    .unwrap();
    let mut job = Store::new(scratch.join("queue")).list().remove(0);
    // As on the worker's last try: a page that can't be reached is recorded, not waited for.
    job.attempts = fastcapture::runner::pages::FETCH_TRIES;
    let fetching = Instant::now();
    fastcapture::runner::pages::fetch_pages(&job, "2026-10-19T12:00").unwrap();
    let fetched = fetching.elapsed();
    for (path, sidecar) in
        fastcapture::capture::sidecar::find(&job.notebook, &job.raw, &job.capture_id)
    {
        let outcome = match &sidecar.page {
            Ok(page) => format!("{} characters of main text", page.markdown.chars().count()),
            Err(reason) => reason.clone(),
        };
        println!("--- page {}: {outcome} ({})", sidecar.url, path.display());
    }
    if fetched.as_millis() > 0 {
        println!("--- pages fetched in {:.1} s", fetched.as_secs_f64());
    }
    let indexes = fastcapture::index::service::Indexes::new(None);
    let snapshot = related.as_deref().map(|notebook| {
        let indexing = Instant::now();
        let snapshot = indexes
            .snapshot(notebook, std::time::Duration::from_secs(120))
            .unwrap_or_else(|| panic!("{} couldn't be indexed", notebook.display()));
        println!(
            "--- {} notes indexed in {:.1} s",
            snapshot.searcher.index().notes.len(),
            indexing.elapsed().as_secs_f64()
        );
        snapshot
    });
    let building = Instant::now();
    let request = request::build_with(&job, "2026-10-19", snapshot.as_deref()).unwrap();
    indexes.stop();
    println!(
        "--- request built in {:.0} ms: {} characters of text, {} images",
        building.elapsed().as_secs_f64() * 1000.0,
        request.user_text.chars().count(),
        request.images.len()
    );
    if !request.capture.candidates.is_empty() {
        println!(
            "--- {} candidates: {}",
            request.capture.candidates.len(),
            request.capture.candidates.join(", ")
        );
    }

    if dry_run {
        println!("{}", request.user_text);
        let _ = std::fs::remove_dir_all(&scratch);
        return;
    }

    let started = Instant::now();
    let result = backend.complete(&request);
    let elapsed = started.elapsed();
    match result {
        Ok(note) => {
            let facts = &request.capture;
            let note = reply::check(
                note,
                &facts.fallback_title,
                &Allowed {
                    notes: &facts.candidates,
                    home_notes: &facts.home_notes,
                    home_folders: &facts.home_folders,
                },
            );
            let text = render::render(&render::NoteFile {
                note: &note,
                captured: &facts.captured,
                capture_id: &facts.capture_id,
                raw: &facts.raw,
                kinds: &facts.kinds,
                model: &backend.label(),
                attachments: &facts.attachments,
                links: &facts.links,
                truncated: &facts.truncated,
                path: Path::new(r"inbox\note.md"),
            });
            println!("{text}");
        }
        Err(error) => println!("No note: {error}"),
    }
    println!("--- {} in {:.1} s", backend.label(), elapsed.as_secs_f64());
    if let Some(usage) = backend.take_usage() {
        println!(
            "--- {} input ({} cached, {} written to the cache), {} output tokens",
            usage.input, usage.cache_read, usage.cache_write, usage.output
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
