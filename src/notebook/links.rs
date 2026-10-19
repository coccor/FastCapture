//! Copied from FastPad `src/library/links.rs` (`Resolver` for wikilinks, `wiki_target` and
//! `relative_target`).
//!
//! Links between files in a notebook: what a wikilink reaches, the shortest wikilink that reaches
//! a note, and Markdown link destinations.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Finds the notes wikilinks point at, from the notebook's notes and their aliases.
#[derive(Debug, Default)]
pub struct Resolver {
    by_path: HashMap<String, PathBuf>,
    by_stem: HashMap<String, Vec<PathBuf>>,
    by_name: HashMap<String, Vec<PathBuf>>,
    by_alias: HashMap<String, Vec<PathBuf>>,
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// `Path::file_stem`, lowercased.
fn stem_key(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_string_lossy().to_lowercase())
}

fn is_note(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

impl Resolver {
    /// `notes` and alias owners are relative to the notebook.
    pub fn new<'a>(
        notes: impl IntoIterator<Item = &'a Path>,
        aliases: impl IntoIterator<Item = (String, PathBuf)>,
    ) -> Self {
        let mut resolver = Self::default();
        for note in notes {
            resolver.by_path.insert(path_key(note), note.to_path_buf());
            if let Some(stem) = stem_key(note) {
                resolver
                    .by_stem
                    .entry(stem)
                    .or_default()
                    .push(note.to_path_buf());
            }
            if let Some(name) = note.file_name() {
                resolver
                    .by_name
                    .entry(name.to_string_lossy().to_lowercase())
                    .or_default()
                    .push(note.to_path_buf());
            }
        }
        for (alias, owner) in aliases {
            resolver
                .by_alias
                .entry(alias.to_lowercase())
                .or_default()
                .push(owner);
        }
        resolver
    }

    /// The note `[[target]]`, written in the note at `from`, points at.
    pub fn resolve_wiki(&self, target: &str, from: &Path) -> Option<PathBuf> {
        let target = target.trim().replace('/', "\\");
        if target.is_empty() {
            return None;
        }
        if target.contains('\\') {
            let folder = from.parent().unwrap_or(Path::new(""));
            for base in [Path::new(""), folder] {
                let Some(joined) = normalize(&base.join(&target)) else {
                    continue;
                };
                for candidate in with_note_extensions(&joined) {
                    if let Some(found) = self.by_path.get(&path_key(&candidate)) {
                        return Some(found.clone());
                    }
                }
            }
            return None;
        }
        let key = target.to_lowercase();
        let candidates = if is_note(Path::new(&target)) {
            self.by_name.get(&key)
        } else {
            self.by_stem.get(&key)
        };
        candidates
            .or_else(|| self.by_alias.get(&key))
            .and_then(|candidates| best(candidates, from))
    }

    fn stem_count(&self, stem: &str) -> usize {
        self.by_stem.get(&stem.to_lowercase()).map_or(0, Vec::len)
            + self.by_alias.get(&stem.to_lowercase()).map_or(0, Vec::len)
    }
}

/// `path`, then with `.md` and `.markdown` when it has no extension.
fn with_note_extensions(path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![path.to_path_buf()];
    if path.extension().is_none() {
        candidates.push(path.with_extension("md"));
        candidates.push(path.with_extension("markdown"));
    }
    candidates
}

/// `path` without `.` and `..` parts, or `None` when it climbs above the notebook or isn't a
/// plain relative path.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.iter().collect())
}

/// The candidate a link from `from` means: the one in `from`'s folder, then the one with the
/// fewest folders, then the first by path (FastPad sorts naturally here).
fn best(candidates: &[PathBuf], from: &Path) -> Option<PathBuf> {
    let folder = path_key(from.parent().unwrap_or(Path::new("")));
    candidates
        .iter()
        .min_by(|a, b| {
            let near = |path: &PathBuf| {
                !path
                    .parent()
                    .is_some_and(|parent| path_key(parent) == folder)
            };
            near(a)
                .cmp(&near(b))
                .then(a.components().count().cmp(&b.components().count()))
                .then_with(|| path_key(a).cmp(&path_key(b)))
        })
        .cloned()
}

