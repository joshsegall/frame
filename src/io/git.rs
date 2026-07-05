//! Minimal git introspection used by the actor-identity layer.
//!
//! Frame's per-clone actor token lives in a gitignored file. To keep every git
//! *worktree* of a clone on one shared identity (rather than each worktree
//! auto-claiming its own token on first mint), the shared token is stored under
//! the git **common directory** — which `git rev-parse --git-common-dir`
//! resolves to the *same* path from the main working tree and every linked
//! worktree.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The absolute git common directory for the repo containing `frame_dir`, or
/// `None` when `frame_dir` is not inside a git repository (or `git` is
/// unavailable). All worktrees of one clone share a single common dir, so a file
/// placed there is visible to every worktree and to no other clone.
pub fn git_common_dir(frame_dir: &Path) -> Option<PathBuf> {
    // Run git from the project root (the parent of `frame/`). A relative result
    // (e.g. `.git` from the main worktree) is resolved against that root; a
    // linked worktree already yields an absolute path to the main `.git`.
    let root = frame_dir.parent()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    let abs = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    abs.canonicalize().ok()
}
