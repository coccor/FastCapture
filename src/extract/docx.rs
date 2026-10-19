//! Word `.docx` files: the text of `word/document.xml`, with headings, list items and tables
//! kept as Markdown-ish lines. A small scanner over the XML is enough, since only a handful of
//! elements matter: paragraphs (`w:p`), their style and numbering, runs of text (`w:t`), tabs,
//! breaks and tables.

use std::io::{Cursor, Read};

/// Larger than this, uncompressed, and the document isn't read: no real document is, and a
/// small file that unpacks to more is a zip bomb.
pub const MAX_XML_BYTES: u64 = 50 * 1024 * 1024;

/// The document's text, or why it couldn't be read.
pub fn read(bytes: &[u8]) -> Result<String, String> {
    read_within(bytes, MAX_XML_BYTES)
}

fn read_within(bytes: &[u8], limit: u64) -> Result<String, String> {
    let not_word = || "it isn't a Word document".to_owned();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| not_word())?;
    let mut file = archive
        .by_name("word/document.xml")
        .map_err(|_| not_word())?;
    let too_large = || "it is too large to read".to_owned();
    if file.size() > limit {
        return Err(too_large());
    }
    // The size in the header can lie: never unpack more than the limit.
    let mut xml = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut xml)
        .map_err(|_| "it couldn't be read".to_owned())?;
    if xml.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(text_of(&String::from_utf8_lossy(&xml)))
}

/// What a paragraph is, from its properties.
#[derive(Default)]
struct Paragraph {
    text: String,
    heading: Option<usize>,
    /// Its list level, from 0, when it is a list item.
    list: Option<usize>,
}

#[derive(Default)]
struct Scanner {
    blocks: Vec<String>,
    paragraph: Option<Paragraph>,
    in_text: bool,
    /// The tables being read, innermost last: rows of cells.
    tables: Vec<Vec<Vec<String>>>,
    cell: Option<String>,
    /// The last block is a list item, so the next item joins it.
    in_list: bool,
}

impl Scanner {
    fn start(&mut self, name: &str, tag: &str) {
        match name {
            "w:p" => self.paragraph = Some(Paragraph::default()),
            "w:pStyle" => {
                if let (Some(paragraph), Some(style)) =
                    (&mut self.paragraph, attribute(tag, "w:val"))
                {
                    paragraph.heading = heading_level(&style);
                }
            }
            "w:numPr" => {
                if let Some(paragraph) = &mut self.paragraph {
                    paragraph.list.get_or_insert(0);
                }
            }
            "w:ilvl" => {
                if let Some(paragraph) = &mut self.paragraph {
                    let level = attribute(tag, "w:val").and_then(|value| value.parse().ok());
                    paragraph.list = Some(level.unwrap_or(0));
                }
            }
            "w:t" => self.in_text = true,
            "w:tab" => self.push_text("\t"),
            "w:br" | "w:cr" => self.push_text("\n"),
            "w:tbl" => self.tables.push(Vec::new()),
            "w:tr" => {
                if let Some(table) = self.tables.last_mut() {
                    table.push(Vec::new());
                }
            }
            "w:tc" => self.cell = Some(String::new()),
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "w:t" => self.in_text = false,
            "w:p" => {
                if let Some(paragraph) = self.paragraph.take() {
                    self.finish_paragraph(paragraph);
                }
            }
            "w:tc" => {
                if let (Some(cell), Some(row)) = (
                    self.cell.take(),
                    self.tables.last_mut().and_then(|table| table.last_mut()),
                ) {
                    row.push(cell.trim().to_owned());
                }
            }
            "w:tbl" => {
                if let Some(table) = self.tables.pop() {
                    let lines: Vec<String> = table
                        .iter()
                        .filter(|row| row.iter().any(|cell| !cell.is_empty()))
                        .map(|row| format!("| {} |", row.join(" | ")))
                        .collect();
                    let text = lines.join("\n");
                    // A table inside a cell becomes part of that cell.
                    match &mut self.cell {
                        Some(cell) => {
                            cell.push(' ');
                            cell.push_str(&text.replace('\n', " "));
                        }
                        None => self.push_block(text, false),
                    }
                }
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: &str) {
        if let Some(paragraph) = &mut self.paragraph {
            paragraph.text.push_str(text);
        }
    }

    fn finish_paragraph(&mut self, paragraph: Paragraph) {
        let text = paragraph
            .text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        let text = text.trim();
        if let Some(cell) = &mut self.cell {
            if !text.is_empty() {
                if !cell.is_empty() {
                    cell.push(' ');
                }
                cell.push_str(&text.replace(['\n', '|'], " "));
            }
            return;
        }
        if text.is_empty() {
            return;
        }
        match (paragraph.heading, paragraph.list) {
            (Some(level), _) => self.push_block(format!("{} {text}", "#".repeat(level)), false),
            (None, Some(level)) => self.push_block(format!("{}- {text}", "  ".repeat(level)), true),
            (None, None) => self.push_block(text.to_owned(), false),
        }
    }

    fn push_block(&mut self, block: String, list_item: bool) {
        if block.is_empty() {
            return;
        }
        match self.blocks.last_mut() {
            Some(last) if list_item && self.in_list => {
                last.push('\n');
                last.push_str(&block);
            }
            _ => self.blocks.push(block),
        }
        self.in_list = list_item;
    }
}

/// `Heading1`…`Heading6` (and `Title`) as a Markdown heading level.
fn heading_level(style: &str) -> Option<usize> {
    if style.eq_ignore_ascii_case("Title") {
        return Some(1);
    }
    let digits = style
        .strip_prefix("Heading")
        .or_else(|| style.strip_prefix("heading"))?;
    digits
        .parse::<usize>()
        .ok()
        .filter(|level| (1..=6).contains(level))
}

/// The value of `name="…"` in a tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let at = tag.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = &rest[1..];
    Some(decode_entities(&value[..value.find(quote)?]))
}

/// XML's five named entities and numeric references.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|&end| end <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of a `document.xml`.
fn text_of(xml: &str) -> String {
    let mut scanner = Scanner::default();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        if scanner.in_text {
            let text = decode_entities(&rest[..open]);
            scanner.push_text(&text);
        }
        rest = &rest[open..];
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        if tag.starts_with(['?', '!']) {
            continue;
        }
        let (closing, body) = match tag.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, tag),
        };
        let name = body
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if closing {
            scanner.end(name);
        } else {
            scanner.start(name, body);
            if body.ends_with('/') {
                scanner.end(name);
            }
        }
    }
    scanner.blocks.join("\n\n")
}

