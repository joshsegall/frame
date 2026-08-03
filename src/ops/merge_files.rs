//! File-level three-way merge of frame markdown, for use as a version-control
//! merge driver.
//!
//! # Why a driver exists
//!
//! A line-based merge is wrong for frame files in a way that is quiet enough to
//! reach a commit. `fr done BAC-179` *relocates* a task from `## Backlog` to
//! `## Done`, which a text merge sees as a deletion in one region and an
//! insertion in another. Any resolution that keeps both hunks yields two
//! BAC-179s — one `[ ]`, one `[x]` — in a file that still looks plausible and
//! that `fr show` disagrees with. Repairing that by hand means line-range
//! surgery on a structure the merge already damaged, which is how a section
//! header gets deleted.
//!
//! [`crate::ops::reconcile`] already merges two versions of a track over their
//! common ancestor by *task identity*, which makes the relocation a non-event:
//! it reads as one task whose section changed. This module is the file-level
//! shell around it — read three paths, merge, write one — and the CLI command
//! `fr merge` is the entry point a version control system invokes.
//!
//! # Not git-specific
//!
//! Every VCS with a custom-merge hook passes the same four things: the ancestor,
//! our version, their version, and where to put the result. Git spells them
//! `%O %A %B` (result overwrites `%A`); Mercurial `$base $local $other`; jj's
//! `merge-tools` and SVN are the same shape. Nothing here knows which one is
//! calling, and only *registration* is git-specific ([`crate::ops::git_setup`]).
//!
//! # Why identity is trustworthy across branches
//!
//! Matching by ID is only sound if two branches cannot mint the same ID for
//! different tasks — and they cannot. Actor tokens namespace mints per working
//! copy, and the durable frontier in [`crate::io::ids`] is shared by every
//! worktree of a clone. So a key present on both sides always denotes *the same
//! task*, and [`crate::ops::reconcile`] needs no renumbering logic here.
//!
//! The one gap is child numbers (`BAC-153.2`), which [`crate::ops::ids`]
//! deliberately leaves uncovered. It is already handled downstream: `fr check`
//! reports the duplicate and `fr clean` renumbers the later copy under its own
//! parent. Nothing for the merge to do.
//!
//! # Conflict markers are never written
//!
//! On conflict this writes a file that still *parses as frame markdown* and
//! reports the conflict through its return value, so the caller can fail the
//! merge. Writing `<<<<<<<` markers would be the one thing guaranteed to make
//! every frame tool — the parser, `fr check`, `fr show` — useless at exactly the
//! moment they are needed.

use std::path::Path;

use crate::model::task::{Metadata, Task};
use crate::model::track::{Track, TrackNode};
use crate::ops::reconcile::{self, Conflict};
use crate::parse::{parse_inbox, parse_track, serialize_inbox, serialize_track};

/// Which frame file shape a path holds. The two merge by different rules —
/// tracks by task identity, the inbox by content — so the kind has to be settled
/// before anything is parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Track,
    Inbox,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Track => "track",
            FileKind::Inbox => "inbox",
        }
    }

    /// Parse the `--kind` flag.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "track" => Some(FileKind::Track),
            "inbox" => Some(FileKind::Inbox),
            _ => None,
        }
    }
}

/// What a merge did, and what it could not decide.
#[derive(Debug)]
pub struct MergeReport {
    pub kind: FileKind,
    /// Tasks or items whose version came from the other side.
    pub took_theirs: usize,
    /// Tasks or items removed because both sides agree they are gone.
    pub deleted: usize,
    /// Tasks the merge declined to decide. Always empty for the inbox, which
    /// never sets a side's content aside.
    pub conflicts: Vec<Conflict>,
}

