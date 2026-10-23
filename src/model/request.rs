//! A request for one note, built from a queued capture: the rules, the user turn, the images,
//! and the facts about the capture the note needs. The same request goes to any backend.
//!
//! The capture's material goes in its order, sharing one budget: each link's kept page, then
//! each file attachment read by its type (PDF, Word, text), then the images (attachments, then
//! scanned PDF pages).

use super::images::{self, MAX_EDGE, MAX_IMAGES, ModelImage};
use super::prompt::{self, Candidate, FileRead, FileSource, LinkPage, LinkSource, Turn};
use crate::capture::{detect, raw, read, sidecar};
use crate::extract::{self, Budget};
use crate::file::home;
use crate::index::search::{CANDIDATES, TOP_TAGS};
use crate::index::service::Snapshot;
use crate::note::render::SourceLink;
use crate::notebook::Conventions;
use crate::notebook::frontmatter::{self, Value};
use crate::notebook::links::{Resolver, wiki_target};
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
    /// The capture's links, with their pages' titles and sites when they were read.
    pub links: Vec<SourceLink>,
    /// Sources cut to fit the request, by name.
    pub truncated: Vec<String>,
    /// The candidate notes offered, by name, the homes among them, and the folders offered:
    /// what the reply may link to and suggest.
    pub candidates: Vec<String>,
    pub home_notes: Vec<String>,
    pub home_folders: Vec<String>,
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

/// The request for `job`, on `today` (`YYYY-MM-DD`), without candidate notes.
pub fn build(job: &Job, today: &str) -> Result<NoteRequest, String> {
    build_with(job, today, None)
}