#[cfg(test)]
pub(crate) mod build {
    use std::io::Write;

    /// A `.docx` holding `body` as the inside of `w:body`.
    pub fn docx(body: &str) -> Vec<u8> {
        zip_with("word/document.xml", &document(body))
    }

    pub fn document(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body>{body}<w:sectPr/></w:body></w:document>"
        )
    }

    pub fn zip_with(name: &str, content: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("[Content_Types].xml", options).unwrap();
        writer.write_all(b"<Types/>").unwrap();
        writer.start_file(name, options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::build::{docx, zip_with};
    use super::*;

    fn p(style: Option<&str>, runs: &str) -> String {
        let props = style
            .map(|style| format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>"))
            .unwrap_or_default();
        format!("<w:p>{props}{runs}</w:p>")
    }

    fn run(text: &str) -> String {
        format!("<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
    }

    fn item(level: u32, text: &str) -> String {
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"{level}\"/>\
             <w:numId w:val=\"1\"/></w:numPr></w:pPr>{}</w:p>",
            run(text)
        )
    }

    #[test]
    fn headings_paragraphs_lists_and_tables() {
        let body = [
            p(Some("Title"), &run("Ledger batch review")),
            p(Some("Heading2"), &run("Findings")),
            p(
                None,
                &format!("{}{}", run("Lag peaked at "), run("40k &amp; recovered.")),
            ),
            p(None, ""),
            item(0, "Only ledger-events"),
            item(1, "partition 3 worst"),
            item(0, "Back by 02:35"),
            p(
                None,
                &format!(
                    "{}<w:r><w:tab/><w:t>tabbed</w:t><w:br/><w:t>next line</w:t></w:r>",
                    run("Col")
                ),
            ),
            "<w:tbl><w:tr><w:tc>".to_owned()
                + &p(None, &run("Group"))
                + "</w:tc><w:tc>"
                + &p(None, &run("Lag"))
                + "</w:tc></w:tr><w:tr><w:tc>"
                + &p(None, &run("ledger-events"))
                + "</w:tc><w:tc>"
                + &p(None, &run("40k"))
                + &p(None, &run("peak"))
                + "</w:tc></w:tr></w:tbl>",
            p(None, &run("&#8220;Done&#x201D; &lt;ok&gt;")),
        ]
        .concat();
        assert_eq!(
            read(&docx(&body)).unwrap(),
            "# Ledger batch review\n\n## Findings\n\nLag peaked at 40k & recovered.\n\n\
             - Only ledger-events\n  - partition 3 worst\n- Back by 02:35\n\n\
             Col\ttabbed\nnext line\n\n\
             | Group | Lag |\n| ledger-events | 40k peak |\n\n\
             “Done” <ok>"
        );
    }

    #[test]
    fn deleted_text_and_field_codes_are_not_read() {
        let body = p(
            None,
            "<w:del><w:r><w:delText>old words</w:delText></w:r></w:del>\
             <w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:t>kept</w:t></w:r>",
        );
        assert_eq!(read(&docx(&body)).unwrap(), "kept");
    }

    #[test]
    fn other_files_and_bombs_are_refused() {
        assert_eq!(
            read(b"plain text"),
            Err("it isn't a Word document".to_owned())
        );
        assert_eq!(
            read(&zip_with("xl/workbook.xml", "<x/>")),
            Err("it isn't a Word document".to_owned())
        );
        let big = docx(&p(None, &run(&"lag ".repeat(10_000))));
        assert_eq!(
            read_within(&big, 1_000),
            Err("it is too large to read".to_owned())
        );
        assert!(read_within(&big, 100_000).is_ok());
    }
}
