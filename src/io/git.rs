//! Minimal git introspection used by the actor-identity layer.
//!
//! Frame's per-clone actor token lives in a gitignored file. To keep every git
//! *worktree* of a clone on one shared identity (rather than each worktree
//! auto-claiming its own token on first mint), the shared token is stored under
//! the git **common directory** — which `git rev-parse --git-common-dir`
//! resolves to the *same* path from the main working tree and every linked
//! worktree.
//!
//! The primary's `null` token predates that file and lives only in the main
//! working tree's local `frame/.actor`, so a linked worktree also needs to
//! locate its clone's **main working tree** ([`main_worktree_frame_dir`]) to
//! inherit that token instead of auto-claiming a new one, and to tell a linked
//! worktree from the main one at all ([`worktree_kind`]).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The three repo paths the actor layer needs, all absolute and canonicalized.
#[derive(Debug, Clone)]
pub struct RepoPaths {
    /// This working tree's own git dir (`<main>/.git/worktrees/<name>` in a
    /// linked worktree, `<root>/.git` in the main one).
    pub git_dir: PathBuf,
    /// The git dir shared by every worktree of this clone.
    pub common_dir: PathBuf,
    /// This working tree's root.
    pub toplevel: PathBuf,
}

/// Which working tree of a clone `frame_dir` lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeKind {
    /// The clone's main working tree (`git_dir == common_dir`).
    Main,
    /// A linked worktree (`git worktree add`). `main_root` is the clone's main
    /// working tree, or `None` when there isn't one to point at (a bare or
    /// `--separate-git-dir` repo).
    Linked { main_root: Option<PathBuf> },
    /// Not inside a git repository (or `git` is unavailable).
    NotGit,
}

/// Resolve [`RepoPaths`] for the repo containing `frame_dir`, or `None` when it
/// is not inside a git repository (or `git` is unavailable).
pub fn repo_paths(frame_dir: &Path) -> Option<RepoPaths> {
    // Run git from the project root (the parent of `frame/`). Relative results
    // (e.g. `.git` from the main worktree) resolve against that root.
    let root = frame_dir.parent()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--absolute-git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines();
    let mut next = || -> Option<PathBuf> {
        let raw = lines.next()?.trim();
        if raw.is_empty() {
            return None;
        }
        let path = PathBuf::from(raw);
        let abs = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        abs.canonicalize().ok()
    };
    Some(RepoPaths {
        git_dir: next()?,
        common_dir: next()?,
        toplevel: next()?,
    })
}

/// The absolute git common directory for the repo containing `frame_dir`, or
/// `None` when `frame_dir` is not inside a git repository (or `git` is
/// unavailable). All worktrees of one clone share a single common dir, so a file
/// placed there is visible to every worktree and to no other clone.
pub fn git_common_dir(frame_dir: &Path) -> Option<PathBuf> {
    repo_paths(frame_dir).map(|p| p.common_dir)
}

/// Which working tree of its clone `frame_dir` lives in. A linked worktree
/// reports the main working tree's root, derived from the common dir
/// (`<main>/.git` → `<main>`) and confirmed by checking that the candidate's
/// `.git` is that same common dir — so a bare or `--separate-git-dir` repo
/// yields `Linked { main_root: None }` rather than a guessed path.
pub fn worktree_kind(frame_dir: &Path) -> WorktreeKind {
    let Some(paths) = repo_paths(frame_dir) else {
        return WorktreeKind::NotGit;
    };
    if paths.git_dir == paths.common_dir {
        return WorktreeKind::Main;
    }
    let main_root = paths.common_dir.parent().filter(|root| {
        root.join(".git")
            .canonicalize()
            .is_ok_and(|g| g == paths.common_dir)
    });
    WorktreeKind::Linked {
        main_root: main_root.map(|r| r.to_path_buf()),
    }
}

/// The *main* working tree's copy of this project's frame directory, when
/// `frame_dir` is inside a linked worktree. `None` from the main worktree
/// itself, outside git, or when the main tree has no frame directory at the same
/// repo-relative path (a bare repo, or a project that only exists on this
/// worktree's branch).
pub fn main_worktree_frame_dir(frame_dir: &Path) -> Option<PathBuf> {
    let WorktreeKind::Linked { main_root } = worktree_kind(frame_dir) else {
        return None; // main worktree (its frame dir is the one we were given) or not git
    };
    let main_root = main_root?;
    let paths = repo_paths(frame_dir)?;
    let rel = frame_dir
        .canonicalize()
        .ok()?
        .strip_prefix(&paths.toplevel)
        .ok()?
        .to_path_buf();
    let candidate = main_root.join(rel);
    candidate.is_dir().then_some(candidate)
}