/// The request for `job`, on `today` (`YYYY-MM-DD`), with candidates from `index`, the
/// notebook's index, when there is one.
pub fn build_with(job: &Job, today: &str, index: Option<&Snapshot>) -> Result<NoteRequest, String> {
    let raw_path = job.notebook.join(&job.raw);
    let text = std::fs::read_to_string(&raw_path)
        .map_err(|error| format!("FastCapture could not read {}: {error}", raw_path.display()))?;
    let info = read::read(&job.raw, &text);
    let front = frontmatter::parse(&text);
    let text_of = |key: &str| match front.as_ref().and_then(|front| front.get(key)) {
        Some(Value::Text(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    };
    let source_window = text_of("source_window");
    let source_url = text_of("source_url");
    let body = &text[front.as_ref().map_or(0, |front| front.body_start)..];
    // What you typed: the body without the attachment link lines.
    let typed: String = body
        .lines()
        .filter(|line| read::link_of(line.trim()).is_none())
        .collect::<Vec<_>>()
        .join("\n");
    let your_note = detect::prose(&typed);
    let urls = detect::urls(&typed);

    let mut budget = Budget::default();

    // Links: the page kept beside the raw source, when it was fetched.
    let kept = sidecar::find(&job.notebook, &job.raw, &job.capture_id);
    let mut pages = Vec::new();
    let mut source_links = Vec::new();
    for url in &urls {
        let page = kept
            .iter()
            .map(|(_, sidecar)| sidecar)
            .rfind(|sidecar| &sidecar.url == url)
            .map(|sidecar| &sidecar.page);
        let mut link = SourceLink {
            url: url.clone(),
            ..SourceLink::default()
        };
        let material = match page {
            Some(Ok(page)) => {
                link.title = page.title.clone();
                link.site = page.site.clone();
                match budget.take(url, page.markdown.clone()) {
                    Some(text) => PageMaterial::Read {
                        title: page.title.clone(),
                        site: page.site.clone(),
                        published: page.published.clone(),
                        text,
                    },
                    None => PageMaterial::NotRead(Some("cut to fit".to_owned())),
                }
            }
            Some(Err(reason)) => PageMaterial::NotRead(Some(reason.clone())),
            None => PageMaterial::NotRead(None),
        };
        source_links.push(link);
        pages.push(material);
    }

    // Files, by type; images wait until after them.
    let mut files = Vec::new();
    let mut image_attachments = Vec::new();
    let mut scanned = Vec::new();
    for attachment in &info.attachments {
        let name = attachment
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if detect::is_image_path(attachment) {
            image_attachments.push((name, attachment));
            continue;
        }
        let material = match std::fs::read(job.notebook.join(attachment)) {
            Ok(bytes) => read_file(&name, &bytes, &mut budget, &mut scanned),
            Err(error) => {
                crate::log::line(&format!("request: {}: {error}", attachment.display()));
                FileMaterial::Unreadable("it couldn't be opened".to_owned())
            }
        };
        files.push((name, material));
    }

    // Images: the attached ones, then scanned PDF pages, while there is room.
    let mut prepared = Vec::new();
    let mut left_out = Vec::new();
    let images = image_attachments
        .into_iter()
        .map(|(name, attachment)| {
            let bytes =
                std::fs::read(job.notebook.join(attachment)).map_err(|error| error.to_string());
            (name, bytes)
        })
        .chain(scanned.into_iter().map(|(name, png)| (name, Ok(png))));
    for (name, bytes) in images {
        if prepared.len() >= MAX_IMAGES {
            left_out.push(name);
            continue;
        }
        match bytes.and_then(|bytes| images::prepare(&bytes)) {
            Ok(image) => prepared.push(image),
            Err(error) => {
                crate::log::line(&format!("request: {name}: {error}"));
                left_out.push(format!("{name} (couldn't be read)"));
            }
        }
    }

    let links: Vec<LinkSource<'_>> = urls
        .iter()
        .zip(&pages)
        .map(|(url, page)| LinkSource {
            url,
            page: match page {
                PageMaterial::Read {
                    title,
                    site,
                    published,
                    text,
                } => LinkPage::Read {
                    title: title.as_deref(),
                    site: site.as_deref(),
                    published: published.as_deref(),
                    text,
                },
                PageMaterial::NotRead(reason) => LinkPage::NotRead(reason.as_deref()),
            },
        })
        .collect();
    let file_sources: Vec<FileSource<'_>> = files
        .iter()
        .map(|(name, material)| FileSource {
            name,
            read: match material {
                FileMaterial::Read { kind, pages, text } => FileRead::Read {
                    kind,
                    pages: *pages,
                    text,
                },
                FileMaterial::Other { kind, size } => FileRead::Other { kind, size },
                FileMaterial::Unreadable(reason) => FileRead::Unreadable(reason),
            },
        })
        .collect();
    // What the capture is about, to find the notes it relates to: your text, the pages'
    // titles, and the start of the material.
    let mut query = your_note.clone();
    for link in &source_links {
        if let Some(title) = &link.title {
            query.push('\n');
            query.push_str(title);
        }
    }
    let material: String = pages
        .iter()
        .filter_map(|page| match page {
            PageMaterial::Read { text, .. } => Some(text.as_str()),
            PageMaterial::NotRead(_) => None,
        })
        .chain(files.iter().filter_map(|(_, file)| match file {
            FileMaterial::Read { text, .. } => Some(text.as_str()),
            _ => None,
        }))
        .flat_map(|text| text.chars().chain(std::iter::once('\n')))
        .take(QUERY_MATERIAL_CHARS)
        .collect();
    query.push('\n');
    query.push_str(&material);
    let conventions = Conventions::load(&job.notebook);
    let own_note = job.note.as_ref().map(|note| note.path.as_path());
    let related = index
        .map(|index| related(index, &query, &conventions, own_note))
        .unwrap_or_default();

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
        files: &file_sources,
        images: prepared.len(),
        left_out: &left_out,
        cut: budget.truncated(),
        source_window: source_window.as_deref(),
        source_url: source_url.as_deref(),
        candidates: &related.candidates,
        home_folders: &related.home_folders,
        notebook_tags: &related.tags,
    });
    let fallback_title = raw::first_words(&your_note)
        .or_else(|| source_links.first().and_then(|link| link.title.clone()))
        .or_else(|| {
            urls.first()
                .and_then(|url| detect::host(url))
                .map(str::to_owned)
        })
        .or_else(|| files.first().map(|(name, _)| name.clone()))
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
            links: source_links,
            truncated: budget.truncated().to_vec(),
            candidates: related
                .candidates
                .iter()
                .map(|candidate| candidate.name.clone())
                .collect(),
            home_notes: related
                .candidates
                .iter()
                .filter(|candidate| candidate.home)
                .map(|candidate| candidate.name.clone())
                .collect(),
            home_folders: related.home_folders,
        },
    })
}

/// How much of the material goes in the query, in characters.
const QUERY_MATERIAL_CHARS: usize = 2_000;

/// The notes a capture relates to, the folders it could be filed in, and the notebook's tags.
#[derive(Default)]
struct Related {
    candidates: Vec<Candidate>,
    home_folders: Vec<String>,
    tags: Vec<String>,
}

