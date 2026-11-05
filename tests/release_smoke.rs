//! Runs the release fastcapture.exe on inputs that break parsers. Release builds differ from
//! test builds in how panics end, so this is the only place that behaviour is checked.
//! `cargo build --release`, then `cargo test --release --test release_smoke -- --ignored`.

use std::path::PathBuf;
use std::process::Command;

fn exe() -> PathBuf {
    std::env::var_os("FASTCAPTURE_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(r"target\release\fastcapture.exe")
        })
}

fn extract(fixture: &str) -> (Option<i32>, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(r"tests\fixtures\bad")
        .join(fixture);
    let exe = exe();
    assert!(exe.is_file(), "build it first: {}", exe.display());
    // A recovered panic is logged: the log goes to a scratch folder, not the developer's own.
    let profile = std::env::temp_dir().join(format!(
        "fastcapture-smoke-{}-{}",
        std::process::id(),
        fixture.replace('.', "-")
    ));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(profile.join("Local")).unwrap();
    std::fs::create_dir_all(profile.join("Roaming")).unwrap();
    let output = Command::new(exe)
        .arg("extract")
        .arg(path)
        .env("LOCALAPPDATA", profile.join("Local"))
        .env("APPDATA", profile.join("Roaming"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&profile);
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
#[ignore = "needs the release exe"]
fn a_pdf_that_panics_its_parser_is_unreadable_not_fatal() {
    let (code, out) = extract("panics.pdf");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("unreadable:"), "{out}");
}

#[test]
#[ignore = "needs the release exe"]
fn other_broken_inputs_end_cleanly() {
    for fixture in [
        "truncated.pdf",
        "not-a-zip.docx",
        "nested.html",
        "word-list-level.html",
        // Overflows inside lopdf, which wraps rather than panics in a release build.
        "pdf-offset-overflow.pdf",
    ] {
        let (code, out) = extract(fixture);
        assert_eq!(code, Some(0), "{fixture}: {out}");
    }
}
