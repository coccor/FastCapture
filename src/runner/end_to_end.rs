//! End to end, without the internet: a capture (a screenshot, a link, a PDF) is filed and queued
//! as the box files it, the worker fetches what it links to and takes it to the Claude backend
//! (both from scripted servers on loopback), and the note it writes is checked.

use super::worker::{Factory, Worker};
use crate::capture::submit::submit;
use crate::capture::{Attachment, Capture};
use crate::model::Backend;
use crate::model::claude::{self, Claude};
use crate::net::test_server::{Scripted, TestServer};
use crate::platform::png::{Pixels, encode_png};
use crate::queue::{State, Store};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct ClaudeAt(String);

impl Factory for ClaudeAt {
    fn backend(&self) -> Result<Box<dyn Backend>, String> {
        Claude::with_base(&self.0, claude::DEFAULT_MODEL, "sk-ant-e2e".into())
            .map(|backend| Box::new(backend) as Box<dyn Backend>)
            .map_err(|error| error.to_string())
    }
}

fn reply() -> String {
    let note = json!({
        "title": "Consumer lag spikes during ledger batch",
        "summary": "Kafka consumer lag on payments jumps to about 40k when the nightly ledger batch runs.",
        "body": "Consumer lag spikes when the ledger batch runs. Seen on the Grafana payments board.",
        "key_points": ["Lag peaks around 02:10 and recovers by 02:35"],
        "tasks": ["Check the batch size in the ledger job"],
        "tags": ["investigate", "payments"],
        "links": [{"note": "Invented note", "new": false}],
        "home": null
    });
    json!({
        "id": "msg_e2e",
        "type": "message",
        "role": "assistant",
        "model": claude::DEFAULT_MODEL,
        "content": [{"type": "text", "text": note.to_string()}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 2400, "cache_read_input_tokens": 1500, "output_tokens": 180}
    })
    .to_string()
}

/// Runs the worker with Claude at `server` until the job is done.
fn run_until_done(queue: &Path, capture_id: &str, server: &TestServer) -> Store {
    let worker = Worker::start(
        queue.to_path_buf(),
        Box::new(ClaudeAt(server.url())),
        Box::new(|| {}),
    );
    let store = Store::new(queue.to_path_buf());
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while store
        .get(capture_id)
        .is_some_and(|job| job.state != State::Done)
    {
        assert!(
            std::time::Instant::now() < until,
            "{:?}",
            store.get(capture_id)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.stop();
    store
}

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Work notes")).unwrap();
        Self(dir)
    }

    fn submit(&self, capture: Capture) -> crate::queue::Job {
        submit(
            &self.0.join("Work notes"),
            &capture,
            Some(&self.0.join("queue")),
            true,
        )
        .unwrap();
        Store::new(self.0.join("queue")).list().remove(0)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn reply_with(title: &str, body: &str) -> String {
    let note = json!({
        "title": title,
        "summary": "",
        "body": body,
        "key_points": [],
        "tasks": [],
        "tags": ["kafka"],
        "links": [],
        "home": null
    });
    json!({
        "id": "msg_e2e",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": note.to_string()}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 900, "output_tokens": 80}
    })
    .to_string()
}

const ARTICLE: &str = "<html><head><title>Consumer lag, explained</title>\
    <meta property=\"og:site_name\" content=\"Kafka notes\"></head><body>\
    <nav><a href=\"/\">Home</a> <a href=\"/blog\">Blog</a></nav><article>\
    <h1>Consumer lag, explained</h1>\
    <p>Consumer lag grows when a group falls behind its topic, which happens during large batches, \
    and it recovers once the batch is done, usually within half an hour of the start.</p>\
    <p>Capping the batch size keeps the lag down, at the cost of a longer run overall, \
    which is usually the better trade for payments systems.</p></article>\
    <footer>All rights reserved.</footer></body></html>";

#[test]
fn a_link_capture_becomes_a_note_about_its_page() {
    let temp = Temp::new("link");
    let pages = TestServer::start(vec![Scripted {
        status: 200,
        headers: vec![(
            "content-type".to_owned(),
            "text/html; charset=utf-8".to_owned(),
        )],
        body: ARTICLE.as_bytes().to_vec(),
        delay: Duration::ZERO,
    }]);
    let url = format!("{}/lag", pages.url());
    let job = temp.submit(Capture {
        text: format!("worth reading before the review\n{url}"),
        ..Capture::default()
    });
    let model = TestServer::start(vec![Scripted::json(
        200,
        &reply_with("Consumer lag explained", "Why lag grows during batches."),
    )]);
    let store = run_until_done(&temp.0.join("queue"), &job.capture_id, &model);

    // The page was fetched once, kept beside the raw source, and sent as material.
    assert_eq!(pages.requests().len(), 1);
    let kept = crate::capture::sidecar::find(&job.notebook, &job.raw, &job.capture_id);
    assert_eq!(kept.len(), 1);
    let request = model.requests().remove(0).json();
    let user_text = request["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(user_text.contains(&format!(
        "<source kind=\"link\" url=\"{url}\" title=\"Consumer lag, explained\" site=\"Kafka notes\">\n\
         # Consumer lag, explained\n\nConsumer lag grows when"
    )));
    assert!(!user_text.contains("All rights reserved"));
    assert!(!user_text.contains("Blog"));

    let written = store.get(&job.capture_id).unwrap().note.unwrap();
    let raw_stem = job.raw.file_stem().unwrap().to_string_lossy().into_owned();
    let expected = format!(
        "---\n\
         type: capture\n\
         captured: {captured}\n\
         capture_id: {id}\n\
         source: \"[[raw/{raw_stem}]]\"\n\
         kind: [text, link]\n\
         url: \"{url}\"\n\
         site: \"Kafka notes\"\n\
         tags: [kafka]\n\
         status: inbox\n\
         model: \"cloud/{model}\"\n\
         ---\n\
         \n\
         # Consumer lag explained\n\
         \n\
         Why lag grows during batches.\n\
         \n\
         ## Source\n\
         \n\
         - [Consumer lag, explained]({url})\n",
        captured = job.created,
        id = job.capture_id,
        model = claude::DEFAULT_MODEL,
    );
    let note = std::fs::read_to_string(job.notebook.join(&written.path)).unwrap();
    assert_eq!(note, expected);
}