/// The best matches for `query` in `index`, never `own_note` (the capture's note, when it is
/// written again), named as links from the capture folder.
fn related(
    index: &Snapshot,
    query: &str,
    conventions: &Conventions,
    own_note: Option<&Path>,
) -> Related {
    let notes = index.searcher.index();
    let resolver = Resolver::new(
        notes.notes.values().map(|note| note.path.as_path()),
        notes.notes.values().flat_map(|note| {
            note.aliases
                .iter()
                .map(|alias| (alias.clone(), note.path.clone()))
        }),
    );
    let from = conventions.capture_folder().join("note.md");
    let own = own_note.map(|path| path.to_string_lossy().to_lowercase());
    let hits: Vec<_> = index
        .searcher
        .search(query, CANDIDATES + 1)
        .into_iter()
        .filter(|hit| own.as_deref() != Some(&*hit.note.path.to_string_lossy().to_lowercase()))
        .take(CANDIDATES)
        .collect();
    let candidates: Vec<Candidate> = hits
        .iter()
        .map(|hit| {
            let note = hit.note;
            Candidate {
                name: wiki_target(&resolver, &note.path, &from),
                path: note.path.clone(),
                aliases: note.aliases.clone(),
                tags: note.tags.clone(),
                kind: note.kind.clone(),
                home: home::is_home_type(note.kind.as_deref(), &conventions.capture.home_types),
                snippet: note.snippet.clone(),
            }
        })
        .collect();
    let home_folders = home::home_folders(
        candidates.iter().map(|candidate| candidate.path.as_path()),
        conventions.capture_folder(),
    );
    let tags = index
        .searcher
        .top_tags(TOP_TAGS)
        .into_iter()
        .map(|tag| tag.name)
        .collect();
    Related {
        candidates,
        home_folders,
        tags,
    }
}

/// A link's page, owned until the turn borrows it.
enum PageMaterial {
    Read {
        title: Option<String>,
        site: Option<String>,
        published: Option<String>,
        text: String,
    },
    NotRead(Option<String>),
}

/// A file attachment's material, owned until the turn borrows it.
enum FileMaterial {
    Read {
        kind: &'static str,
        pages: Option<usize>,
        text: String,
    },
    Other {
        kind: String,
        size: String,
    },
    Unreadable(String),
}

