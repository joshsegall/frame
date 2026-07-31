//! Automatic repair for a subset of [`crate::ops::check`] findings.
//!
//! # Why this is not part of `fr clean`
//!
//! `fr clean` is frame's maintenance command, and it already repairs four of the
//! diagnostics check reports: missing IDs, missing `added:`/`resolved:` dates,
//! duplicate IDs, and tasks sitting in the wrong section. Anything repairable
//! that belongs there should go there, not here — two commands fixing the same
//! finding would drift.
//!
//! The line between them is not how destructive a repair is. `fr clean` already
//! archives tasks and renumbers IDs, both destructive. The line is **whether the
//! repair is correct with nobody watching**:
//!
//! > `fr clean` runs unattended — `auto_clean = true` runs it after every file
//! > reload in the TUI (see `doc/concepts.md`). So it may only do what a user
//! > would be happy to have happen silently, in the background, without being
//! > told. Everything else belongs here, behind `fr check --fix`, which is
//! > invoked deliberately after a diagnosis has been read.
//!
//! Assigning an ID passes that test — it happens constantly and is the point of
//! the feature. Closing a code fence does not: it edits prose the user wrote,
//! and they may be halfway through writing it.
//!
//! # What is deliberately not repaired
//!
//! Most check findings have no safe automatic repair, and the reasons are worth
//! keeping next to the code that could otherwise be tempted to add them:
//!
//! - `IdReissuedAfterArchive` — renumbering a live task rewrites an ID that
//!   other work may already reference.
//! - `ActorNameCollision` — the repair is `fr actor merge`, which renumbers a
//!   whole namespace. A human call, already documented as one.
//! - `ActorTokenRetiredButHeld` — reactivate the token, or claim a fresh one?
//!   That is an identity decision.
//! - `ActorTokenUnregistered` — already self-heals on the next mint.
//! - `LostTask` — the recovery system flagged content *for human review*.
//!   Clearing the tag automatically defeats the purpose.
//! - `LocalFileCommitted` where git already **tracks** the file — needs
//!   `git rm --cached`; mutating the git index is outside frame's remit. The
//!   not-yet-ignored half *is* repaired here.
//! - `IdFrontierUnreadable` — check deliberately leaves the store in place so the
//!   warning names a file still worth inspecting (`doc/architecture.md`).
//! - `DanglingDep` — removing the dep discards intent; the blocker may be about
//!   to be created.
//! - `BrokenRef` / `BrokenSpec` — a path can be legitimately absent on the
//!   current branch. Deleting refs after a branch switch would be badly wrong.

use serde::Serialize;

use std::path::Path;

use crate::model::project::Project;
use crate::model::task::{Metadata, Task};
use crate::model::track::TrackNode;
use crate::ops::check::{CheckResult, CheckWarning};

/// A single repair: what would change, and enough information to apply it.
///
/// Ordered as reported. Each variant knows whether applying it removes bytes —
/// see [`Repair::deletes`], which is what gates the confirmation prompt.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Repair {
    /// Append a closing fence to a task note that leaves one open.
    #[serde(rename = "close_note_fence")]
    CloseNoteFence {
        track_id: String,
        /// `None` for a task with no ID yet; `title` locates it instead.
        task_id: Option<String>,
        title: String,
        /// The unclosed opening fence, as reported by check.
        fence: String,
        /// The line that will be appended.
        closer: String,
    },
    /// Append a closing fence to an inbox item body that leaves one open.
    #[serde(rename = "close_inbox_fence")]
    CloseInboxFence {
        /// 1-based, matching `fr inbox` and `fr triage`.
        index: usize,
        title: String,
        fence: String,
        closer: String,
    },
    /// Add a working-copy-local frame file to `.gitignore`.
    #[serde(rename = "add_gitignore_entry")]
    AddGitignoreEntry {
        /// Repo-relative path, e.g. `frame/.actor`.
        path: String,
    },
    /// Drop the extra copies of a task duplicated inside the archives, keeping
    /// the first. **Deletes.**
    #[serde(rename = "dedupe_archived_task")]
    DedupeArchivedTask {
        task_id: String,
        /// How many copies exist across the archives; one will remain.
        total: usize,
        /// Archive paths holding it, as check reports them.
        archives: Vec<String>,
    },
    /// Remove the leftover frontier-store backup. **Deletes.**
    #[serde(rename = "remove_frontier_backup")]
    RemoveFrontierBackup { path: String },
}

