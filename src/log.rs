//! The diagnostic log, `%LocalAppData%\FastCapture\log\fastcapture.log`. It
//! records what the app did and what failed, never capture text, page content, prompts or keys.
//! Each line is scrubbed before it is written: URLs keep only their host, and paths in a notebook
//! or the profile only their first folder. Logging never fails the caller.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE: &str = "fastcapture.log";
const OLD_LOG_FILE: &str = "fastcapture.old.log";
/// Past this size the log is moved to `fastcapture.old.log` and started again.
const MAX_BYTES: u64 = 1024 * 1024;

/// Appends one timestamped line to the log.
pub fn line(message: &str) {
    // Tests exercise failure paths that log; they must not write into the user's real log.
    if cfg!(test) {
        return;
    }
    let notebooks = match NOTEBOOKS.read() {
        Ok(notebooks) => notebooks.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let profile = std::env::var("USERPROFILE").ok();
    let message = scrub(message, &notebooks, profile.as_deref());
    if let Some(dir) = crate::paths::log_dir() {
        let _ = append(&dir, SystemTime::now(), &message);
    }
}

/// The notebooks whose paths `line` shortens, lowercase with `\` separators.
static NOTEBOOKS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// The extensions of the files whose paths get logged: FastCapture's own files, its temps, and
/// the attachments it reads. A path ends right after one of them.
const FILE_EXTENSIONS: [&str; 19] = [
    "md",
    "txt",
    "json",
    "ini",
    "log",
    "tmp",
    "toml",
    "exe",
    "dll",
    "fastcapture-tmp",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "pdf",
    "docx",
    "html",
    "htm",
];

/// Sets the notebooks whose paths the log shortens to `<notebook>\<first folder>\…`.
pub fn set_notebooks(roots: &[PathBuf]) {
    let roots = roots.iter().map(|root| normalize_root(root)).collect();
    match NOTEBOOKS.write() {
        Ok(mut notebooks) => *notebooks = roots,
        Err(poisoned) => *poisoned.into_inner() = roots,
    }
}

/// `root` lowercase, with `\` separators and no trailing separator.
fn normalize_root(root: &Path) -> String {
    fold_root(&root.to_string_lossy())
}

fn fold_root(root: &str) -> String {
    let folded: String = root.chars().flat_map(fold).collect();
    folded.trim_end_matches('\\').to_owned()
}

/// `c` as paths compare it: lowercase, `/` read as `\`.
fn fold(c: char) -> std::char::ToLowercase {
    if c == '/' { '\\' } else { c }.to_lowercase()
}

/// `message` without what a capture holds.
///
/// - Each `http://` or `https://` URL, up to whitespace or a quote, becomes its host. Punctuation
///   at its end stays as text.
/// - A path in a notebook becomes `<notebook>`, `<notebook>\<folder>` or
///   `<notebook>\<first folder>\…`. A path in the profile becomes the same with
///   `%USERPROFILE%`. The kept component is always a folder: a file directly under the root
///   becomes `<notebook>\…`.
/// - A path ends right after a file extension (`.md`, `.pdf`, …) followed by whitespace,
///   punctuation or the end, when one comes before the hard end: `: `, a quote, a line break or
///   the end of the message. Without one, it ends at whitespace followed by an uppercase letter,
///   or at the hard end.
fn scrub(message: &str, notebooks: &[String], profile: Option<&str>) -> String {
    let mut roots: Vec<(String, &str)> = notebooks
        .iter()
        .map(|root| (fold_root(root), "<notebook>"))
        .collect();
    // A notebook inside another is matched before the one around it.
    roots.sort_by_key(|(root, _)| std::cmp::Reverse(root.chars().count()));
    roots.extend(profile.map(|profile| (fold_root(profile), "%USERPROFILE%")));
    roots.retain(|(root, _)| !root.is_empty());

    let mut out = String::with_capacity(message.len());
    let mut at = 0;
    while let Some(rest) = message.get(at..) {
        let Some(c) = rest.chars().next() else {
            break;
        };
        if let Some((replacement, used)) = url_at(rest).or_else(|| path_at(rest, &roots)) {
            out.push_str(&replacement);
            at += used;
        } else {
            out.push(c);
            at += c.len_utf8();
        }
    }
    out
}

/// The host of the URL `rest` starts with, and how many bytes of `rest` the URL takes. The URL
/// runs to whitespace or a quote; punctuation at its end is left as text.
fn url_at(rest: &str) -> Option<(String, usize)> {
    let scheme = ["https://", "http://"].into_iter().find(|scheme| {
        rest.as_bytes()
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme.as_bytes()))
    })?;
    let end = rest
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() || c == '"' || c == '\'')
        .map_or(rest.len(), |(index, _)| index);
    let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>']);
    let after = url.get(scheme.len()..).unwrap_or_default();
    let authority = after
        .split(['/', '?', '#', '\\'])
        .next()
        .unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    Some((host.to_owned(), url.len().max(scheme.len())))
}

