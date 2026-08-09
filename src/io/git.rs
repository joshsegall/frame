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
    WorktreeKind::Linked {
        main_root: main_root_of(&paths),
    }
}

/// The clone's main working tree, derived from the common dir (`<main>/.git` →
/// `<main>`) and confirmed by checking that the candidate's `.git` *is* that
/// common dir — so a bare or `--separate-git-dir` repo yields `None` rather than a
/// guessed path.
fn main_root_of(paths: &RepoPaths) -> Option<PathBuf> {
    paths
        .common_dir
        .parent()
        .filter(|root| {
            root.join(".git")
                .canonicalize()
                .is_ok_and(|g| g == paths.common_dir)
        })
        .map(|root| root.to_path_buf())
}

/// One working tree of a clone, as `git worktree list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// The working tree's root, absolute and canonicalized where possible.
    pub path: PathBuf,
    /// The short name of the checked-out branch, or `None` when the working tree
    /// is detached or bare.
    pub branch: Option<String>,
}

/// Every working tree of the clone containing `root` — the main one first, then
/// each linked worktree, in git's order. `None` when `root` is not inside a git
/// repository, or git is unavailable.
///
/// One call answers two questions a project listing needs: which registered
/// paths are live worktrees of this clone (git's own answer, so it holds for a
/// worktree beside its parent as well as one nested under it), and what each has
/// checked out — the branch being what identifies a worktree to a person, far
/// more than its path.
pub fn worktree_list(root: &Path) -> Option<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;

    // Porcelain format: blank-line-separated records, each opening with
    // `worktree <path>`. `branch refs/heads/<name>` is absent for a detached or
    // bare tree, which is exactly the `None` case.
    let mut trees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            trees.extend(current.take());
            let path = PathBuf::from(path.trim());
            let path = path.canonicalize().unwrap_or(path);
            current = Some(WorktreeInfo { path, branch: None });
        } else if let Some(git_ref) = line.strip_prefix("branch ")
            && let Some(tree) = current.as_mut()
        {
            let git_ref = git_ref.trim();
            tree.branch = Some(
                git_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(git_ref)
                    .to_string(),
            );
        }
    }
    trees.extend(current);
    Some(trees)
}

/// This working tree's standing within its clone, when it is a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedWorktree {
    /// How a person refers to this working copy: the branch it has checked out,
    /// or its directory name when detached.
    pub label: String,
    /// The clone's main working tree, when there is one to name (see
    /// [`main_root_of`]). This is also where the clone's shared state lives, which
    /// is why it is worth reporting rather than merely implying.
    pub main_root: Option<PathBuf>,
}

