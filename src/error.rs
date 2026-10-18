//! The error type shared by FastCapture's modules.

use std::fmt;
use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// A file operation failed on `path`.
    Io { path: PathBuf, source: io::Error },
    /// A Win32 call failed with `GetLastError()` or an `HRESULT`.
    Win32 { call: &'static str, code: u32 },
    /// Anything else, already worded for the user.
    Message(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Win32 { call, code } => write!(f, "{call} failed (0x{code:08X})"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_what_failed() {
        let error = Error::io(r"C:\notes\raw", io::Error::from(io::ErrorKind::NotFound));
        assert!(error.to_string().starts_with(r"C:\notes\raw: "));
        let error = Error::Win32 {
            call: "RegisterHotKey",
            code: 1409,
        };
        assert_eq!(error.to_string(), "RegisterHotKey failed (0x00000581)");
        assert_eq!(
            Error::Message("no notebook".into()).to_string(),
            "no notebook"
        );
    }
}
