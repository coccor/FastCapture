//! Frames between `fastcapture add` and the tray app. Each message is a 4-byte
//! little-endian length, then `FCAP`, a version byte, a kind byte and its fields. Strings and
//! paths are UTF-16 (so every Windows path survives), each a 4-byte unit count then the units.

use std::path::PathBuf;

/// The largest message either side accepts: 1 MB of text is 2 MB of UTF-16, plus paths.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

const MAGIC: &[u8; 4] = b"FCAP";
const VERSION: u8 = 1;
const KIND_ADD: u8 = 1;
const KIND_FILED: u8 = 0x81;
const KIND_FAILED: u8 = 0x82;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    /// Capture `text` (URL lines included) and `attachments` into `notebook`, or the tray app's
    /// own notebook.
    Add {
        text: String,
        attachments: Vec<PathBuf>,
        notebook: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reply {
    /// The raw source, absolute.
    Filed(PathBuf),
    /// `exit_code` is what `add` exits with: 1 when filing failed, 2 for a request it refuses.
    Failed { exit_code: u8, message: String },
}

struct Writer(Vec<u8>);

impl Writer {
    fn new(kind: u8) -> Self {
        let mut bytes = vec![0; 4];
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.push(kind);
        Self(bytes)
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn wide(&mut self, units: impl ExactSizeIterator<Item = u16>) {
        self.u32(units.len() as u32);
        for unit in units {
            self.0.extend_from_slice(&unit.to_le_bytes());
        }
    }

    fn text(&mut self, text: &str) {
        self.wide(text.encode_utf16().collect::<Vec<_>>().into_iter());
    }

    fn path(&mut self, path: &std::path::Path) {
        use std::os::windows::ffi::OsStrExt;
        self.wide(
            path.as_os_str()
                .encode_wide()
                .collect::<Vec<_>>()
                .into_iter(),
        );
    }

    fn finish(mut self) -> Result<Vec<u8>, String> {
        let body = self.0.len() - 4;
        if body > MAX_FRAME_BYTES {
            return Err("The capture is too large to send to FastCapture.".to_owned());
        }
        self.0[..4].copy_from_slice(&(body as u32).to_le_bytes());
        Ok(self.0)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if self.bytes.len() < count {
            return Err("truncated frame".to_owned());
        }
        let (head, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn wide(&mut self) -> Result<Vec<u16>, String> {
        let count = self.u32()? as usize;
        let bytes = self.take(count.checked_mul(2).ok_or("bad length")?)?;
        Ok(bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect())
    }

    fn text(&mut self) -> Result<String, String> {
        String::from_utf16(&self.wide()?).map_err(|_| "invalid UTF-16".to_owned())
    }

    fn path(&mut self) -> Result<PathBuf, String> {
        use std::os::windows::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_wide(&self.wide()?)))
    }

    fn end(&self) -> Result<(), String> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err("trailing bytes in frame".to_owned())
        }
    }
}

/// The body after the length prefix, checked for the magic and version; returns its kind.
fn open(body: &[u8]) -> Result<(u8, Reader<'_>), String> {
    let mut reader = Reader { bytes: body };
    if reader.take(4)? != MAGIC {
        return Err("not a FastCapture frame".to_owned());
    }
    if reader.u8()? != VERSION {
        return Err("unsupported frame version".to_owned());
    }
    let kind = reader.u8()?;
    Ok((kind, reader))
}

pub fn encode_request(request: &Request) -> Result<Vec<u8>, String> {
    match request {
        Request::Add {
            text,
            attachments,
            notebook,
        } => {
            let mut writer = Writer::new(KIND_ADD);
            writer.text(text);
            writer.u32(attachments.len() as u32);
            for path in attachments {
                writer.path(path);
            }
            match notebook {
                Some(notebook) => {
                    writer.u8(1);
                    writer.path(notebook);
                }
                None => writer.u8(0),
            }
            writer.finish()
        }
    }
}

/// A request from a frame's body (without the length prefix).
pub fn decode_request(body: &[u8]) -> Result<Request, String> {
    let (kind, mut reader) = open(body)?;
    if kind != KIND_ADD {
        return Err(format!("unknown request kind {kind}"));
    }
    let text = reader.text()?;
    let count = reader.u32()? as usize;
    if count > 10_000 {
        return Err("too many attachments".to_owned());
    }
    let attachments = (0..count)
        .map(|_| reader.path())
        .collect::<Result<Vec<_>, _>>()?;
    let notebook = match reader.u8()? {
        0 => None,
        1 => Some(reader.path()?),
        _ => return Err("bad notebook flag".to_owned()),
    };
    reader.end()?;
    Ok(Request::Add {
        text,
        attachments,
        notebook,
    })
}

