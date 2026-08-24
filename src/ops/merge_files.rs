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
use crate::ops::reconcile::{self, Conflict, ConflictReason};
use crate::parse::{
    parse_archive, parse_inbox, parse_track, serialize_archive, serialize_inbox, serialize_track,
};

/// Which frame file shape a path holds. The three merge by different rules —
/// tracks by task identity within sections, a done archive by task identity over
/// a flat list, the inbox by content — so the kind has to be settled before
/// anything is parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Track,
    /// A done-task archive, `frame/archive/<track>.md`. **Not a track**: it has
    /// no `## Section` headers, so `parse_track` reads the entire task list as
    /// literal text and hands the merge nothing to match on. That is not
    /// theoretical — it is the bug this variant exists to fix, and it silently
    /// discarded one side of every archive merge.
    Archive,
    Inbox,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Track => "track",
            FileKind::Archive => "archive",
            FileKind::Inbox => "inbox",
        }
    }

    /// What one merged unit is called in a message.
    pub fn unit(self) -> &'static str {
        match self {
            FileKind::Track | FileKind::Archive => "task(s)",
            FileKind::Inbox => "item(s)",
        }
    }

    /// What everything *around* the units is called in a message. Its own
    /// phrase rather than `unit()` in a sentence, because "the text around the
    /// task(s)" is not something anybody says.
    pub fn surroundings(self) -> &'static str {
        match self {
            FileKind::Track | FileKind::Archive => "the text around the tasks",
            FileKind::Inbox => "the text around the items",
        }
    }

    /// Parse the `--kind` flag.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "track" => Some(FileKind::Track),
            "archive" => Some(FileKind::Archive),
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
    /// Whether the text around the tasks — headings, prose, the title — came
    /// from the other side. Reported separately from [`Self::took_theirs`]
    /// because it is not a task and the message has to be able to say so.
    pub took_their_shell: bool,
    /// Tasks the merge declined to decide, plus at most one conflict that names
    /// no task: the surrounding text, or an unaccounted loss. Never empty
    /// without the caller halting.
    pub conflicts: Vec<Conflict>,
    /// Lines of the other side that a field-level merge replaced rather than
    /// dropped — see [`reconcile::Trace::superseded`]. Read only by
    /// [`unaccounted_additions`], which would otherwise report every
    /// field-merged task as a defect.
    pub superseded: Vec<String>,
}

impl MergeReport {
    /// Whether the merge resolved everything, i.e. whether the caller may treat
    /// the file as merged rather than conflicted.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// Whether anything at all came across from the other side. What makes a
    /// clean merge worth a line of output.
    pub fn took_anything(&self) -> bool {
        self.took_theirs > 0 || self.deleted > 0 || self.took_their_shell
    }
}

/// Which frame file a repo-relative path holds, or `None` when it is not a file
/// this merge understands.
///
/// The caller is expected to *decline* on `None` rather than guess. Declining
/// halts the merge with our side intact and the path unmerged; it does not hand
/// the file back to the VCS's own merge, which git only performs for a path no
/// driver was configured for (see [`crate::cli::handlers::cmd_merge`]).
///
/// `project.toml` and `actors.toml` line-merge acceptably — but that is because
/// they are never routed here, not because this turns them down. What makes
/// `actors.toml` safe under git's own merge is its serialization (one line per
/// actor, sorted by token; see [`crate::io::actors::serialize_registry`]), not
/// anything in this module. What declining is for is a
/// file that *is* routed and whose shape we cannot name: stopping is right
/// there, because parsing it as the wrong shape is how data goes missing.
///
/// Matching is on path *shape*, not on the project on disk, so this stays
/// correct when the VCS runs it against temp files or from another directory.
///
/// # Every shape is named; there is no catch-all
///
/// This used to answer `Track` for *anything* `.md` with `tracks` or `archive`
/// anywhere above it. That defaulting is what made the archive bug silent: a
/// done archive got a kind it does not have, was parsed as a track, and lost a
/// side — where returning `None` would have stopped the merge with both versions
/// still on disk for someone to look at.
///
/// So the shapes are enumerated, the immediate parent decides, and an
/// unrecognised path is `None`. A new routed file shape has to be added here
/// deliberately, and `every_routed_pattern_has_a_kind` fails the build until it
/// is.
///
/// # Matched from the end, not from the root
///
/// The parent and its parent are read off the *trailing* components, so nothing
/// assumes where the project sits. `subdir/frame/archive/main.md` — what git
/// passes as `%P` for a project in a subdirectory of the repo, since `%P` is
/// relative to the repo root while the `.gitattributes` pattern that routed it
/// is relative to its own directory — resolves the same as `frame/archive/main.md`,
/// and so does an absolute path from a VCS that hands back one.
pub fn kind_for_path(path: &str) -> Option<FileKind> {
    // A VCS may hand back either separator on Windows; normalize before matching.
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty()).collect();
    let file = *components.last()?;

    if !file.ends_with(".md") {
        return None;
    }
    if file == "inbox.md" {
        // Only the project's own inbox. `archive/inbox.md` is not a thing frame
        // writes, and guessing at one would be the same mistake again.
        return (components.len() >= 2 && components[components.len() - 2] == "frame")
            .then_some(FileKind::Inbox);
    }

    // The immediate parent settles it, and its own parent disambiguates the two
    // directories named for tracks.
    let parent = components.get(components.len().checked_sub(2)?)?;
    let grandparent = components
        .len()
        .checked_sub(3)
        .and_then(|i| components.get(i));

    match (*parent, grandparent) {
        // `frame/tracks/<x>.md` — a live track.
        ("tracks", Some(&"frame")) => Some(FileKind::Track),
        // `frame/archive/<x>.md` — a done-task archive: a flat list, no sections.
        ("archive", Some(&"frame")) => Some(FileKind::Archive),
        // `frame/archive/_tracks/<x>.md` — a whole track `fr track archive`
        // moved intact, sections and all, so it really is a track.
        ("_tracks", Some(&"archive")) => Some(FileKind::Track),
        _ => None,
    }
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
            took_their_shell: result.took_their_shell,
            conflicts: result.conflicts,
            superseded: result.superseded,
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

