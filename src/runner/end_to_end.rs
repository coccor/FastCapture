//! End to end, without the internet: a capture (a screenshot, a link, a PDF) is filed and queued
//! as the box files it, the worker fetches what it links to and takes it to the Claude backend
//! or an OpenAI-compatible API (all scripted servers on loopback), and the note it writes, and
//! the tokens it used, are checked.

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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
fn run_until_done(
    queue: &Path,
    capture_id: &str,
    server: &TestServer,
    indexes: Option<crate::index::service::Indexes>,
) -> Store {
    let worker = Worker::start(
        queue.to_path_buf(),
        Box::new(ClaudeAt(server.url())),
        indexes,
        None,
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
    let store = run_until_done(&temp.0.join("queue"), &job.capture_id, &model, None);

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
    let store = run_until_done(&temp.0.join("queue"), &job.capture_id, &model, None);

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
    let store = run_until_done(&queue, &job.capture_id, &server, None);

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

#[test]
fn a_capture_links_its_notebook_and_is_filed_under_a_topic() {
    use crate::file::{self, Target};
    use crate::index::service::Indexes;
    let temp = Temp::new("filing");
    let notebook = temp.0.join("Work notes");
    let write = |path: &str, text: &str| {
        let path = notebook.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let topic = "---\ntype: topic\n---\n# Payments incidents\n\nEvery payments outage, newest last.\n\n- 2026-10-01 [[Card declines]]\n";
    write(r"Payments\Payments incidents.md", topic);
    write(
        r"Payments\Ledger batch.md",
        "# Ledger batch\n\nThe nightly ledger batch writes millions of records; consumer lag follows.\n",
    );
    write(r"Recipes\Soup.md", "# Soup\n\nLeeks and potatoes.\n");
    let job = temp.submit(Capture {
        text: "consumer lag spikes during ledger batch, another payments incident".to_owned(),
        ..Capture::default()
    });
    let note = json!({
        "title": "Consumer lag spikes during ledger batch",
        "summary": "Kafka consumer lag on payments jumps to about 40k during the ledger batch.",
        "body": "Consumer lag spikes when the ledger batch runs.",
        "key_points": [],
        "tasks": [],
        "tags": ["payments"],
        "links": [
            {"note": "Ledger batch", "new": false},
            {"note": "Invented note", "new": false}
        ],
        "home": {"kind": "topic", "note": "Payments incidents", "reason": "Another outage."}
    });
    let model = TestServer::start(vec![Scripted::json(
        200,
        &json!({
            "id": "msg_e2e",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": note.to_string()}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1200, "output_tokens": 150}
        })
        .to_string(),
    )]);
    let indexes = Indexes::new(None);
    let store = run_until_done(
        &temp.0.join("queue"),
        &job.capture_id,
        &model,
        Some(indexes.clone()),
    );

    // The request offered the notebook's related notes, the topic note as a home.
    let request = model.requests().remove(0).json();
    let user_text = request["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(user_text.contains("<candidate_notes>\n"), "{user_text}");
    assert!(user_text.contains(
        "<note name=\"Payments incidents\" folder=\"Payments\" type=\"topic\" home=\"yes\">"
    ));
    assert!(user_text.contains("<note name=\"Ledger batch\" folder=\"Payments\">"));
    assert!(user_text.contains("<home_folders>\nPayments/\n</home_folders>"));
    assert!(!user_text.contains("Soup"));

    // The note keeps the real link and the home, not the invented one.
    let written = store.get(&job.capture_id).unwrap().note.unwrap();
    let text = std::fs::read_to_string(notebook.join(&written.path)).unwrap();
    assert!(
        text.contains("suggested_home: \"[[Payments incidents]]\"\n"),
        "{text}"
    );
    assert!(
        text.ends_with("## Related\n\n- [[Ledger batch]]\n"),
        "{text}"
    );
    assert!(!text.contains("Invented"));

    // Filed under the topic: listed there, moved beside it.
    let (filed, _) = file::file_note(
        &notebook,
        &written.path,
        &Target::Topic(PathBuf::from(r"Payments\Payments incidents.md")),
        "2026-10-19",
        &|to| to.file_stem().unwrap().to_string_lossy().into_owned(),
    )
    .unwrap();
    assert_eq!(
        filed.path,
        PathBuf::from(r"Payments\Consumer lag spikes during ledger batch.md")
    );
    assert_eq!(
        std::fs::read_to_string(notebook.join(r"Payments\Payments incidents.md")).unwrap(),
        format!(
            "{topic}- 2026-10-19 [[Consumer lag spikes during ledger batch]]: Kafka consumer lag on payments jumps to about 40k during the ledger batch.\n"
        )
    );
    let moved = std::fs::read_to_string(notebook.join(&filed.path)).unwrap();
    assert!(moved.contains("status: filed\n") && !moved.contains("suggested_home"));
    assert!(!notebook.join(&written.path).exists());
    indexes.stop();
}

/// An OpenAI-compatible cloud API at a scripted server, with a monthly cap read each time.
struct OpenAiAt(String, Arc<AtomicU64>);

impl Factory for OpenAiAt {
    fn backend(&self) -> Result<Box<dyn Backend>, String> {
        crate::model::openai::OpenAi::new(
            &format!("{}/v1", self.0),
            "gpt-test",
            Some("sk-e2e".into()),
            "cloud",
            Duration::from_secs(5),
        )
        .map(|backend| Box::new(backend) as Box<dyn Backend>)
        .map_err(|error| error.to_string())
    }

    fn token_cap(&self) -> Option<u64> {
        Some(self.1.load(Ordering::SeqCst)).filter(|cap| *cap > 0)
    }
}

/// A chat completion with a note titled `title` that used 1,000 input tokens (400 cached) and
/// 200 output tokens.
fn chat_reply(title: &str) -> String {
    let note = json!({
        "title": title,
        "summary": "Lag during the batch.",
        "body": "Consumer lag spikes when the ledger batch runs.",
        "key_points": [],
        "tasks": [],
        "tags": ["kafka"],
        "links": [],
        "home": null
    });
    json!({
        "id": "chatcmpl-e2e",
        "object": "chat.completion",
        "choices": [{"index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "content": note.to_string(), "refusal": null}}],
        "usage": {"prompt_tokens": 1000, "completion_tokens": 200,
            "prompt_tokens_details": {"cached_tokens": 400}}
    })
    .to_string()
}

/// Waits up to ten seconds for `done`.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < until, "waited for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_openai_compatible_api_writes_notes_counts_their_tokens_and_stops_at_the_cap() {
    let temp = Temp::new("openai");
    let queue = temp.0.join("queue");
    let ledger = temp.0.join(super::usage::FILE);
    let server = TestServer::start(vec![
        Scripted::json(200, &chat_reply("Lag during the ledger batch")),
        Scripted::json(200, &chat_reply("Batch size and lag")),
    ]);
    // The first note's 1,200 tokens reach the cap.
    let cap = Arc::new(AtomicU64::new(1_200));
    let first = temp.submit(Capture {
        text: "lag spikes during ledger batch".to_owned(),
        ..Capture::default()
    });
    let worker = Worker::start(
        queue.clone(),
        Box::new(OpenAiAt(server.url(), cap.clone())),
        None,
        Some(ledger.clone()),
        Box::new(|| {}),
    );
    let store = Store::new(queue.clone());
    wait_for("the first note", || {
        store
            .get(&first.capture_id)
            .is_some_and(|job| job.state == State::Done)
    });

    let written = store.get(&first.capture_id).unwrap().note.unwrap();
    let note = std::fs::read_to_string(temp.0.join("Work notes").join(&written.path)).unwrap();
    assert!(note.contains("model: \"cloud/gpt-test\""), "{note}");
    assert!(note.contains("# Lag during the ledger batch"), "{note}");
    let seen = &server.requests()[0];
    assert_eq!(seen.path, "/v1/chat/completions");
    assert_eq!(seen.header("authorization"), Some("Bearer sk-e2e"));
    let body = seen.json();
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["response_format"]["type"], "json_schema");
    let today = super::usage::today();
    assert_eq!(
        super::usage::Ledger::load(&ledger).day(&today),
        crate::model::Usage {
            input: 600,
            cache_read: 400,
            cache_write: 0,
            output: 200
        }
    );

    // The next capture waits: this month's tokens have reached the cap.
    let second = temp.submit(Capture {
        text: "batch size vs lag".to_owned(),
        ..Capture::default()
    });
    let second = if second.capture_id == first.capture_id {
        store
            .list()
            .into_iter()
            .find(|job| job.capture_id != first.capture_id)
            .unwrap()
    } else {
        second
    };
    worker.waker().wake();
    wait_for("the cap's pause", || {
        matches!(
            worker.status(),
            super::Status::Paused(Some(ref reason)) if reason.starts_with("This month's token cap (1,200) is reached")
        )
    });
    assert_eq!(store.get(&second.capture_id).unwrap().state, State::Queued);
    assert_eq!(server.requests().len(), 1, "nothing more was sent");

    // A higher cap lets it run.
    cap.store(10_000, Ordering::SeqCst);
    worker.waker().wake();
    wait_for("the second note", || {
        store
            .get(&second.capture_id)
            .is_some_and(|job| job.state == State::Done)
    });
    worker.stop();
    assert_eq!(
        super::usage::Ledger::load(&ledger).month(&today).total(),
        2_400
    );
}
