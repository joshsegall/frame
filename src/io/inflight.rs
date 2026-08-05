//! The in-flight operation marker: `frame/.inflight`.
//!
//! # What it is for
//!
//! Ordering (see `doc/architecture.md`, "Multi-file writes") guarantees that an
//! interrupted multi-file operation never *loses* work. It does not make the
//! resulting state *visible*, and measurement showed both real interrupted
//! states report `✓ project is valid`:
//!
//! - `fr mv --track` cut after the target write — the task is in both tracks
//!   under different IDs.
//! - `fr track archive` cut after the config write — config says archived while
//!   the file is still in `tracks/`.
//!
//! Neither is detectable in principle. Two tasks with different IDs in different
//! tracks is a legitimate state, and so is a config entry whose file has not been
//! read yet. The only thing that knows something went wrong is the process that
//! was doing it — so it writes that down before it starts.
//!
//! # Lifecycle
//!
//! [`InFlight::begin`] writes the marker; [`InFlight::commit`] marks the
//! operation complete. `Drop` removes the file **only if committed**:
//!
//! | Outcome | `commit` | `Drop` | Marker after |
//! |---|---|---|---|
//! | success | yes | runs, removes | gone |
//! | error return (`?`) | no | runs, leaves it | present |
//! | process killed | no | never runs | present |
//!
//! The error path and the death path converge on the same observable state
//! without being handled separately — which matters, because the gap between
//! those two is exactly where `fr mv --track`'s previous recovery-log mitigation
//! failed.
//!
//! # Intent, not a step log
//!
//! The marker records what the operation *meant to do*. What remains is derived
//! by inspecting current state (see `crate::ops::recover`), so nothing has to be
//! written mid-operation to track progress — that would mean more writes inside
//! the very window being protected.
//!
//! # Rules
//!
//! - **Breadcrumb, not mutex.** No command refuses to run because a marker
//!   exists. Every write command already holds `frame/.lock` (flock(2)), so only
//!   one operation writes at a time and only one marker can exist; flock is
//!   released by the kernel on process death, so there is no stale lock to break.
//! - **Never blocks, never accumulates.** The next write command recovers the
//!   operation and clears the marker.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name inside `frame/`. Listed in
/// [`crate::io::project_io::LOCAL_ONLY_FRAME_FILES`] — it is per-working-copy and
/// must never be committed.
pub const MARKER_FILE: &str = ".inflight";

pub fn marker_path(frame_dir: &Path) -> PathBuf {
    frame_dir.join(MARKER_FILE)
}

/// What the interrupted operation was trying to do.
///
/// Each variant carries exactly what [`crate::ops::recover`] needs to work out
/// the remaining steps and to verify them before acting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    /// One or more tasks moved between tracks, re-minted into the mover's
    /// namespace.
    ///
    /// A list rather than a single pair because the TUI's bulk move writes the
    /// same two files for N tasks, and an interruption duplicates all of them.
    /// The CLI's `fr mv --track` passes one.
    CrossTrackMove {
        moves: Vec<MovedTask>,
        source_track: String,
        target_track: String,
    },
    /// A track marked archived in config, its file moved to `archive/_tracks/`.
    TrackArchive { track_id: String, file: String },
    /// The inverse: a track marked active in config, its file moved back out of
    /// `archive/_tracks/` into `tracks/`.
    ///
    /// Same two writes in the same order as [`Operation::TrackArchive`], and the
    /// interruption is worse. A cut archive leaves the file in `tracks/` where
    /// everything can still read it; a cut un-archive leaves the config naming a
    /// file that is not there, and `load_project` skips such a track — so the
    /// track and every task in it drop out of the project until this completes.
    TrackUnarchive { track_id: String, file: String },
    /// A track's id changed: its file renamed, its archive renamed, and the
    /// config entry rewritten to match.
    ///
    /// The files move before the config does, so an interruption leaves the
    /// config pointing at a name nothing answers to — and `load_project` skips
    /// a track whose file is missing, so the track and every task in it drop
    /// out of the project silently. `fr check` reports that state now
    /// (`track_file_missing` / `track_file_unreferenced`); this is what lets
    /// the next command simply finish the job.
    TrackRename {
        old_id: String,
        new_id: String,
        /// The `file` field as config still has it, relative to `frame/`.
        old_file: String,
        /// Where the file was moved to, relative to `frame/`.
        new_file: String,
    },
    /// One or more actor namespaces renumbered into a target, sources retired.
    ActorMerge {
        sources: Vec<String>,
        target: String,
    },
    /// An inbox item promoted to a task and removed from the inbox.
    Triage {
        /// 1-based, as `fr inbox` and `fr triage` report it.
        index: usize,
        title: String,
        track_id: String,
    },
}

impl Operation {
    /// Short name for messages, matching the command the user ran.
    pub fn name(&self) -> &'static str {
        match self {
            Operation::CrossTrackMove { .. } => "mv --track",
            Operation::TrackArchive { .. } => "track archive",
            Operation::TrackUnarchive { .. } => "track activate",
            Operation::TrackRename { .. } => "track rename --id",
            Operation::ActorMerge { .. } => "actor merge",
            Operation::Triage { .. } => "triage",
        }
    }
}