#[test]
fn a_pdf_capture_sends_its_text_and_its_scanned_page() {
    use crate::extract::pdf::build::{Page, pdf};
    let temp = Temp::new("pdf");
    let file = temp.0.join("review.pdf");
    std::fs::write(
        &file,
        pdf(
            &[
                Page::Text(&["Ledger batch review: lag peaked at forty thousand messages."]),
                Page::Picture,
            ],
            false,
        ),
    )
    .unwrap();
    let job = temp.submit(Capture {
        text: "the review".to_owned(),
        attachments: vec![Attachment::File(file)],
        ..Capture::default()
    });
    let model = TestServer::start(vec![Scripted::json(
        200,
        &reply_with("Ledger batch review", "The review of the night run."),
    )]);
    let store = run_until_done(&temp.0.join("queue"), &job.capture_id, &model);

    let request = model.requests().remove(0).json();
    let content = request["messages"][0]["content"].as_array().unwrap();
    // The scanned page as an image, then the user turn with the text pages.
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "image");
    let user_text = content[1]["text"].as_str().unwrap();
    assert!(user_text.contains(
        "<attachment name=\"review.pdf\" kind=\"pdf\" pages=\"2\">\n## Page 1\n\nLedger batch review"
    ));
    assert!(user_text.contains("1 image is attached above"));

    let written = store.get(&job.capture_id).unwrap().note.unwrap();
    let note = std::fs::read_to_string(job.notebook.join(&written.path)).unwrap();
    assert!(note.contains("kind: [text, file]\n"), "{note}");
    assert!(
        note.ends_with("## Attachments\n\n[review.pdf](../attachments/review.pdf)\n"),
        "{note}"
    );
}

#[test]
fn a_screenshot_capture_becomes_a_note() {
    let temp = std::env::temp_dir().join(format!("fastcapture-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let notebook = temp.join("Work notes");
    let queue = temp.join("queue");
    std::fs::create_dir_all(&notebook).unwrap();
    let png = encode_png(&Pixels {
        width: 40,
        height: 30,
        bgra: vec![90; 40 * 30 * 4],
    })
    .unwrap();
    submit(
        &notebook,
        &Capture {
            text: "lag spikes during ledger batch??".to_owned(),
            attachments: vec![Attachment::Png(png.clone())],
            source_window: None,
            source_url: None,
        },
        Some(&queue),
        true,
    )
    .unwrap();
    let job = Store::new(queue.clone()).list().remove(0);

    let server = TestServer::start(vec![Scripted::json(200, &reply())]);
    let store = run_until_done(&queue, &job.capture_id, &server);

    // What was sent: the screenshot as it was, then the user turn.
    let request = server.requests().remove(0).json();
    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["source"]["media_type"], "image/png");
    assert_eq!(
        content[0]["source"]["data"],
        crate::model::images::base64(&png)
    );
    let user_text = content[1]["text"].as_str().unwrap();
    assert!(user_text.contains("<your_note>\nlag spikes during ledger batch??\n</your_note>"));
    assert!(user_text.contains("1 image is attached above"));

    // What was written, byte for byte.
    let done = store.get(&job.capture_id).unwrap();
    let written = done.note.unwrap();
    assert_eq!(
        written.path,
        PathBuf::from(r"inbox\Consumer lag spikes during ledger batch.md")
    );
    let raw_stem = job.raw.file_stem().unwrap().to_string_lossy().into_owned();
    let image = format!("{raw_stem}.png").replace(' ', "%20");
    let expected = format!(
        "---\n\
         type: capture\n\
         captured: {captured}\n\
         capture_id: {id}\n\
         source: \"[[raw/{raw_stem}]]\"\n\
         kind: [text, screenshot]\n\
         tags: [investigate, payments]\n\
         summary: \"Kafka consumer lag on payments jumps to about 40k when the nightly ledger batch runs.\"\n\
         status: inbox\n\
         model: \"cloud/{model}\"\n\
         ---\n\
         \n\
         # Consumer lag spikes during ledger batch\n\
         \n\
         > Kafka consumer lag on payments jumps to about 40k when the nightly ledger batch runs.\n\
         \n\
         Consumer lag spikes when the ledger batch runs. Seen on the Grafana payments board.\n\
         \n\
         ## Key points\n\
         \n\
         - Lag peaks around 02:10 and recovers by 02:35\n\
         \n\
         ## Tasks\n\
         \n\
         - [ ] Check the batch size in the ledger job\n\
         \n\
         ## Attachments\n\
         \n\
         ![](../attachments/{image})\n",
        captured = job.created,
        id = job.capture_id,
        model = claude::DEFAULT_MODEL,
    );
    let note = std::fs::read_to_string(notebook.join(&written.path)).unwrap();
    assert_eq!(note, expected);
    let _ = std::fs::remove_dir_all(&temp);
}
