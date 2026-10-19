//! Before a note is written: each link's page, fetched once and kept beside the raw source, so
//! the note can be written from it and written again later without fetching it again.

use crate::capture::detect;
use crate::capture::sidecar::{self, PageFacts, Sidecar};
use crate::extract::page;
use crate::model::BackendError;
use crate::net::fetch::{self, Fetched};
use crate::net::http::HttpError;
use crate::notebook::frontmatter;
use crate::queue::Job;

/// Links fetched per capture; the rest are given to the model as addresses.
pub const MAX_LINKS: usize = 3;
/// Tries after which a page that can't be reached (offline, a mistyped host) is recorded as
/// failed, so the note is written without it rather than waiting for ever.
pub const FETCH_TRIES: u32 = 3;

/// Fetches the pages of `job`'s links that have no page kept yet, or whose last fetch failed,
/// with `fetched` (`YYYY-MM-DDTHH:MM`) as the time. A page that can't be reached yet makes the
/// job wait and try again, up to `FETCH_TRIES`.
pub fn fetch_pages(job: &Job, fetched: &str) -> Result<(), BackendError> {
    fetch_pages_with(job, fetched, &fetch::fetch)
}

fn fetch_pages_with(
    job: &Job,
    fetched: &str,
    fetch: &dyn Fn(&str) -> Result<Fetched, HttpError>,
) -> Result<(), BackendError> {
    // A raw source that can't be read fails the job when the request is built.
    let Ok(text) = std::fs::read_to_string(job.notebook.join(&job.raw)) else {
        return Ok(());
    };
    let body = &text[frontmatter::parse(&text).map_or(0, |front| front.body_start)..];
    let kept = sidecar::find(&job.notebook, &job.raw, &job.capture_id);
    for url in detect::urls(body).into_iter().take(MAX_LINKS) {
        let known = kept.iter().find(|(_, sidecar)| sidecar.url == url);
        if known.is_some_and(|(_, sidecar)| sidecar.page.is_ok()) {
            continue;
        }
        let page = match fetch(&url) {
            Err(HttpError::Transient(reason)) if job.attempts < FETCH_TRIES => {
                return Err(BackendError::Retry {
                    after: None,
                    reason: format!("The page at {url} couldn't be fetched yet: {reason}"),
                });
            }
            Err(HttpError::Transient(reason) | HttpError::Permanent(reason)) => {
                Err(format!("failed ({reason})"))
            }
            Ok(Fetched::Status(status)) => Err(format!("failed ({status})")),
            Ok(Fetched::NotPage { content_type }) => Err(format!("not a page ({content_type})")),
            Ok(Fetched::Page {
                url: page_url,
                html,
            }) => {
                let page = page::extract(&html, &page_url);
                if page.markdown.is_empty() {
                    Err("no main text".to_owned())
                } else {
                    Ok(PageFacts {
                        page_url: (page_url != url).then_some(page_url),
                        title: page.title,
                        site: page.site,
                        published: page.published,
                        markdown: page.markdown,
                    })
                }
            }
        };
        if let Err(reason) = &page {
            crate::log::line(&format!("pages: {reason}"));
        }
        let sidecar = Sidecar {
            capture_id: job.capture_id.clone(),
            url,
            fetched: fetched.to_owned(),
            page,
        };
        let replacing = known.map(|(path, _)| path.as_path());
        sidecar::write(&job.notebook, &job.raw, &sidecar, replacing)
            .map_err(|error| BackendError::Fail(format!("The page couldn't be saved: {error}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Capture;
    use crate::capture::submit::submit;
    use crate::queue::Store;
    use std::cell::RefCell;
    use std::path::PathBuf;

    struct Notebook(PathBuf);

    impl Notebook {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("fastcapture-pages-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("Notes")).unwrap();
            Self(dir)
        }

        fn capture(&self, text: &str) -> Job {
            submit(
                &self.0.join("Notes"),
                &Capture {
                    text: text.to_owned(),
                    ..Capture::default()
                },
                Some(&self.0.join("queue")),
                false,
            )
            .unwrap();
            Store::new(self.0.join("queue")).list().remove(0)
        }
    }

    impl Drop for Notebook {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ARTICLE: &str = "<html><head><title>Lag</title></head><body><article>\
        <p>Consumer lag grows when a group falls behind, which happens during large batches, \
        and it recovers once the batch is done, usually within half an hour of the start.</p>\
        <p>Capping the batch size keeps the lag down, at the cost of a longer run overall, \
        which is usually the better trade for payments.</p></article></body></html>";

    fn pages(job: &Job) -> Vec<Sidecar> {
        sidecar::find(&job.notebook, &job.raw, &job.capture_id)
            .into_iter()
            .map(|(_, sidecar)| sidecar)
            .collect()
    }

    #[test]
    fn each_link_is_fetched_once_and_kept() {
        let notebook = Notebook::new("kept");
        let job = notebook.capture("read later\nhttps://a.example/post\nhttps://b.example/x");
        let asked = RefCell::new(Vec::new());
        let fetch = |url: &str| {
            asked.borrow_mut().push(url.to_owned());
            Ok(match url {
                "https://a.example/post" => Fetched::Page {
                    url: "https://a.example/post/".to_owned(),
                    html: ARTICLE.to_owned(),
                },
                _ => Fetched::Status(403),
            })
        };
        fetch_pages_with(&job, "2026-10-19T10:31", &fetch).unwrap();
        let kept = pages(&job);
        assert_eq!(kept.len(), 2);
        let page = kept[0].page.as_ref().unwrap();
        assert_eq!(kept[0].url, "https://a.example/post");
        assert_eq!(page.page_url.as_deref(), Some("https://a.example/post/"));
        assert_eq!(page.title.as_deref(), Some("Lag"));
        assert!(page.markdown.starts_with("Consumer lag grows"));
        assert_eq!(kept[1].page, Err("failed (403)".to_owned()));

        // Again (as R does): the page is kept, the failure is tried again in its place.
        fetch_pages_with(&job, "2026-10-19T11:00", &fetch).unwrap();
        assert_eq!(
            *asked.borrow(),
            [
                "https://a.example/post",
                "https://b.example/x",
                "https://b.example/x"
            ]
        );
        let kept = pages(&job);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].fetched, "2026-10-19T11:00");
    }

    #[test]
    fn an_unreachable_page_waits_then_is_recorded() {
        let notebook = Notebook::new("offline");
        let mut job = notebook.capture("https://typo.example/");
        let offline = |_: &str| Err(HttpError::Transient("name not resolved".to_owned()));
        job.attempts = 1;
        assert!(matches!(
            fetch_pages_with(&job, "t", &offline),
            Err(BackendError::Retry { after: None, .. })
        ));
        assert!(pages(&job).is_empty());
        job.attempts = FETCH_TRIES;
        fetch_pages_with(&job, "t", &offline).unwrap();
        assert_eq!(
            pages(&job)[0].page,
            Err("failed (name not resolved)".to_owned())
        );
    }

    #[test]
    fn other_types_and_empty_pages_are_recorded() {
        let notebook = Notebook::new("types");
        let job = notebook.capture("https://a.example/f.zip\nhttps://a.example/portal");
        let fetch = |url: &str| {
            Ok(if url.ends_with(".zip") {
                Fetched::NotPage {
                    content_type: "application/zip".to_owned(),
                }
            } else {
                Fetched::Page {
                    url: url.to_owned(),
                    html: "<ul><li><a href=/a>A</a></li></ul>".to_owned(),
                }
            })
        };
        fetch_pages_with(&job, "t", &fetch).unwrap();
        let reasons: Vec<_> = pages(&job)
            .into_iter()
            .map(|sidecar| sidecar.page)
            .collect();
        assert_eq!(
            reasons,
            [
                Err("not a page (application/zip)".to_owned()),
                Err("no main text".to_owned())
            ]
        );
    }

    #[test]
    fn a_real_fetch_goes_through_the_test_server() {
        use crate::net::test_server::{Scripted, TestServer};
        let server = TestServer::start(vec![Scripted {
            status: 200,
            headers: vec![("content-type".to_owned(), "text/html".to_owned())],
            body: ARTICLE.as_bytes().to_vec(),
            delay: std::time::Duration::ZERO,
        }]);
        let notebook = Notebook::new("real");
        let job = notebook.capture(&format!("{}/lag", server.url()));
        fetch_pages(&job, "t").unwrap();
        assert!(pages(&job)[0].page.is_ok());
        assert_eq!(server.requests()[0].path, "/lag");
    }
}