/// One task's before/after ids in a [`Operation::CrossTrackMove`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MovedTask {
    pub old_id: String,
    pub new_id: String,
}

/// The on-disk marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    /// The command as the user typed it, for the message they will read.
    pub command: String,
    /// RFC3339, seconds precision.
    pub started: String,
    #[serde(flatten)]
    pub operation: Operation,
}

/// Read the marker, if one is present and parseable.
///
/// An unparseable marker is treated as absent: it cannot be recovered from, and
/// refusing to proceed would let a corrupt file wedge the tool — the opposite of
/// the breadcrumb-not-mutex rule.
pub fn read(frame_dir: &Path) -> Option<Marker> {
    let text = fs::read_to_string(marker_path(frame_dir)).ok()?;
    toml::from_str(&text).ok()
}

/// Remove the marker. Absent is success — recovery must stay idempotent.
pub fn clear(frame_dir: &Path) -> io::Result<()> {
    match fs::remove_file(marker_path(frame_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// A marker held for the duration of a multi-file operation.
///
/// Call [`InFlight::commit`] once every write has landed. Anything else — an
/// early `?`, a panic, a kill — leaves the marker for the next command to
/// recover.
#[must_use = "an uncommitted InFlight marks the operation as interrupted"]
pub struct InFlight {
    path: PathBuf,
    committed: bool,
}

impl InFlight {
    /// Write the marker for `operation` before its first write lands.
    ///
    /// The caller must already hold the project lock. Any marker already present
    /// belongs to an operation that never finished; recovering it is the caller's
    /// job (via `crate::ops::recover`) and should have happened when the lock was
    /// taken, so reaching here with one still in place means recovery declined to
    /// act. Overwrite rather than block — the previous marker's details are
    /// already in the recovery log by then.
    pub fn begin(frame_dir: &Path, operation: Operation, command: &str) -> io::Result<Self> {
        let marker = Marker {
            command: command.to_string(),
            started: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            operation,
        };
        let text = toml::to_string_pretty(&marker)
            .map_err(|e| io::Error::other(format!("serialize {MARKER_FILE}: {e}")))?;
        let path = marker_path(frame_dir);
        // Deliberately not `atomic_write`: that path is fault-injectable, and a
        // test cutting a track write must not have its marker cut too.
        fs::write(&path, text)?;
        Ok(InFlight {
            path,
            committed: false,
        })
    }

    /// Every write landed — the operation is complete.
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn op() -> Operation {
        Operation::CrossTrackMove {
            moves: vec![MovedTask {
                old_id: "A-001".into(),
                new_id: "B-001".into(),
            }],
            source_track: "a".into(),
            target_track: "b".into(),
        }
    }

    #[test]
    fn committing_removes_the_marker() {
        let tmp = TempDir::new().unwrap();
        let guard = InFlight::begin(tmp.path(), op(), "fr mv A-001 --track b").unwrap();
        assert!(read(tmp.path()).is_some(), "marker written on begin");
        guard.commit();
        assert!(read(tmp.path()).is_none(), "marker removed on commit");
    }

    #[test]
    fn dropping_without_committing_leaves_the_marker() {
        let tmp = TempDir::new().unwrap();
        {
            let _guard = InFlight::begin(tmp.path(), op(), "fr mv A-001 --track b").unwrap();
            // falls out of scope without commit, as an early `?` would
        }
        let marker = read(tmp.path()).expect("marker should survive an uncommitted drop");
        assert_eq!(marker.operation, op());
        assert_eq!(marker.command, "fr mv A-001 --track b");
    }

    #[test]
    fn round_trips_every_operation_shape() {
        let tmp = TempDir::new().unwrap();
        for operation in [
            op(),
            Operation::TrackArchive {
                track_id: "a".into(),
                file: "tracks/a.md".into(),
            },
            Operation::ActorMerge {
                sources: vec!["x".into(), "z".into()],
                target: "y".into(),
            },
            Operation::Triage {
                index: 2,
                title: "an item".into(),
                track_id: "a".into(),
            },
        ] {
            let guard = InFlight::begin(tmp.path(), operation.clone(), "cmd").unwrap();
            std::mem::forget(guard); // leave it, as a crash would
            assert_eq!(read(tmp.path()).unwrap().operation, operation);
            clear(tmp.path()).unwrap();
        }
    }

    #[test]
    fn clearing_an_absent_marker_succeeds() {
        let tmp = TempDir::new().unwrap();
        clear(tmp.path()).unwrap();
        clear(tmp.path()).unwrap();
    }

    /// A corrupt marker must not wedge anything — it reads as absent.
    #[test]
    fn an_unparseable_marker_reads_as_absent() {
        let tmp = TempDir::new().unwrap();
        fs::write(marker_path(tmp.path()), "not toml {{{").unwrap();
        assert!(read(tmp.path()).is_none());
    }
}
