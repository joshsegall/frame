//! Completing an interrupted multi-file operation.
//!
//! # Roll forward, not back
//!
//! Rolling *back* would need undo records — effectively a copy of the prior state
//! of every file touched — which is what git already stores for this data.
//! Rolling *forward* needs none of that: the remaining steps are the same steps
//! the operation would have taken, and they complete an intent the user already
//! expressed.
//!
//! Handing this to a human is the worse option, not the safer one. "Delete
//! whichever copy is wrong" invites deleting the right one — after a cross-track
//! move the two copies carry *different* IDs and may have diverged — and every
//! manual edit is a fresh chance to do damage. Where the remaining work is
//! determinate, doing it beats describing it.
//!
//! # Intent in, state inspected
//!
//! [`crate::io::inflight`] records what the operation meant to do. What is *left*
//! is derived here by looking at the project as it stands, so nothing has to be
//! written mid-operation to track progress — that would mean more writes inside
//! the very window being protected.
//!
//! | Interrupted | Inspected | Remaining |
//! |---|---|---|
//! | `mv --track` | does the target hold the new ID? | yes → drop the old ID from the source; no → nothing landed |
//! | `track archive` | is the file still in `tracks/`? | yes → move it to `archive/_tracks/` |
//! | `actor merge` | is a source token still active? | yes → retire it |
//! | `triage` | is the item still in the inbox *and* present as a task? | yes → drop the inbox item |
//!
//! # Preconditions gate every destructive step
//!
//! The source copy of a moved task is removed only after confirming the target
//! copy is really there. If a precondition fails — a hand edit, a `git checkout`
//! between the crash and the recovery — this does **not** guess. It leaves
//! everything alone and reports [`Outcome::Indeterminate`], which is the narrow
//! case where a human is genuinely the right answer rather than the default.
//!
//! Every outcome is announced by the caller and written to the recovery log, so
//! an automatic choice that turns out to be wrong stays visible and reversible.

use crate::io::inflight::{self, Marker, MovedTask, Operation};
use crate::io::project_io;
use crate::model::project::Project;
use crate::model::track::TrackNode;

/// What recovery did.
#[derive(Debug)]
pub enum Outcome {
    /// The operation was already complete; only the marker remained.
    AlreadyComplete { operation: String },
    /// Remaining steps were applied.
    Completed {
        operation: String,
        /// Human-readable, one line per step taken.
        steps: Vec<String>,
    },
    /// A precondition did not hold, so nothing was changed.
    Indeterminate { operation: String, reason: String },
}

impl Outcome {
    pub fn operation(&self) -> &str {
        match self {
            Outcome::AlreadyComplete { operation }
            | Outcome::Completed { operation, .. }
            | Outcome::Indeterminate { operation, .. } => operation,
        }
    }
}

/// Recover a pending operation, if there is one.
///
/// Returns `None` when no marker is present, which is the overwhelmingly common
/// case — one `stat` on the write path.
///
/// The caller must hold the project lock. On anything but
/// [`Outcome::Indeterminate`] the marker is cleared; an indeterminate one is
/// deliberately left in place so `fr check` keeps reporting it until a human
/// looks.
pub fn recover_pending(project: &mut Project) -> Option<Outcome> {
    let marker = inflight::read(&project.frame_dir)?;
    let outcome = apply(project, &marker);

    if !matches!(outcome, Outcome::Indeterminate { .. }) {
        let _ = inflight::clear(&project.frame_dir);
    }

    log(project, &marker, &outcome);
    Some(outcome)
}

fn apply(project: &mut Project, marker: &Marker) -> Outcome {
    let operation = marker.operation.name().to_string();
    match &marker.operation {
        Operation::CrossTrackMove {
            moves,
            source_track,
            target_track,
        } => recover_cross_track_move(project, operation, moves, source_track, target_track),
        Operation::TrackArchive { track_id, file } => {
            recover_track_archive(project, operation, track_id, file)
        }
        Operation::ActorMerge { sources, target } => {
            recover_actor_merge(project, operation, sources, target)
        }
        Operation::Triage {
            index,
            title,
            track_id,
        } => recover_triage(project, operation, *index, title, track_id),
    }
}

// ---------------------------------------------------------------------------
// Cross-track move
// ---------------------------------------------------------------------------