/// A file attachment read by its type within `budget`. A PDF's scanned pages are rendered and
/// added to `scanned`, named for the request.
fn read_file(
    name: &str,
    bytes: &[u8],
    budget: &mut Budget,
    scanned: &mut Vec<(String, Vec<u8>)>,
) -> FileMaterial {
    let extension = Path::new(name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let (kind, pages, text) = match extension.as_str() {
        "pdf" => {
            let pdf = match extract::pdf::read(bytes, budget.pages_left()) {
                Ok(pdf) => pdf,
                Err(reason) => return FileMaterial::Unreadable(reason),
            };
            budget.take_pages(name, pdf.page_count);
            let wanted = pdf.scanned();
            if !wanted.is_empty() {
                match extract::pdf::render(bytes, &wanted, MAX_EDGE) {
                    Ok(pngs) => scanned.extend(
                        wanted
                            .iter()
                            .zip(pngs)
                            .map(|(index, png)| (format!("{name} page {}", index + 1), png)),
                    ),
                    Err(error) => crate::log::line(&format!("request: {name}: {error}")),
                }
            }
            ("pdf", Some(pdf.page_count), pdf.text())
        }
        "docx" => match extract::docx::read(bytes) {
            Ok(text) => ("docx", None, text),
            Err(reason) => return FileMaterial::Unreadable(reason),
        },
        _ => match extract::text::extract(name, bytes) {
            Some(read) => {
                if read.cut {
                    budget.cut(name);
                }
                ("text", None, read.text)
            }
            None => {
                return FileMaterial::Other {
                    kind: if extension.is_empty() {
                        "file".to_owned()
                    } else {
                        extension
                    },
                    size: size_text(bytes.len()),
                };
            }
        },
    };
    match budget.take(name, text) {
        Some(text) => FileMaterial::Read { kind, pages, text },
        None => FileMaterial::Unreadable("cut to fit".to_owned()),
    }
}

/// `512 bytes`, `34 KB`, `2.1 MB`.
fn size_text(bytes: usize) -> String {
    const KB: usize = 1024;
    if bytes < KB {
        format!("{bytes} bytes")
    } else if bytes < KB * KB {
        format!("{} KB", bytes.div_ceil(KB))
    } else {
        format!("{:.1} MB", bytes as f64 / (KB * KB) as f64)
    }
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
                source_url: None,
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
        assert!(text.contains("<attachment name=\"ledger.log\" kind=\"text\">\nx\n</attachment>"));
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

    fn capture(temp: &Temp, capture: Capture) -> Job {
        submit(
            &temp.0.join("Work notes"),
            &capture,
            Some(&temp.0.join("queue")),
            false,
        )
        .unwrap();
        Store::new(temp.0.join("queue")).list().remove(0)
    }

    #[test]
    fn kept_pages_go_in_their_source_blocks() {
        let temp = temp("pages");
        let job = capture(
            &temp,
            Capture {
                text: "https://a.example/post\nhttps://b.example/x\nhttps://c.example/".to_owned(),
                source_url: Some("https://copied.example/".to_owned()),
                ..Capture::default()
            },
        );
        let page = |url: &str, page| sidecar::Sidecar {
            capture_id: job.capture_id.clone(),
            url: url.to_owned(),
            fetched: "2026-10-19T10:31".to_owned(),
            page,
        };
        let read = page(
            "https://a.example/post",
            Ok(sidecar::PageFacts {
                title: Some("Lag, explained".to_owned()),
                site: Some("A".to_owned()),
                published: Some("2026-10-18".to_owned()),
                markdown: "# Lag\n\nIt grows.".to_owned(),
                ..sidecar::PageFacts::default()
            }),
        );
        sidecar::write(&job.notebook, &job.raw, &read, None).unwrap();
        let failed = page("https://b.example/x", Err("failed (403)".to_owned()));
        sidecar::write(&job.notebook, &job.raw, &failed, None).unwrap();

        let request = build(&job, "2026-10-19").unwrap();
        let text = &request.user_text;
        assert!(
            text.contains("The user's text was copied from the page https://copied.example/.\n")
        );
        assert!(text.contains(
            "<source kind=\"link\" url=\"https://a.example/post\" title=\"Lag, explained\" \
             site=\"A\" published=\"2026-10-18\">\n# Lag\n\nIt grows.\n</source>"
        ));
        assert!(text.contains(
            "<source kind=\"link\" url=\"https://b.example/x\" fetched=\"no\" reason=\"failed (403)\"/>"
        ));
        assert!(text.contains("<source kind=\"link\" url=\"https://c.example/\" fetched=\"no\"/>"));
        let links = &request.capture.links;
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].title.as_deref(), Some("Lag, explained"));
        assert_eq!(links[1].title, None);
        assert_eq!(request.capture.fallback_title, "Lag, explained");
        assert!(request.capture.truncated.is_empty());
    }

    #[test]
    fn files_are_read_by_type_within_the_budget() {
        use crate::extract::pdf::build::{Page, pdf};
        let temp = temp("files");
        let write = |name: &str, bytes: &[u8]| {
            let path = temp.0.join(name);
            fs::write(&path, bytes).unwrap();
            Attachment::File(path)
        };
        let report = pdf(
            &[
                Page::Text(&["Ledger batch review: lag peaked at forty thousand messages."]),
                Page::Picture,
            ],
            false,
        );
        let docx = crate::extract::docx::build::docx(
            "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Plan</w:t></w:r></w:p>",
        );
        let long: String = (1..=1000).map(|n| format!("line {n}\n")).collect();
        let huge = "word ".repeat(13_000);
        let job = capture(
            &temp,
            Capture {
                text: "files".to_owned(),
                attachments: vec![
                    write("review.pdf", &report),
                    write("plan.docx", &docx),
                    write("server.log", long.as_bytes()),
                    write("data.bin", &[0, 1, 2, 3, 0, 0, 7]),
                    write("locked.docx", b"not a zip"),
                    write("huge.txt", huge.as_bytes()),
                    Attachment::Png(png(4, 4)),
                ],
                ..Capture::default()
            },
        );
        let request = build(&job, "2026-10-19").unwrap();
        let text = &request.user_text;
        assert!(
            text.contains("<attachment name=\"review.pdf\" kind=\"pdf\" pages=\"2\">\n## Page 1\n\nLedger batch review"),
            "{text}"
        );
        assert!(
            text.contains("## Page 2\n\n(A scanned page, attached as an image.)\n</attachment>")
        );
        assert!(
            text.contains("<attachment name=\"plan.docx\" kind=\"docx\">\n# Plan\n</attachment>")
        );
        assert!(text.contains("… 600 lines cut …"));
        assert!(
            text.contains(
                "<attachment name=\"data.bin\" kind=\"bin\" size=\"7 bytes\" read=\"no\"/>"
            )
        );
        assert!(text.contains(
            "<attachment name=\"locked.docx\" read=\"no\" reason=\"it isn't a Word document\"/>"
        ));
        assert!(text.contains("<attachment name=\"huge.txt\" kind=\"text\">\nword word"));
        assert!(text.contains("… cut to fit …\n</attachment>"));
        assert!(text.contains("Cut to fit this request: server.log, huge.txt."));
        // The pasted image, then the scanned page.
        assert_eq!(request.images.len(), 2);
        assert_eq!((request.images[0].width, request.images[0].height), (4, 4));
        assert_eq!(request.images[1].height, MAX_EDGE);
        assert!(text.contains("2 images are attached above"));
        assert_eq!(request.capture.truncated, ["server.log", "huge.txt"]);
    }

    /// The index of `notebook` as the service would build it.
    fn snapshot(notebook: &Path) -> Snapshot {
        use crate::index::notes::NoteIndex;
        use crate::index::search::Searcher;
        let raw = Conventions::load(notebook).raw_folder().to_path_buf();
        let scan = crate::index::scan::scan(notebook, &[raw], 10_000).unwrap();
        let (index, _) = NoteIndex::build(notebook, &scan.entries, NoteIndex::default());
        Snapshot {
            searcher: Searcher::new(index),
            folders: scan.folders,
            truncated: false,
        }
    }

    #[test]
    fn the_notes_a_capture_relates_to_become_candidates() {
        let temp = temp("candidates");
        let notebook = temp.0.join("Work notes");
        let note = |path: &str, text: &str| {
            let path = notebook.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        note(
            r"Payments\Payments incidents.md",
            "---\ntype: topic\ntags: [incidents]\n---\n# Payments incidents\n\nLedger batch outages.\n",
        );
        note(
            r"Payments\Lag.md",
            "# Lag\n\nLedger lag during the nightly batch. #kafka\n",
        );
        note(r"Search\Lag.md", "# Lag\n\nSearch indexing lag.\n");
        note(r"Recipes\Soup.md", "# Soup\n\nLeeks and potatoes.\n");
        note(
            r"inbox\Earlier capture.md",
            "# Earlier capture\n\nlag spikes during ledger batch\n",
        );
        let mut job = capture(
            &temp,
            Capture {
                text: "lag spikes during ledger batch".to_owned(),
                ..Capture::default()
            },
        );
        // Written before and now written again: its own note isn't a candidate.
        job.note = Some(crate::note::write::Written {
            path: PathBuf::from(r"inbox\Earlier capture.md"),
            stamp: crate::note::write::Stamp {
                len: 0,
                modified_ms: 0,
            },
        });
        let request = build_with(&job, "2026-10-19", Some(&snapshot(&notebook))).unwrap();
        let facts = &request.capture;
        let mut names = facts.candidates.clone();
        names.sort();
        assert_eq!(names, ["Payments incidents", "Payments/Lag", "Search/Lag"]);
        assert_eq!(facts.home_notes, ["Payments incidents"]);
        let mut folders = facts.home_folders.clone();
        folders.sort();
        assert_eq!(folders, ["Payments/", "Search/"]);
        let text = &request.user_text;
        assert!(text.contains("<note name=\"Payments/Lag\" folder=\"Payments\" tags=\"kafka\">\nLedger lag during the nightly batch. #kafka\n</note>"), "{text}");
        assert!(text.contains(
            "<note name=\"Payments incidents\" folder=\"Payments\" type=\"topic\" home=\"yes\" tags=\"incidents\">"
        ));
        assert!(!text.contains("Earlier capture") && !text.contains("Soup"));
        assert!(text.contains("Tags the notebook uses most: incidents, kafka."));

        // Without the index, nothing is offered.
        let plain = build(&job, "2026-10-19").unwrap();
        assert!(plain.capture.candidates.is_empty());
        assert!(plain.user_text.contains("No candidate notes are given."));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_text(512), "512 bytes");
        assert_eq!(size_text(34 * 1024 + 1), "35 KB");
        assert_eq!(size_text(2_200_000), "2.1 MB");
    }
}
