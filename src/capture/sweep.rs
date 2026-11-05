//! Temp files a crash left behind by the atomic saver.
//!
//! The saver writes `.<name>.<pid>.<n>.fastcapture-tmp` next to `<name>`, syncs it, then swaps it
//! in. A crash between the two leaves the temp behind, and for a raw source whose file never
//! appeared the temp is the only copy of the capture. At startup each temp is put back, removed
//! or left alone by `decide`; anything uncertain is left alone.

use crate::index::scan::{NOTE_LIMIT, is_note, scan};
use crate::notebook::Conventions;
use crate::platform::{process, wide_path};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

const SUFFIX: &str = ".fastcapture-tmp";

/// A temp younger than this may belong to a save still under way, whoever its owner.
const SETTLED: Duration = Duration::from_secs(10 * 60);

/// What the sweep does with one temp file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Move it in under its target's name.
    Restore,
    /// Remove it.
    Delete,
    /// Leave it where it is.
    Skip,
}

/// How many temps a sweep put back and removed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Swept {
    pub restored: usize,
    pub deleted: usize,
}

impl std::ops::AddAssign for Swept {
    fn add_assign(&mut self, other: Self) {
        self.restored += other.restored;
        self.deleted += other.deleted;
    }
}

/// For a saver temp `.<name>.<pid>.<n>.fastcapture-tmp`, the sibling `<name>` it was written for
/// and the id of the process that wrote it. `None` for any other file.
pub fn temp_target(path: &Path) -> Option<(PathBuf, u32)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix('.')?.strip_suffix(SUFFIX)?;
    let (rest, counter) = rest.rsplit_once('.')?;
    let (target, pid) = rest.rsplit_once('.')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if target.is_empty() || target == "." || target == ".." || !digits(counter) || !digits(pid) {
        return None;
    }
    counter.parse::<u32>().ok()?;
    Some((path.with_file_name(target), pid.parse().ok()?))
}

/// The fate of one temp: left alone while its owner runs or it is younger than ten minutes,
/// removed when its target exists or it holds an attachment, and otherwise put back, since then
/// it is the only copy.
pub fn decide(
    target_exists: bool,
    in_attachments: bool,
    owner_running: bool,
    age: Duration,
) -> Action {
    if owner_running || age < SETTLED {
        Action::Skip
    } else if target_exists || in_attachments {
        Action::Delete
    } else {
        Action::Restore
    }
}

/// Sweeps the temps directly in each folder (not its subfolders); `true` marks an attachments
/// folder. A temp whose target, age or owner can't be told is left alone.
pub fn sweep(folders: &[(PathBuf, bool)]) -> Swept {
    let mut swept = Swept::default();
    for (folder, in_attachments) in folders {
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let temp = entry.path();
            let Some((target, owner)) = temp_target(&temp) else {
                continue;
            };
            let Ok(target_exists) = target.try_exists() else {
                continue;
            };
            let Some(age) = age_of(&temp) else {
                continue;
            };
            // A note saved into an attachments folder (one at the notebook's root, say) is still
            // a note: its temp is put back like any other, never thrown away as an attachment.
            let attachment = *in_attachments && !is_note(&target);
            match decide(target_exists, attachment, process::running(owner), age) {
                Action::Skip => {}
                Action::Delete => match std::fs::remove_file(&temp) {
                    Ok(()) => swept.deleted += 1,
                    Err(error) => crate::log::line(&format!(
                        "sweep: a temp file was not removed: error {}",
                        error.raw_os_error().unwrap_or(0)
                    )),
                },
                Action::Restore => match restore(&temp, &target) {
                    Ok(()) => swept.restored += 1,
                    Err(code) => crate::log::line(&format!(
                        "sweep: a temp file was not put back: error {code}"
                    )),
                },
            }
        }
    }
    swept
}

