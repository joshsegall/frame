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
//!   other work may already reference. `ChildIdNotUnderParent` *is* repaired
//!   here despite rewriting an ID too, and the difference is that it has one
//!   correct answer: a subtask's ID must extend its parent's, so which task
//!   changes and what it becomes are both determined. A reissued number instead
//!   asks which of two legitimate holders should move — a judgment call, and one
//!   where the archived holder cannot move at all.
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
    /// Drop the extra copies of a task duplicated inside the archives, keeping
    /// the first. **Destructive.**
    #[serde(rename = "dedupe_archived_task")]
    DedupeArchivedTask {
        task_id: String,
        /// How many copies exist across the archives; one will remain.
        total: usize,
        /// Archive paths holding it, as check reports them.
        archives: Vec<String>,
    },
    /// Remove the leftover frontier-store backup. **Destructive.**
    #[serde(rename = "remove_frontier_backup")]
    RemoveFrontierBackup { path: String },
    /// Give a subtask whose ID does not extend its parent's the next free child
    /// number under that parent, rekeying its own descendants and rewriting every
    /// `dep:` that pointed at the old ID. **Destructive** — the old ID stops existing
    /// anywhere in the project, and frame cannot rewrite a reference held outside
    /// it (a commit message, a PR, a note someone made).
    ///
    /// The new ID is not known until apply: [`plan`] reads the check result and
    /// never the project, and the free number depends on the parent's other
    /// children.
    #[serde(rename = "renumber_subtask")]
    RenumberSubtask {
        track_id: String,
        task_id: String,
        parent_id: String,
    },
    /// Move a top-level task into the section its state calls for.
    ///
    /// Purely positional — the task, its state and its subtasks are untouched —
    /// so nothing here is irreversible and it needs no confirmation. This is the
    /// same operation `fr clean` performs unasked; having it here is what lets
    /// someone read the diagnosis first, and what makes the finding actionable
    /// for a project that never runs clean.
    #[serde(rename = "move_task_to_section")]
    MoveTaskToSection {
        track_id: String,
        task_id: String,
        from: crate::model::track::SectionKind,
        to: crate::model::track::SectionKind,
    },
    /// Clear an in-flight marker that recovery declined to act on. **Destructive.**
    ///
    /// Only reachable when automatic recovery found a precondition it could not
    /// verify, so the operation was left alone and the marker kept. Clearing it
    /// is the user saying they have looked; without this the warning would stand
    /// forever with no way to acknowledge it.
    #[serde(rename = "clear_inflight_marker")]
    ClearInflightMarker { operation: String, command: String },
}

/// A section's name as it appears in the file, for messages.
pub fn section_name(kind: crate::model::track::SectionKind) -> &'static str {
    use crate::model::track::SectionKind;
    match kind {
        SectionKind::Backlog => "## Backlog",
        SectionKind::Parked => "## Parked",
        SectionKind::Done => "## Done",
    }
}