pub fn encode_reply(reply: &Reply) -> Vec<u8> {
    let mut writer = match reply {
        Reply::Filed(path) => {
            let mut writer = Writer::new(KIND_FILED);
            writer.path(path);
            writer
        }
        Reply::Failed { exit_code, message } => {
            let mut writer = Writer::new(KIND_FAILED);
            writer.u8(*exit_code);
            // A reply is always small enough: cut an unusually long message.
            let message: String = message.chars().take(4096).collect();
            writer.text(&message);
            writer
        }
    };
    let body = writer.0.len() - 4;
    writer.0[..4].copy_from_slice(&(body as u32).to_le_bytes());
    writer.0
}

pub fn decode_reply(body: &[u8]) -> Result<Reply, String> {
    let (kind, mut reader) = open(body)?;
    let reply = match kind {
        KIND_FILED => Reply::Filed(reader.path()?),
        KIND_FAILED => Reply::Failed {
            exit_code: reader.u8()?,
            message: reader.text()?,
        },
        _ => return Err(format!("unknown reply kind {kind}")),
    };
    reader.end()?;
    Ok(reply)
}

/// The body length a frame's 4-byte prefix announces, when within the limit.
pub fn body_length(prefix: [u8; 4]) -> Result<usize, String> {
    let length = u32::from_le_bytes(prefix) as usize;
    if length > MAX_FRAME_BYTES {
        Err("frame too large".to_owned())
    } else {
        Ok(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(frame: &[u8]) -> &[u8] {
        let length = body_length([frame[0], frame[1], frame[2], frame[3]]).unwrap();
        assert_eq!(length, frame.len() - 4);
        &frame[4..]
    }

    #[test]
    fn requests_round_trip_with_any_windows_path() {
        use std::os::windows::ffi::OsStringExt;
        // An unpaired surrogate: a valid Windows name that isn't valid Unicode.
        let odd = PathBuf::from(std::ffi::OsString::from_wide(&[0x43, 0x3A, 0x5C, 0xD800]));
        let request = Request::Add {
            text: "zăpadă 🦀\nhttps://example.com".to_owned(),
            attachments: vec![PathBuf::from(r"C:\a b\ledger.log"), odd],
            notebook: Some(PathBuf::from(r"D:\Notes")),
        };
        let frame = encode_request(&request).unwrap();
        assert_eq!(decode_request(body(&frame)), Ok(request));
        let bare = Request::Add {
            text: String::new(),
            attachments: Vec::new(),
            notebook: None,
        };
        assert_eq!(
            decode_request(body(&encode_request(&bare).unwrap())),
            Ok(bare)
        );
    }

    #[test]
    fn replies_round_trip() {
        for reply in [
            Reply::Filed(PathBuf::from(r"D:\Notes\raw\x.md")),
            Reply::Failed {
                exit_code: 2,
                message: "big.bin is 21 MB".to_owned(),
            },
        ] {
            assert_eq!(decode_reply(body(&encode_reply(&reply))), Ok(reply));
        }
    }

    #[test]
    fn bad_frames_are_refused() {
        let frame = encode_request(&Request::Add {
            text: "x".to_owned(),
            attachments: Vec::new(),
            notebook: None,
        })
        .unwrap();
        let good = body(&frame).to_vec();
        assert!(
            decode_request(&good[..good.len() - 1]).is_err(),
            "truncated"
        );
        let mut extra = good.clone();
        extra.push(0);
        assert!(decode_request(&extra).is_err(), "trailing");
        let mut magic = good.clone();
        magic[0] = b'X';
        assert!(decode_request(&magic).is_err());
        let mut version = good;
        version[4] = 9;
        assert!(decode_request(&version).is_err());
        assert!(body_length(((MAX_FRAME_BYTES + 1) as u32).to_le_bytes()).is_err());
        let huge = Request::Add {
            text: "x".repeat(MAX_FRAME_BYTES / 2 + 1),
            attachments: Vec::new(),
            notebook: None,
        };
        assert!(encode_request(&huge).is_err());
    }
}