impl Repair {
    /// Whether applying this removes bytes that cannot be reconstructed from
    /// what remains. Repairs that only add are applied without confirmation;
    /// these require `--yes` or an interactive `y`.
    pub fn deletes(&self) -> bool {
        match self {
            Repair::CloseNoteFence { .. }
            | Repair::CloseInboxFence { .. }
            | Repair::AddGitignoreEntry { .. } => false,
            Repair::DedupeArchivedTask { .. } | Repair::RemoveFrontierBackup { .. } => true,
        }
    }

    /// One line, for the plan the user reads before confirming.
    pub fn describe(&self) -> String {
        match self {
            Repair::CloseNoteFence {
                track_id,
                task_id,
                title,
                fence,
                ..
            } => {
                let who = task_id.clone().unwrap_or_else(|| format!("\"{title}\""));
                format!("[{track_id}] {who}: close note fence opened by `{fence}`")
            }
            Repair::CloseInboxFence {
                index,
                title,
                fence,
                ..
            } => {
                format!("inbox {index} \"{title}\": close body fence opened by `{fence}`")
            }
            Repair::AddGitignoreEntry { path } => {
                format!(".gitignore: add `{path}`")
            }
            Repair::DedupeArchivedTask {
                task_id,
                total,
                archives,
            } => {
                format!(
                    "{task_id}: delete {} duplicate archive cop{} ({}), keeping one",
                    total - 1,
                    if *total == 2 { "y" } else { "ies" },
                    archives.join(", ")
                )
            }
            Repair::RemoveFrontierBackup { path } => {
                format!("delete stale frontier backup {path}")
            }
        }
    }
}

/// Outcome of applying a plan.
#[derive(Debug, Default, Serialize)]
pub struct FixResult {
    pub applied: Vec<Repair>,
    /// Repairs that could not be applied, with why. A task renamed or removed
    /// between the plan and the apply lands here rather than failing the run.
    pub skipped: Vec<SkippedRepair>,
}