/// The shortened form of the path `rest` starts with, if it is in one of `roots`, and how many
/// bytes of `rest` the path takes.
fn path_at(rest: &str, roots: &[(String, &str)]) -> Option<(String, usize)> {
    let (start, name) = roots
        .iter()
        .find_map(|(root, name)| root_len(rest, root).map(|len| (len, *name)))?;
    let (len, ends_with_file) = path_len(&rest[start..]);
    let end = start + len;
    let mut components = rest[start..end]
        .split(['\\', '/'])
        .filter(|component| !component.is_empty());
    let shortened = match (components.next(), components.next()) {
        (None, _) => name.to_owned(),
        (Some(first), None) if ends_with_file || has_file_extension(first) => {
            format!("{name}\\…")
        }
        (Some(first), None) => format!("{name}\\{first}"),
        (Some(first), Some(_)) => format!("{name}\\{first}\\…"),
    };
    Some((shortened, end))
}

/// How many bytes of `rest` the folded `root` takes, when `rest` starts with it and the root is
/// not just the start of a longer name.
fn root_len(rest: &str, root: &str) -> Option<usize> {
    let mut wanted = root.chars();
    let mut end = None;
    for (index, c) in rest.char_indices() {
        for folded in fold(c) {
            if wanted.next() != Some(folded) {
                return None;
            }
        }
        if wanted.as_str().is_empty() {
            end = Some(index + c.len_utf8());
            break;
        }
    }
    let end = end?;
    let next = rest[end..].chars().next();
    next.is_none_or(|c| {
        matches!(c, '\\' | '/' | '"' | '\'' | ':' | ')' | ',' | ';') || c.is_whitespace()
    })
    .then_some(end)
}

/// How many bytes of `rest`, the part of a path after its root, still belong to the path, and
/// whether it ended right after a file extension.
fn path_len(rest: &str) -> (usize, bool) {
    let hard_end = hard_end(rest);
    if let Some(end) = extension_end(&rest[..hard_end], rest) {
        return (end, true);
    }
    let mut chars = rest[..hard_end].char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let next = chars.peek().map(|&(_, next)| next);
        if c.is_whitespace() && next.is_some_and(char::is_uppercase) {
            return (index, false);
        }
    }
    (hard_end, false)
}

/// Where a path in `rest` ends at the latest: at `: ` (or a `:` that ends the message), a quote,
/// a line break, or the end.
fn hard_end(rest: &str) -> usize {
    let mut chars = rest.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let next = chars.peek().map(|&(_, next)| next);
        let ends = match c {
            '"' | '\r' | '\n' => true,
            ':' => next.is_none_or(|next| next == ' '),
            _ => false,
        };
        if ends {
            return index;
        }
    }
    rest.len()
}

/// The end of the first file extension in `path` that is followed, in `rest`, by whitespace,
/// punctuation that can't go on a name (not a separator, `-`, `_`, or a `.` with more after it),
/// or the end.
fn extension_end(path: &str, rest: &str) -> Option<usize> {
    path.char_indices()
        .filter(|&(_, c)| c == '.')
        .find_map(|(dot, _)| {
            FILE_EXTENSIONS.iter().find_map(|extension| {
                let end = dot + 1 + extension.len();
                let candidate = path.get(dot + 1..end)?;
                let mut after = rest.get(end..)?.chars();
                let boundary = match after.next() {
                    None => true,
                    // `x.html.fastcapture-tmp` goes on; `x.md.` at the end of a sentence stops.
                    Some('.') => after.next().is_none_or(char::is_whitespace),
                    Some('\\' | '/' | '-' | '_') => false,
                    Some(next) => next.is_whitespace() || next.is_ascii_punctuation(),
                };
                (candidate.eq_ignore_ascii_case(extension) && boundary).then_some(end)
            })
        })
}

