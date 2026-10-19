//! Text, logs and code read for the model. Decoding is adapted from FastPad
//! `src/file/encoding.rs` (its BOM handling), with UTF-16 without a BOM, a Windows-1252
//! fallback and a binary check added, because attachments come from anywhere.

use std::path::Path;

/// Logs longer than this many lines keep their first and last `LOG_KEEP`.
const LOG_LINES: usize = 2 * LOG_KEEP;
const LOG_KEEP: usize = 200;
/// How much of the start of a file the encoding and binary checks look at.
const SNIFF: usize = 8192;

/// A file's text, ready to go inside an attachment block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Text {
    pub text: String,
    /// Lines were cut from the middle of a long log.
    pub cut: bool,
}

/// The file's text, fenced when it is code and cut when it is a long log. `None` for a file
/// that isn't text.
pub fn extract(name: &str, bytes: &[u8]) -> Option<Text> {
    let text = decode(bytes)?;
    let text = text.replace("\r\n", "\n");
    let path = Path::new(name);
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if is_log(path, &extension) {
        let (text, cut) = cut_log(&text);
        return Some(Text { text, cut });
    }
    let text = match fence_language(&extension) {
        Some(language) => fence(&text, language),
        None => text,
    };
    Some(Text { text, cut: false })
}

/// The bytes as text: by BOM, then UTF-16 by its zero bytes, then UTF-8, then Windows-1252.
/// `None` when the bytes look binary.
pub fn decode(bytes: &[u8]) -> Option<String> {
    if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return utf8_or_1252(body);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(body, u16::from_le_bytes);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(body, u16::from_be_bytes);
    }
    match utf16_without_bom(bytes) {
        Some(true) => return decode_utf16(bytes, u16::from_le_bytes),
        Some(false) => return decode_utf16(bytes, u16::from_be_bytes),
        None => {}
    }
    utf8_or_1252(bytes)
}

fn utf8_or_1252(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(SNIFF)];
    if looks_binary(head) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        Err(_) => Some(bytes.iter().map(|&byte| windows_1252(byte)).collect()),
    }
}

/// A NUL, or more than one in ten characters a control character that text files don't use.
fn looks_binary(head: &[u8]) -> bool {
    if head.contains(&0) {
        return true;
    }
    let controls = head
        .iter()
        .filter(|&&byte| byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B))
        .count();
    controls * 10 > head.len()
}

/// `Some(true)` for UTF-16LE, `Some(false)` for UTF-16BE: most of the high bytes of the start are
/// zero (ASCII text in UTF-16) and almost none of the low ones are.
fn utf16_without_bom(bytes: &[u8]) -> Option<bool> {
    let head = &bytes[..bytes.len().min(SNIFF) & !1];
    if head.len() < 4 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let pairs = head.len() / 2;
    let (even, odd) = head
        .as_chunks::<2>()
        .0
        .iter()
        .fold((0, 0), |(even, odd), pair| {
            (
                even + usize::from(pair[0] == 0),
                odd + usize::from(pair[1] == 0),
            )
        });
    let mostly = |count: usize| count * 10 >= pairs * 7;
    let rarely = |count: usize| count * 20 <= pairs;
    if mostly(odd) && rarely(even) {
        Some(true)
    } else if mostly(even) && rarely(odd) {
        Some(false)
    } else {
        None
    }
}

fn decode_utf16(bytes: &[u8], read: fn([u8; 2]) -> u16) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes.as_chunks::<2>().0.iter().map(|chunk| read(*chunk));
    let text = char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .ok()?;
    let head: Vec<u8> = text.bytes().take(SNIFF).collect();
    (!looks_binary(&head)).then_some(text)
}

/// Windows-1252, with the five unassigned bytes kept as the C1 controls of the same number.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// `.log`, `.out` and `.err` files, and files with `log` as a word of their name
/// (`server-log.txt`, `build_log`).
fn is_log(path: &Path, extension: &str) -> bool {
    if matches!(extension, "log" | "out" | "err") {
        return true;
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|stem| {
            stem.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == "log" || word == "logs")
        })
}

/// The first and last `LOG_KEEP` lines of a long log, with a line saying how many were cut.
fn cut_log(text: &str) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= LOG_LINES {
        return (text.to_owned(), false);
    }
    let cut = lines.len() - LOG_LINES;
    let mut kept = lines[..LOG_KEEP].join("\n");
    kept.push_str(&format!("\n… {cut} lines cut …\n"));
    kept.push_str(&lines[lines.len() - LOG_KEEP..].join("\n"));
    (kept, true)
}

