//! Moving a note to another folder: a free name there, and its own relative links rewritten so
//! they keep reaching their files. FastPad's `edits_for_moves_with` does this from its Markdown
//! parser's links; here a note's links are read line by line, leaving out code. Wikilinks need
//! no change: they name notes, or paths from the notebook's root. Other notes' links to the
//! moved note aren't rewritten either; a capture just written has none.

use crate::notebook::links::relative_target;
use crate::notebook::title;
use std::path::{Component, Path, PathBuf};

/// Where the note at `note` (relative to the notebook at `root`) goes in `folder`: its own name,
/// or the first free `name 2.md`, `name 3.md`…
pub fn destination(root: &Path, note: &Path, folder: &Path) -> PathBuf {
    let stem = note
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned());
    let extension = note
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_else(|| "md".to_owned());
    let directory = root.join(folder);
    folder.join(title::free_name(&stem, &extension, |name| {
        directory.join(name).exists()
    }))
}

/// `text`, the note moving from `from` to `to` (both relative), with each relative Markdown link
/// to a file that `exists` rewritten to reach it from `to`.
pub fn moved_text(text: &str, from: &Path, to: &Path, exists: &dyn Fn(&Path) -> bool) -> String {
    let mut output = String::with_capacity(text.len());
    let mut fence: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            output.push_str(line);
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            output.push_str(line);
            continue;
        }
        output.push_str(&moved_line(line, from, to, exists));
    }
    output
}

/// One line's links rewritten, outside its code spans.
fn moved_line(line: &str, from: &Path, to: &Path, exists: &dyn Fn(&Path) -> bool) -> String {
    let mut output = String::with_capacity(line.len());
    let mut at = 0;
    let mut in_code = false;
    let bytes = line.as_bytes();
    while at < line.len() {
        if bytes[at] == b'`' {
            in_code = !in_code;
        }
        if !in_code && line[at..].starts_with("](") {
            let start = at + 2;
            if let Some((range, target)) = destination_at(line, start)
                && let Some(new) = new_target(target, from, to, exists)
            {
                output.push_str(&line[at..range.start]);
                output.push_str(&new);
                at = range.end;
                continue;
            }
        }
        let next = line[at..].chars().next().map_or(1, char::len_utf8);
        output.push_str(&line[at..at + next]);
        at += next;
    }
    output
}

/// The link destination starting at `start` (just after `](`): its path's byte range, without a
/// `#fragment`, and the path.
fn destination_at(line: &str, start: usize) -> Option<(std::ops::Range<usize>, &str)> {
    let rest = &line[start..];
    let (offset, dest) = if let Some(angled) = rest.strip_prefix('<') {
        (1, &angled[..angled.find('>')?])
    } else {
        let end = rest.find(|c: char| c.is_whitespace() || c == ')')?;
        (0, &rest[..end])
    };
    let path = dest.split('#').next().unwrap_or(dest);
    let begin = start + offset;
    Some((begin..begin + path.len(), path))
}

/// The new text of a relative link `target` written in a note moving from `from` to `to`, when
/// it reaches a file that `exists` and the moved note would no longer reach it.
fn new_target(
    target: &str,
    from: &Path,
    to: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<String> {
    if !is_local_destination(target) {
        return None;
    }
    let decoded = percent_decode(target).replace('/', "\\");
    let folder = |note: &Path| note.parent().unwrap_or(Path::new("")).to_path_buf();
    let reached = normalize(&folder(from).join(&decoded))?;
    if !exists(&reached) {
        return None;
    }
    let after = normalize(&folder(to).join(&decoded));
    if after.is_some_and(|after| {
        after
            .to_string_lossy()
            .eq_ignore_ascii_case(&reached.to_string_lossy())
    }) {
        return None;
    }
    Some(relative_target(to, &reached, target))
}

// Copied from FastPad `src/library/facts.rs` and `src/library/links.rs`.

/// Whether a Markdown link destination may be a file in the notebook: no scheme (`https:`,
/// `mailto:`), not just a `#fragment`, not a network path.
fn is_local_destination(dest: &str) -> bool {
    let dest = dest.trim();
    if dest.is_empty()
        || dest.starts_with('#')
        || dest.starts_with("//")
        || dest.starts_with("\\\\")
    {
        return false;
    }
    let scheme = dest.find(':').is_some_and(|colon| {
        colon > 1
            && dest[..colon]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    !scheme
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = text.get(index + 1..index + 3)
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            output.push(value);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_links_to_files_keep_reaching_them() {
        let note = "---\nsource: \"[[raw/2026-10-19 lag]]\"\n---\n\
                    ![shot](../attachments/2026-10-19%20lag.png)\n\
                    [log](../attachments/a.log#L4 \"the log\") and [web](https://x.example/a.md)\n\
                    [gone](../attachments/missing.png) `[code](../attachments/a.log)`\n\
                    ```\n[fenced](../attachments/a.log)\n```\n";
        let exists = |path: &Path| {
            [r"attachments\2026-10-19 lag.png", r"attachments\a.log"]
                .iter()
                .any(|known| Path::new(known) == path)
        };
        let moved = moved_text(
            note,
            Path::new(r"inbox\Lag.md"),
            Path::new(r"Payments\Incidents\Lag.md"),
            &exists,
        );
        assert_eq!(
            moved,
            "---\nsource: \"[[raw/2026-10-19 lag]]\"\n---\n\
             ![shot](../../attachments/2026-10-19%20lag.png)\n\
             [log](../../attachments/a.log#L4 \"the log\") and [web](https://x.example/a.md)\n\
             [gone](../attachments/missing.png) `[code](../attachments/a.log)`\n\
             ```\n[fenced](../attachments/a.log)\n```\n"
        );
        // A move within the same depth that still reaches the file changes nothing.
        assert_eq!(
            moved_text(
                note,
                Path::new(r"inbox\Lag.md"),
                Path::new(r"Payments\Lag.md"),
                &exists
            ),
            note
        );
    }

    #[test]
    fn a_clash_takes_the_next_number() {
        let root = std::env::temp_dir().join(format!("fastcapture-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Payments")).unwrap();
        let note = Path::new(r"inbox\Lag.md");
        assert_eq!(
            destination(&root, note, Path::new("Payments")),
            PathBuf::from(r"Payments\Lag.md")
        );
        std::fs::write(root.join(r"Payments\Lag.md"), "").unwrap();
        std::fs::write(root.join(r"Payments\Lag 2.md"), "").unwrap();
        assert_eq!(
            destination(&root, note, Path::new("Payments")),
            PathBuf::from(r"Payments\Lag 3.md")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