fn has_file_extension(component: &str) -> bool {
    component.rsplit_once('.').is_some_and(|(_, extension)| {
        FILE_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

fn append(dir: &Path, now: SystemTime, message: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = fs::rename(&path, dir.join(OLD_LOG_FILE));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let message = message.replace(['\r', '\n'], " ");
    writeln!(file, "{} {message}", utc_stamp(now))
}

/// `YYYY-MM-DDTHH:MM:SSZ` for `time`.
fn utc_stamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fastcapture-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap_day = UNIX_EPOCH + Duration::from_secs(1_709_210_096);
        assert_eq!(utc_stamp(leap_day), "2024-02-29T12:34:56Z");
        let today = UNIX_EPOCH + Duration::from_secs(1_792_281_600);
        assert_eq!(utc_stamp(today), "2026-10-18T00:00:00Z");
    }

    #[test]
    fn lines_are_appended_on_one_line_each() {
        let dir = temp_dir("append");
        append(&dir, UNIX_EPOCH, "first").unwrap();
        append(&dir, UNIX_EPOCH, "second\r\nline").unwrap();
        let text = fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(
            text,
            "1970-01-01T00:00:00Z first\n1970-01-01T00:00:00Z second  line\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_log_is_moved_aside() {
        let dir = temp_dir("rotate");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOG_FILE), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append(&dir, UNIX_EPOCH, "fresh").unwrap();
        assert_eq!(
            fs::metadata(dir.join(OLD_LOG_FILE)).unwrap().len(),
            MAX_BYTES + 1
        );
        assert_eq!(
            fs::read_to_string(dir.join(LOG_FILE)).unwrap(),
            "1970-01-01T00:00:00Z fresh\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn urls_become_their_host() {
        assert_eq!(
            scrub(
                "The page at https://example.com/a/b?c couldn't be fetched yet: failed (403)",
                &[],
                None
            ),
            "The page at example.com couldn't be fetched yet: failed (403)"
        );
    }

    #[test]
    fn notebook_paths_keep_their_first_folder_and_the_error() {
        let notebooks = vec![r"d:\notes".to_owned()];
        assert_eq!(
            scrub(
                r"FastCapture could not save D:\Notes\raw\2026 lag spikes.md: Access is denied.",
                &notebooks,
                None
            ),
            r"FastCapture could not save <notebook>\raw\…: Access is denied."
        );
        assert_eq!(scrub("D:/Notes", &notebooks, None), "<notebook>");
    }

    #[test]
    fn other_paths_in_the_profile_lose_their_names() {
        assert_eq!(
            scrub(
                r"usage: C:\Users\ana\AppData\Local\FastCapture\usage.txt couldn't be read: x",
                &[],
                Some(r"c:\users\ana")
            ),
            r"usage: %USERPROFILE%\AppData\… couldn't be read: x"
        );
    }

    #[test]
    fn a_folder_directly_in_the_notebook_is_kept_whole() {
        let notebooks = vec![r"d:\notes".to_owned()];
        assert_eq!(
            scrub(r"index: D:\Notes\raw", &notebooks, None),
            r"index: <notebook>\raw"
        );
        assert_eq!(
            scrub(r"index: d:/notes/raw/ Access is denied.", &notebooks, None),
            r"index: <notebook>\raw Access is denied."
        );
    }

    #[test]
    fn a_title_with_capitals_ends_at_its_extension() {
        let notebooks = vec![r"d:\notes".to_owned()];
        assert_eq!(
            scrub(
                r"D:\Notes\raw\2026 Lag Spikes.md couldn't be read: x",
                &notebooks,
                None
            ),
            r"<notebook>\raw\… couldn't be read: x"
        );
        assert_eq!(
            scrub(r"D:\Notes\raw\Ana Plan.PDF, then more", &notebooks, None),
            r"<notebook>\raw\…, then more"
        );
        assert_eq!(
            scrub(r"D:\Notes\raw\x.html.fastcapture-tmp", &notebooks, None),
            r"<notebook>\raw\…"
        );
        assert_eq!(
            scrub(r"saved D:\Notes\raw\A b.md. Done", &notebooks, None),
            r"saved <notebook>\raw\…. Done"
        );
    }

    #[test]
    fn a_note_at_the_top_of_the_notebook_loses_its_name() {
        let notebooks = vec![r"d:\notes".to_owned()];
        assert_eq!(
            scrub(
                r"could not save D:\Notes\Secret title.md: Access is denied.",
                &notebooks,
                None
            ),
            r"could not save <notebook>\…: Access is denied."
        );
        assert_eq!(
            scrub(r"read D:\Notes\Secret title.md", &notebooks, None),
            r"read <notebook>\…"
        );
        assert_eq!(
            scrub(r"read C:\Users\ana\Secret.txt", &[], Some(r"c:\users\ana")),
            r"read %USERPROFILE%\…"
        );
        assert_eq!(
            scrub(r"index: D:\Notes\raw: gone", &notebooks, None),
            r"index: <notebook>\raw: gone"
        );
    }

    #[test]
    fn a_longer_folder_name_is_not_the_notebook() {
        let notebooks = vec![r"d:\notes".to_owned()];
        assert_eq!(
            scrub(r"saved D:\Notes2\x.md", &notebooks, None),
            r"saved D:\Notes2\x.md"
        );
    }

    #[test]
    fn two_paths_in_one_message_are_both_scrubbed() {
        let notebooks = vec![r"d:\notes".to_owned()];
        assert_eq!(
            scrub(
                r"move D:\Notes\inbox\Secret plan.md to D:\Notes\Projects\Secret plan.md: Access is denied.",
                &notebooks,
                None
            ),
            r"move <notebook>\inbox\… to <notebook>\Projects\…: Access is denied."
        );
    }

    #[test]
    fn urls_end_at_whitespace_or_a_quote_and_leave_trailing_punctuation() {
        assert_eq!(
            scrub(
                "links (https://a.example/x/y), \"http://b.example:8080/z\".",
                &[],
                None
            ),
            "links (a.example), \"b.example:8080\"."
        );
        assert_eq!(
            scrub("see HTTPS://user:pw@c.example/secret.", &[], None),
            "see c.example."
        );
    }

    #[test]
    fn with_no_notebooks_only_the_profile_is_scrubbed() {
        assert_eq!(
            scrub(r"read D:\Notes\raw\x.md: gone", &[], Some(r"c:\users\ana")),
            r"read D:\Notes\raw\x.md: gone"
        );
        assert_eq!(scrub("nothing here", &[], None), "nothing here");
    }

    #[test]
    fn a_notebook_in_the_profile_is_named_as_the_notebook() {
        let notebooks = vec![r"c:\users\ana\notes".to_owned()];
        assert_eq!(
            scrub(
                r"a C:\Users\Ana\Notes\raw\x.md: y, b C:\Users\Ana\Documents\z.md: w",
                &notebooks,
                Some(r"C:\Users\Ana")
            ),
            r"a <notebook>\raw\…: y, b %USERPROFILE%\Documents\…: w"
        );
    }

    #[test]
    fn scrubbing_never_panics_on_odd_text() {
        let notebooks = vec![r"d:\ñotes".to_owned(), String::new(), r"\".to_owned()];
        for message in [
            "",
            "https://",
            "http://ß/",
            r"D:\ÑOTES\ü\İx",
            r"İİİ D:\Ñotes",
            "\u{130}\u{130}https://é.example/\"",
            r"C:\Users\Ana:",
            r"D:\ñotes\",
        ] {
            let _ = scrub(message, &notebooks, Some(r"c:\users\ana"));
        }
    }

    #[test]
    fn notebook_roots_are_stored_lowercase_with_backslashes() {
        assert_eq!(normalize_root(Path::new("D:/Notes/")), r"d:\notes");
    }
}
