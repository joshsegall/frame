//! `fr git setup` — make a clone frame-ready.
//!
//! Three things have to be true for a frame project in git to behave, and
//! historically none of them had a command that made them true:
//!
//! 1. **`.gitignore` covers working-copy-local files.** `fr init` writes the
//!    blanket pattern, but only at init — a project created before an entry
//!    existed never acquires it (see `check_local_files_ignored`).
//! 2. **`.gitattributes` routes frame markdown to the merge driver**, so a merge
//!    goes through [`crate::ops::merge_files`] instead of a line-based merge that
//!    duplicates any task a `fr done` relocated.
//! 3. **`.git/config` registers the driver itself.**
//!
//! Only (1) and (2) are repo content. (3) is per-clone machine state that cannot
//! be committed, which is the reason this is a command a human runs after
//! cloning rather than a file anyone can check in — and the reason `fr check`
//! reports an unregistered driver, since a teammate who clones gets the
//! `.gitattributes` and silently falls back to text merges without it.
//!
//! # Why `fr check --fix` does not do any of this
//!
//! It used to do part of it: `--fix` added the `.gitignore` pattern and nothing
//! else. That left a user unable to predict which of the four git-readiness
//! items `--fix` would repair. One command owns the whole surface instead, and
//! `--fix` points at it.
//!
//! # Outside git this does nothing
//!
//! Not an error — a project that is not in a repo has no config to be wrong, and
//! writing a `.gitignore` where there is no repo would be surprising. Same rule
//! the check follows.

use std::path::Path;

use crate::io::project_io::LOCAL_ONLY_FRAME_FILES;

/// The `merge.frame.*` driver name, in both `.git/config` and `.gitattributes`.
pub const DRIVER_NAME: &str = "frame";

/// What git runs. `fr` unqualified rather than an absolute path: the config is
/// per-clone but survives the binary being upgraded or moved, and every other
/// way of invoking frame already assumes it is on `PATH`.
pub const DRIVER_COMMAND: &str = "fr merge --base %O --ours %A --theirs %B --path %P";

/// A description of the driver, shown by `git config --list` and in the manual.
pub const DRIVER_DESCRIPTION: &str = "frame markdown three-way merge";

/// How to merge the *virtual ancestors* of a criss-cross merge.
///
/// The default (`text`) would run git's line-based merge to synthesize a base
/// and could leave conflict markers in it — which frame would then *parse*, the
/// one remaining path by which markers reach the parser. `binary` takes one
/// ancestor whole instead, which is a fine approximation of a base and cannot
/// produce a file frame does not understand.
pub const DRIVER_RECURSIVE: &str = "binary";

/// The frame directory's name, and therefore its path relative to the project
/// root — which is where `.gitignore` and `.gitattributes` are written.
///
/// **Both files' patterns are relative to the directory holding them**, not to
/// the repository root, because a git pattern containing a slash always is. So
/// this is the right prefix for every project, at the repo root or below it, and
/// there is nothing to compute. Computing it *was* the bug: `fr git setup` used
/// the frame directory's path relative to the git toplevel, so a project in
/// `sub/` got `sub/frame/archive/*.md` written into `sub/.gitattributes`, where
/// it means `sub/sub/frame/...` and matches nothing. Nothing routed to the merge
/// driver and nothing was ignored, silently, while the file looked plausible.
pub(crate) const FRAME_REL: &str = "frame";

/// One representative path per routed shape, relative to the project root.
///
/// Derived from [`attribute_lines`] so the two cannot drift: this is what
/// `fr check` hands to `git check-attr` to find out whether routing actually
/// works, and a pattern added there is probed here without anyone remembering
/// to. The paths need not exist — `check-attr` matches patterns, not files.
pub fn routed_paths() -> Vec<String> {
    attribute_lines(FRAME_REL)
        .iter()
        .filter_map(|line| line.split_whitespace().next())
        .map(|pattern| pattern.replace('*', "sample"))
        .collect()
}