#[derive(Debug, Serialize)]
pub struct SkippedRepair {
    pub repair: Repair,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// Turn a [`CheckResult`] into the repairs that can be applied for it.
///
/// **Derived from the check result, not re-derived from the project**, so the
/// plan is exactly what check reported — one warning in, at most one repair out.
/// Re-deriving looked simpler and was wrong: check only warns about a
/// `.gitignore` entry whose file *exists* and is not already ignored, so a
/// project-derived plan offered to add all six entries in response to a single
/// warning about one.
///
/// The `_ => {}` arm is the list of findings with no safe automatic repair,
/// enumerated with reasons in this module's header.
pub fn plan(check: &CheckResult) -> Vec<Repair> {
    let mut plan = Vec::new();
    for warning in &check.warnings {
        match warning {
            CheckWarning::UnclosedNoteFence {
                track_id,
                task_id,
                title,
                fence,
            } => plan.push(Repair::CloseNoteFence {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                title: title.clone(),
                closer: closer_for(fence),
                fence: fence.clone(),
            }),
            CheckWarning::UnclosedInboxFence {
                index,
                title,
                fence,
            } => plan.push(Repair::CloseInboxFence {
                index: *index,
                title: title.clone(),
                closer: closer_for(fence),
                fence: fence.clone(),
            }),
            // Only the not-yet-ignored half. A path git already tracks needs
            // `git rm --cached` too, which this will not do.
            CheckWarning::LocalFileCommitted {
                path,
                tracked: false,
            } => plan.push(Repair::AddGitignoreEntry { path: path.clone() }),
            CheckWarning::DuplicateArchivedId {
                task_id,
                total,
                archives,
            } => plan.push(Repair::DedupeArchivedTask {
                task_id: task_id.clone(),
                total: *total,
                archives: archives.clone(),
            }),
            CheckWarning::IdFrontierWasReset { path } => {
                plan.push(Repair::RemoveFrontierBackup { path: path.clone() })
            }
            _ => {}
        }
    }
    plan
}

/// The closing line for an opening fence.
///
/// CommonMark: a closer needs at least as many backticks as the opener and no
/// info string. Matching the opener's run length exactly satisfies both and keeps
/// the block visually paired.
fn closer_for(opening_fence: &str) -> String {
    let ticks = opening_fence.chars().take_while(|c| *c == '`').count();
    "`".repeat(ticks.max(3))
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// Apply `plan` to `project` in memory, returning what landed.
///
/// Does not write to disk — the caller saves the tracks and inbox that changed,
/// under the project lock, so this stays testable without a filesystem. The
/// `.gitignore` repair is the exception: it edits a file outside `frame/` that
/// has no in-memory model.
///
/// Idempotent. Re-running against an already-repaired project produces an empty
/// plan, so a second `--fix` is a no-op — the property `fr clean`'s archive
/// append had to learn the hard way.
pub fn apply(project: &mut Project, plan: &[Repair]) -> FixResult {
    let mut result = FixResult::default();

    for repair in plan {
        match repair {
            Repair::CloseNoteFence {
                track_id,
                task_id,
                title,
                closer,
                ..
            } => match apply_note_fence(project, track_id, task_id.as_deref(), title, closer) {
                Ok(()) => result.applied.push(repair.clone()),
                Err(reason) => result.skipped.push(SkippedRepair {
                    repair: repair.clone(),
                    reason,
                }),
            },
            Repair::CloseInboxFence {
                index,
                title,
                closer,
                ..
            } => match apply_inbox_fence(project, *index, title, closer) {
                Ok(()) => result.applied.push(repair.clone()),
                Err(reason) => result.skipped.push(SkippedRepair {
                    repair: repair.clone(),
                    reason,
                }),
            },
            Repair::AddGitignoreEntry { path } => {
                // `path` is repo-relative, as check reports it, so the
                // `.gitignore` it belongs in is the repo's — not the project
                // root, which differs when a frame project lives in a
                // subdirectory of the repo.
                match crate::io::git::repo_paths(&project.frame_dir) {
                    Some(paths) => {
                        match crate::io::project_io::append_gitignore_entry(&paths.toplevel, path) {
                            Ok(()) => result.applied.push(repair.clone()),
                            Err(e) => result.skipped.push(SkippedRepair {
                                repair: repair.clone(),
                                reason: e.to_string(),
                            }),
                        }
                    }
                    None => result.skipped.push(SkippedRepair {
                        repair: repair.clone(),
                        reason: "not a git repository".to_string(),
                    }),
                }
            }
            Repair::DedupeArchivedTask {
                task_id, archives, ..
            } => match dedupe_archived(&project.frame_dir, task_id, archives) {
                Ok(()) => result.applied.push(repair.clone()),
                Err(reason) => result.skipped.push(SkippedRepair {
                    repair: repair.clone(),
                    reason,
                }),
            },
            Repair::RemoveFrontierBackup { path } => {
                match std::fs::remove_file(path) {
                    Ok(()) => result.applied.push(repair.clone()),
                    // Already gone is the outcome we wanted, not a failure —
                    // `--fix` must stay idempotent.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        result.applied.push(repair.clone())
                    }
                    Err(e) => result.skipped.push(SkippedRepair {
                        repair: repair.clone(),
                        reason: e.to_string(),
                    }),
                }
            }
        }
    }

    result
}

/// Keep the first archived copy of `task_id` and drop the rest.
///
/// Every copy removed goes to the recovery log first, with its source text, so a
/// duplicate that was hand-edited after the first write is recoverable rather
/// than gone. That is the same guarantee `fr clean`'s archive append makes when
/// it drops a live copy that diverged from its archived twin.
///
/// The surviving copy is the first encountered, in the order check reports the
/// archives. Remaining tasks are untouched and still clean, so they serialize
/// verbatim: the file is byte-identical apart from the removed blocks.
fn dedupe_archived(frame_dir: &Path, task_id: &str, archives: &[String]) -> Result<(), String> {
    let mut seen = false;

    for rel in archives {
        let path = frame_dir.join(rel);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let mut track = crate::parse::parse_track(&content);

        let mut removed = Vec::new();
        for node in &mut track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                tasks.retain(|task| {
                    if task.id.as_deref() != Some(task_id) {
                        return true;
                    }
                    if !seen {
                        seen = true;
                        return true;
                    }
                    removed.push(task.clone());
                    false
                });
            }
        }

        if removed.is_empty() {
            continue;
        }

        for task in &removed {
            crate::io::recovery::log_recovery(
                frame_dir,
                crate::io::recovery::RecoveryEntry {
                    timestamp: chrono::Utc::now(),
                    category: crate::io::recovery::RecoveryCategory::Delete,
                    description: format!("duplicate archive copy of {task_id} removed"),
                    fields: vec![
                        ("Archive".to_string(), rel.clone()),
                        ("Task".to_string(), task.title.clone()),
                    ],
                    body: task.source_text.clone().unwrap_or_default().join("\n"),
                },
            );
        }

