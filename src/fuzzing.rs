//! Entry points for fuzzing the parsers that read what FastCapture didn't write: a note's front
//! matter, a fetched page, a model's reply, the Copilot CLI's messages and a PDF. Each takes any
//! bytes and throws the result away; a panic, a hang or a stack overflow is the finding.

/// A note's front matter, read from the bytes as lossy UTF-8.
pub fn front_matter(data: &[u8]) {
    let _ = crate::notebook::frontmatter::parse(&String::from_utf8_lossy(data));
}

/// A fetched page, read for its main text and metadata.
pub fn page(data: &[u8]) {
    let _ =
        crate::extract::page::extract(&String::from_utf8_lossy(data), "https://example.invalid/");
}

/// A model's reply, read as a note.
pub fn reply(data: &[u8]) {
    let _ = crate::note::reply::parse(&String::from_utf8_lossy(data));
}

/// The Copilot CLI's output, split into messages until none is whole or one is malformed.
pub fn rpc(data: &[u8]) {
    let mut decoder = crate::model::copilot::rpc::Decoder::default();
    decoder.push(data);
    while let Some(message) = decoder.message() {
        if message.is_err() {
            break;
        }
    }
}

/// A PDF, read without the guard that turns a panic in the parser into an error.
pub fn pdf(data: &[u8]) {
    let _ = crate::extract::pdf::read_unguarded(data, 20);
}

/// A target: any bytes in, nothing out.
pub type Target = fn(&[u8]);

/// Every target, by name.
pub const TARGETS: &[(&str, Target)] = &[
    ("front_matter", front_matter),
    ("page", page),
    ("reply", reply),
    ("rpc", rpc),
    ("pdf", pdf),
];