/// The target is written before the source, so for each moved task the possible
/// states are: neither write landed (nothing to do), or the target landed and
/// the source still holds the old copy (drop it).
///
/// A bulk move carries many tasks in one marker, and they are recovered
/// independently — an interruption lands somewhere in the middle of the batch,
/// so some will need the source copy dropped and others will not.
fn recover_cross_track_move(
    project: &mut Project,
    operation: String,
    moves: &[MovedTask],
    source_track: &str,
    target_track: &str,
) -> Outcome {
    let mut to_drop = Vec::new();

    for moved in moves {
        let target_has_it = project
            .tracks
            .iter()
            .find(|(id, _)| id == target_track)
            .map(|(_, t)| crate::ops::task_ops::find_task_in_track(t, &moved.new_id).is_some())
            .unwrap_or(false);
        let source_has_it = project
            .tracks
            .iter()
            .find(|(id, _)| id == source_track)
            .map(|(_, t)| crate::ops::task_ops::find_task_in_track(t, &moved.old_id).is_some())
            .unwrap_or(false);

        // The precondition for the destructive step: the target copy is really
        // there, so dropping the source copy completes the move. Anything else
        // means either the move finished or nothing landed — no work either way.
        if target_has_it && source_has_it {
            to_drop.push(moved);
        }
    }

    if to_drop.is_empty() {
        return Outcome::AlreadyComplete { operation };
    }

    let Some((_, track)) = project.tracks.iter_mut().find(|(id, _)| id == source_track) else {
        return Outcome::Indeterminate {
            operation,
            reason: format!("source track '{source_track}' is gone"),
        };
    };
    let mut steps = Vec::new();
    for moved in &to_drop {
        if !remove_task(track, &moved.old_id) {
            return Outcome::Indeterminate {
                operation,
                reason: format!(
                    "{} could not be removed from '{source_track}'",
                    moved.old_id
                ),
            };
        }
        steps.push(format!(
            "removed {} from '{source_track}' — it is already in '{target_track}' as {}",
            moved.old_id, moved.new_id
        ));
    }

    if let Err(e) = save_track(project, source_track) {
        return Outcome::Indeterminate {
            operation,
            reason: format!("could not write '{source_track}': {e}"),
        };
    }

    Outcome::Completed { operation, steps }
}

