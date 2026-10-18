//! Copied from FastPad `src/library/title.rs` (only the parts FastCapture uses).
//!
//! Turning a typed name or the first words of a capture into a safe file name, and the image
//! extensions FastPad shows.

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Image files FastPad lists and shows. SVG is listed but opens
/// as text, so `is_raster_image_extension` leaves it out.
pub const IMAGE_EXTENSIONS: [&str; 16] = [
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "dib", "ico", "tif", "tiff", "webp", "heic",
    "heif", "avif", "svg",
];

pub fn is_image_extension(extension: &str) -> bool {
    IMAGE_EXTENSIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(extension))
}

/// An image that is linked as `![](…)` and can be shown, rather than a file.
pub fn is_raster_image_extension(extension: &str) -> bool {
    is_image_extension(extension) && !extension.eq_ignore_ascii_case("svg")
}

/// A filename stem Windows accepts: no `<>:"/\|?*` or control characters, no trailing dots,
/// spaces or ellipsis, not a reserved device name, never empty.
pub fn sanitize_stem(name: &str) -> String {
    clean_stem(name).unwrap_or_else(|| "Untitled".to_owned())
}

/// `sanitize_stem`'s cleaning, with `None` for a name that cleans to nothing.
fn clean_stem(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .filter(|c| !c.is_control())
        .collect();
    let trimmed = cleaned
        .trim()
        .trim_end_matches(['.', ' ', '…'])
        .trim()
        .to_owned();
    if trimmed.is_empty() {
        return None;
    }
    // Windows reads "con.txt" and "con .txt" as the device too: neutralise the part before the
    // first dot.
    let device = trimmed.split('.').next().unwrap_or("").trim_end();
    if RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(device))
    {
        let (name, rest) = trimmed.split_at(device.len());
        return Some(format!("{name}_{rest}"));
    }
    Some(trimmed)
}

/// `stem.extension`, or just `stem` for an empty extension.
pub fn file_name(stem: &str, extension: &str) -> String {
    if extension.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{extension}")
    }
}

/// `stem.extension`, or `stem N.extension` with the first free N from 2. An empty extension
/// names an extensionless file.
pub fn free_name(stem: &str, extension: &str, exists: impl Fn(&str) -> bool) -> String {
    let first = file_name(stem, extension);
    if !exists(&first) {
        return first;
    }
    (2..10_000)
        .map(|number| file_name(&format!("{stem} {number}"), extension))
        .find(|candidate| !exists(candidate))
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_drop_invalid_characters_trailing_dots_and_reserved_names() {
        assert_eq!(sanitize_stem(r#"a/b: "c"? <d>|*"#), "ab c d");
        assert_eq!(sanitize_stem("Notes...  "), "Notes");
        assert_eq!(sanitize_stem("Long title…"), "Long title");
        assert_eq!(sanitize_stem("con"), "con_");
        assert_eq!(sanitize_stem("LPT9.draft"), "LPT9_.draft");
        assert_eq!(sanitize_stem("con.foo"), "con_.foo");
        assert_eq!(sanitize_stem("con .foo"), "con_ .foo");
        assert_eq!(sanitize_stem("console"), "console");
        assert_eq!(sanitize_stem("  \t "), "Untitled");
        assert_eq!(sanitize_stem("tab\there"), "tabhere");
        assert_eq!(sanitize_stem("..."), "Untitled");
    }

    #[test]
    fn clashing_names_get_the_first_free_number() {
        let taken = ["Plan.md", "Plan 2.md"];
        let exists = |name: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(name));
        assert_eq!(free_name("Plan", "md", exists), "Plan 3.md");
        assert_eq!(free_name("plan", "md", |_| false), "plan.md");
        assert_eq!(free_name("PLAN", "MD", exists), "PLAN 3.MD");
        assert_eq!(free_name("README", "", |name| name == "README"), "README 2");
    }

    #[test]
    fn images_are_told_from_other_files() {
        assert!(is_raster_image_extension("PNG"));
        assert!(is_raster_image_extension("webp"));
        assert!(!is_raster_image_extension("svg"));
        assert!(is_image_extension("svg"));
        assert!(!is_image_extension("pdf"));
    }
}