/// The shortest wikilink target that reaches `note` from the note at `from`: its stem when no
/// other note shares it, else its path from the notebook root with `/`, leaving out `.md`.
pub fn wiki_target(resolver: &Resolver, note: &Path, from: &Path) -> String {
    let markdown = is_note(note);
    let stem = note
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = if markdown {
        note.with_extension("")
    } else {
        note.to_path_buf()
    }
    .to_string_lossy()
    .replace('\\', "/");
    // The first form that reaches the note from where the link is: a bare name can mean a note
    // in the linking note's own folder, so a root note may need `./name`.
    let mut candidates = Vec::new();
    if markdown && resolver.stem_count(&stem) <= 1 {
        candidates.push(stem);
    }
    candidates.push(path.clone());
    candidates.push(format!("./{path}"));
    candidates
        .into_iter()
        .find(|candidate| {
            resolver
                .resolve_wiki(candidate, from)
                .is_some_and(|reached| path_key(&reached) == path_key(note))
        })
        .unwrap_or(path)
}

/// `to` relative to `from`'s folder with `/`, spaces percent-encoded when `original` encoded
/// anything or had none to encode, so the destination stays one Markdown token. Both paths are
/// relative to the notebook. Pass `""` as `original` for a new link.
pub fn relative_target(from: &Path, to: &Path, original: &str) -> String {
    let base: Vec<_> = from
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .collect();
    let target: Vec<_> = to.components().collect();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(a, b)| {
            a.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
        })
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_owned(), base.len() - common).collect();
    parts.extend(
        target[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    let joined = parts.join("/");
    if original.contains('%') || (!original.contains(' ') && joined.contains(' ')) {
        joined.replace('%', "%25").replace(' ', "%20")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unique_name_is_enough_and_a_shared_one_takes_the_path() {
        let notes = [
            Path::new(r"Payments\Payments incidents.md"),
            Path::new(r"Payments\Lag.md"),
            Path::new(r"Search\Lag.md"),
            Path::new("Kafka.md"),
        ];
        let resolver = Resolver::new(
            notes.iter().copied(),
            [("Streams".to_owned(), PathBuf::from("Kafka.md"))],
        );
        let from = Path::new(r"inbox\new.md");
        assert_eq!(wiki_target(&resolver, notes[0], from), "Payments incidents");
        assert_eq!(wiki_target(&resolver, notes[1], from), "Payments/Lag");
        assert_eq!(wiki_target(&resolver, notes[2], from), "Search/Lag");
        assert_eq!(wiki_target(&resolver, notes[3], from), "Kafka");
        assert_eq!(
            resolver.resolve_wiki("streams", from),
            Some(PathBuf::from("Kafka.md"))
        );
        assert_eq!(
            resolver.resolve_wiki("Lag", Path::new(r"Search\x.md")),
            Some(PathBuf::from(r"Search\Lag.md")),
            "the note in the linking note's folder wins"
        );
    }

    #[test]
    fn targets_climb_out_of_the_notes_folder_and_encode_spaces() {
        assert_eq!(
            relative_target(
                Path::new(r"raw\2026-10-19 1030 lag.md"),
                Path::new(r"attachments\2026-10-19 1030 lag.png"),
                ""
            ),
            "../attachments/2026-10-19%201030%20lag.png"
        );
        assert_eq!(
            relative_target(Path::new("inbox.md"), Path::new(r"attachments\a.log"), ""),
            "attachments/a.log"
        );
        assert_eq!(
            relative_target(Path::new(r"Raw\x.md"), Path::new(r"raw\img\y.png"), ""),
            "img/y.png",
            "folders match without regard to case"
        );
        assert_eq!(
            relative_target(Path::new(r"a\b\c.md"), Path::new(r"d\100%.png"), ""),
            "../../d/100%.png",
            "nothing to encode keeps the text"
        );
        assert_eq!(
            relative_target(Path::new(r"a\c.md"), Path::new(r"d\100% done.png"), ""),
            "../d/100%25%20done.png"
        );
    }
}
