//! What a capture holds before it is written.

use std::path::PathBuf;

/// Something attached to a capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attachment {
    /// A pasted or snipped image, already PNG.
    Png(Vec<u8>),
    /// A file to copy in, keeping its name. The original is never moved or changed.
    File(PathBuf),
}

/// One capture: your text (URL lines included), attachments, and where it came from.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Capture {
    pub text: String,
    pub attachments: Vec<Attachment>,
    /// The title of the window in front when the hotkey was pressed, when `capture_source` is on.
    pub source_window: Option<String>,
    /// The page pasted rich text was copied from, as the browser said.
    pub source_url: Option<String>,
}

impl Capture {
    /// Nothing to save: no text and no attachments.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }
}

/// What a capture contains, for its `kind:` and the box's footer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    Text,
    Link,
    /// A pasted or snipped image.
    Screenshot,
    /// An image file that was dropped or attached.
    Image,
    File,
}

impl Kind {
    /// The word written in front matter.
    pub fn key(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Link => "link",
            Self::Screenshot => "screenshot",
            Self::Image => "image",
            Self::File => "file",
        }
    }

    /// The word shown in the box's footer.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Link => "Link",
            Self::Screenshot => "Screenshot",
            Self::Image => "Image",
            Self::File => "File",
        }
    }
}
