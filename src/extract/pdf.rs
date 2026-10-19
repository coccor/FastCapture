//! PDFs: the text of each page, and pictures of the pages that have none (scans), for the model
//! to read. Text comes from pdf-extract; pages are rendered by Windows (`Windows.Data.Pdf`).

use crate::platform::png::ComScope;
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Storage::Streams::{DataReader, DataWriter, InMemoryRandomAccessStream};

/// A page with less text than this, in letters and digits, is taken for a scan.
pub const SCANNED_BELOW: usize = 40;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageText {
    Text(String),
    /// Almost no text: a scan or a picture, to be rendered and read as an image.
    Scanned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pdf {
    /// How many pages the document has.
    pub page_count: usize,
    /// The first pages, as many as were asked for.
    pub pages: Vec<PageText>,
}

impl Pdf {
    /// The pages read, each under a `## Page n` heading; a scanned page says it is attached as
    /// an image.
    pub fn text(&self) -> String {
        self.pages
            .iter()
            .enumerate()
            .map(|(index, page)| match page {
                PageText::Text(text) => format!("## Page {}\n\n{text}", index + 1),
                PageText::Scanned => {
                    format!(
                        "## Page {}\n\n(A scanned page, attached as an image.)",
                        index + 1
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The numbers (from 0) of the scanned pages read.
    pub fn scanned(&self) -> Vec<u32> {
        (0u32..)
            .zip(&self.pages)
            .filter(|(_, page)| **page == PageText::Scanned)
            .map(|(index, _)| index)
            .collect()
    }
}

/// The text of the first `max_pages` pages of the PDF in `bytes`. A PDF that needs a password,
/// or that can't be parsed, is an error saying so.
pub fn read(bytes: &[u8], max_pages: usize) -> Result<Pdf, String> {
    // A malformed file can make the parser panic; it is just a PDF that can't be read.
    catch_unwind(AssertUnwindSafe(|| read_unguarded(bytes, max_pages)))
        .unwrap_or_else(|_| Err("it couldn't be read".to_owned()))
}

fn read_unguarded(bytes: &[u8], max_pages: usize) -> Result<Pdf, String> {
    let mut document =
        pdf_extract::Document::load_mem(bytes).map_err(|_| "it couldn't be read".to_owned())?;
    if document.is_encrypted() && document.decrypt("").is_err() {
        return Err("it needs a password".to_owned());
    }
    let page_count = document.get_pages().len();
    let mut pages = Vec::new();
    for number in (1..=page_count.min(max_pages)).map(|n| n as u32) {
        let mut text = String::new();
        {
            let mut output = pdf_extract::PlainTextOutput::new(&mut text);
            // A page whose text can't be read is treated like a scan.
            let _ = pdf_extract::output_doc_page(&document, &mut output, number);
        }
        let text = tidy(&text);
        let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
        pages.push(if letters < SCANNED_BELOW {
            PageText::Scanned
        } else {
            PageText::Text(text)
        });
    }
    Ok(Pdf { page_count, pages })
}

/// Trailing spaces off every line, runs of blank lines down to one, and the ends trimmed.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in text.lines().map(str::trim_end) {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

/// The pages numbered `pages` (from 0) of the PDF in `bytes`, rendered as PNGs with their long
/// side `long_edge` pixels.
pub fn render(bytes: &[u8], pages: &[u32], long_edge: u32) -> Result<Vec<Vec<u8>>, String> {
    let _com = ComScope::enter();
    render_with_windows(bytes, pages, long_edge)
        .map_err(|error| format!("Windows couldn't render it: {error}"))
}

fn render_with_windows(
    bytes: &[u8],
    pages: &[u32],
    long_edge: u32,
) -> windows::core::Result<Vec<Vec<u8>>> {
    let input = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&input)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    // Keep the stream open when the writer goes.
    writer.DetachStream()?;
    input.Seek(0)?;
    let document = PdfDocument::LoadFromStreamAsync(&input)?.join()?;
    let mut rendered = Vec::new();
    for &index in pages {
        let page = document.GetPage(index)?;
        let size = page.Size()?;
        let options = PdfPageRenderOptions::new()?;
        if size.Height > size.Width {
            options.SetDestinationHeight(long_edge)?;
        } else {
            options.SetDestinationWidth(long_edge)?;
        }
        let output = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&output, &options)?
            .join()?;
        let size = output.Size()? as u32;
        let reader = DataReader::CreateDataReader(&output.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.join()?;
        let mut png = vec![0u8; size as usize];
        reader.ReadBytes(&mut png)?;
        rendered.push(png);
    }
    Ok(rendered)
}

/// Small PDFs written for the tests: text pages, picture-only pages, and an encrypted one.
#[cfg(test)]
pub(crate) mod build {
    pub enum Page<'a> {
        /// Lines of text in Helvetica.
        Text(&'a [&'a str]),
        /// A grey square drawn as an image, and no text.
        Picture,
    }

    pub fn pdf(pages: &[Page<'_>], encrypted: bool) -> Vec<u8> {
        let mut objects: Vec<Vec<u8>> = Vec::new();
        // 1 catalog, 2 pages, 3 font, 4 image, then a page and its content per page.
        let kids: Vec<String> = (0..pages.len())
            .map(|i| format!("{} 0 R", 5 + 2 * i))
            .collect();
        objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        objects.push(
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                pages.len()
            )
            .into_bytes(),
        );
        objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        let pixels = vec![0x80u8; 64 * 64];
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Length {} >>\nstream\n",
            pixels.len()
        )
        .into_bytes();
        image.extend_from_slice(&pixels);
        image.extend_from_slice(b"\nendstream");
        objects.push(image);
        for (i, page) in pages.iter().enumerate() {
            let content = match page {
                Page::Text(lines) => {
                    let mut stream = String::from("BT /F1 12 Tf 72 720 Td 14 TL\n");
                    for line in *lines {
                        let escaped = line
                            .replace('\\', "\\\\")
                            .replace('(', "\\(")
                            .replace(')', "\\)");
                        stream.push_str(&format!("({escaped}) Tj T*\n"));
                    }
                    stream.push_str("ET");
                    stream
                }
                Page::Picture => "q 400 0 0 400 100 300 cm /Im1 Do Q".to_owned(),
            };
            objects.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                     /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 4 0 R >> >> \
                     /Contents {} 0 R >>",
                    6 + 2 * i
                )
                .into_bytes(),
            );
            objects.push(
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
                .into_bytes(),
            );
        }
        let mut trailer_extra = String::new();
        if encrypted {
            // A standard handler whose user password isn't empty: these O and U values match
            // no password, so it can't be opened without one.
            objects.push(
                format!(
                    "<< /Filter /Standard /V 1 /R 2 /O <{}> /U <{}> /P -4 >>",
                    "11".repeat(32),
                    "22".repeat(32)
                )
                .into_bytes(),
            );
            trailer_extra = format!(
                " /Encrypt {} 0 R /ID [<{}> <{}>]",
                objects.len(),
                "33".repeat(16),
                "33".repeat(16)
            );
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(object);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R{trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::{Page, pdf};
    use super::*;

    const FIRST: &[&str] = &[
        "Ledger batch review",
        "Lag on the ledger-events group peaked at forty thousand messages.",
    ];
    const SECOND: &[&str] = &["Next steps: cap the batch size and alert on lag earlier."];

    #[test]
    fn text_pages_are_read_and_scans_are_told_apart() {
        let bytes = pdf(
            &[Page::Text(FIRST), Page::Picture, Page::Text(SECOND)],
            false,
        );
        let read = read(&bytes, 20).unwrap();
        assert_eq!(read.page_count, 3);
        match &read.pages[0] {
            PageText::Text(text) => {
                assert!(text.contains("Ledger batch review"), "{text:?}");
                assert!(text.contains("forty thousand"), "{text:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(read.pages[1], PageText::Scanned);
        assert_eq!(read.scanned(), [1]);
        let text = read.text();
        assert!(
            text.starts_with("## Page 1\n\nLedger batch review"),
            "{text}"
        );
        assert!(text.contains(
            "## Page 2\n\n(A scanned page, attached as an image.)\n\n## Page 3\n\nNext steps"
        ));
    }

    #[test]
    fn only_the_pages_asked_for_are_read() {
        let pages: Vec<Page> = (0..25).map(|_| Page::Text(SECOND)).collect();
        let read = read(&pdf(&pages, false), 20).unwrap();
        assert_eq!(read.page_count, 25);
        assert_eq!(read.pages.len(), 20);
    }

    #[test]
    fn locked_and_broken_files_say_why() {
        let locked = pdf(&[Page::Text(FIRST)], true);
        assert_eq!(read(&locked, 20), Err("it needs a password".to_owned()));
        assert_eq!(
            read(b"%PDF-1.4\nnot really", 20),
            Err("it couldn't be read".to_owned())
        );
        let mut cut = pdf(&[Page::Text(FIRST)], false);
        cut.truncate(cut.len() / 2);
        assert!(read(&cut, 20).is_err());
    }

    #[test]
    fn a_scanned_page_is_rendered_by_windows() {
        let bytes = pdf(&[Page::Text(FIRST), Page::Picture], false);
        let rendered = render(&bytes, &[1], 1568).unwrap();
        assert_eq!(rendered.len(), 1);
        let png = &rendered[0];
        assert!(png.starts_with(b"\x89PNG"));
        // Width and height from the IHDR chunk: US Letter, its long side 1,568 px.
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!(width, 1212);
        assert_eq!(height, 1568);
    }
}
