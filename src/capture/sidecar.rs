//! A link's page, kept beside its raw source: `raw/<stem>.page.md` (then `.page 2.md`, …, one
//! per link), with what the fetch found in front matter (`type: page`, so listings of raw
//! sources pass it by) and the page's main content as Markdown below. A page is written once;
//! only a sidecar recording a failed fetch is replaced, when the page is fetched again.

use super::raw::quoted;
use crate::notebook::frontmatter::{self, Value};
use crate::platform::saver;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sidecar {
    pub capture_id: String,
    /// The link as captured.
    pub url: String,
    /// `YYYY-MM-DDTHH:MM`.
    pub fetched: String,
    /// The page, or why there is none (`failed (403)`, `no main text`, …).
    pub page: Result<PageFacts, String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageFacts {
    /// Where the page came from, when redirects led elsewhere.
    pub page_url: Option<String>,
    pub title: Option<String>,
    pub site: Option<String>,
    pub published: Option<String>,
    pub markdown: String,
}

impl Sidecar {
    pub fn text(&self) -> String {
        let mut text = format!(
            "---\ntype: page\ncapture_id: {}\nurl: {}\nfetched: {}\n",
            self.capture_id,
            quoted(&self.url),
            self.fetched
        );
        let mut body = String::new();
        match &self.page {
            Ok(page) => {
                let fields = [
                    ("page_url", &page.page_url),
                    ("page_title", &page.title),
                    ("site", &page.site),
                    ("published", &page.published),
                ];
                for (key, value) in fields {
                    if let Some(value) = value {
                        text.push_str(&format!("{key}: {}\n", quoted(value)));
                    }
                }
                body = format!("\n{}\n", page.markdown.trim_end());
            }
            Err(reason) => text.push_str(&format!("fetch: {}\n", quoted(reason))),
        }
        text.push_str("---\n");
        text.push_str(&body);
        text
    }

    pub fn parse(text: &str) -> Option<Self> {
        let front = frontmatter::parse(text)?;
        // The reader takes the quotes off but leaves the escapes `quoted` put in.
        let get = |key: &str| match front.get(key) {
            Some(Value::Text(value)) if !value.is_empty() => {
                Some(value.replace("\\\"", "\"").replace("\\\\", "\\"))
            }
            _ => None,
        };
        if get("type")? != "page" {
            return None;
        }
        let page = match get("fetch") {
            Some(reason) => Err(reason),
            None => Ok(PageFacts {
                page_url: get("page_url"),
                title: get("page_title"),
                site: get("site"),
                published: get("published"),
                markdown: text[front.body_start..].trim().to_owned(),
            }),
        };
        Some(Self {
            capture_id: get("capture_id")?,
            url: get("url")?,
            fetched: get("fetched").unwrap_or_default(),
            page,
        })
    }
}

/// `raw/<stem>.page.md`, `raw/<stem>.page 2.md`, … for the raw source at `raw`.
fn name(raw: &Path, index: usize) -> PathBuf {
    let stem = raw
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = if index == 1 {
        format!("{stem}.page.md")
    } else {
        format!("{stem}.page {index}.md")
    };
    raw.with_file_name(name)
}

/// The sidecars of the raw source at `raw` (relative to `notebook`) for `capture_id`, with their
/// paths relative to the notebook, in the order they were written.
pub fn find(notebook: &Path, raw: &Path, capture_id: &str) -> Vec<(PathBuf, Sidecar)> {
    (1..)
        .map(|index| name(raw, index))
        .take_while(|path| notebook.join(path).exists())
        .filter_map(|path| {
            let text = std::fs::read_to_string(notebook.join(&path)).ok()?;
            let sidecar = Sidecar::parse(&text)?;
            (sidecar.capture_id == capture_id).then_some((path, sidecar))
        })
        .collect()
}

/// Writes `sidecar` for the raw source at `raw`: over `replacing` (a sidecar that recorded a
/// failure) when given, else under the next free name. Returns its path, relative to the
/// notebook.
pub fn write(
    notebook: &Path,
    raw: &Path,
    sidecar: &Sidecar,
    replacing: Option<&Path>,
) -> Result<PathBuf, String> {
    let text = sidecar.text();
    if let Some(path) = replacing {
        saver::save_atomic(&notebook.join(path), text.as_bytes())
            .map_err(|error| error.to_string())?;
        return Ok(path.to_path_buf());
    }
    for index in 1.. {
        let path = name(raw, index);
        match saver::save_atomic_new(&notebook.join(&path), text.as_bytes()) {
            Ok(()) => return Ok(path),
            Err(error) if saver::is_already_exists(&error) => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!("an unbounded search for a free name ends in a return")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::store::raw_sources;

    fn page() -> Sidecar {
        Sidecar {
            capture_id: "01ABC".to_owned(),
            url: "https://example.com/a?b=\"c\"".to_owned(),
            fetched: "2026-10-19T10:31".to_owned(),
            page: Ok(PageFacts {
                page_url: Some("https://example.com/a/".to_owned()),
                title: Some("Lag: a story".to_owned()),
                site: Some("Example".to_owned()),
                published: None,
                markdown: "# Lag\n\nBody text.".to_owned(),
            }),
        }
    }

    #[test]
    fn a_sidecar_reads_back_as_it_was_written() {
        let sidecar = page();
        assert_eq!(
            sidecar.text(),
            "---\ntype: page\ncapture_id: 01ABC\nurl: \"https://example.com/a?b=\\\"c\\\"\"\n\
             fetched: 2026-10-19T10:31\npage_url: \"https://example.com/a/\"\n\
             page_title: \"Lag: a story\"\nsite: \"Example\"\n---\n\n# Lag\n\nBody text.\n"
        );
        assert_eq!(Sidecar::parse(&sidecar.text()), Some(sidecar));
        let failed = Sidecar {
            page: Err("failed (403)".to_owned()),
            ..page()
        };
        assert!(failed.text().contains("fetch: \"failed (403)\"\n---\n"));
        assert_eq!(Sidecar::parse(&failed.text()), Some(failed));
    }

    #[test]
    fn sidecars_are_named_after_their_raw_source_and_listings_pass_them_by() {
        let notebook =
            std::env::temp_dir().join(format!("fastcapture-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&notebook);
        std::fs::create_dir_all(notebook.join("raw")).unwrap();
        let raw = Path::new(r"raw\2026-10-19 1030 lag.md");
        std::fs::write(
            notebook.join(raw),
            "---\ntype: raw\ncapture_id: 01ABC\ncaptured: 2026-10-19T10:30\n---\n\nlag\n",
        )
        .unwrap();

        let failed = Sidecar {
            page: Err("failed (503)".to_owned()),
            ..page()
        };
        let first = write(&notebook, raw, &failed, None).unwrap();
        assert_eq!(first, Path::new(r"raw\2026-10-19 1030 lag.page.md"));
        let second = write(&notebook, raw, &page(), None).unwrap();
        assert_eq!(second, Path::new(r"raw\2026-10-19 1030 lag.page 2.md"));
        // Fetched again: the failure is replaced, in its place.
        assert_eq!(write(&notebook, raw, &page(), Some(&first)).unwrap(), first);

        let found = find(&notebook, raw, "01ABC");
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|(_, sidecar)| sidecar.page.is_ok()));
        assert!(find(&notebook, raw, "OTHER").is_empty());
        let names: Vec<PathBuf> = found.into_iter().map(|(path, _)| path).collect();
        assert_eq!(names, [first, second]);
        // Only the raw source is a raw source.
        let raws = raw_sources(&notebook, Path::new("raw"));
        assert_eq!(raws.len(), 1);
        assert_eq!(raws[0].path, raw);
        let _ = std::fs::remove_dir_all(&notebook);
    }
}