/// The folders a notebook's saves go to: its raw folder, its capture folder and the attachments
/// folder for a raw source (marked `true`), each absolute and listed once. A folder that is both
/// counts as not an attachments folder, so a raw source's temp there is put back, never removed.
pub fn folders_of(conventions: &Conventions) -> Vec<(PathBuf, bool)> {
    let root = &conventions.root;
    let raw = conventions.raw_folder();
    let candidates = [
        (root.join(raw), false),
        (root.join(conventions.capture_folder()), false),
        (
            root.join(conventions.attachments_folder_for(&raw.join("x.md"))),
            true,
        ),
    ];
    let mut folders: Vec<(PathBuf, bool)> = Vec::new();
    for (folder, in_attachments) in candidates {
        match folders
            .iter_mut()
            .find(|(seen, _)| same_folder(seen, &folder))
        {
            Some(seen) => seen.1 &= in_attachments,
            None => folders.push((folder, in_attachments)),
        }
    }
    folders
}

/// Sweeps one notebook: the folders its saves go to, then every other folder in it that holds a
/// temp, none of them taken for an attachments folder.
pub fn sweep_notebook(conventions: &Conventions) -> Swept {
    let mut swept = sweep(&folders_of(conventions));
    if let Ok(found) = scan(&conventions.root, &[], NOTE_LIMIT) {
        let mut parents: Vec<(PathBuf, bool)> = Vec::new();
        for temp in &found.temps {
            let parent = conventions
                .root
                .join(temp.parent().unwrap_or(Path::new("")));
            if !parents.iter().any(|(seen, _)| same_folder(seen, &parent)) {
                parents.push((parent, false));
            }
        }
        swept += sweep(&parents);
    }
    swept
}