/// The fence language for a code file's extension, from FastPad's language list. Markdown and
/// plain text aren't fenced.
fn fence_language(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "sh" | "bash" | "zsh" => "bash",
        "bat" | "cmd" => "bat",
        "c" | "h" => "c",
        "cs" | "csx" => "csharp",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "inl" => "cpp",
        "css" => "css",
        "env" => "sh",
        "html" | "htm" | "xhtml" => "html",
        "ini" | "cfg" | "conf" | "inf" | "reg" => "ini",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "json" | "jsonc" | "json5" | "jsonl" | "geojson" | "webmanifest" | "code-workspace" => {
            "json"
        }
        "ps1" | "psm1" | "psd1" => "powershell",
        "properties" => "properties",
        "py" | "pyw" | "pyi" => "python",
        "rs" => "rust",
        "sql" => "sql",
        "svg" | "xml" | "xaml" | "xsd" | "xsl" | "xslt" | "csproj" | "vbproj" | "fsproj"
        | "props" | "targets" | "config" | "resx" | "nuspec" | "manifest" | "plist" | "rss"
        | "atom" => "xml",
        "toml" => "toml",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "yaml" | "yml" => "yaml",
        _ => return None,
    })
}

/// `text` in a fence longer than any run of backticks inside it.
fn fence(text: &str, language: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let body = text.trim_end_matches('\n');
    format!("{fence}{language}\n{body}\n{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str, write: fn(u16) -> [u8; 2]) -> Vec<u8> {
        text.encode_utf16().flat_map(write).collect()
    }

    #[test]
    fn every_encoding_decodes() {
        let text = "zăpadă 🦀 ok";
        assert_eq!(decode(text.as_bytes()).unwrap(), text);
        assert_eq!(
            decode(&[b"\xEF\xBB\xBF", text.as_bytes()].concat()).unwrap(),
            text
        );
        let le = utf16(text, u16::to_le_bytes);
        let be = utf16(text, u16::to_be_bytes);
        assert_eq!(decode(&[&[0xFF, 0xFE][..], &le].concat()).unwrap(), text);
        assert_eq!(decode(&[&[0xFE, 0xFF][..], &be].concat()).unwrap(), text);
        let plain = "a log line from a Windows tool\r\nsecond line\r\n";
        assert_eq!(decode(&utf16(plain, u16::to_le_bytes)).unwrap(), plain);
        assert_eq!(decode(&utf16(plain, u16::to_be_bytes)).unwrap(), plain);
        // Not UTF-8: read as Windows-1252.
        assert_eq!(
            decode(b"caf\xE9 \x80 5 \x93ok\x94").unwrap(),
            "café € 5 “ok”"
        );
    }

    #[test]
    fn binary_files_are_not_text() {
        assert_eq!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None);
        assert_eq!(decode(b"MZ\x90\0\x03\0\0\0"), None);
        assert_eq!(decode(&[1, 2, 3, 4, 5, 6, 7, 8, b'a', b'b']), None);
        assert!(extract("data.bin", b"PK\x03\x04\x14\0\0\0").is_none());
        // Escape sequences and form feeds are text.
        assert!(decode(b"\x1b[31mred\x1b[0m\n\x0cpage").is_some());
    }

    #[test]
    fn a_long_log_keeps_its_first_and_last_200_lines() {
        let log: String = (1..=1000).map(|n| format!("line {n}\r\n")).collect();
        let read = extract("server.log", log.as_bytes()).unwrap();
        assert!(read.cut);
        let lines: Vec<&str> = read.text.lines().collect();
        assert_eq!(lines.len(), 401);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[199], "line 200");
        assert_eq!(lines[200], "… 600 lines cut …");
        assert_eq!(lines[201], "line 801");
        assert_eq!(lines[400], "line 1000");

        let short: String = (1..=400).map(|n| format!("line {n}\n")).collect();
        let read = extract("build_log.txt", short.as_bytes()).unwrap();
        assert!(!read.cut);
        assert_eq!(read.text, short);
        // A long file that isn't a log is left whole (the budget cuts it, if anything).
        let long = (1..=1000).map(|n| format!("{n}\n")).collect::<String>();
        assert!(!extract("catalog.txt", long.as_bytes()).unwrap().cut);
    }

    #[test]
    fn code_is_fenced_with_its_language() {
        let read = extract("main.rs", b"fn main() {}\r\n").unwrap();
        assert_eq!(read.text, "```rust\nfn main() {}\n```");
        let read = extract("README.md", b"# Title\n").unwrap();
        assert_eq!(read.text, "# Title\n");
        let read = extract("notes.txt", b"plain").unwrap();
        assert_eq!(read.text, "plain");
        // Backticks inside the code get a longer fence.
        let read = extract("x.py", b"s = '````'\n").unwrap();
        assert_eq!(read.text, "`````python\ns = '````'\n`````");
    }
}
