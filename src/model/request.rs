//! A request for one note, built from a queued capture: the rules, the user turn, the images,
//! and the facts about the capture the note needs. The same request goes to any backend.

use super::images::{self, MAX_IMAGES, ModelImage};
use super::prompt::{self, LinkSource, Turn};
use crate::capture::{detect, raw, read};
use crate::notebook::frontmatter::{self, Value};
use crate::queue::Job;
use std::path::{Path, PathBuf};

/// What the written note needs to know about its capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFacts {
    pub notebook: PathBuf,
    pub capture_id: String,
    /// `YYYY-MM-DDTHH:MM`.
    pub captured: String,
    /// The raw source, relative to the notebook.
    pub raw: PathBuf,
    pub kinds: Vec<String>,
    /// The raw source's attachments, relative to the notebook.
    pub attachments: Vec<PathBuf>,
    /// The title to use when the model gives none.
    pub fallback_title: String,
}

#[derive(Clone, Debug)]
pub struct NoteRequest {
    pub system: &'static str,
    pub user_text: String,
    pub images: Vec<ModelImage>,
    pub capture: CaptureFacts,
}

impl NoteRequest {
    /// The same request with a note about what was wrong with the last reply.
    pub fn corrected(&self, problems: &str) -> Self {
        let mut request = self.clone();
        request.user_text.push_str(&prompt::correction(problems));
        request
    }
}