/// Whether two folders are the same, ignoring case and a trailing separator.
fn same_folder(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| -> Vec<String> {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    key(a) == key(b)
}

/// How long ago `path` was last written; `None` when that can't be read or lies in the future.
fn age_of(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

/// Moves `temp` in as `target`, failing rather than replacing a `target` that exists.
fn restore(temp: &Path, target: &Path) -> Result<(), u32> {
    let temp_wide = wide_path(temp);
    let target_wide = wide_path(target);
    let ok = unsafe {
        MoveFileExW(
            temp_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::scan::tests::Scratch;
    use crate::notebook::capture_ini::CaptureSettings;
    use crate::notebook::notebook_ini::NotebookSettings;

    /// Sets every file under `folder` back by `by`.
    fn age(folder: &Path, by: Duration) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                age(&path, by);
            } else {
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(SystemTime::now() - by)
                    .unwrap();
            }
        }
    }

    #[test]
    fn temp_names_give_their_target_and_owner() {
        assert_eq!(
            temp_target(Path::new(
                r"D:\n\raw\.2026-11-05 1200 lag.md.4242.7.fastcapture-tmp"
            )),
            Some((PathBuf::from(r"D:\n\raw\2026-11-05 1200 lag.md"), 4242))
        );
        assert_eq!(temp_target(Path::new(r"D:\n\raw\lag.md")), None);
        assert_eq!(
            temp_target(Path::new(r"D:\n\raw\.x.notapid.1.fastcapture-tmp")),
            None
        );
    }

    #[test]
    fn the_sweep_restores_deletes_and_skips_by_the_rules() {
        let old = Duration::from_secs(3600);
        assert!(matches!(decide(false, false, false, old), Action::Restore));
        assert!(matches!(decide(false, true, false, old), Action::Delete));
        assert!(matches!(decide(true, false, false, old), Action::Delete));
        assert!(matches!(decide(false, false, true, old), Action::Skip));
        assert!(matches!(
            decide(false, false, false, Duration::from_secs(60)),
            Action::Skip
        ));
    }

    #[test]
    fn a_whole_raw_source_left_as_a_temp_comes_back() {
        let scratch = Scratch::new("sweep");
        // A pid that isn't running: process ids are multiples of 4, so 3 never is.
        scratch.file(
            r"raw\.a.md.3.0.fastcapture-tmp",
            "---\ntype: raw\n---\nhello",
        );
        scratch.file(r"attachments\.b.png.3.0.fastcapture-tmp", "partial");
        age(&scratch.0, Duration::from_secs(3600));
        let swept = sweep(&[
            (scratch.0.join("raw"), false),
            (scratch.0.join("attachments"), true),
        ]);
        assert_eq!((swept.restored, swept.deleted), (1, 1));
        assert!(scratch.0.join(r"raw\a.md").exists());
        assert!(
            !scratch
                .0
                .join(r"attachments\.b.png.3.0.fastcapture-tmp")
                .exists()
        );
    }

    #[test]
    fn a_note_in_an_attachments_folder_comes_back_too() {
        let scratch = Scratch::new("sweep-note-in-attachments");
        scratch.file(r".c.md.3.0.fastcapture-tmp", "a note");
        age(&scratch.0, Duration::from_secs(3600));
        let swept = sweep(&[(scratch.0.clone(), true)]);
        assert_eq!((swept.restored, swept.deleted), (1, 0));
        assert!(scratch.0.join("c.md").exists());
    }

    #[test]
    fn a_young_temp_or_one_this_process_owns_stays_and_a_superseded_one_goes() {
        let scratch = Scratch::new("sweep-kept");
        let own = std::process::id();
        scratch.file(&format!(r"raw\.mine.md.{own}.0.fastcapture-tmp"), "mine");
        scratch.file(r"raw\.done.md.3.0.fastcapture-tmp", "older");
        scratch.file(r"raw\done.md", "saved");
        age(&scratch.0, Duration::from_secs(3600));
        scratch.file(r"raw\.young.md.3.1.fastcapture-tmp", "young");
        let swept = sweep(&[(scratch.0.join("raw"), false)]);
        assert_eq!((swept.restored, swept.deleted), (0, 1));
        assert!(
            scratch
                .0
                .join(format!(r"raw\.mine.md.{own}.0.fastcapture-tmp"))
                .exists()
        );
        assert!(
            scratch
                .0
                .join(r"raw\.young.md.3.1.fastcapture-tmp")
                .exists()
        );
        assert!(!scratch.0.join(r"raw\.done.md.3.0.fastcapture-tmp").exists());
        assert_eq!(
            std::fs::read_to_string(scratch.0.join(r"raw\done.md")).unwrap(),
            "saved"
        );
    }

    #[test]
    fn an_attachments_folder_that_is_also_the_raw_folder_is_not_taken_for_one() {
        let conventions = |attachments: Option<&str>| Conventions {
            root: PathBuf::from(r"D:\n"),
            notebook: NotebookSettings {
                capture_folder: PathBuf::from("notes"),
                attachments_folder: attachments.map(PathBuf::from),
                ..NotebookSettings::default()
            },
            capture: CaptureSettings {
                raw_folder: PathBuf::from("raw"),
                ..CaptureSettings::default()
            },
            obsidian: None,
            warnings: Vec::new(),
        };
        assert_eq!(
            folders_of(&conventions(None)),
            vec![
                (PathBuf::from(r"D:\n\raw"), false),
                (PathBuf::from(r"D:\n\notes"), false),
                (PathBuf::from(r"D:\n\attachments"), true),
            ]
        );
        assert_eq!(
            folders_of(&conventions(Some("RAW"))),
            vec![
                (PathBuf::from(r"D:\n\raw"), false),
                (PathBuf::from(r"D:\n\notes"), false),
            ]
        );
    }

    #[test]
    fn a_notebook_sweep_reaches_temps_in_any_folder_it_walks() {
        let scratch = Scratch::new("sweep-notebook");
        scratch.file(r"projects\deep\.plan.md.3.0.fastcapture-tmp", "plan");
        age(&scratch.0, Duration::from_secs(3600));
        let swept = sweep_notebook(&Conventions::load(&scratch.0));
        assert_eq!((swept.restored, swept.deleted), (1, 0));
        assert!(scratch.0.join(r"projects\deep\plan.md").exists());
    }
}