/// Merge three versions of a **done-task archive**, as text.
///
/// An archive is a flat list of done tasks with unique IDs, so this is a
/// *simpler* merge than a track's: no sections, so no relocation to reconcile,
/// and position carries no meaning. The overwhelmingly common case is both sides
/// having appended different tasks since the base, and the answer is the union.
///
/// # Why not through `merge_track_text`
///
/// Because `parse_track` on an archive finds no `## ` header, collapses the file
/// into one `TrackNode::Literal`, and yields zero tasks — after which the merge
/// has no identity to work with and `rebuild` hands back ours verbatim, silently
/// dropping their side. That is the bug; see `doc/architecture.md` § Merging
/// Under Version Control.
///
/// # What is carried across
///
/// The merged task list goes back into an [`Archive`] whose header and trailing
/// text are decided the same way a track's are — [`reconcile::decide_shell`]'s
/// rule, whoever changed it wins and both changing it is a conflict. The file's
/// line ending stays ours either way: a CRLF archive comes out of a merge CRLF.
///
/// **Ours used to win that unconditionally, and nothing said so.** Frame
/// understands neither region, which is a good reason not to merge them *line by
/// line* and no reason at all to drop one side in silence and exit 0 — a header
/// naming where the archive came from, or a note under the last task, is content
/// somebody wrote, and losing it is the same loss as losing a task.
///
/// `stamp` is unused. No `conflict:` marker is written into an archive — see
/// [`crate::cli::handlers::cmd_merge`] for why the halt carries that job here.
pub fn merge_archive_text(
    base: &str,
    ours: &str,
    theirs: &str,
    _stamp: &str,
) -> (String, MergeReport) {
    let base = parse_archive(base);
    let mut ours = parse_archive(ours);
    let theirs = parse_archive(theirs);

    let before: std::collections::HashSet<String> =
        ours.tasks.iter().map(reconcile::task_key).collect();

    let (merged, mut trace) =
        reconcile::reconcile_archive_tasks(&base.tasks, &ours.tasks, &theirs.tasks);

    // Counted against what the file held, not per decided key: for an archive
    // the number worth printing is how many tasks it gained and lost, and a key
    // both sides already agreed on is neither.
    let after: std::collections::HashSet<String> = merged.iter().map(reconcile::task_key).collect();
    let took_theirs = after.difference(&before).count();
    let deleted = before.difference(&after).count();

    ours.tasks = merged;

    // An archive's shell is everything that is not a task: the `# Archive — x`
    // heading and whatever a person wrote above the first task, plus the note,
    // rule or comment below the last one.
    let shell = |a: &crate::model::archive::Archive| {
        let mut lines = a.header.clone();
        lines.extend(a.trailing.iter().cloned());
        lines
    };
    let choice = reconcile::decide_shell(
        &reconcile::shell_signature(&shell(&base)),
        &reconcile::shell_signature(&shell(&ours)),
        &reconcile::shell_signature(&shell(&theirs)),
    );
    let took_their_shell = choice == reconcile::ShellChoice::Theirs;
    if took_their_shell {
        ours.header = theirs.header.clone();
        ours.trailing = theirs.trailing.clone();
    } else if choice == reconcile::ShellChoice::Diverged {
        trace.conflicts.push(Conflict {
            key: reconcile::SURROUNDING_TEXT_KEY.to_string(),
            reason: ConflictReason::SurroundingTextDiverged,
            theirs: shell(&theirs),
        });
    }

    (
        serialize_archive(&ours),
        MergeReport {
            kind: FileKind::Archive,
            took_theirs,
            deleted,
            took_their_shell,
            conflicts: trace.conflicts,
            superseded: trace.superseded,
        },
    )
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
            // The inbox has no shell decision to report: its header merges as a
            // multiset, so neither side's lines are ever the ones set aside.
            took_their_shell: false,
            conflicts: Vec::new(),
            // The inbox merges by content as a multiset and has no fields to
            // merge, so nothing is ever superseded there.
            superseded: Vec::new(),
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
    let (merged, mut report) = match kind {
        FileKind::Track => merge_track_text(base, ours, theirs, stamp),
        FileKind::Archive => merge_archive_text(base, ours, theirs, stamp),
        FileKind::Inbox => merge_inbox_text(base, ours, theirs),
    };

    if report.conflicts.is_empty() {
        let lost = unaccounted_additions(base, theirs, &merged, &report.superseded);
        if !lost.is_empty() {
            report.conflicts.push(Conflict {
                key: reconcile::UNACCOUNTED_KEY.to_string(),
                reason: ConflictReason::UnaccountedLoss,
                theirs: lost,
            });
        }
    }

    (merged, report)
}