/// One line per frame file shape that must route to the driver.
///
/// `pub(crate)` so `merge_files` can assert that every pattern routed here has a
/// kind [`crate::ops::merge_files::kind_for_path`] recognises. Routing a shape
/// the merge cannot name is how a done archive came to be parsed as a track.
pub(crate) fn attribute_lines(frame_rel: &str) -> Vec<String> {
    let frame_rel = frame_rel.trim_end_matches('/');
    vec![
        format!("{frame_rel}/tracks/*.md merge={DRIVER_NAME}"),
        format!("{frame_rel}/archive/*.md merge={DRIVER_NAME}"),
        format!("{frame_rel}/archive/_tracks/*.md merge={DRIVER_NAME}"),
        format!("{frame_rel}/inbox.md merge={DRIVER_NAME}"),
    ]
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// What one setup step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    /// Already correct; nothing written. Re-running is expected to produce this.
    AlreadyCorrect,
    Changed,
    /// Could not be done, with the reason.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Step {
    pub name: &'static str,
    pub status: StepStatus,
    /// Specifics worth showing — the lines added, the entries collapsed.
    pub detail: Vec<String>,
}

impl Step {
    fn already(name: &'static str) -> Self {
        Step {
            name,
            status: StepStatus::AlreadyCorrect,
            detail: Vec::new(),
        }
    }
    fn changed(name: &'static str, detail: Vec<String>) -> Self {
        Step {
            name,
            status: StepStatus::Changed,
            detail,
        }
    }
    fn failed(name: &'static str, why: String) -> Self {
        Step {
            name,
            status: StepStatus::Failed(why),
            detail: Vec::new(),
        }
    }
    pub fn changed_anything(&self) -> bool {
        matches!(self.status, StepStatus::Changed)
    }
}

#[derive(Debug)]
pub struct SetupReport {
    pub steps: Vec<Step>,
    /// `false` when the project is not in a git repo, in which case `steps` is
    /// empty and nothing was written.
    pub in_git: bool,
    /// Set when `fr` is not resolvable on `PATH`. The driver is registered as a
    /// bare `fr`, so git could not run it — worth saying at setup time rather
    /// than leaving it to be discovered mid-rebase.
    pub fr_not_on_path: bool,
}

impl SetupReport {
    pub fn changed_anything(&self) -> bool {
        self.steps.iter().any(|s| s.changed_anything())
    }
    pub fn failed(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.status, StepStatus::Failed(_)))
    }
}

// ---------------------------------------------------------------------------
// .gitignore
// ---------------------------------------------------------------------------

/// The legacy per-file `.gitignore` entries the blanket pattern replaces, in
/// every spelling a hand-edited file plausibly holds.
///
/// Only entries the blanket pattern **actually covers** may appear here. The
/// pattern matches dotfiles *directly* inside the frame directory, not nested
/// ones, and every local-only file lives at that top level by rule — so the set
/// is exactly [`LOCAL_ONLY_FRAME_FILES`], and a line like `frame/archive/.foo`
/// is not in it and is left alone.
fn legacy_entries(frame_rel: &str) -> Vec<String> {
    let frame_rel = frame_rel.trim_end_matches('/');
    let mut out = Vec::new();
    for name in LOCAL_ONLY_FRAME_FILES {
        let base = format!("{frame_rel}/{name}");
        // A directory entry is commonly written with a trailing slash, and any
        // entry may carry a leading one to anchor it at the repo root.
        out.push(base.clone());
        out.push(format!("{base}/"));
        out.push(format!("/{base}"));
        out.push(format!("/{base}/"));
    }
    out
}

