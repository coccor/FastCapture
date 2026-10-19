//! End to end, without the internet: a capture with text and a screenshot is filed and queued
//! as the box files it, the worker takes it to the Claude backend (talking to a scripted server
//! on loopback), and the note it writes is checked byte for byte.

use super::worker::{Factory, Worker};
use crate::capture::submit::submit;
use crate::capture::{Attachment, Capture};
use crate::model::Backend;
use crate::model::claude::{self, Claude};
use crate::net::test_server::{Scripted, TestServer};
use crate::platform::png::{Pixels, encode_png};
use crate::queue::{State, Store};
use serde_json::json;
use std::path::PathBuf;
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
        },
        Some(&queue),
        true,
    )
    .unwrap();
    let job = Store::new(queue.clone()).list().remove(0);

    let server = TestServer::start(vec![Scripted::json(200, &reply())]);
    let worker = Worker::start(
        queue.clone(),
        Box::new(ClaudeAt(server.url())),
        Box::new(|| {}),
    );
    let store = Store::new(queue);
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while store
        .get(&job.capture_id)
        .is_some_and(|job| job.state != State::Done)
    {
        assert!(
            std::time::Instant::now() < until,
            "{:?}",
            store.get(&job.capture_id)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    worker.stop();

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