/// Lines only *they* added that did not reach the merged file.
///
/// # Why this is the question, and the only one worth asking here
///
/// A line in `theirs` that is missing from the result is usually right: we
/// deleted the task it belonged to, or we edited it and they did not. Checking
/// that every line survived would report a working merge as broken. But a line
/// that **is not in the ancestor** is different — nobody can have deleted
/// something that never existed for them to delete — so an addition of theirs
/// that vanished is a loss with no decision behind it, every time.
///
/// That makes this the one audit a merge can be held to without re-deriving
/// every decision it just made, and it is exactly the audit that was missing
/// when this driver silently emptied one side of every archive merge, and again
/// when it silently kept one side of every heading, title and note.
///
/// # It runs only on an otherwise-clean merge
///
/// A conflict already halts the operation and puts the discarded side in front
/// of a person — and a conflicted task is *supposed* to lose their lines, so
/// auditing then would report the merge working as designed. Silence is the
/// failure mode; silence is what this watches.
///
/// # What counts as the same line
///
/// Whitespace-normalized, blanks dropped. Blank lines are moved around by tasks
/// and by the serializer, and a line somebody respaced by hand is not a line
/// anybody added.
///
/// # Why `superseded` has to exist
///
/// The premise above — an addition of theirs that vanished had no decision
/// behind it — held exactly while a task was the unit of decision: the result
/// took one side's task lines verbatim or the other's. A **field-merged** task
/// breaks it honestly. Its task line is synthesized from both sides, so theirs'
/// version of that line is absent from the result by decision, and reading the
/// merge line by line cannot see the difference between that and a loss.
///
/// So the merge names the lines it superseded and this subtracts them. That is
/// a hole in the audit only if the merge can supersede a line without deciding
/// about it, which is what [`reconcile::merge_fields`]'s exhaustive `Task`
/// destructuring rules out: it decides every field or it conflicts, and a
/// conflict does not reach here at all.
fn unaccounted_additions(
    base: &str,
    theirs: &str,
    merged: &str,
    superseded: &[String],
) -> Vec<String> {
    fn normalize(line: &str) -> String {
        line.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    fn index(text: &str) -> std::collections::HashSet<String> {
        text.lines()
            .map(normalize)
            .filter(|line| !line.is_empty())
            .collect()
    }

    let base = index(base);
    let merged = index(merged);
    let superseded: std::collections::HashSet<String> = superseded
        .iter()
        .map(|line| normalize(line))
        .filter(|line| !line.is_empty())
        .collect();
    theirs
        .lines()
        .filter(|line| {
            let normalized = normalize(line);
            !normalized.is_empty()
                && !base.contains(&normalized)
                && !merged.contains(&normalized)
                && !superseded.contains(&normalized)
        })
        .map(|line| line.to_string())
        .collect()
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

    /// Every pattern `fr git setup` routes to the driver, and the kind it must
    /// resolve to. Adding a routed pattern without adding it here fails
    /// `every_routed_pattern_has_a_kind` below.
    const ROUTED: &[(&str, FileKind)] = &[
        ("frame/tracks/*.md", FileKind::Track),
        ("frame/archive/*.md", FileKind::Archive),
        ("frame/archive/_tracks/*.md", FileKind::Track),
        ("frame/inbox.md", FileKind::Inbox),
    ];

    /// **The test this whole fix exists to install.**
    ///
    /// `.gitattributes` decides what git hands the driver; `kind_for_path`
    /// decides how the driver reads it. When those two disagree, a file is
    /// parsed as a shape it does not have — which for `frame/archive/*.md` meant
    /// a done archive read as a track, zero tasks found, and one side of every
    /// archive merge silently discarded.
    ///
    /// Deriving the patterns from `attribute_lines` rather than restating them
    /// is the point: a new routed shape cannot be added without deciding, here,
    /// what it parses as.
    #[test]
    fn every_routed_pattern_has_a_kind() {
        let lines = crate::ops::git_setup::attribute_lines("frame");
        let patterns: Vec<&str> = lines
            .iter()
            .map(|l| l.split_whitespace().next().unwrap_or_default())
            .collect();

        assert_eq!(
            patterns.len(),
            ROUTED.len(),
            "`fr git setup` routes {} patterns but {} have a declared kind — \
             add the new one to ROUTED and give it a kind in `kind_for_path`.\n\
             routed: {patterns:?}",
            patterns.len(),
            ROUTED.len()
        );

        for (pattern, expected) in ROUTED {
            assert!(
                patterns.contains(pattern),
                "{pattern} is expected to be routed but `attribute_lines` does not write it: \
                 {patterns:?}"
            );
            // A pattern is a glob; a representative path exercises the matcher.
            let path = pattern.replace('*', "sample");
            assert_eq!(
                kind_for_path(&path),
                Some(*expected),
                "{path} (routed as {pattern}) must merge as {expected:?}"
            );

            // The same file in a project that is not at the repo root, and via
            // an absolute path. See `a_project_below_the_repo_root_still_routes`
            // for why both of these are shapes the driver really receives.
            for prefix in ["subdir/", "a/b/c/", "/Users/x/proj/"] {
                let nested = format!("{prefix}{path}");
                assert_eq!(
                    kind_for_path(&nested),
                    Some(*expected),
                    "{nested} must merge as {expected:?} — matching is on the trailing \
                     components, never anchored at the repo root"
                );
            }
        }
    }

    /// `.gitattributes` patterns are relative to the directory holding the file,
    /// but git passes `%P` relative to the **repo root** — so a project in a
    /// subdirectory routes on `frame/archive/*.md` and then hands the driver
    /// `subdir/frame/archive/main.md`. Anchoring the match at the root would
    /// decline every merge in such a project.
    ///
    /// The same goes for a path that did not come from git at all: the driver is
    /// not git-specific, and it already locates the project from the file being
    /// merged rather than from the working directory.
    #[test]
    fn a_project_below_the_repo_root_still_routes() {
        assert_eq!(
            kind_for_path("subdir/frame/archive/main.md"),
            Some(FileKind::Archive)
        );
        assert_eq!(
            kind_for_path("subdir/frame/archive/_tracks/old.md"),
            Some(FileKind::Track)
        );
        assert_eq!(
            kind_for_path("subdir/frame/inbox.md"),
            Some(FileKind::Inbox)
        );
        // Absolute, as another VCS or a hand-run may give it.
        assert_eq!(
            kind_for_path("/Users/x/proj/frame/archive/main.md"),
            Some(FileKind::Archive)
        );
        assert_eq!(
            kind_for_path("/Users/x/proj/frame/tracks/main.md"),
            Some(FileKind::Track)
        );
        // Windows separators, nested.
        assert_eq!(
            kind_for_path(r"subdir\frame\archive\main.md"),
            Some(FileKind::Archive)
        );

        // And none of that loosens the strictness: a deeper directory under
        // `archive/` is still not a shape frame knows.
        assert_eq!(kind_for_path("subdir/frame/archive/deep/x.md"), None);
        assert_eq!(kind_for_path("subdir/doc/inbox.md"), None);
    }

    #[test]
    fn track_paths_are_recognized() {
        assert_eq!(kind_for_path("frame/tracks/main.md"), Some(FileKind::Track));
        // A whole track `fr track archive` moved intact: sections and all, so it
        // really is a track — unlike its neighbour one directory up.
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

    /// The regression itself: a done archive is not a track, and must not be
    /// given a track's kind.
    #[test]
    fn a_done_archive_is_not_a_track() {
        assert_eq!(
            kind_for_path("frame/archive/main.md"),
            Some(FileKind::Archive)
        );
        assert_eq!(
            kind_for_path("sub/dir/frame/archive/main.md"),
            Some(FileKind::Archive)
        );
    }

    #[test]
    fn inbox_is_recognized_by_name() {
        assert_eq!(kind_for_path("frame/inbox.md"), Some(FileKind::Inbox));
    }

    /// With the catch-all gone, a path that merely *contains* a frame-ish
    /// directory name is declined rather than guessed at.
    #[test]
    fn only_the_known_shapes_route() {
        // Not directly under `archive/` or `tracks/`.
        assert_eq!(kind_for_path("frame/archive/deep/nested/x.md"), None);
        assert_eq!(kind_for_path("frame/tracks/sub/x.md"), None);
        // The right directory names, but not under `frame/`.
        assert_eq!(kind_for_path("docs/archive/notes.md"), None);
        assert_eq!(kind_for_path("tracks/main.md"), None);
        // An inbox somewhere else is not the project inbox.
        assert_eq!(kind_for_path("doc/inbox.md"), None);
    }

    /// The driver must decline anything it does not understand, which stops the
    /// merge rather than parsing a file as a shape it does not have.
    /// `project.toml` is not routed here at all and line-merges fine; a driver
    /// that answered for it would break a merge that works today.
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

    // --- Archive merges ---
    //
    // Promoted from `scratch/archive-merge-repro/repro.sh`, whose variants
    // reproduced the loss end to end against a real `git merge`.

    /// A done archive as `fr clean` writes one: a header, a flat task list, and
    /// whatever a person left below it. No `## Section` headers anywhere.
    fn archive(tasks: &[&str]) -> String {
        let mut s = String::from("# Archive — main\n\n");
        for t in tasks {
            s.push_str(t);
            s.push('\n');
        }
        s.push_str("\n<!-- notes below the list -->\n");
        s
    }

    const A1: &str = "- [x] `MAI-001` base one\n  - resolved: 2026-08-02";
    const A2: &str = "- [x] `MAI-002` base two\n  - resolved: 2026-08-02";
    const OURS_3: &str = "- [x] `MAI-003` ours three\n  - resolved: 2026-08-09";
    const THEIRS_5: &str = "- [x] `MAI-005` theirs five\n  - resolved: 2026-08-10";

    /// `repro.sh normal` — the case that silently lost work: two clones each ran
    /// `fr clean` since the common ancestor. The answer is the union.
    #[test]
    fn both_sides_appended_and_both_survive() {
        let (merged, report) = merge_archive_text(
            &archive(&[A1, A2]),
            &archive(&[A1, A2, OURS_3]),
            &archive(&[A1, A2, THEIRS_5]),
            STAMP,
        );

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        for id in ["MAI-001", "MAI-002", "MAI-003", "MAI-005"] {
            assert!(
                merged.contains(id),
                "{id} must survive the merge:\n{merged}"
            );
        }
        assert_eq!(report.took_theirs, 1, "one task came across");
        assert_eq!(report.deleted, 0);
        assert_eq!(report.kind, FileKind::Archive);
        // And it is still an archive, not a track: no section headers appear.
        assert!(!merged.contains("##"), "no section leaked in:\n{merged}");
        assert_eq!(crate::parse::parse_archive(&merged).tasks.len(), 4);
    }

    /// `repro.sh empty` — neither side had an archive before, so both created
    /// one. Every task on both sides is an addition.
    #[test]
    fn an_absent_base_archive_keeps_both_sides() {
        let (merged, report) =
            merge_archive_text("", &archive(&[OURS_3]), &archive(&[THEIRS_5]), STAMP);

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        assert!(
            merged.contains("MAI-003") && merged.contains("MAI-005"),
            "{merged}"
        );
    }

    /// `repro.sh dedupe` — one side's archive was rewritten wholesale rather
    /// than appended to, so every line differs as text. Identity does not care.
    #[test]
    fn a_wholesale_rewrite_still_merges_by_identity() {
        // Their side reordered and re-spaced everything a rewrite would touch.
        let theirs = format!(
            "# Archive — main\n\n{}\n{}\n{}\n\n<!-- notes below the list -->\n",
            THEIRS_5, A2, A1
        );

        let (merged, report) = merge_archive_text(
            &archive(&[A1, A2]),
            &archive(&[A1, A2, OURS_3]),
            &theirs,
            STAMP,
        );

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        for id in ["MAI-001", "MAI-002", "MAI-003", "MAI-005"] {
            assert!(merged.contains(id), "{id} must survive:\n{merged}");
        }
    }

    /// The same ID carrying different content on both sides. Ours stays in the
    /// file, theirs is handed back for the recovery log, and **no marker is
    /// written** — the file must still parse as an archive.
    #[test]
    fn a_double_edit_conflicts_keeps_ours_and_writes_no_marker() {
        let base = archive(&[A1]);
        let ours = archive(&["- [x] `MAI-001` our title\n  - resolved: 2026-08-02"]);
        let theirs = archive(&["- [x] `MAI-001` their title\n  - resolved: 2026-08-02"]);

        let (merged, report) = merge_archive_text(&base, &ours, &theirs, STAMP);

        assert!(!report.is_clean(), "this must not merge silently");
        assert_eq!(report.conflicts.len(), 1);
        assert!(merged.contains("our title"), "ours is kept:\n{merged}");
        assert!(!merged.contains("their title"), "theirs is not:\n{merged}");
        assert!(
            report.conflicts[0]
                .theirs
                .join("\n")
                .contains("their title"),
            "theirs is handed back for the log: {:?}",
            report.conflicts[0].theirs
        );

        // The two things that must never appear in an archive.
        assert!(!merged.contains("conflict:"), "no marker:\n{merged}");
        assert!(!merged.contains("<<<<"), "no conflict markers:\n{merged}");
        let reparsed = crate::parse::parse_archive(&merged);
        assert_eq!(reparsed.tasks.len(), 1, "and it still parses:\n{merged}");
    }

    /// Ours' header, the trailing note under the list, and the file's line
    /// ending all survive a merge that changed the task list.
    #[test]
    fn header_trailing_and_crlf_survive() {
        let base = archive(&[A1]).replace('\n', "\r\n");
        let ours = format!(
            "# Archive — main\r\n\r\n> a note someone added up here\r\n\r\n{}\r\n\r\n<!-- tail -->\r\n",
            A1.replace('\n', "\r\n")
        );
        let theirs = archive(&[A1, THEIRS_5]).replace('\n', "\r\n");

        let (merged, report) = merge_archive_text(&base, &ours, &theirs, STAMP);

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        assert!(merged.contains("MAI-005"), "theirs landed:\n{merged:?}");
        assert!(
            merged.contains("> a note someone added up here"),
            "ours' header survives:\n{merged:?}"
        );
        assert!(
            merged.contains("<!-- tail -->"),
            "trailing survives:\n{merged:?}"
        );
        assert!(
            !merged.contains("\n\n") || merged.contains("\r\n"),
            "line ending is preserved"
        );
        assert!(
            merged.lines().all(|l| !l.ends_with('\r')) || merged.contains("\r\n"),
            "CRLF throughout, not mixed:\n{merged:?}"
        );
        for line in merged.split("\r\n") {
            assert!(
                !line.contains('\n'),
                "a bare LF survived into a CRLF file:\n{merged:?}"
            );
        }
    }

    // --- The text around the tasks, at the driver level ---

    /// An archive's header used to be ours unconditionally, so a heading only
    /// they changed came back as ours with exit 0 and nothing said.
    #[test]
    fn an_archive_header_only_they_changed_is_taken() {
        let base = archive(&[A1]);
        let theirs = base.replace(
            "# Archive — main",
            "# Archive — main (quarterly; see RUNBOOK)",
        );

        let (merged, report) = merge_archive_text(&base, &base, &theirs, STAMP);

        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        assert!(report.took_their_shell, "and the caller is told");
        assert!(merged.contains("see RUNBOOK"), "{merged}");
    }

    /// Both sides changed it: ours stands and theirs is recorded, rather than
    /// theirs going nowhere. The key names no task, so no marker is written —
    /// the non-zero exit is what carries it, as it does for every archive
    /// conflict.
    #[test]
    fn an_archive_header_both_sides_changed_conflicts() {
        let base = archive(&[A1]);
        let ours = base.replace("# Archive — main", "# Archive — main (ours)");
        let theirs = base.replace("# Archive — main", "# Archive — main (theirs)");

        let (merged, report) = merge_archive_text(&base, &ours, &theirs, STAMP);

        assert!(!report.is_clean());
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].key, reconcile::SURROUNDING_TEXT_KEY);
        assert!(merged.contains("(ours)"), "{merged}");
        assert!(
            !merged.contains("conflict:"),
            "no marker in an archive:\n{merged}"
        );
        assert!(
            report.conflicts[0].theirs.join("\n").contains("(theirs)"),
            "theirs reaches the recovery log: {:?}",
            report.conflicts[0].theirs
        );
    }

    /// The whole reported failure, at the level the VCS sees: a heading only
    /// they repaired, against a different one only we added. It used to return
    /// our file byte for byte and report the merge clean.
    #[test]
    fn a_track_heading_frame_does_not_model_no_longer_merges_clean() {
        let base = "# Backend\n\n## Backlog\n\n- [ ] `BAC-254` Rework the reds\n";
        let ours = base.replace(
            "## Backlog",
            "## OPEN:\n\n- what about the reds?\n\n## Backlog",
        );
        let theirs = base.replace(
            "## Backlog",
            "## Notes\n\nNow `_rejected`, was `_reds`.\n\n## Backlog",
        );

        let (merged, report) = merge_text(FileKind::Track, base, &ours, &theirs, STAMP);

        assert!(!report.is_clean(), "it must not report clean:\n{merged}");
        assert_eq!(merged, ours, "ours is what is kept");
        assert!(
            report.conflicts[0].theirs.join("\n").contains("_rejected"),
            "and theirs is what is handed back: {:?}",
            report.conflicts[0].theirs
        );
    }

    /// The audit. It cannot be reached through the merge any more, so it is
    /// exercised directly: a result that quietly failed to carry a line only
    /// they added is a defect, whatever produced it.
    #[test]
    fn a_line_only_they_added_going_missing_is_caught() {
        let base = "# A\n\n## Backlog\n";
        let theirs = "# A\n\n## Note\n\nsomething they wrote\n\n## Backlog\n";

        assert!(
            unaccounted_additions(base, theirs, base, &[])
                .iter()
                .any(|l| l.contains("something they wrote")),
            "a line theirs added and the result lacks is unaccounted for"
        );
        assert!(
            unaccounted_additions(base, theirs, theirs, &[]).is_empty(),
            "and a result that carried it is not"
        );
    }

    /// What the audit must *not* do. The overwhelmingly common merge is one
    /// where we edited a line and they did not: their version of it is missing
    /// from the result on purpose, and reporting that would make the check
    /// useless by making it always fire.
    #[test]
    fn a_line_we_edited_and_they_did_not_is_accounted_for() {
        let base = "# A\n\n## Backlog\n\n- [ ] `A-001` One\n";
        let ours = base.replace("One", "One, edited here");

        let (merged, report) = merge_text(FileKind::Track, base, &ours, base, STAMP);
        assert!(report.is_clean(), "conflicts: {:?}", report.conflicts);
        assert!(merged.contains("One, edited here"), "{merged}");
    }

    /// And whitespace is not content: a line somebody respaced by hand did not
    /// become a line they added.
    #[test]
    fn respacing_a_line_is_not_adding_one() {
        let base = "# A\n\n## Backlog\n\n- [ ] `A-001` One\n";
        let theirs = "# A\n\n##   Backlog\n\n- [ ]  `A-001`  One\n";
        assert!(unaccounted_additions(base, theirs, base, &[]).is_empty());
    }

    /// Two ID-less archived tasks sharing a title cannot be matched across
    /// sides, so the merge declines rather than guessing — the guard
    /// `reconcile_track` has always had, which the flat subtask path does not.
    #[test]
    fn an_ambiguous_title_declines_rather_than_guessing() {
        let dup = "- [x] duplicate title\n- [x] duplicate title";
        let base = archive(&[dup]);
        let ours = archive(&[dup, OURS_3]);
        let theirs = archive(&[dup, THEIRS_5]);

        let (merged, report) = merge_archive_text(&base, &ours, &theirs, STAMP);

        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.reason == reconcile::ConflictReason::AmbiguousTitle),
            "the ambiguous pair is reported: {:?}",
            report.conflicts
        );
        // Both sides' unambiguous additions still land.
        assert!(
            merged.contains("MAI-003") && merged.contains("MAI-005"),
            "{merged}"
        );
    }

    /// Order is deterministic: ours in ours' order, then their additions.
    /// Position carries no meaning in an archive, so any stable order will do —
    /// but it must be stable, and the same set must come out either way round.
    #[test]
    fn merged_order_is_deterministic_and_the_set_is_symmetric() {
        let base = archive(&[A1]);
        let ours = archive(&[A1, OURS_3]);
        let theirs = archive(&[A1, THEIRS_5]);

        let (a_then_b, _) = merge_archive_text(&base, &ours, &theirs, STAMP);
        let (again, _) = merge_archive_text(&base, &ours, &theirs, STAMP);
        assert_eq!(a_then_b, again, "the same inputs give the same bytes");

        let ids = |s: &str| {
            let mut v: Vec<String> = crate::parse::parse_archive(s)
                .tasks
                .iter()
                .filter_map(|t| t.id.as_ref().map(|i| i.to_string()))
                .collect();
            v.sort();
            v
        };
        let (b_then_a, _) = merge_archive_text(&base, &theirs, &ours, STAMP);
        assert_eq!(
            ids(&a_then_b),
            ids(&b_then_a),
            "the task *set* does not depend on which side is ours"
        );

        // Ours' order is kept, and their addition follows it.
        let order: Vec<String> = crate::parse::parse_archive(&a_then_b)
            .tasks
            .iter()
            .filter_map(|t| t.id.as_ref().map(|i| i.to_string()))
            .collect();
        assert_eq!(order, vec!["MAI-001", "MAI-003", "MAI-005"]);
    }

    // --- Field-level conflict ---
    //
    // The six real conflicts a project running several concurrent writers
    // reported, classified by which fields each side touched. Fixed cases, not
    // property seeds: each names a shape the merge must get right, and a case
    // that is a fixed test cannot be orphaned by a change to a generator.

    /// A track with one task, built from the metadata lines given.
    fn one_task(state: char, title: &str, meta: &[&str]) -> String {
        let mut text = format!("# Main\n\n## Backlog\n\n- [{state}] `BAC-1` {title}\n");
        for line in meta {
            text.push_str(&format!("  - {line}\n"));
        }
        text.push_str("\n## Done\n");
        text
    }

    /// `BAC-b243`, first shape: they edited the note, both made the *same* edit
    /// to `ref:`. Whole-task equality already handled "both did the identical
    /// thing"; this is one field identical and another one-sided, which it did
    /// not, and the task conflicted for it.
    #[test]
    fn a_one_sided_note_and_an_identical_ref_edit_merge() {
        let base = one_task(' ', "Task", &["ref: doc/a.md"]);
        let ours = one_task(' ', "Task", &["ref: doc/a.md, doc/design/new.md"]);
        let theirs = one_task(
            ' ',
            "Task",
            &["ref: doc/a.md, doc/design/new.md", "note: what we found"],
        );

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        assert!(merged.contains("doc/design/new.md"), "{merged}");
        assert!(merged.contains("what we found"), "{merged}");
    }

    /// `BAC-b243`, second shape: a note from us and a `ref:` from them. Two
    /// fields, one writer each, and nothing to adjudicate between them.
    #[test]
    fn a_note_from_one_side_and_a_ref_from_the_other_merge() {
        let base = one_task(' ', "Task", &["added: 2026-01-01"]);
        let ours = one_task(' ', "Task", &["added: 2026-01-01", "note: ours"]);
        let theirs = one_task(' ', "Task", &["added: 2026-01-01", "ref: doc/theirs.md"]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        assert!(merged.contains("note: ours"), "{merged}");
        assert!(merged.contains("doc/theirs.md"), "{merged}");
    }

    /// `SEC-b61`: we finished it — state and `resolved:` — while they wrote the
    /// note. Three fields, no overlap.
    #[test]
    fn finishing_a_task_and_annotating_it_are_not_a_conflict() {
        let base = one_task(' ', "Task", &["added: 2026-01-01"]);
        let ours = one_task('x', "Task", &["added: 2026-01-01", "resolved: 2026-02-02"]);
        let theirs = one_task(' ', "Task", &["added: 2026-01-01", "note: theirs"]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        assert!(merged.contains("- [x]"), "our state survives:\n{merged}");
        assert!(merged.contains("resolved: 2026-02-02"), "{merged}");
        assert!(merged.contains("note: theirs"), "{merged}");
    }

    /// A multi-line note, in the block form `doc/format.md` documents: `note:`
    /// alone on its line, the body indented under it. That form is one
    /// `Metadata::Note` holding the whole body, which is what makes the note a
    /// single field.
    ///
    /// Worth spelling out rather than writing inline, because the *other* shape
    /// — text on the `note:` line **and** indented lines under it — is not a
    /// note at all: the continuation lines are content the parser cannot
    /// attribute, and they travel as `Task::trailing_lines`. A test written that
    /// way passes for the wrong reason, exercising the stranded-line field
    /// instead of the note.
    fn block_note(body: &[&str]) -> String {
        let mut text = "# Main\n\n## Backlog\n\n- [ ] `BAC-1` Task\n  - note:\n".to_string();
        for line in body {
            text.push_str(&format!("    {line}\n"));
        }
        text.push_str("\n## Done\n");
        text
    }

    /// `BAC-b125` / `BAC-b150`: both appended to the note, at the same point, in
    /// different words. **This must still conflict** — the note is one opaque
    /// value, and two writers disagreeing inside it is the case no automatic
    /// answer is right for.
    #[test]
    fn two_notes_appended_at_once_still_conflict() {
        let base = block_note(&["shared history"]);
        let ours = block_note(&["shared history", "ours found it"]);
        let theirs = block_note(&["shared history", "theirs found it"]);

        let (_, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
        assert_eq!(report.conflicts[0].reason, ConflictReason::BothEdited);
    }

    /// The case field granularity does **not** reach, pinned so the limit is a
    /// stated fact rather than something rediscovered later: two sides editing
    /// one note in line ranges nowhere near each other. A plain three-way line
    /// merge resolves this; the note is one value, so frame conflicts.
    ///
    /// It is the shape a dated-episode note accumulates — appends at the tail
    /// against an edit far above — and it is what would have to change for the
    /// note to merge. Flip this test when it does.
    #[test]
    fn a_note_both_sides_edited_far_apart_conflicts_for_now() {
        let base = block_note(&["intro", "middle", "tail"]);
        let ours = block_note(&["intro", "middle", "tail", "appended by us"]);
        let theirs = block_note(&["intro, corrected", "middle", "tail"]);

        let (_, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
        assert_eq!(report.conflicts[0].reason, ConflictReason::BothEdited);
    }

    /// Both sides removed the same path and added the same one: identical set
    /// operations, which conflicted because the *value* differed from the
    /// ancestor on both sides.
    #[test]
    fn identical_ref_set_edits_are_not_a_conflict() {
        let base = one_task(' ', "Task", &["ref: doc/investigations/BAC-241.md"]);
        let side = one_task(' ', "Task", &["ref: doc/design/effects.md"]);

        let (merged, report) = merge_track_text(&base, &side, &side, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        assert!(merged.contains("doc/design/effects.md"), "{merged}");
        assert!(!merged.contains("BAC-241"), "the removal holds:\n{merged}");
    }

    /// Each side added a different path. A `ref:` is a set of file paths, so two
    /// writers adding to it have not disagreed about anything.
    #[test]
    fn refs_added_by_both_sides_are_unioned() {
        let base = one_task(' ', "Task", &["ref: doc/a.md"]);
        let ours = one_task(' ', "Task", &["ref: doc/a.md, doc/ours.md"]);
        let theirs = one_task(' ', "Task", &["ref: doc/a.md, doc/theirs.md"]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        for path in ["doc/a.md", "doc/ours.md", "doc/theirs.md"] {
            assert!(merged.contains(path), "{path} missing from:\n{merged}");
        }
    }

    /// A removal on one side is honoured against an addition on the other. The
    /// alternative — a union that resurrects what somebody deleted — is the
    /// failure a set merge is most likely to have.
    #[test]
    fn a_removed_ref_stays_removed_when_the_other_side_adds_one() {
        let base = one_task(' ', "Task", &["ref: doc/a.md, doc/b.md"]);
        let ours = one_task(' ', "Task", &["ref: doc/a.md, doc/b.md, doc/ours.md"]);
        let theirs = one_task(' ', "Task", &["ref: doc/a.md"]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        assert!(merged.contains("doc/a.md"), "{merged}");
        assert!(merged.contains("doc/ours.md"), "{merged}");
        assert!(
            !merged.contains("doc/b.md"),
            "their removal holds:\n{merged}"
        );
    }

    /// Tags ride on the task line, which the P10 property excuses when both
    /// sides changed it — so the one case the property cannot see gets a fixed
    /// test instead. Two sides tagging one task differently is a divergent edit
    /// to one field and still conflicts.
    #[test]
    fn tags_changed_by_both_sides_conflict_and_ours_survives() {
        let base = one_task(' ', "Task #core", &[]);
        let ours = one_task(' ', "Task #core #ours", &[]);
        let theirs = one_task(' ', "Task #core #theirs", &[]);

        let (merged, report) = merge_track_text(&base, &ours, &theirs, STAMP);
        assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
        assert!(merged.contains("#ours"), "{merged}");
    }

    /// A title from one side and a state from the other rebuild the task line
    /// from both. The merged line is neither side's, which is what
    /// `Trace::superseded` exists to tell the audit — so this also checks the
    /// audit stayed quiet rather than reporting the merge as a defect.
    #[test]
    fn a_title_and_a_state_recombine_into_one_task_line() {
        let base = one_task(' ', "One", &["added: 2026-01-01"]);
        let ours = one_task(' ', "Retitled", &["added: 2026-01-01"]);
        let theirs = one_task('x', "One", &["added: 2026-01-01"]);

        let (merged, report) = merge_text(FileKind::Track, &base, &ours, &theirs, STAMP);
        assert!(report.is_clean(), "{:?}", report.conflicts);
        assert!(merged.contains("- [x] `BAC-1` Retitled"), "{merged}");
    }

    /// Two sides moving one task to *different* sections is a divergent edit to
    /// the one field `merge_fields` does not hold — the section — and
    /// `section_for` prefers ours, so without an explicit check the two moves
    /// would compose silently into ours.
    #[test]
    fn two_different_section_moves_still_conflict() {
        let base = "# Main\n\n## Backlog\n\n- [ ] `BAC-1` Task\n\n## Parked\n\n## Done\n";
        let ours = "# Main\n\n## Backlog\n\n## Parked\n\n- [~] `BAC-1` Task\n\n## Done\n";
        let theirs = "# Main\n\n## Backlog\n\n## Parked\n\n## Done\n\n- [x] `BAC-1` Task\n";

        let (_, report) = merge_track_text(base, ours, theirs, STAMP);
        assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
        assert_eq!(report.conflicts[0].reason, ConflictReason::BothEdited);
    }
}
