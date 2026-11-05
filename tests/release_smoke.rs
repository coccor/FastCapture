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
    let output = Command::new(exe).arg("extract").arg(path).output().unwrap();
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
    for fixture in ["truncated.pdf", "not-a-zip.docx", "nested.html"] {
        let (code, out) = extract(fixture);
        assert_eq!(code, Some(0), "{fixture}: {out}");
    }
}