/// This working tree's standing within its clone, or `None` from the clone's main
/// working tree and outside git — there, nothing needs distinguishing.
///
/// This exists because every worktree of a clone reports the same project name
/// (`project.toml` is committed), so anything showing only the name cannot say
/// which working copy it is showing.
///
/// One `git` call from the main working tree, which is the common case; a linked
/// one pays a second to learn its branch.
pub fn linked_worktree(frame_dir: &Path) -> Option<LinkedWorktree> {
    let paths = repo_paths(frame_dir)?;
    if paths.git_dir == paths.common_dir {
        return None; // the main working tree
    }
    let branch = worktree_list(&paths.toplevel)
        .and_then(|trees| trees.into_iter().find(|tree| tree.path == paths.toplevel))
        .and_then(|tree| tree.branch);
    // Detached, or a tree git declined to list: the directory name at least
    // differs between the worktrees of one clone. Note the fallbacks never turn
    // into `None` — that answer is reserved for "not a linked worktree", so a
    // caller can trust it.
    let label = branch
        .or_else(|| {
            paths
                .toplevel
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "detached".to_string());
    Some(LinkedWorktree {
        label,
        main_root: main_root_of(&paths),
    })
}

/// How this working tree should name itself when it is not its clone's main one.
/// See [`linked_worktree`], of which this is the label alone.
pub fn linked_worktree_label(frame_dir: &Path) -> Option<String> {
    linked_worktree(frame_dir).map(|w| w.label)
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

/// Which of `paths` `.gitignore` covers. A *tracked* path is reported as not
/// ignored — which is the honest answer for "will this get committed?", since
/// ignore rules don't apply to files already in the index.
///
/// `dir` is where git runs, and `paths` are resolved against it — repo-relative
/// from the toplevel, or project-relative from a project root somewhere inside
/// the repo. Matches come back spelled exactly as they were passed in, so a
/// caller can compare them against what it sent.
pub fn ignored_paths(dir: &Path, paths: &[String]) -> Option<Vec<String>> {
    git_paths(dir, &["check-ignore"], paths)
}

/// Which of `rel_paths` (repo-relative) have unstaged modifications — the working
/// tree copy differs from what is staged in the index.
fn modified_paths(toplevel: &Path, rel_paths: &[String]) -> Option<Vec<String>> {
    git_paths(toplevel, &["diff", "--name-only"], rel_paths)
}

/// Files and directories git leaves in a working tree's git dir while a
/// multi-step operation is unfinished. Each is removed when the operation
/// finishes or is aborted, so their presence means "git is mid-surgery".
const OPERATION_MARKERS: [&str; 6] = [
    "rebase-merge",     // rebase -i, rebase --merge (directory)
    "rebase-apply",     // rebase --apply, am (directory)
    "MERGE_HEAD",       // merge
    "CHERRY_PICK_HEAD", // cherry-pick
    "REVERT_HEAD",      // revert
    "BISECT_LOG",       // bisect
];

/// Whether git is part-way through a multi-step operation (rebase, merge,
/// cherry-pick, revert, bisect, am) on the working tree containing `frame_dir`.
///
/// The markers live in this worktree's own git dir, not the common dir, so a
/// rebase in one worktree does not report as in progress from another.
/// `false` when `frame_dir` is not in a repo, or git is unavailable.
pub fn operation_in_progress(frame_dir: &Path) -> bool {
    let Some(paths) = repo_paths(frame_dir) else {
        return false;
    };
    OPERATION_MARKERS
        .iter()
        .any(|marker| paths.git_dir.join(marker).exists())
}

/// Of `abs_paths`, those whose current contents **git itself wrote**: the path is
/// tracked and byte-identical to the index.
///
/// This is the discriminator between "a human edited this file" and "git put this
/// file here". `git restore`, `git checkout`, `git stash`, and the checkouts a
/// rebase performs all leave a file exactly matching the index; saving from an
/// editor leaves it differing from the index. Untracked files are never
/// reported — `git diff` ignores them, so "no diff" would otherwise be read as
/// "unmodified".
///
/// Empty when `frame_dir` is not in a repo or git is unavailable, which callers
/// should read as "no evidence git did this" rather than as an error.
pub fn index_clean_paths(frame_dir: &Path, abs_paths: &[PathBuf]) -> Vec<PathBuf> {
    let Some(paths) = repo_paths(frame_dir) else {
        return Vec::new();
    };
    // Canonicalize before relativizing: `toplevel` is canonical, and watcher
    // paths need not be (`/tmp` vs `/private/tmp` on macOS). A path that cannot
    // be canonicalized no longer exists, so it is not an index-clean file.
    let relativized: Vec<(String, PathBuf)> = abs_paths
        .iter()
        .filter_map(|p| {
            let canon = p.canonicalize().ok()?;
            let rel = canon.strip_prefix(&paths.toplevel).ok()?.to_str()?;
            Some((rel.to_string(), p.clone()))
        })
        .collect();
    if relativized.is_empty() {
        return Vec::new();
    }
    let rels: Vec<String> = relativized.iter().map(|(rel, _)| rel.clone()).collect();
    let (Some(tracked), Some(modified)) = (
        tracked_paths(&paths.toplevel, &rels),
        modified_paths(&paths.toplevel, &rels),
    ) else {
        return Vec::new();
    };
    relativized
        .into_iter()
        .filter(|(rel, _)| tracked.contains(rel) && !modified.contains(rel))
        .map(|(_, abs)| abs)
        .collect()
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

    /// Add a linked worktree at `at`, checking out a new branch `branch`.
    pub(crate) fn add_worktree(main_root: &Path, at: &Path, branch: &str) -> bool {
        let Some(at) = at.to_str() else { return false };
        git(main_root, &["worktree", "add", "-q", "-b", branch, at])
    }

    /// Build `<tmp>/repo` as a git repo whose `frame/tracks/main.md` is committed
    /// at HEAD with no working-tree changes. Returns the frame dir, or `None`
    /// when git is unavailable.
    pub(crate) fn repo_with_committed_track(tmp: &Path) -> Option<PathBuf> {
        let root = tmp.join("repo");
        let tracks = root.join("frame").join("tracks");
        std::fs::create_dir_all(&tracks).ok()?;
        if !git(&root, &["init", "-q"]) {
            return None;
        }
        std::fs::write(tracks.join("main.md"), "# Main\n\n## Done\n").ok()?;
        if !git(&root, &["add", "."]) {
            return None;
        }
        let committed = git(
            &root,
            &[
                "-c",
                "user.name=frame-test",
                "-c",
                "user.email=frame@test.invalid",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        committed.then(|| root.join("frame"))
    }

    /// Run `git restore` over a path, as a user would to undo an edit.
    pub(crate) fn restore(root: &Path, rel: &str) -> bool {
        git(root, &["restore", "--", rel])
    }

    /// Stage and commit everything in `root`, so the working tree is settled.
    pub(crate) fn commit_all(root: &Path) -> bool {
        git(root, &["add", "-A"])
            && git(
                root,
                &[
                    "-c",
                    "user.name=frame-test",
                    "-c",
                    "user.email=frame@test.invalid",
                    "commit",
                    "-q",
                    "-m",
                    "wip",
                ],
            )
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
    fn worktree_list_reports_every_tree_of_the_clone_with_its_branch() {
        let tmp = TempDir::new().unwrap();
        let Some((main_frame, wt_frame)) = testutil::repo_with_worktree(tmp.path()) else {
            return; // git unavailable
        };
        let main_root = main_frame.parent().unwrap().canonicalize().unwrap();
        let wt_root = wt_frame.parent().unwrap().canonicalize().unwrap();

        // The same list from either tree — the clone is what is being described,
        // not the tree the question was asked from.
        for asked_from in [&main_root, &wt_root] {
            let trees = worktree_list(asked_from).expect("in a repo");
            let paths: Vec<_> = trees.iter().map(|t| t.path.clone()).collect();
            assert_eq!(paths, vec![main_root.clone(), wt_root.clone()]);
            // The main tree is on a branch; the linked one was added detached,
            // which is the `None` case rather than a missing record.
            assert!(trees[0].branch.is_some(), "main tree branch");
            assert_eq!(trees[1].branch, None, "detached worktree");
        }

        // A named branch comes back short, not as `refs/heads/<name>`.
        assert!(testutil::add_worktree(
            &main_root,
            &tmp.path().join("wt2"),
            "feature"
        ));
        let trees = worktree_list(&main_root).expect("in a repo");
        assert!(
            trees.iter().any(|t| t.branch.as_deref() == Some("feature")),
            "named branch reported short: {trees:?}"
        );
    }

    /// Only a linked worktree needs to say which working copy it is.
    #[test]
    fn only_a_linked_worktree_has_a_label() {
        let tmp = TempDir::new().unwrap();
        let Some((main_frame, wt_frame)) = testutil::repo_with_worktree(tmp.path()) else {
            return; // git unavailable
        };
        assert_eq!(
            linked_worktree_label(&main_frame),
            None,
            "main working tree"
        );
        // The test worktree is detached, so it falls back to its directory name.
        assert_eq!(linked_worktree_label(&wt_frame).as_deref(), Some("wt"));

        // On a branch, that is the label — it is what a person calls the worktree.
        let main_root = main_frame.parent().unwrap().canonicalize().unwrap();
        let named = tmp.path().join("wt-named");
        assert!(testutil::add_worktree(&main_root, &named, "feature"));
        std::fs::create_dir_all(named.join("frame")).unwrap();
        assert_eq!(
            linked_worktree_label(&named.join("frame")).as_deref(),
            Some("feature")
        );
    }

    #[test]
    fn worktree_list_is_none_outside_a_repo() {
        let tmp = TempDir::new().unwrap();
        if repo_paths(&tmp.path().join("frame")).is_none() {
            assert_eq!(worktree_list(tmp.path()), None);
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
            // Outside a repo there is no evidence git did anything, so auto-clean
            // keeps its pre-existing behaviour rather than being suppressed.
            assert!(!operation_in_progress(&frame_dir));
            assert!(index_clean_paths(&frame_dir, &[frame_dir.join("tracks/main.md")]).is_empty());
        }
    }

    #[test]
    fn operation_in_progress_detects_file_and_directory_markers() {
        let tmp = TempDir::new().unwrap();
        let Some(frame_dir) = testutil::repo_with_committed_track(tmp.path()) else {
            return; // git unavailable
        };
        assert!(!operation_in_progress(&frame_dir), "settled repo");

        let git_dir = repo_paths(&frame_dir).unwrap().git_dir;

        // A merge leaves a marker *file*.
        std::fs::write(git_dir.join("MERGE_HEAD"), "").unwrap();
        assert!(operation_in_progress(&frame_dir), "merge in progress");
        std::fs::remove_file(git_dir.join("MERGE_HEAD")).unwrap();
        assert!(!operation_in_progress(&frame_dir), "merge finished");

        // A rebase leaves a marker *directory* — `.exists()` covers both.
        std::fs::create_dir(git_dir.join("rebase-merge")).unwrap();
        assert!(operation_in_progress(&frame_dir), "rebase in progress");
    }

    /// The discriminator behind auto-clean suppression: an editor save must look
    /// different from git putting a file back.
    #[test]
    fn index_clean_paths_separates_git_writes_from_editor_writes() {
        let tmp = TempDir::new().unwrap();
        let Some(frame_dir) = testutil::repo_with_committed_track(tmp.path()) else {
            return; // git unavailable
        };
        let root = frame_dir.parent().unwrap().to_path_buf();
        let track = frame_dir.join("tracks").join("main.md");

        // Committed and untouched: git owns this content.
        assert_eq!(
            index_clean_paths(&frame_dir, std::slice::from_ref(&track)),
            vec![track.clone()],
            "unmodified tracked file"
        );

        // Hand-edited: a person owns this content, so auto-clean should run.
        std::fs::write(&track, "# Main\n\n## Done\n\n- [x] `M-1` Ticked\n").unwrap();
        assert!(
            index_clean_paths(&frame_dir, std::slice::from_ref(&track)).is_empty(),
            "edited file must not read as a git write"
        );

        // `git restore` — the exact command that fought the TUI — puts it back.
        assert!(testutil::restore(&root, "frame/tracks/main.md"));
        assert_eq!(
            index_clean_paths(&frame_dir, std::slice::from_ref(&track)),
            vec![track],
            "restored file reads as a git write"
        );

        // Untracked files are never reported: `git diff` shows them no diff, and
        // reading that as "unmodified" would suppress cleaning brand-new tracks.
        let untracked = frame_dir.join("tracks").join("new.md");
        std::fs::write(&untracked, "# New\n").unwrap();
        assert!(
            index_clean_paths(&frame_dir, &[untracked]).is_empty(),
            "untracked file"
        );
    }
}
