//! Embeds the application manifest (Common Controls v6, per-monitor DPI v2, UTF-8 code page)
//! and the icon into the FastCapture executable, as Win32 resources 1 (RT_MANIFEST) and 1
//! (RT_ICON group). The icon is drawn by `src/icon.rs`. Adapted from FastPad's `build.rs`.

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/fastcapture.manifest");
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-env-changed=RC");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    // `cfg!` in a build script describes the host. A non-Windows host (CI's Linux Clippy
    // type-check of the MSVC target) has no rc.exe and never links.
    if !cfg!(windows) {
        println!(
            "cargo:warning=skipping the manifest resource: rc.exe needs a Windows host (type-check only)"
        );
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = manifest_dir.join("assets").join("fastcapture.manifest");
    let script = out_dir.join("fastcapture.rc");
    let resource = out_dir.join("fastcapture.res");

    let ico = out_dir.join("fastcapture.ico");
    std::fs::write(&ico, icon::ico(&[16, 20, 24, 32, 40, 48, 64])).expect("write fastcapture.ico");

    // rc.exe treats backslashes in string literals as escapes. 24 is RT_MANIFEST.
    let manifest_literal = manifest.display().to_string().replace('\\', "/");
    let ico_literal = ico.display().to_string().replace('\\', "/");
    std::fs::write(
        &script,
        format!("1 24 \"{manifest_literal}\"\n1 ICON \"{ico_literal}\"\n"),
    )
    .expect("write fastcapture.rc");

    let rc = find_resource_compiler();
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&script)
        .status()
        .unwrap_or_else(|error| panic!("failed to run '{}': {error}", rc.display()));
    assert!(status.success(), "'{}' failed with {status}", rc.display());

    println!("cargo:rustc-link-arg-bins={}", resource.display());
}

fn find_resource_compiler() -> PathBuf {
    if let Some(rc) = env::var_os("RC") {
        return PathBuf::from(rc);
    }
    if let Some(rc) = env::var_os("PATH")
        .iter()
        .flat_map(env::split_paths)
        .map(|dir| dir.join("rc.exe"))
        .find(|candidate| candidate.is_file())
    {
        return rc;
    }
    let kits = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join(r"Windows Kits\10\bin");
    newest_sdk_rc(&kits).unwrap_or_else(|| {
        panic!(
            "rc.exe was not found on PATH or under '{}'. Install the Windows 10/11 SDK or set RC.",
            kits.display()
        )
    })
}

fn newest_sdk_rc(kits: &Path) -> Option<PathBuf> {
    let host = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    std::fs::read_dir(kits)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let version = entry
                .file_name()
                .to_str()?
                .split('.')
                .map(|part| part.parse::<u32>().ok())
                .collect::<Option<Vec<_>>>()?;
            let rc = entry.path().join(host).join("rc.exe");
            rc.is_file().then_some((version, rc))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, rc)| rc)
}