impl MergeReport {
    /// Whether the merge resolved everything, i.e. whether the caller may treat
    /// the file as merged rather than conflicted.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Which frame file a repo-relative path holds, or `None` when it is not a file
/// this merge understands.
///
/// The caller is expected to *decline* on `None` rather than guess, so the VCS
/// falls back to its own merge. That matters for `project.toml` and
/// `actors.toml`: both are TOML that line-merges fine (`actors.toml` serializes
/// in insertion order precisely so it does), and a driver that tried to parse
/// them as markdown would turn a working merge into a failure.
///
/// Matching is on path *shape*, not on the project on disk, so this stays
/// correct when the VCS runs it against temp files or from another directory.
pub fn kind_for_path(path: &str) -> Option<FileKind> {
    // A VCS may hand back either separator on Windows; normalize before matching.
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty()).collect();
    let file = components.last()?;

    if !file.ends_with(".md") {
        return None;
    }
    if *file == "inbox.md" {
        return Some(FileKind::Inbox);
    }
    // Track files live in `tracks/`; their archives — including whole archived
    // tracks under `archive/_tracks/` — are the same shape and merge the same
    // way.
    let in_track_dir = components
        .iter()
        .rev()
        .skip(1)
        .any(|c| *c == "tracks" || *c == "archive");
    in_track_dir.then_some(FileKind::Track)
}

/// Merge three versions of a track, as text.
///
/// Kept separate from the file layer so the merge itself is testable without
/// touching a filesystem or a VCS.
///
/// `stamp` dates the `conflict:` markers left on undecided tasks, and ties each
/// one to its recovery-log entry. Taken as a parameter rather than read from the
/// clock so the merge stays a pure function of its inputs.
pub fn merge_track_text(
    base: &str,
    ours: &str,
    theirs: &str,
    stamp: &str,
) -> (String, MergeReport) {
    let mut result =
        reconcile::reconcile_track(&parse_track(base), &parse_track(ours), &parse_track(theirs));
    mark_conflicts(&mut result.track, &result.conflicts, stamp);
    (
        serialize_track(&result.track),
        MergeReport {
            kind: FileKind::Track,
            took_theirs: result.took_theirs,
            deleted: result.deleted,
            conflicts: result.conflicts,
        },
    )
}

/// Record each unresolved conflict on the task it belongs to.
///
/// # Why the file has to say so
///
/// No conflict markers are written, which is what keeps the file readable — but
/// it also means nothing in the file itself would show that a merge gave up on a
/// task. The VCS marks the *path* unmerged, and staging it would then quietly
/// commit our side and discard theirs, with only scrolled-away stderr to say so.
/// A `conflict:` line makes that loss a fact in the file, which `fr check`
/// reports as an error until `fr merge --resolve` clears it.
///
/// Replaces any marker already present rather than adding a second: a rebase
/// replays many commits, and the same task conflicting twice is one unresolved
/// conflict, not two.
fn mark_conflicts(track: &mut Track, conflicts: &[Conflict], stamp: &str) {
    if conflicts.is_empty() {
        return;
    }
    let wanted: std::collections::HashMap<&str, &Conflict> =
        conflicts.iter().map(|c| (c.key.as_str(), c)).collect();

    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            mark_in_tasks(tasks, &wanted, stamp);
        }
    }
}

fn mark_in_tasks(
    tasks: &mut [Task],
    wanted: &std::collections::HashMap<&str, &Conflict>,
    stamp: &str,
) {
    for task in tasks {
        if let Some(conflict) = wanted.get(reconcile::task_key(task).as_str()) {
            task.metadata
                .retain(|m| !matches!(m, Metadata::Conflict(_)));
            task.metadata.push(Metadata::Conflict(format!(
                "{} {stamp}",
                conflict.reason.slug()
            )));
            // Without this the serializer emits the task's stored source lines
            // verbatim and the marker never reaches the file.
            task.dirty = true;
        }
        mark_in_tasks(&mut task.subtasks, wanted, stamp);
    }
}

