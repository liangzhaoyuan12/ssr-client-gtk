//! Windows-only resource embedding (GOAL §11 Phase 8.8 / table C2).
//!
//! On Windows the packaging script (`build-win.ps1`)
//! generates `packaging/windows/app.rc` from `icon.png` (+ the version from
//! `Cargo.toml`); this build script compiles it with MSYS2's `windres` and
//! links the result in, so the exe carries the app icon and a VERSIONINFO
//! block. Everything is best-effort: a missing `app.rc` (source build on a
//! box without ImageMagick) or a missing `windres` only means the exe uses
//! the default icon — it must never fail the build.
//!
//! Nothing happens on Linux/macOS: `CARGO_CFG_TARGET_OS` is not `windows`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/app.rc");
    println!("cargo:rerun-if-changed=packaging/windows/app.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = std::path::Path::new("packaging/windows/app.rc");
    if !rc.exists() {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ssr-client-gtk-resource.o");
    let status = Command::new("windres")
        .args(["-O", "coff", "-i"])
        .arg(rc)
        .arg("-o")
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", out.display());
        }
        other => {
            println!("cargo:warning=windres failed ({other:?}); the exe keeps the default icon")
        }
    }
}