/// The `.gitignore` a project should have, given what it has now.
///
/// Returns `None` when the file is already correct, so the caller writes nothing
/// and reports the run as idempotent.
///
/// Rules, in order of how much damage getting them wrong would do:
///
/// - **Only exact matches against [`legacy_entries`] are removed.** An
///   unrecognised line is never touched, whatever it looks like.
/// - The blanket pattern is inserted **where the first removed line was**, so a
///   preceding `# frame` comment still describes what follows it.
/// - With nothing to remove and no pattern present, it is appended with its own
///   comment header — the same shape `fr init` writes.
/// - Every other line, comment, and blank keeps its place.
/// - **A dead pattern this command itself wrote is removed too**, when
///   `stale_prefix` names one. Only the exact line
///   `<stale_prefix>/.*` — the output of an older `fr git setup` at a prefix
///   that matches nothing from here. A prefixed line that is *not* that exact
///   string belongs to somebody else and is left alone, including a genuine
///   entry for a different project written into a shared file.
pub fn planned_gitignore(
    existing: &str,
    frame_rel: &str,
    stale_prefix: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let pattern = crate::io::project_io::gitignore_pattern_for(frame_rel);
    let mut legacy = legacy_entries(frame_rel);
    if let Some(stale) = stale_prefix {
        legacy.push(crate::io::project_io::gitignore_pattern_for(stale));
    }

    let has_pattern = existing.lines().any(|l| l.trim() == pattern);
    let removed: Vec<String> = existing
        .lines()
        .filter(|l| legacy.iter().any(|e| e == l.trim()))
        .map(|l| l.trim().to_string())
        .collect();

    if has_pattern && removed.is_empty() {
        return None;
    }

    let mut out: Vec<String> = Vec::new();
    let mut inserted = has_pattern;
    for line in existing.lines() {
        if legacy.iter().any(|e| e == line.trim()) {
            // The first legacy line becomes the blanket pattern; the rest, which
            // it now covers, simply go.
            if !inserted {
                out.push(pattern.clone());
                inserted = true;
            }
            continue;
        }
        out.push(line.to_string());
    }

    if !inserted {
        if !out.is_empty() && !out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            out.push(String::new());
        }
        out.push("# frame — working-copy-local files (never commit these)".to_string());
        out.push(pattern.clone());
    }

    let mut text = out.join("\n");
    text.push('\n');
    Some((text, removed))
}

// ---------------------------------------------------------------------------
// .gitattributes
// ---------------------------------------------------------------------------