/// Merge three versions of the inbox, as text.
///
/// Reports no conflicts by construction — the inbox merges as a multiset and
/// keeps both versions of a doubly-edited item rather than setting one aside.
/// See [`crate::ops::reconcile::reconcile_inbox`] for why that is right there and
/// wrong for tracks.
pub fn merge_inbox_text(base: &str, ours: &str, theirs: &str) -> (String, MergeReport) {
    let (base, _) = parse_inbox(base);
    let (ours, _) = parse_inbox(ours);
    let (theirs, _) = parse_inbox(theirs);
    let result = reconcile::reconcile_inbox(&base, &ours, &theirs);
    (
        serialize_inbox(&result.inbox),
        MergeReport {
            kind: FileKind::Inbox,
            took_theirs: result.took_theirs,
            deleted: result.deleted,
            conflicts: Vec::new(),
        },
    )
}

pub fn merge_text(
    kind: FileKind,
    base: &str,
    ours: &str,
    theirs: &str,
    stamp: &str,
) -> (String, MergeReport) {
    match kind {
        FileKind::Track => merge_track_text(base, ours, theirs, stamp),
        FileKind::Inbox => merge_inbox_text(base, ours, theirs),
    }
}

/// Why a file-level merge could not run at all.
///
/// Distinct from a *conflict*, which is a merge that ran and left something
/// undecided. The caller maps these to different exit statuses because a VCS
/// treats them differently: a conflict stops the operation for a human, an error
/// means the driver itself is broken.
#[derive(Debug)]
pub enum MergeFileError {
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Write {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for MergeFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeFileError::Read { path, source } => {
                write!(f, "cannot read {}: {}", path.display(), source)
            }
            MergeFileError::Write { path, source } => {
                write!(f, "cannot write {}: {}", path.display(), source)
            }
        }
    }
}

impl std::error::Error for MergeFileError {}