impl Repair {
    /// Whether applying this destroys something that cannot be reconstructed
    /// from what remains. Repairs that only add are applied without
    /// confirmation; these require `--yes` or an interactive `y`.
    ///
    /// Named for the consequence rather than the mechanism, because the two have
    /// already diverged: `RenumberSubtask` removes no bytes at all — it rewrites
    /// an ID — but the old ID stops existing anywhere in the project and frame
    /// cannot rewrite a reference held outside it. That is the same
    /// irreversibility a deletion has, and the gate is about irreversibility.
    pub fn destructive(&self) -> bool {
        match self {
            Repair::CloseNoteFence { .. }
            | Repair::CloseInboxFence { .. }
            | Repair::MoveTaskToSection { .. } => false,
            Repair::DedupeArchivedTask { .. }
            | Repair::RemoveFrontierBackup { .. }
            | Repair::ClearInflightMarker { .. }
            | Repair::RenumberSubtask { .. } => true,
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
            Repair::RenumberSubtask {
                track_id,
                task_id,
                parent_id,
            } => {
                format!(
                    "[{track_id}] {task_id}: renumber under its parent {parent_id} \
                     (its id does not extend the parent's); deps follow"
                )
            }
            Repair::MoveTaskToSection {
                track_id,
                task_id,
                from,
                to,
            } => {
                format!(
                    "[{track_id}] {task_id}: move from {} to {} (its state belongs there)",
                    section_name(*from),
                    section_name(*to)
                )
            }
            Repair::ClearInflightMarker { command, .. } => {
                format!(
                    "clear the in-flight marker for `{command}` (recovery could not complete it)"
                )
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
    /// Tracks changed as a side effect, beyond the one a repair names: renumbering
    /// a subtask rewrites `dep:` lines wherever they point at the old ID, which
    /// can be any track. Folded into [`tracks_touched`]; not part of the JSON
    /// shape, which reports repairs rather than files.
    #[serde(skip)]
    pub also_touched: Vec<String>,
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
            CheckWarning::DuplicateArchivedId {
                task_id,
                total,
                archives,
            } => plan.push(Repair::DedupeArchivedTask {
                task_id: task_id.clone(),
                total: *total,
                archives: archives.clone(),
            }),
            CheckWarning::ChildIdNotUnderParent {
                track_id,
                task_id,
                parent_id,
            } => plan.push(Repair::RenumberSubtask {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                parent_id: parent_id.clone(),
            }),
            CheckWarning::TaskInWrongSection {
                track_id,
                task_id,
                expected,
                actual,
            } => plan.push(Repair::MoveTaskToSection {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                from: *actual,
                to: *expected,
            }),
            CheckWarning::IdFrontierWasReset { path } => {
                plan.push(Repair::RemoveFrontierBackup { path: path.clone() })
            }
            CheckWarning::InterruptedOperation {
                operation, command, ..
            } => plan.push(Repair::ClearInflightMarker {
                operation: operation.clone(),
                command: command.clone(),
            }),
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
            Repair::MoveTaskToSection {
                track_id,
                task_id,
                from,
                to,
            } => {
                // Re-check the current section rather than trusting the plan:
                // `fr clean` may have reconciled it between the diagnosis and
                // the repair, which is not a failure — it is the same move
                // arriving from the other direction.
                let moved = project
                    .tracks
                    .iter_mut()
                    .find(|(id, _)| id == track_id)
                    .map(|(_, track)| (track,))
                    .and_then(|(track,)| {
                        let now = crate::ops::task_ops::top_level_section(track, task_id)?;
                        // `now == *to` means clean already reconciled it between
                        // the diagnosis and the repair — the same move arriving
                        // from the other direction, not a failure.
                        Some(
                            now == *to
                                || crate::ops::task_ops::move_task_between_sections(
                                    track, task_id, now, *to,
                                )
                                .is_some(),
                        )
                    });
                match moved {
                    Some(true) => result.applied.push(repair.clone()),
                    _ => result.skipped.push(SkippedRepair {
                        repair: repair.clone(),
                        reason: format!(
                            "{task_id} is no longer a top-level task in {}",
                            section_name(*from)
                        ),
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
            Repair::RenumberSubtask {
                track_id,
                task_id,
                parent_id,
            } => match apply_renumber_subtask(project, track_id, task_id, parent_id) {
                Ok(touched) => {
                    result.also_touched.extend(touched);
                    result.applied.push(repair.clone())
                }
                Err(reason) => result.skipped.push(SkippedRepair {
                    repair: repair.clone(),
                    reason,
                }),
            },
            Repair::ClearInflightMarker { .. } => {
                match crate::io::inflight::clear(&project.frame_dir) {
                    Ok(()) => result.applied.push(repair.clone()),
                    Err(e) => result.skipped.push(SkippedRepair {
                        repair: repair.clone(),
                        reason: e.to_string(),
                    }),
                }
            }
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
///
/// **An archive is not a track.** `fr clean` writes `# Archive — <track>` and
/// then bare task lines, with no `## Section` header
/// (`clean.rs`'s archive append), and both other readers —
/// [`crate::io::project_io::load_archives`] and the mint scan in
/// [`crate::ops::ids`] — skip to the first task line and parse from there. This
/// reads them the same way. Walking `TrackNode::Section` instead, as this did
/// until `tests/damaged_corpus.rs` ran it against an archive `fr clean` had
/// actually produced, finds nothing in a real archive: the repair reported
/// "no longer appears in the archives" and silently changed nothing.
fn dedupe_archived(frame_dir: &Path, task_id: &str, archives: &[String]) -> Result<(), String> {
    let mut seen = false;

    for rel in archives {
        let path = frame_dir.join(rel);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;

        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("- ["))
            .unwrap_or(lines.len());
        let (tasks, _) = crate::parse::parse_tasks(&lines, start, 0, 0);

        let mut removed = Vec::new();
        let kept: Vec<Task> = tasks
            .into_iter()
            .filter(|task| {
                if task.id.as_deref() != Some(task_id) {
                    return true;
                }
                if !seen {
                    seen = true;
                    return true;
                }
                removed.push(task.clone());
                false
            })
            .collect();

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

        // Everything above the first task line is header — the `# Archive` title
        // and any blank line under it — and is carried verbatim.
        let mut out = lines[..start].join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&crate::parse::serialize_tasks(&kept, 0).join("\n"));
        out.push('\n');
        crate::io::recovery::atomic_write(&path, out.as_bytes())
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }

    if seen {
        Ok(())
    } else {
        Err(format!("{task_id} no longer appears in the archives"))
    }
}

/// Give `task_id` the next free child number under `parent_id`.
///
/// The new number is minted in **the namespace the task's own ID already
/// carries**, not this working copy's. The task is not being created here, only
/// put back where its ID says it belongs; re-minting it into the repairing
/// clone's namespace would quietly reattribute someone else's task.
///
/// Returns the tracks left dirty — the one holding the task, plus any whose
/// `dep:` lines were rewritten.
fn apply_renumber_subtask(
    project: &mut Project,
    track_id: &str,
    task_id: &str,
    parent_id: &str,
) -> Result<Vec<String>, String> {
    use crate::model::task_id::TaskId;
    use crate::ops::task_ops;

    let track = project
        .tracks
        .iter()
        .find(|(id, _)| id == track_id)
        .map(|(_, t)| t)
        .ok_or_else(|| format!("track '{track_id}' not found"))?;

    let parent = task_ops::find_task_in_track(track, parent_id)
        .ok_or_else(|| format!("parent '{parent_id}' not found"))?;
    let parent_task_id = parent
        .id
        .as_ref()
        .filter(|id| id.is_structured())
        .ok_or_else(|| format!("parent '{parent_id}' has no structured id"))?;

    // Re-establish that the finding still holds. The plan was computed from a
    // check result; anything else in the same run may have moved the task since.
    let current = parent
        .subtasks
        .iter()
        .find_map(|sub| sub.id.as_ref().filter(|id| id.as_str() == task_id))
        .ok_or_else(|| format!("'{task_id}' is no longer a subtask of '{parent_id}'"))?;
    if current.is_child_of(parent_task_id) {
        return Err(format!("'{task_id}' already extends '{parent_id}'"));
    }

    let token = current.leaf_token().cloned();
    let number = task_ops::next_child_number(parent, token.as_ref()) as u32;
    let new_id = TaskId::child_of(parent_task_id, number, token.as_ref());

    let track = project
        .tracks
        .iter_mut()
        .find(|(id, _)| id == track_id)
        .map(|(_, t)| t)
        .expect("track was found immutably a moment ago");
    let task = task_ops::find_task_mut_in_track(track, task_id)
        .expect("task was found immutably a moment ago");
    // Descendants have to follow: `BAC-207.1` under a renamed `BAC-207` would
    // otherwise become the very defect being repaired.
    let mappings = task_ops::rekey_subtree(task, new_id.as_str(), token.as_ref());

    for (old, new) in &mappings {
        task_ops::update_dep_references(&mut project.tracks, old, new);
    }

    Ok(project
        .tracks
        .iter()
        .filter(|(_, t)| task_ops::track_has_dirty_task(t))
        .map(|(id, _)| id.clone())
        .collect())
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
            Repair::CloseNoteFence { track_id, .. }
            | Repair::RenumberSubtask { track_id, .. }
            | Repair::MoveTaskToSection { track_id, .. } => Some(track_id.clone()),
            _ => None,
        })
        .collect();
    out.extend(result.also_touched.iter().cloned());
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

/// How many repairs in `plan` cannot be undone. This is the confirmation gate —
/// a caller should not have to know which variants those are.
pub fn destructive_count(plan: &[Repair]) -> usize {
    plan.iter().filter(|r| r.destructive()).count()
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
            CheckWarning::DuplicateArchivedId {
                task_id: "T-9".into(),
                total: 2,
                archives: vec!["archive/t.md".into()],
            },
            CheckWarning::IdFrontierWasReset {
                path: "/x/frame-ids.toml.bak".into(),
            },
        ]));
        assert_eq!(plan.len(), 4);
    }

    /// The findings with no safe automatic repair must produce nothing. If a new
    /// warning is added and someone wants it repaired, that is a deliberate
    /// decision made in `plan`, not a default.
    #[test]
    fn plan_ignores_warnings_with_no_safe_repair() {
        let plan = plan(&result_with(vec![
            // Git readiness belongs to `fr git setup`, whichever half is wrong:
            // a tracked file needs `git rm --cached` too, and an unignored one
            // needs a `.gitignore` line that `--fix` used to add on its own.
            // Splitting the surface between two commands left nobody able to
            // predict which half `--fix` would repair.
            CheckWarning::LocalFileCommitted {
                path: "frame/.actor".into(),
                tracked: true,
            },
            CheckWarning::LocalFileCommitted {
                path: "frame/.lock".into(),
                tracked: false,
            },
            // The driver lives in `.git/config`, which is machine state rather
            // than project content — further outside `--fix`'s remit still.
            CheckWarning::MergeDriverUnregistered,
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
            // Where a stranded line was meant to go is a guess. Frame keeps it
            // where it found it and says so; re-indenting it is the user's call.
            CheckWarning::StrandedLine {
                track_id: "t".into(),
                before_task_id: Some("T-6".into()),
                before_title: "task".into(),
                line: "**Shape.** prose that lost its indent".into(),
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
            Repair::MoveTaskToSection {
                track_id: "t".into(),
                task_id: "T-1".into(),
                from: crate::model::track::SectionKind::Backlog,
                to: crate::model::track::SectionKind::Done,
            },
        ];
        assert_eq!(destructive_count(&additive), 0);

        let mut mixed = additive;
        mixed.push(Repair::RemoveFrontierBackup { path: "/x".into() });
        mixed.push(Repair::DedupeArchivedTask {
            task_id: "T-1".into(),
            total: 3,
            archives: vec!["archive/t.md".into()],
        });
        assert_eq!(destructive_count(&mixed), 2);
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

    /// An archive has no `## Section` header — `fr clean` writes a title and
    /// then bare task lines. Reading one as a track finds no tasks at all, which
    /// is how this repair shipped doing nothing on every archive frame produces.
    #[test]
    fn dedupe_reads_the_archive_shape_clean_actually_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        let path = frame_dir.join("archive").join("main.md");
        std::fs::write(
            &path,
            "# Archive — main\n\n\
             - [x] `M-900` Twice\n  - resolved: 2026-01-01\n\
             - [x] `M-900` Twice\n  - resolved: 2026-01-01\n\
             - [x] `M-901` Once\n  - resolved: 2026-01-02\n",
        )
        .unwrap();

        dedupe_archived(&frame_dir, "M-900", &["archive/main.md".to_string()]).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after.matches("`M-900`").count(), 1, "{after}");
        assert!(
            after.contains("`M-901`"),
            "untouched task survives: {after}"
        );
        assert!(
            after.starts_with("# Archive — main\n\n"),
            "header carried verbatim: {after}"
        );

        // Idempotent: the one remaining copy is not the duplicate.
        dedupe_archived(&frame_dir, "M-900", &["archive/main.md".to_string()]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            after,
            "a second run must change nothing"
        );
    }

    // --- Renumbering a subtask whose id escaped its parent ---

    fn project_with(tracks: Vec<(&str, &str)>) -> Project {
        use crate::model::config::{
            AgentConfig, CleanConfig, IdConfig, ProjectConfig, ProjectInfo, TrackConfig, UiConfig,
        };
        Project {
            root: std::path::PathBuf::from("/tmp/fix-test"),
            frame_dir: std::path::PathBuf::from("/tmp/fix-test/frame"),
            config: ProjectConfig {
                project: ProjectInfo {
                    name: "test".to_string(),
                },
                agent: AgentConfig::default(),
                tracks: tracks
                    .iter()
                    .map(|(id, _)| TrackConfig {
                        id: id.to_string(),
                        name: id.to_string(),
                        state: "active".to_string(),
                        file: format!("tracks/{id}.md"),
                    })
                    .collect(),
                clean: CleanConfig::default(),
                ids: IdConfig {
                    prefixes: indexmap::IndexMap::new(),
                },
                ui: UiConfig::default(),
            },
            tracks: tracks
                .into_iter()
                .map(|(id, src)| (id.to_string(), crate::parse::parse_track(src)))
                .collect(),
            inbox: None,
        }
    }

    /// Plan and apply, driven by what check actually reported — the same path
    /// the CLI takes.
    fn fix_all(project: &mut Project) -> FixResult {
        let plan = plan(&crate::ops::check::check_project(project));
        apply(project, &plan)
    }

    #[test]
    fn a_misparented_subtask_is_planned_for_renumbering() {
        let plan = plan(&result_with(vec![CheckWarning::ChildIdNotUnderParent {
            track_id: "main".into(),
            task_id: "M-007".into(),
            parent_id: "M-001".into(),
        }]));
        assert_eq!(plan.len(), 1);
        assert!(matches!(plan[0], Repair::RenumberSubtask { .. }));
        // It rewrites an id out of existence, so it needs consent.
        assert_eq!(destructive_count(&plan), 1);
    }

    #[test]
    fn renumbering_puts_the_subtask_under_its_parent() {
        let mut project = project_with(vec![(
            "main",
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - [ ] `M-001.1` Sibling
  - [ ] `M-007` Escaped

## Done
",
        )]);

        let result = fix_all(&mut project);
        assert_eq!(result.applied.len(), 1);
        assert!(result.skipped.is_empty());

        let subs = &project.tracks[0].1.backlog()[0].subtasks;
        assert_eq!(subs[1].id.as_deref(), Some("M-001.2"));
        assert!(subs[1].dirty);
        assert_eq!(tracks_touched(&result), vec!["main".to_string()]);

        // And the finding is gone.
        assert!(fix_all(&mut project).applied.is_empty());
    }

    /// The escaped subtask's own children follow it, or they become the very
    /// defect being repaired.
    #[test]
    fn renumbering_carries_descendants_and_their_deps() {
        let mut project = project_with(vec![
            (
                "main",
                "\
# Main

## Backlog

- [ ] `M-001` Parent
  - [ ] `M-007` Escaped
    - [ ] `M-007.1` Child of the escapee

## Done
",
            ),
            (
                "other",
                "\
# Other

## Backlog

- [ ] `O-001` Waiting
  - dep: M-007.1

## Done
",
            ),
        ]);

        let result = fix_all(&mut project);
        assert_eq!(result.applied.len(), 1);

        let escaped = &project.tracks[0].1.backlog()[0].subtasks[0];
        assert_eq!(escaped.id.as_deref(), Some("M-001.1"));
        assert_eq!(escaped.subtasks[0].id.as_deref(), Some("M-001.1.1"));

        let waiting = &project.tracks[1].1.backlog()[0];
        assert!(
            waiting
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Dep(d) if d == &vec!["M-001.1.1".to_string()])),
            "dep should follow the rekey: {:?}",
            waiting.metadata
        );

        // Both files have to be saved, not just the one the repair names.
        assert_eq!(
            tracks_touched(&result),
            vec!["main".to_string(), "other".to_string()]
        );
    }

    /// The task is put back where its id says it belongs; it is not reassigned
    /// to whoever happens to be running the repair. The namespace comes from the
    /// id already on the task.
    #[test]
    fn renumbering_keeps_the_id_in_its_own_namespace() {
        let mut project = project_with(vec![(
            "main",
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - [ ] `M-001.1` Ours
  - [ ] `M-b12` Theirs, escaped

## Done
",
        )]);

        fix_all(&mut project);

        let subs = &project.tracks[0].1.backlog()[0].subtasks;
        assert_eq!(subs[1].id.as_deref(), Some("M-001.b1"));
    }

    /// A plan is applied against a project that may have moved since check ran.
    /// A repair whose finding no longer holds is skipped with a reason, not
    /// forced through onto whatever task now answers to that id.
    #[test]
    fn a_repair_whose_finding_went_away_is_skipped() {
        let mut project = project_with(vec![(
            "main",
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - [ ] `M-001.1` Already fine

## Done
",
        )]);

        let stale = vec![Repair::RenumberSubtask {
            track_id: "main".into(),
            task_id: "M-001.1".into(),
            parent_id: "M-001".into(),
        }];
        let result = apply(&mut project, &stale);
        assert!(result.applied.is_empty());
        assert_eq!(result.skipped.len(), 1);
        assert!(
            result.skipped[0].reason.contains("already extends"),
            "reason: {}",
            result.skipped[0].reason
        );
    }
}