/// A file rewrite that is worth doing, and what it changes.
#[derive(Debug)]
pub struct Planned {
    pub content: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// The `.gitattributes` a project should have, or `None` when it is already
/// right.
///
/// Existing lines are never rewritten — a project may deliberately route some
/// other path elsewhere, and a pattern already mapped to a different driver is
/// left as the user set it. Only missing lines are appended.
///
/// **One exception, and it is deliberately narrow.** When `stale_prefix` names
/// one, a line that is character-for-character something an older `fr git setup`
/// wrote at that prefix is removed. Those lines are dead where they sit: a
/// pattern with a slash resolves against the directory of the file holding it,
/// so `sub/frame/*.md` inside `sub/.gitattributes` means `sub/sub/frame/*.md`.
/// The test is exact-match against [`attribute_lines`] output — a prefixed line
/// that differs by so much as its driver name is somebody's deliberate routing
/// and is left alone.
pub fn planned_gitattributes(
    existing: &str,
    frame_rel: &str,
    stale_prefix: Option<&str>,
) -> Option<Planned> {
    let dead: Vec<String> = stale_prefix.map(attribute_lines).unwrap_or_default();

    let kept: Vec<&str> = existing
        .lines()
        .filter(|l| !dead.iter().any(|d| d == l.trim()))
        .collect();
    let removed: Vec<String> = existing
        .lines()
        .filter(|l| dead.iter().any(|d| d == l.trim()))
        .map(|l| l.trim().to_string())
        .collect();

    // A pattern already mentioned — with any driver — counts as configured, so
    // re-running never fights a deliberate override. Checked against what
    // *survives* the removal above, so a dead line cannot vouch for the live one
    // it was written instead of.
    let added: Vec<String> = attribute_lines(frame_rel)
        .into_iter()
        .filter(|line| {
            let pattern = line.split_whitespace().next().unwrap_or_default();
            !kept
                .iter()
                .any(|p| p.split_whitespace().next() == Some(pattern))
        })
        .collect();

    if added.is_empty() && removed.is_empty() {
        return None;
    }

    let mut content = if removed.is_empty() {
        existing.to_string()
    } else {
        let mut text = kept.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        text
    };
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if content.is_empty() && !added.is_empty() {
        content.push_str("# frame — merge track and inbox files by task identity\n");
    }
    for line in &added {
        content.push_str(line);
        content.push('\n');
    }
    Some(Planned {
        content,
        added,
        removed,
    })
}

// ---------------------------------------------------------------------------
// The driver in .git/config
// ---------------------------------------------------------------------------

/// Whether this clone has the merge driver registered.
///
/// `None` outside a git repo, or when `git` cannot be run — the same
/// three-state answer [`crate::io::git::repo_paths`] gives, so a caller can tell
/// "not registered" from "no repo to register in" and stay quiet about the
/// latter.
pub fn driver_registered(root: &Path) -> Option<bool> {
    crate::io::git::repo_paths(&root.join("frame"))?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get", &format!("merge.{DRIVER_NAME}.driver")])
        .output()
        .ok()?;
    Some(output.status.success() && !output.stdout.is_empty())
}

fn git_config_set(root: &Path, key: &str, value: &str) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        // `--replace-all` so a key that somehow became multivalued collapses to
        // one entry rather than accumulating another.
        .args(["config", "--replace-all", key, value])
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

fn git_config_get(root: &Path, key: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get", key])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---------------------------------------------------------------------------
// Driving it all
// ---------------------------------------------------------------------------

/// Apply every setup step, or with `dry_run` report what would change.
///
/// Idempotent: a second run reports `AlreadyCorrect` throughout and writes
/// nothing.
/// `root` is the **project** root — the directory holding `frame/`, and the one
/// both files are written to. The prefix used inside them is always
/// [`FRAME_REL`]; see there for why it is not computed from anything.
pub fn run(root: &Path, dry_run: bool) -> SetupReport {
    let mut report = SetupReport {
        steps: Vec::new(),
        in_git: crate::io::git::repo_paths(&root.join(FRAME_REL)).is_some(),
        fr_not_on_path: false,
    };
    if !report.in_git {
        return report;
    }

    // What an older frame wrote here, if it differs — lines to clean up rather
    // than leave sitting dead in a committed file.
    let stale = stale_prefix(root);
    let stale = stale.as_deref();

    report.steps.push(step_gitignore(root, stale, dry_run));
    report.steps.push(step_gitattributes(root, stale, dry_run));
    report.steps.push(step_driver(root, dry_run));
    report.fr_not_on_path = !fr_on_path();
    report
}

/// The prefix a **previous** `fr git setup` would have written here: the frame
/// directory relative to the git toplevel rather than to the project root.
///
/// `None` when it agrees with [`FRAME_REL`], which is every project at the repo
/// root — so nothing is looked for and nothing can be removed there. For a
/// project below the root it names exactly the four `.gitattributes` lines and
/// the one `.gitignore` pattern that the old computation produced, which are
/// dead where they sit and are what [`planned_gitignore`] and
/// [`planned_gitattributes`] are allowed to delete.
fn stale_prefix(root: &Path) -> Option<String> {
    let frame_dir = root.join(FRAME_REL);
    let paths = crate::io::git::repo_paths(&frame_dir)?;
    let rel = frame_dir
        .canonicalize()
        .ok()?
        .strip_prefix(
            paths
                .toplevel
                .canonicalize()
                .ok()
                .as_deref()
                .unwrap_or(&paths.toplevel),
        )
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    (rel != FRAME_REL && !rel.is_empty()).then_some(rel)
}

fn step_gitignore(root: &Path, stale_prefix: Option<&str>, dry_run: bool) -> Step {
    const NAME: &str = ".gitignore";
    let path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let Some((content, removed)) = planned_gitignore(&existing, FRAME_REL, stale_prefix) else {
        return Step::already(NAME);
    };

    let pattern = crate::io::project_io::gitignore_pattern_for(FRAME_REL);
    let dead = stale_prefix.map(crate::io::project_io::gitignore_pattern_for);
    let (dead_lines, collapsed): (Vec<&String>, Vec<&String>) = removed
        .iter()
        .partition(|r| dead.as_deref() == Some(r.as_str()));

    let mut detail = vec![format!("+ {pattern}")];
    if !collapsed.is_empty() {
        detail.push(format!(
            "- collapsed {} per-file entr{} into it",
            collapsed.len(),
            if collapsed.len() == 1 { "y" } else { "ies" }
        ));
        detail.extend(collapsed.iter().map(|r| format!("  - {r}")));
    }
    for line in dead_lines {
        detail.push(format!("- removed {line} — matched nothing from here"));
    }

    if dry_run {
        return Step::changed(NAME, detail);
    }
    match crate::io::dryrun::write(&path, content) {
        Ok(()) => Step::changed(NAME, detail),
        Err(e) => Step::failed(NAME, format!("could not write .gitignore: {e}")),
    }
}

fn step_gitattributes(root: &Path, stale_prefix: Option<&str>, dry_run: bool) -> Step {
    const NAME: &str = ".gitattributes";
    let path = root.join(".gitattributes");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let Some(plan) = planned_gitattributes(&existing, FRAME_REL, stale_prefix) else {
        return Step::already(NAME);
    };
    let mut detail: Vec<String> = plan.added.iter().map(|l| format!("+ {l}")).collect();
    if !plan.removed.is_empty() {
        detail.push(format!(
            "- removed {} line{} that matched nothing from here",
            plan.removed.len(),
            if plan.removed.len() == 1 { "" } else { "s" }
        ));
        detail.extend(plan.removed.iter().map(|r| format!("  - {r}")));
    }
    if dry_run {
        return Step::changed(NAME, detail);
    }
    match crate::io::dryrun::write(&path, plan.content) {
        Ok(()) => Step::changed(NAME, detail),
        Err(e) => Step::failed(NAME, format!("could not write .gitattributes: {e}")),
    }
}

fn step_driver(root: &Path, dry_run: bool) -> Step {
    const NAME: &str = "merge driver (.git/config)";
    let wanted = [
        (format!("merge.{DRIVER_NAME}.name"), DRIVER_DESCRIPTION),
        (format!("merge.{DRIVER_NAME}.driver"), DRIVER_COMMAND),
        (format!("merge.{DRIVER_NAME}.recursive"), DRIVER_RECURSIVE),
    ];

    let stale: Vec<&(String, &str)> = wanted
        .iter()
        .filter(|(key, value)| git_config_get(root, key).as_deref() != Some(*value))
        .collect();
    if stale.is_empty() {
        return Step::already(NAME);
    }

    let detail = vec![format!("+ {DRIVER_COMMAND}")];
    if dry_run {
        return Step::changed(NAME, detail);
    }
    for (key, value) in &stale {
        if let Err(e) = git_config_set(root, key, value) {
            return Step::failed(NAME, format!("could not set {key}: {e}"));
        }
    }
    Step::changed(NAME, detail)
}

/// Whether a bare `fr` resolves to something runnable.
///
/// The driver is registered as `fr`, so if it does not resolve, git reports a
/// failed driver and conflicts every frame file — a confusing way to learn about
/// a `PATH` problem, and one that surfaces mid-rebase. Cheaper to say now.
fn fr_on_path() -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join("fr");
        candidate.is_file()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- .gitignore migration ---

    #[test]
    fn a_project_with_no_gitignore_gets_the_pattern() {
        let (content, removed) = planned_gitignore("", "frame", None).unwrap();
        assert!(content.contains("frame/.*"));
        assert!(removed.is_empty());
    }

    #[test]
    fn an_existing_pattern_is_left_alone() {
        assert!(planned_gitignore("target/\nframe/.*\n", "frame", None).is_none());
    }

    /// The migration this exists for: a project predating the blanket pattern
    /// carries one line per local-only file.
    #[test]
    fn legacy_per_file_entries_collapse_into_the_pattern() {
        let existing = "\
target/

# frame — working-copy-local files (never commit these)
frame/.state.json
frame/.lock
frame/.recovery.log
frame/.actor
frame/.rescue/
";
        let (content, removed) = planned_gitignore(existing, "frame", None).unwrap();

        assert_eq!(removed.len(), 5);
        assert!(content.contains("frame/.*"));
        for legacy in [
            "frame/.state.json",
            "frame/.lock",
            "frame/.recovery.log",
            "frame/.actor",
            "frame/.rescue/",
        ] {
            assert!(
                !content.lines().any(|l| l.trim() == legacy),
                "{legacy} survived:\n{content}"
            );
        }
        // Unrelated content and the explanatory comment keep their place.
        assert!(content.contains("target/"));
        assert!(content.contains("# frame — working-copy-local files"));
        // The pattern lands where the first removed line was, under that comment.
        let comment_at = content.find("# frame —").unwrap();
        let pattern_at = content.find("frame/.*").unwrap();
        assert!(pattern_at > comment_at);
    }

    #[test]
    fn leading_and_trailing_slash_spellings_are_recognized() {
        let existing = "/frame/.actor\nframe/.rescue/\n/frame/.lock/\n";
        let (content, removed) = planned_gitignore(existing, "frame", None).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(!content.contains(".actor"));
        assert!(!content.contains(".rescue"));
    }

    /// The dangerous direction: removing a line frame did not put there. Only
    /// exact matches against the known set may go.
    #[test]
    fn unrecognized_lines_are_never_removed() {
        let existing = "\
frame/.actor
frame/notes.md
frame/archive/.keep
!frame/.important
frame-other/.actor
";
        let (content, removed) = planned_gitignore(existing, "frame", None).unwrap();
        assert_eq!(removed, vec!["frame/.actor"]);
        assert!(content.contains("frame/notes.md"));
        // Nested dotfiles are NOT covered by `frame/.*`, so removing them would
        // silently start committing them.
        assert!(content.contains("frame/archive/.keep"));
        assert!(content.contains("!frame/.important"));
        assert!(content.contains("frame-other/.actor"));
    }

    /// The prefix is whatever the caller says, and legacy entries follow it —
    /// the pure function has no opinion about where the project sits.
    ///
    /// `run` always passes [`FRAME_REL`], because the file is written beside the
    /// frame directory and a git pattern is relative to its own file. This test
    /// pins the parameter's meaning, not a project layout.
    #[test]
    fn legacy_entries_follow_the_prefix_they_are_given() {
        let existing = "sub/frame/.actor\nframe/.actor\n";
        let (content, removed) = planned_gitignore(existing, "sub/frame", None).unwrap();

        assert_eq!(removed, vec!["sub/frame/.actor"]);
        assert!(content.contains("sub/frame/.*"));
        // A different prefix's entry is not this one's business.
        assert!(content.contains("frame/.actor"));
    }

    #[test]
    fn gitignore_migration_is_idempotent() {
        let existing = "target/\nframe/.actor\nframe/.lock\n";
        let (once, _) = planned_gitignore(existing, "frame", None).unwrap();
        assert!(
            planned_gitignore(&once, "frame", None).is_none(),
            "second run would rewrite:\n{once}"
        );
    }

    // --- .gitattributes ---

    #[test]
    fn attributes_are_added_when_absent() {
        let plan = planned_gitattributes("", "frame", None).unwrap();
        assert_eq!(plan.added.len(), 4);
        assert!(plan.removed.is_empty());
        assert!(plan.content.contains("frame/tracks/*.md merge=frame"));
        assert!(plan.content.contains("frame/inbox.md merge=frame"));
    }

    #[test]
    fn attributes_are_idempotent() {
        let once = planned_gitattributes("", "frame", None).unwrap().content;
        assert!(planned_gitattributes(&once, "frame", None).is_none());
    }

    #[test]
    fn existing_content_is_preserved() {
        let existing = "*.png binary\n";
        let plan = planned_gitattributes(existing, "frame", None).unwrap();
        assert!(plan.content.contains("*.png binary"));
        assert!(plan.content.contains("merge=frame"));
    }

    /// A pattern the user already mapped somewhere else is theirs, not ours.
    #[test]
    fn a_deliberate_override_is_not_fought() {
        let existing = "frame/inbox.md merge=union\n";
        let plan = planned_gitattributes(existing, "frame", None).unwrap();
        assert!(plan.content.contains("frame/inbox.md merge=union"));
        assert!(!plan.added.iter().any(|l| l.starts_with("frame/inbox.md")));
    }

    // --- Outside git ---

    #[test]
    fn outside_a_repo_nothing_happens() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("frame")).unwrap();

        let report = run(tmp.path(), false);

        assert!(!report.in_git);
        assert!(report.steps.is_empty());
        assert!(!tmp.path().join(".gitignore").exists());
        assert!(!tmp.path().join(".gitattributes").exists());
    }

    #[test]
    fn driver_registered_is_none_outside_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("frame")).unwrap();
        assert_eq!(driver_registered(tmp.path()), None);
    }
}