/// Remove a top-level task by ID from whichever section holds it.
fn remove_task(track: &mut crate::model::track::Track, task_id: &str) -> bool {
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            let before = tasks.len();
            tasks.retain(|t| t.id.as_deref() != Some(task_id));
            if tasks.len() != before {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Track archive
// ---------------------------------------------------------------------------

/// Config is written before the file is moved, so the remaining step is the
/// move. Verified by the file still being in `tracks/`.
fn recover_track_archive(
    project: &mut Project,
    operation: String,
    track_id: &str,
    file: &str,
) -> Outcome {
    let live = project.frame_dir.join(file);
    if !live.exists() {
        return Outcome::AlreadyComplete { operation };
    }
    match crate::ops::track_ops::archive_track_file(&project.frame_dir, track_id, file) {
        Ok(()) => Outcome::Completed {
            operation,
            steps: vec![format!(
                "moved {file} to archive/_tracks/{track_id}.md — config already had it archived"
            )],
        },
        Err(e) => Outcome::Indeterminate {
            operation,
            reason: format!("could not move {file}: {e}"),
        },
    }
}

// ---------------------------------------------------------------------------
// Actor merge
// ---------------------------------------------------------------------------

/// Tracks and archives are renumbered before the registry is written, so the
/// remaining step is retiring the source tokens. Safe to repeat: retiring an
/// already-retired token is a no-op here.
fn recover_actor_merge(
    project: &mut Project,
    operation: String,
    sources: &[String],
    target: &str,
) -> Outcome {
    let Ok(mut registry) = crate::io::actors::read_actors(&project.frame_dir) else {
        return Outcome::Indeterminate {
            operation,
            reason: "actors.toml could not be read".to_string(),
        };
    };

    let today = crate::io::actors::today();
    let mut retired = Vec::new();
    for token in sources {
        if registry.retire(token, &today).is_ok() {
            retired.push(token.clone());
        }
    }

    if retired.is_empty() {
        return Outcome::AlreadyComplete { operation };
    }

    if let Err(e) = crate::io::actors::write_actors(&project.frame_dir, &registry) {
        return Outcome::Indeterminate {
            operation,
            reason: format!("could not write actors.toml: {e}"),
        };
    }

    Outcome::Completed {
        operation,
        steps: vec![format!(
            "retired {} into '{target}' — ids were already renumbered",
            retired.join(", ")
        )],
    }
}

// ---------------------------------------------------------------------------
// Triage
// ---------------------------------------------------------------------------

/// The track is written before the inbox, so the remaining step is dropping the
/// inbox item. Gated on the item still being there *and* still matching the
/// recorded title, so a shifted or edited inbox is not silently mangled.
fn recover_triage(
    project: &mut Project,
    operation: String,
    index: usize,
    title: &str,
    track_id: &str,
) -> Outcome {
    let Some(inbox) = &project.inbox else {
        return Outcome::AlreadyComplete { operation };
    };
    let Some(item) = inbox.items.get(index.saturating_sub(1)) else {
        return Outcome::AlreadyComplete { operation };
    };
    if item.title != title {
        // The inbox moved on. Removing by index now would delete the wrong item.
        return Outcome::AlreadyComplete { operation };
    }

    // Precondition for the destructive step: the task really did land.
    let landed = project
        .tracks
        .iter()
        .find(|(id, _)| id == track_id)
        .map(|(_, track)| task_titled(track, title))
        .unwrap_or(false);
    if !landed {
        return Outcome::Indeterminate {
            operation,
            reason: format!(
                "inbox item {index} \"{title}\" is still in the inbox but no matching task \
                 exists in '{track_id}' — triage it again rather than losing it"
            ),
        };
    }

    let Some(inbox) = project.inbox.as_mut() else {
        return Outcome::AlreadyComplete { operation };
    };
    inbox.items.remove(index.saturating_sub(1));
    if let Err(e) = project_io::save_inbox(&project.frame_dir, inbox) {
        return Outcome::Indeterminate {
            operation,
            reason: format!("could not write inbox.md: {e}"),
        };
    }

    Outcome::Completed {
        operation,
        steps: vec![format!(
            "removed inbox item {index} \"{title}\" — it is already a task in '{track_id}'"
        )],
    }
}

fn task_titled(track: &crate::model::track::Track, title: &str) -> bool {
    fn walk(tasks: &[crate::model::task::Task], title: &str) -> bool {
        tasks
            .iter()
            .any(|t| t.title == title || walk(&t.subtasks, title))
    }
    track.nodes.iter().any(|node| match node {
        TrackNode::Section { tasks, .. } => walk(tasks, title),
        TrackNode::Literal(_) => false,
    })
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn save_track(project: &Project, track_id: &str) -> Result<(), project_io::ProjectError> {
    let file = project
        .config
        .tracks
        .iter()
        .find(|tc| tc.id == track_id)
        .map(|tc| tc.file.clone())
        .ok_or(project_io::ProjectError::NotAProject)?;
    let track = project
        .tracks
        .iter()
        .find(|(id, _)| id == track_id)
        .map(|(_, t)| t)
        .ok_or(project_io::ProjectError::NotAProject)?;
    project_io::save_track(&project.frame_dir, &file, track)
}

/// Every recovery is written to the recovery log, including the ones that did
/// nothing. An automatic decision is only defensible if it leaves a trail.
fn log(project: &Project, marker: &Marker, outcome: &Outcome) {
    let (description, body) = match outcome {
        Outcome::AlreadyComplete { operation } => (
            format!("interrupted `{operation}` needed no recovery"),
            String::new(),
        ),
        Outcome::Completed { operation, steps } => (
            format!("interrupted `{operation}` completed automatically"),
            steps.join("\n"),
        ),
        Outcome::Indeterminate { operation, reason } => (
            format!("interrupted `{operation}` could not be completed automatically"),
            reason.clone(),
        ),
    };

    crate::io::recovery::log_recovery(
        &project.frame_dir,
        crate::io::recovery::RecoveryEntry {
            timestamp: chrono::Utc::now(),
            category: crate::io::recovery::RecoveryCategory::Write,
            description,
            fields: vec![
                ("Command".to_string(), marker.command.clone()),
                ("Started".to_string(), marker.started.clone()),
            ],
            body,
        },
    );
}
