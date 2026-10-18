//! Copied from FastPad `src/library/links.rs` (only `relative_target`).
//!
//! Markdown link destinations between files in a notebook.

use std::path::Path;

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