/// Merge `base`/`ours`/`theirs` and write the result over `ours`.
///
/// Writing to `ours` is the VCS convention: git's `%A` is a temporary file
/// holding our side that the driver is expected to overwrite with the result.
///
/// A **missing base is treated as empty** rather than as an error. Git creates
/// an empty ancestor file for an add/add — two branches that each created the
/// same track — and that merges correctly here: with no ancestor every task on
/// both sides is an addition, and because IDs cannot collide across branches
/// they all survive. `ours` and `theirs` must exist; a missing one is a caller
/// error, not a merge case.
pub fn merge_files(
    kind: FileKind,
    base: &Path,
    ours: &Path,
    theirs: &Path,
    stamp: &str,
) -> Result<MergeReport, MergeFileError> {
    let base_text = match std::fs::read_to_string(base) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(MergeFileError::Read {
                path: base.to_path_buf(),
                source: e,
            });
        }
    };
    let ours_text = std::fs::read_to_string(ours).map_err(|e| MergeFileError::Read {
        path: ours.to_path_buf(),
        source: e,
    })?;
    let theirs_text = std::fs::read_to_string(theirs).map_err(|e| MergeFileError::Read {
        path: theirs.to_path_buf(),
        source: e,
    })?;

    let (merged, report) = merge_text(kind, &base_text, &ours_text, &theirs_text, stamp);

    crate::io::recovery::atomic_write(ours, merged.as_bytes()).map_err(|e| {
        MergeFileError::Write {
            path: ours.to_path_buf(),
            source: e,
        }
    })?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: &str = "2026-08-03T04:08:38Z";

    // --- Path → kind ---

    #[test]
    fn track_paths_are_recognized() {
        assert_eq!(kind_for_path("frame/tracks/main.md"), Some(FileKind::Track));
        assert_eq!(
            kind_for_path("frame/archive/main.md"),
            Some(FileKind::Track)
        );
        assert_eq!(
            kind_for_path("frame/archive/_tracks/old.md"),
            Some(FileKind::Track)
        );
        // A project in a subdirectory reports a longer path; only the shape matters.
        assert_eq!(
            kind_for_path("sub/dir/frame/tracks/main.md"),
            Some(FileKind::Track)
        );
    }

    #[test]
    fn inbox_is_recognized_by_name() {
        assert_eq!(kind_for_path("frame/inbox.md"), Some(FileKind::Inbox));
    }

    /// The driver must decline anything it does not understand so the VCS falls
    /// back to its own merge. Parsing `project.toml` as markdown would break a
    /// merge that works today.
    #[test]
    fn other_paths_are_declined() {
        assert_eq!(kind_for_path("frame/project.toml"), None);
        assert_eq!(kind_for_path("frame/actors.toml"), None);
        assert_eq!(kind_for_path("README.md"), None);
        assert_eq!(kind_for_path("doc/tracks.txt"), None);
    }

    #[test]
    fn windows_separators_are_normalized() {
        assert_eq!(
            kind_for_path(r"frame\tracks\main.md"),
            Some(FileKind::Track)
        );
    }

    // --- Track merges ---

    fn track(tasks_backlog: &[&str], tasks_done: &[&str]) -> String {
        let mut s = String::from("# Main\n\n## Backlog\n\n");
        for t in tasks_backlog {
            s.push_str(t);
            s.push('\n');
        }
        s.push_str("\n## Parked\n\n## Done\n\n");
        for t in tasks_done {
            s.push_str(t);
            s.push('\n');
        }
        s
    }

    /// The originating incident: one side finishes a task (which *moves* it
    /// between sections) while the other appends a new one. A text merge
    /// duplicates the moved task; this must not.
    #[test]
    fn a_move_and_an_append_both_land() {
        let base = track(&["- [ ] `BAC-179` Fix the thing"], &[]);
        let ours = track(&[], &["- [x] `BAC-179` Fix the thing"]);
        let theirs = track(
            &[
                "- [ ] `BAC-179` Fix the thing",
                "- [ ] `BAC-180` Another thing",
            ],
            &[],
        );

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        assert_eq!(merged.matches("BAC-179").count(), 1, "merged:\n{merged}");
        assert!(merged.contains("BAC-180"));
        // The finished task is in Done, and stayed finished.
        let done = merged.split_once("## Done").unwrap().1;
        assert!(done.contains("- [x] `BAC-179`"), "merged:\n{merged}");
    }

    /// Section headers are structure, not content — the merge rebuilds them
    /// rather than splicing lines, so none can be lost.
    #[test]
    fn section_headers_survive() {
        let base = track(&["- [ ] `BAC-1` One"], &[]);
        let ours = track(&["- [ ] `BAC-1` One", "- [ ] `BAC-2` Two"], &[]);
        let theirs = track(&[], &["- [x] `BAC-1` One"]);

        let (merged, _) = merge_track_text(&base, &ours, &theirs, STAMP);

        assert!(merged.contains("## Backlog"));
        assert!(merged.contains("## Parked"));
        assert!(merged.contains("## Done"));
    }

    /// With no common ancestor every task is an addition. Because IDs cannot
    /// collide across branches, both sides survive whole.
    #[test]
    fn empty_base_keeps_both_sides() {
        let ours = track(&["- [ ] `BAC-1` Ours"], &[]);
        let theirs = track(&["- [ ] `BAC-2` Theirs"], &[]);

        let (merged, report) = merge_track_text("", &ours, &theirs, STAMP);

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        assert!(merged.contains("BAC-1"));
        assert!(merged.contains("BAC-2"));
    }

    /// Both sides editing one task differently is the case with no right answer.
    /// Ours is kept and theirs is reported, never silently dropped.
    #[test]
    fn a_double_edit_conflicts_and_keeps_ours() {
        let base = track(&["- [ ] `BAC-1` Original"], &[]);
        let ours = track(&["- [ ] `BAC-1` Our title"], &[]);
        let theirs = track(&["- [ ] `BAC-1` Their title"], &[]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);

        assert!(!report.is_clean());
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].key, "#BAC-1");
        assert!(merged.contains("Our title"));
        // Theirs is preserved in the report for the caller to record.
        assert!(
            report.conflicts[0]
                .theirs
                .join("\n")
                .contains("Their title")
        );
    }

    /// Whatever comes out has to be a file frame can read back, conflict or not.
    /// A merge that emitted anything else would break every tool that could
    /// diagnose it.
    #[test]
    fn output_round_trips_even_when_conflicted() {
        let base = track(&["- [ ] `BAC-1` Original"], &[]);
        let ours = track(&["- [ ] `BAC-1` Our title"], &[]);
        let theirs = track(&["- [ ] `BAC-1` Their title"], &[]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert!(!report.is_clean());

        assert!(!merged.contains("<<<<<<<"));
        let reparsed = serialize_track(&parse_track(&merged));
        assert_eq!(reparsed, merged, "merged output is not stable");
    }

    // --- Inbox merges ---

    #[test]
    fn inbox_captures_from_both_sides_survive() {
        let base = "# Inbox\n\n- one\n";
        let ours = "# Inbox\n\n- one\n- ours\n";
        let theirs = "# Inbox\n\n- one\n- theirs\n";

        let (merged, report) = merge_inbox_text(base, ours, theirs);

        assert!(report.is_clean());
        assert!(merged.contains("- one"));
        assert!(merged.contains("- ours"));
        assert!(merged.contains("- theirs"));
    }

    #[test]
    fn inbox_removal_agreed_by_both_sides_sticks() {
        let base = "# Inbox\n\n- one\n- two\n";
        let ours = "# Inbox\n\n- two\n";
        let theirs = "# Inbox\n\n- two\n";

        let (merged, _) = merge_inbox_text(base, ours, theirs);

        assert!(!merged.contains("- one"));
        assert!(merged.contains("- two"));
    }

    // --- File layer ---

    #[test]
    fn merge_files_writes_over_ours() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("base.md");
        let ours_path = dir.path().join("ours.md");
        let theirs_path = dir.path().join("theirs.md");

        std::fs::write(&base_path, track(&["- [ ] `BAC-1` One"], &[])).unwrap();
        std::fs::write(&ours_path, track(&["- [ ] `BAC-1` One"], &[])).unwrap();
        std::fs::write(
            &theirs_path,
            track(&["- [ ] `BAC-1` One", "- [ ] `BAC-2` Two"], &[]),
        )
        .unwrap();

        let report =
            merge_files(FileKind::Track, &base_path, &ours_path, &theirs_path, STAMP).unwrap();

        assert!(report.is_clean());
        assert_eq!(report.took_theirs, 1);
        let written = std::fs::read_to_string(&ours_path).unwrap();
        assert!(written.contains("BAC-2"));
        // Their side is untouched — only `ours` is the output path.
        assert!(
            !std::fs::read_to_string(&theirs_path)
                .unwrap()
                .contains("<<<")
        );
    }

    /// Git creates an empty ancestor for an add/add, but a driver invoked by hand
    /// may simply not have one. Treated as empty rather than as a failure.
    #[test]
    fn missing_base_file_is_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ours_path = dir.path().join("ours.md");
        let theirs_path = dir.path().join("theirs.md");
        std::fs::write(&ours_path, track(&["- [ ] `BAC-1` Ours"], &[])).unwrap();
        std::fs::write(&theirs_path, track(&["- [ ] `BAC-2` Theirs"], &[])).unwrap();

        let report = merge_files(
            FileKind::Track,
            &dir.path().join("nonexistent.md"),
            &ours_path,
            &theirs_path,
            STAMP,
        )
        .unwrap();

        assert!(report.is_clean());
        let written = std::fs::read_to_string(&ours_path).unwrap();
        assert!(written.contains("BAC-1") && written.contains("BAC-2"));
    }

    #[test]
    fn a_missing_side_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let ours_path = dir.path().join("ours.md");
        std::fs::write(&ours_path, track(&[], &[])).unwrap();

        let err = merge_files(
            FileKind::Track,
            &dir.path().join("base.md"),
            &ours_path,
            &dir.path().join("theirs.md"),
            STAMP,
        );

        assert!(matches!(err, Err(MergeFileError::Read { .. })));
    }
}