/// The request for `job`, on `today` (`YYYY-MM-DD`).
pub fn build(job: &Job, today: &str) -> Result<NoteRequest, String> {
    let raw_path = job.notebook.join(&job.raw);
    let text = std::fs::read_to_string(&raw_path)
        .map_err(|error| format!("FastCapture could not read {}: {error}", raw_path.display()))?;
    let info = read::read(&job.raw, &text);
    let front = frontmatter::parse(&text);
    let source_window = match front.as_ref().and_then(|front| front.get("source_window")) {
        Some(Value::Text(window)) if !window.is_empty() => Some(window.clone()),
        _ => None,
    };
    let body = &text[front.as_ref().map_or(0, |front| front.body_start)..];
    // What you typed: the body without the attachment link lines.
    let typed: String = body
        .lines()
        .filter(|line| read::link_of(line.trim()).is_none())
        .collect::<Vec<_>>()
        .join("\n");
    let your_note = detect::prose(&typed);
    let urls = detect::urls(&typed);

    let mut prepared = Vec::new();
    let mut files = Vec::new();
    let mut left_out = Vec::new();
    for attachment in &info.attachments {
        let name = attachment
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !detect::is_image_path(attachment) {
            files.push(name);
            continue;
        }
        if prepared.len() >= MAX_IMAGES {
            left_out.push(name);
            continue;
        }
        match std::fs::read(job.notebook.join(attachment))
            .map_err(|error| error.to_string())
            .and_then(|bytes| images::prepare(&bytes))
        {
            Ok(image) => prepared.push(image),
            Err(error) => {
                crate::log::line(&format!("request: {}: {error}", attachment.display()));
                left_out.push(format!("{name} (couldn't be read)"));
            }
        }
    }

    let links: Vec<LinkSource<'_>> = urls
        .iter()
        .map(|url| LinkSource { url, text: None })
        .collect();
    let notebook_name = job
        .notebook
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| job.notebook.display().to_string());
    let user_text = prompt::user_turn(&Turn {
        today,
        notebook: &notebook_name,
        your_note: &your_note,
        links: &links,
        files: &files,
        images: prepared.len(),
        left_out: &left_out,
        source_window: source_window.as_deref(),
    });
    let fallback_title = raw::first_words(&your_note)
        .or_else(|| {
            urls.first()
                .and_then(|url| detect::host(url))
                .map(str::to_owned)
        })
        .or_else(|| files.first().cloned())
        .unwrap_or_else(|| stem_of(&job.raw));
    Ok(NoteRequest {
        system: prompt::RULES,
        user_text,
        images: prepared,
        capture: CaptureFacts {
            notebook: job.notebook.clone(),
            capture_id: job.capture_id.clone(),
            captured: info.captured.clone(),
            raw: job.raw.clone(),
            kinds: info.kinds.clone(),
            attachments: info.attachments.clone(),
            fallback_title,
        },
    })
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::submit::submit;
    use crate::capture::{Attachment, Capture};
    use crate::platform::png::{Pixels, encode_png};
    use crate::queue::Store;
    use std::fs;

    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp(name: &str) -> Temp {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-request-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Work notes")).unwrap();
        Temp(dir)
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        encode_png(&Pixels {
            width,
            height,
            bgra: vec![200; (width * height * 4) as usize],
        })
        .unwrap()
    }

    #[test]
    fn a_capture_becomes_a_request_with_its_images_and_files() {
        let temp = temp("full");
        let notebook = temp.0.join("Work notes");
        let log = temp.0.join("ledger.log");
        fs::write(&log, b"x").unwrap();
        submit(
            &notebook,
            &Capture {
                text: "lag spikes during ledger batch??\nhttps://grafana.example/d/x".to_owned(),
                attachments: vec![Attachment::Png(png(3000, 1000)), Attachment::File(log)],
                source_window: Some("Grafana".to_owned()),
            },
            Some(&temp.0.join("queue")),
            false,
        )
        .unwrap();
        let job = Store::new(temp.0.join("queue")).list().remove(0);
        let request = build(&job, "2026-10-19").unwrap();

        assert_eq!(request.system, prompt::RULES);
        assert_eq!(request.images.len(), 1);
        assert_eq!(
            (request.images[0].width, request.images[0].height),
            (1568, 523)
        );
        let text = &request.user_text;
        assert!(text.starts_with("Today is 2026-10-19. Notebook: Work notes.\nCaptured from the window \"Grafana\".\n"), "{text}");
        assert!(text.contains("<your_note>\nlag spikes during ledger batch??\n</your_note>"));
        assert!(text.contains(
            "<source kind=\"link\" url=\"https://grafana.example/d/x\" fetched=\"no\"/>"
        ));
        assert!(text.contains("<attachment name=\"ledger.log\" read=\"no\"/>"));
        assert!(text.contains("1 image is attached above"));
        assert!(
            !text.contains("](../attachments"),
            "attachment links aren't your text"
        );

        let facts = &request.capture;
        assert_eq!(facts.capture_id, job.capture_id);
        assert_eq!(facts.kinds, ["text", "link", "screenshot", "file"]);
        assert_eq!(facts.attachments.len(), 2);
        assert_eq!(facts.fallback_title, "lag spikes during ledger batch");
        assert!(
            request
                .corrected("\"title\" is missing")
                .user_text
                .ends_with("Answer again with the JSON object only.")
        );
    }

    #[test]
    fn images_past_the_limit_or_unreadable_are_listed_as_left_out() {
        let temp = temp("left-out");
        let notebook = temp.0.join("Work notes");
        let broken = temp.0.join("broken.png");
        fs::write(&broken, b"not a png").unwrap();
        let mut attachments: Vec<Attachment> = (0..MAX_IMAGES)
            .map(|_| Attachment::Png(png(4, 4)))
            .collect();
        attachments.insert(0, Attachment::File(broken));
        attachments.push(Attachment::Png(png(4, 4)));
        submit(
            &notebook,
            &Capture {
                attachments,
                ..Capture::default()
            },
            Some(&temp.0.join("queue")),
            false,
        )
        .unwrap();
        let job = Store::new(temp.0.join("queue")).list().remove(0);
        let request = build(&job, "2026-10-19").unwrap();
        assert_eq!(request.images.len(), MAX_IMAGES);
        assert!(
            request
                .user_text
                .contains("Left out of this request: broken.png (couldn't be read), "),
            "{}",
            request.user_text
        );
        assert!(request.user_text.contains("<your_note>\n\n</your_note>"));
        assert_eq!(
            request.capture.fallback_title,
            stem_of(&job.raw),
            "no words, no link, no file"
        );
    }
}