        let out = crate::parse::serialize_track(&track);
        crate::io::recovery::atomic_write(&path, out.as_bytes())
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }

    if seen {
        Ok(())
    } else {
        Err(format!("{task_id} no longer appears in the archives"))
    }
}

fn apply_note_fence(
    project: &mut Project,
    track_id: &str,
    task_id: Option<&str>,
    title: &str,
    closer: &str,
) -> Result<(), String> {
    let track = project
        .tracks
        .iter_mut()
        .find(|(id, _)| id == track_id)
        .map(|(_, t)| t)
        .ok_or_else(|| format!("track '{track_id}' not found"))?;

    match task_id {
        Some(id) => {
            let task = crate::ops::task_ops::find_task_mut_in_track(track, id)
                .ok_or_else(|| format!("task '{id}' not found"))?;
            if close_open_fence(task, closer) {
                Ok(())
            } else {
                Err("note no longer has an unclosed fence".to_string())
            }
        }
        // An ID-less task is located by title. Ambiguous titles are possible, so
        // the first still-unclosed match is the one to repair; a second run
        // repairs the next.
        None => {
            for node in &mut track.nodes {
                if let TrackNode::Section { tasks, .. } = node
                    && close_first_open_fence_by_title(tasks, title, closer)
                {
                    return Ok(());
                }
            }
            Err(format!("no task \"{title}\" with an unclosed fence"))
        }
    }
}

/// Append `closer` to the first note on `task` that leaves a fence open.
/// Returns whether anything changed.
fn close_open_fence(task: &mut Task, closer: &str) -> bool {
    let mut closed = false;
    for meta in &mut task.metadata {
        if let Metadata::Note(body) = meta
            && crate::ops::check::unclosed_fence(body).is_some()
        {
            body.push('\n');
            body.push_str(closer);
            closed = true;
            break;
        }
    }
    if closed {
        task.dirty = true;
    }
    closed
}

fn close_first_open_fence_by_title(tasks: &mut [Task], title: &str, closer: &str) -> bool {
    for task in tasks.iter_mut() {
        if task.title == title && close_open_fence(task, closer) {
            return true;
        }
        if close_first_open_fence_by_title(&mut task.subtasks, title, closer) {
            return true;
        }
    }
    false
}

fn apply_inbox_fence(
    project: &mut Project,
    index: usize,
    title: &str,
    closer: &str,
) -> Result<(), String> {
    let inbox = project
        .inbox
        .as_mut()
        .ok_or_else(|| "no inbox".to_string())?;
    let item = inbox
        .items
        .get_mut(index.saturating_sub(1))
        .ok_or_else(|| format!("inbox item {index} not found"))?;
    if item.title != title {
        return Err(format!("inbox item {index} is no longer \"{title}\""));
    }
    let body = item
        .body
        .as_mut()
        .ok_or_else(|| format!("inbox item {index} has no body"))?;
    if crate::ops::check::unclosed_fence(body).is_none() {
        return Err("body no longer has an unclosed fence".to_string());
    }
    body.push('\n');
    body.push_str(closer);
    item.dirty = true;
    Ok(())
}

