//! What the log says about the PC: the Windows build, and what kind of place a notebook is in.

use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

/// `GetDriveTypeW`'s answer for a network drive.
const DRIVE_REMOTE: u32 = 4;

/// The Windows build, from `RtlGetVersion`, which doesn't lie about the version as
/// `GetVersionEx` does to programs without a manifest naming it.
pub fn windows_build() -> Option<u32> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    #[repr(C)]
    struct VersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }
    type RtlGetVersion = unsafe extern "system" fn(*mut VersionInfo) -> i32;
    let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    unsafe {
        let ntdll = GetModuleHandleW(name.as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let function = GetProcAddress(ntdll, c"RtlGetVersion".as_ptr().cast())?;
        let function: RtlGetVersion = std::mem::transmute(function);
        let mut info: VersionInfo = std::mem::zeroed();
        info.size = size_of::<VersionInfo>() as u32;
        (function(&mut info) == 0).then_some(info.build)
    }
}

/// What kind of place the notebook at `path` is in: `missing`, `network`, `onedrive` or `local`.
pub fn notebook_kind(path: &Path) -> &'static str {
    kind_of(
        path.exists(),
        drive_type(path),
        under_onedrive(path, &onedrive_roots()),
    )
}

/// The kind from what was found: whether the folder exists, its drive's `GetDriveTypeW` value,
/// and whether it is under a OneDrive folder.
fn kind_of(exists: bool, drive_type: u32, under_onedrive: bool) -> &'static str {
    if !exists {
        "missing"
    } else if drive_type == DRIVE_REMOTE {
        "network"
    } else if under_onedrive {
        "onedrive"
    } else {
        "local"
    }
}

/// The drive type of `path`'s root, 0 when it has none.
fn drive_type(path: &Path) -> u32 {
    let mut root = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => root.push(component),
            Component::RootDir => {
                root.push(component);
                break;
            }
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        return 0;
    }
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain([0]).collect();
    unsafe { GetDriveTypeW(wide.as_ptr()) }
}

fn onedrive_roots() -> Vec<String> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect()
}

/// Whether `path` is one of `roots` or inside one, ignoring case and `/` against `\`.
fn under_onedrive(path: &Path, roots: &[String]) -> bool {
    let fold = |text: &str| text.to_lowercase().replace('/', "\\");
    let path = fold(&path.to_string_lossy());
    roots.iter().any(|root| {
        let root = fold(root);
        let root = root.trim_end_matches('\\');
        !root.is_empty()
            && path
                .strip_prefix(root)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notebook_is_one_kind() {
        assert_eq!(kind_of(false, DRIVE_REMOTE, true), "missing");
        assert_eq!(kind_of(true, DRIVE_REMOTE, false), "network");
        assert_eq!(kind_of(true, 3, true), "onedrive");
        assert_eq!(kind_of(true, 3, false), "local");
    }

    #[test]
    fn onedrive_is_matched_by_folder_not_by_prefix() {
        let roots = ["C:\\Users\\Me\\OneDrive".to_owned()];
        assert!(under_onedrive(
            Path::new("c:/users/me/onedrive/Notes"),
            &roots
        ));
        assert!(!under_onedrive(
            Path::new("C:\\Users\\Me\\OneDrive2\\Notes"),
            &roots
        ));
        assert!(!under_onedrive(Path::new("C:\\Notes"), &[]));
    }

    #[test]
    fn this_pcs_build_is_known() {
        assert!(windows_build().is_some());
        assert_eq!(notebook_kind(Path::new("C:\\no\\such\\folder")), "missing");
    }
}
