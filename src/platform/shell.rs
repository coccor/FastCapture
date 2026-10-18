//! The Windows shell: opening a file with its default program or in FastPad, the Recycle Bin,
//! and known folders.

use super::{last_error, wide_path};
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, ShellExecuteW};

/// `SW_SHOWNORMAL`.
const SHOW_NORMAL: i32 = 1;

/// Opens `path` with its default program (the default editor for `.md` and `.ini`, Explorer for a
/// folder).
pub fn open(path: &Path) -> crate::Result<()> {
    execute(path, None)
}

/// Runs `program` with `file` as its argument, in quotes.
pub fn open_with(program: &Path, file: &Path) -> crate::Result<()> {
    execute(program, Some(&format!("\"{}\"", file.display())))
}

/// Opens `path` in FastPad when it is installed, else with the default program.
pub fn open_in_editor(path: &Path) -> crate::Result<()> {
    match find_fastpad() {
        Some(fastpad) => open_with(&fastpad, path),
        None => open(path),
    }
}

/// FastPad's executable, from the `App Paths\FastPad.exe` entry its installer writes (per user,
/// then per machine), when the file is there.
pub fn find_fastpad() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\App Paths\FastPad.exe"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .find_map(|root| {
            let mut buffer = vec![0u16; 1024];
            let mut bytes = (buffer.len() * 2) as u32;
            let status = unsafe {
                RegGetValueW(
                    root,
                    key.as_ptr(),
                    std::ptr::null(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
            if status != 0 {
                return None;
            }
            buffer.truncate((bytes as usize / 2).saturating_sub(1));
            let path = PathBuf::from(String::from_utf16_lossy(&buffer).trim_matches('"'));
            path.is_file().then_some(path)
        })
}

/// Sends `paths` to the Recycle Bin without asking. A path that is already gone is skipped.
pub fn recycle(paths: &[PathBuf]) -> crate::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    let existing: Vec<&PathBuf> = paths.iter().filter(|path| path.exists()).collect();
    if existing.is_empty() {
        return Ok(());
    }
    // A list of NUL-terminated paths, ending with an extra NUL.
    let mut from: Vec<u16> = Vec::new();
    for path in existing {
        from.extend(path.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut operation: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    operation.wFunc = FO_DELETE;
    operation.pFrom = from.as_ptr();
    operation.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
    let status = unsafe { SHFileOperationW(&mut operation) };
    if status != 0 || operation.fAnyOperationsAborted != 0 {
        return Err(crate::Error::Win32 {
            call: "SHFileOperationW",
            code: status as u32,
        });
    }
    Ok(())
}

fn execute(file: &Path, parameters: Option<&str>) -> crate::Result<()> {
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file = wide_path(file);
    let parameters: Option<Vec<u16>> =
        parameters.map(|text| text.encode_utf16().chain(std::iter::once(0)).collect());
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            parameters
                .as_ref()
                .map_or(std::ptr::null(), |wide| wide.as_ptr()),
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