/// Which tracks a plan touches, so the caller knows what to save.
pub fn tracks_touched(result: &FixResult) -> Vec<String> {
    let mut out: Vec<String> = result
        .applied
        .iter()
        .filter_map(|r| match r {
            Repair::CloseNoteFence { track_id, .. } => Some(track_id.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether a plan changed the inbox, so the caller knows to save it.
pub fn inbox_touched(result: &FixResult) -> bool {
    result
        .applied
        .iter()
        .any(|r| matches!(r, Repair::CloseInboxFence { .. }))
}

/// How many repairs in `plan` remove bytes. This is the confirmation gate — a
/// caller should not have to know which variants are the deleting ones.
pub fn deleting_count(plan: &[Repair]) -> usize {
    plan.iter().filter(|r| r.deletes()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::check::CheckError;

    fn result_with(warnings: Vec<CheckWarning>) -> CheckResult {
        CheckResult {
            errors: Vec::new(),
            warnings,
            ..Default::default()
        }
    }

    #[test]
    fn closer_matches_the_opening_run_length() {
        assert_eq!(closer_for("```"), "```");
        assert_eq!(closer_for("```rust"), "```");
        assert_eq!(closer_for("````"), "````");
        assert_eq!(closer_for("`````lace"), "`````");
        // A closer is never shorter than three, whatever it was handed.
        assert_eq!(closer_for(""), "```");
    }

    #[test]
    fn plan_covers_exactly_the_repairable_warnings() {
        let plan = plan(&result_with(vec![
            CheckWarning::UnclosedNoteFence {
                track_id: "t".into(),
                task_id: Some("T-1".into()),
                title: "task".into(),
                fence: "```rust".into(),
            },
            CheckWarning::UnclosedInboxFence {
                index: 1,
                title: "item".into(),
                fence: "```".into(),
            },
            CheckWarning::LocalFileCommitted {
                path: "frame/.actor".into(),
                tracked: false,
            },
            CheckWarning::DuplicateArchivedId {
                task_id: "T-9".into(),
                total: 2,
                archives: vec!["archive/t.md".into()],
            },
            CheckWarning::IdFrontierWasReset {
                path: "/x/frame-ids.toml.bak".into(),
            },
        ]));
        assert_eq!(plan.len(), 5);
    }

    /// The findings with no safe automatic repair must produce nothing. If a new
    /// warning is added and someone wants it repaired, that is a deliberate
    /// decision made in `plan`, not a default.
    #[test]
    fn plan_ignores_warnings_with_no_safe_repair() {
        let plan = plan(&result_with(vec![
            // A tracked file needs `git rm --cached` as well, so it is not ours.
            CheckWarning::LocalFileCommitted {
                path: "frame/.actor".into(),
                tracked: true,
            },
            CheckWarning::IdReissuedAfterArchive {
                task_id: "T-1".into(),
                tracks: vec!["t".into()],
                archives: vec!["archive/t.md".into()],
            },
            CheckWarning::ActorNameCollision {
                name: "host".into(),
                tokens: vec!["a".into(), "b".into()],
            },
            CheckWarning::LostTask {
                track_id: "t".into(),
                task_id: "T-2".into(),
            },
            CheckWarning::IdFrontierUnreadable {
                path: "/x".into(),
                detail: "bad".into(),
            },
            CheckWarning::MissingId {
                track_id: "t".into(),
                title: "x".into(),
            },
            CheckWarning::MissingAddedDate {
                track_id: "t".into(),
                task_id: "T-3".into(),
            },
            CheckWarning::MissingResolvedDate {
                track_id: "t".into(),
                task_id: "T-4".into(),
            },
            CheckWarning::DoneInBacklog {
                track_id: "t".into(),
                task_id: "T-5".into(),
            },
        ]));
        assert!(
            plan.is_empty(),
            "expected no repairs, got: {:?}",
            plan.iter().map(Repair::describe).collect::<Vec<_>>()
        );
    }

    /// Errors are never repaired. `DuplicateId` in particular is already resolved
    /// by `fr clean`; repairing it here too would be the drift this module exists
    /// to avoid.
    #[test]
    fn plan_ignores_errors() {
        let mut check = result_with(Vec::new());
        check.errors.push(CheckError::DuplicateId {
            task_id: "T-1".into(),
            track_ids: vec!["a".into(), "b".into()],
        });
        assert!(plan(&check).is_empty());
    }

    #[test]
    fn only_deleting_repairs_are_counted_for_confirmation() {
        let additive = vec![
            Repair::CloseNoteFence {
                track_id: "t".into(),
                task_id: None,
                title: "x".into(),
                fence: "```".into(),
                closer: "```".into(),
            },
            Repair::AddGitignoreEntry {
                path: "frame/.actor".into(),
            },
        ];
        assert_eq!(deleting_count(&additive), 0);

        let mut mixed = additive;
        mixed.push(Repair::RemoveFrontierBackup { path: "/x".into() });
        mixed.push(Repair::DedupeArchivedTask {
            task_id: "T-1".into(),
            total: 3,
            archives: vec!["archive/t.md".into()],
        });
        assert_eq!(deleting_count(&mixed), 2);
    }

    #[test]
    fn close_open_fence_appends_and_dirties() {
        let mut task = Task::new(
            crate::model::task::TaskState::Todo,
            Some("T-1".into()),
            "t".into(),
        );
        task.metadata
            .push(Metadata::Note("Example:\n```rust\nlet x = 1;".into()));
        task.dirty = false;

        assert!(close_open_fence(&mut task, "```"));
        assert!(task.dirty);

        let Some(Metadata::Note(body)) = task.metadata.first() else {
            panic!("note missing");
        };
        assert!(body.ends_with("\n```"));
        assert!(
            crate::ops::check::unclosed_fence(body).is_none(),
            "fence should now be balanced"
        );

        // Idempotent: a balanced note is left alone.
        assert!(!close_open_fence(&mut task, "```"));
    }
}
