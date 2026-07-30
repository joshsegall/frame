//! Stamps the build with the commit it came from, for `fr --version` and
//! `fr info` (see `src/version.rs`).
//!
//! A build outside a git checkout — a published crate, a source tarball, a
//! vendored copy — simply doesn't set `FRAME_GIT_HASH`, and the version string
//! falls back to the crate version alone. There is no dirty marker on purpose:
//! Cargo only re-runs this script when one of the `rerun-if-changed` paths below
//! changes, so a "clean" flag computed here would go stale the moment a source
//! file was edited, which is worse than not claiming anything.

use std::path::PathBuf;
use std::process::Command;

fn git(manifest_dir: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    match git(&manifest_dir, &["rev-parse", "--short", "HEAD"]) {
        Some(hash) => {
            println!("cargo:rustc-env=FRAME_GIT_HASH={hash}");
            println!("cargo:rustc-env=FRAME_VERSION_LONG={version} ({hash})");
        }
        None => {
            // Unset, so `option_env!("FRAME_GIT_HASH")` is None.
            println!("cargo:rustc-env=FRAME_VERSION_LONG={version}");
        }
    }

    // Re-stamp when HEAD moves. `HEAD` itself only changes on checkout, so the
    // branch's ref file (or `packed-refs`, if it has been packed) is what changes
    // on a plain commit — watch all three.
    let git_dir = git(&manifest_dir, &["rev-parse", "--absolute-git-dir"]).map(PathBuf::from);
    let common_dir = git(
        &manifest_dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(PathBuf::from);
    if let Some(dir) = &git_dir {
        println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
    }
    if let Some(dir) = &common_dir {
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("packed-refs").display()
        );
        if let Some(head_ref) = git(&manifest_dir, &["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", dir.join(head_ref).display());
        }
    }
}