/// Run `git` in `toplevel` over `rel_paths` and collect the repo-relative paths
/// it prints. `None` when git is unavailable or errors out (exit 128); an exit
/// status of 1 with no output is a legitimate empty result, not a failure —
/// `check-ignore` uses it to mean "nothing matched".
fn git_paths(toplevel: &Path, args: &[&str], rel_paths: &[String]) -> Option<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .args(args)
        .arg("--")
        .args(rel_paths)
        .output()
        .ok()?;
    if output.status.code() == Some(128) {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.lines().map(|l| l.trim().to_string()).collect())
}

/// Which of `rel_paths` (repo-relative) git already **tracks** — i.e. they are in
/// the index and their contents are committed (or staged) into shared history.
pub fn tracked_paths(toplevel: &Path, rel_paths: &[String]) -> Option<Vec<String>> {
    git_paths(toplevel, &["ls-files", "--cached"], rel_paths)
}

/// Which of `rel_paths` (repo-relative) `.gitignore` covers. A *tracked* path is
/// reported as not ignored — which is the honest answer for "will this get
/// committed?", since ignore rules don't apply to files already in the index.
pub fn ignored_paths(toplevel: &Path, rel_paths: &[String]) -> Option<Vec<String>> {
    git_paths(toplevel, &["check-ignore"], rel_paths)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(cwd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Build `<tmp>/main` (a git repo with one commit) and a linked worktree
    /// `<tmp>/wt`, each containing an empty `frame/` directory. Returns the two
    /// frame dirs `(main, worktree)`, or `None` when git is unavailable — every
    /// caller skips its assertions in that case rather than failing.
    pub(crate) fn repo_with_worktree(tmp: &Path) -> Option<(PathBuf, PathBuf)> {
        let main = tmp.join("main");
        std::fs::create_dir_all(&main).ok()?;
        if !git(&main, &["init", "-q"]) {
            return None;
        }
        // `git worktree add` needs a commit to point the new tree at.
        let committed = git(
            &main,
            &[
                "-c",
                "user.name=frame-test",
                "-c",
                "user.email=frame@test.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        if !committed {
            return None;
        }
        let wt = tmp.join("wt");
        if !git(&main, &["worktree", "add", "-q", "--detach", wt.to_str()?]) {
            return None;
        }
        let main_frame = main.join("frame");
        let wt_frame = wt.join("frame");
        std::fs::create_dir_all(&main_frame).ok()?;
        std::fs::create_dir_all(&wt_frame).ok()?;
        Some((main_frame, wt_frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn worktree_kind_distinguishes_main_from_linked() {
        let tmp = TempDir::new().unwrap();
        let Some((main_frame, wt_frame)) = testutil::repo_with_worktree(tmp.path()) else {
            return; // git unavailable
        };
        assert_eq!(worktree_kind(&main_frame), WorktreeKind::Main);
        match worktree_kind(&wt_frame) {
            WorktreeKind::Linked { main_root } => {
                let expected = main_frame.parent().unwrap().canonicalize().unwrap();
                assert_eq!(main_root.map(|r| r.canonicalize().unwrap()), Some(expected));
            }
            other => panic!("expected Linked, got {other:?}"),
        }
    }

    #[test]
    fn main_worktree_frame_dir_resolves_only_from_a_linked_worktree() {
        let tmp = TempDir::new().unwrap();
        let Some((main_frame, wt_frame)) = testutil::repo_with_worktree(tmp.path()) else {
            return;
        };
        // From the main tree there is nothing to point at.
        assert_eq!(main_worktree_frame_dir(&main_frame), None);
        // From the linked worktree it resolves to the main tree's frame dir.
        assert_eq!(
            main_worktree_frame_dir(&wt_frame).map(|p| p.canonicalize().unwrap()),
            Some(main_frame.canonicalize().unwrap())
        );
    }

    #[test]
    fn non_git_project_has_no_repo_paths() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        // A bare directory outside any repo: no common dir, no worktree kind.
        // (Guard against the tempdir itself sitting inside a repo.)
        if repo_paths(&frame_dir).is_none() {
            assert_eq!(worktree_kind(&frame_dir), WorktreeKind::NotGit);
            assert_eq!(git_common_dir(&frame_dir), None);
            assert_eq!(main_worktree_frame_dir(&frame_dir), None);
        }
    }
}
