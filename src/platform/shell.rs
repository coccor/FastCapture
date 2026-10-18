//! The Windows shell: opening a file with its default program, and known folders.

use super::{last_error, wide_path};
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, ShellExecuteW};

/// `SW_SHOWNORMAL`.
const SHOW_NORMAL: i32 = 1;

/// Opens `path` with its default program (the default editor for `.md` and `.ini`, Explorer for a
/// folder).
pub fn open(path: &Path) -> crate::Result<()> {
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file = wide_path(path);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SHOW_NORMAL,
        )
    };
    // ShellExecuteW reports success as a value over 32.
    if result as usize > 32 {
        Ok(())
    } else {
        Err(last_error("ShellExecuteW"))
    }
}

/// The user's Documents folder, wherever it is redirected to.
pub fn documents_folder() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut raw = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut raw) };
    if result != 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return None;
    }
    let length = (0..)
        .take_while(|&index| unsafe { *raw.add(index) } != 0)
        .count();
    let path = std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_documents_folder_is_found() {
        let documents = super::documents_folder().unwrap();
        assert!(documents.is_absolute(), "{}", documents.display());
    }
}
