//! P8 — two writers over one project.
//!
//! > For a settled project, and any interleaving of a TUI session and a CLI
//! > writer, at quiesce: every title either writer acknowledged writing is
//! > somewhere under `frame/` — or, where the two genuinely raced for one task,
//! > in the recovery log in an entry that names it, exactly once and on one side
//! > only. Every track the CLI created or archived is still in the state it left
//! > it in, no ID appears twice, and every file is settled.
//!
//! # The gap this closes
//!
//! Frame's two write paths defend themselves differently, and only one of them
//! has ever been tested under contention.
//!
//! The **CLI** is disciplined. `lock_and_load` takes the project lock and *then*
//! reads, returning the lock alongside the project so no caller can get the
//! order wrong. A CLI command has no stale-state window at all: it reads once
//! and writes once, both inside one lock.
//!
//! The **TUI** cannot do that — it holds state across many writes — so it has
//! three other mechanisms: a baseline recorded after every successful save, a
//! three-way merge (`ops::reconcile`) for when a save failed and the file moved
//! underneath it, and `track_changed_on_disk` consulted at six specific sites.
//! When this suite was written it had **no** check at all on the ordinary save
//! path: `save_track_locked` serialized memory and wrote, unconditionally.
//!
//! That made the file watcher load-bearing for correctness rather than
//! freshness. An external write is noticed by `io::watcher`, delivered as a
//! path, and reloaded — but between the write landing and the event loop
//! polling it, any TUI action rewrote the whole track file from stale memory,
//! and that gap is sub-millisecond and entirely ordinary rather than an exotic
//! filesystem assumption. `FrameWatcher::start` can also fail outright, and the
//! TUI carries on without it.
//!
//! Two hand-written cases in `cli_integration.rs` cover contention, both from
//! the CLI side, which is the side that was already fixed. Nothing covered the
//! TUI side, which is what this is for.
//!
//! # Overlapping edits, and the three answers the oracle needs
//!
//! This suite began with the two writers steered onto **disjoint tasks**, and
//! the argument was that overlap has no right answer: `reconcile` keeps ours and
//! writes theirs to the recovery log, which is defensible but means "the title
//! is gone from the project and that is correct". An oracle that has to accept
//! "gone, but mentioned in `.recovery.log`" cannot tell a documented conflict
//! resolution from a plain lost update.
//!
//! That is true of an oracle with **two** answers. It stops being true with
//! three. When the two writers touch one task, what happened is one of:
//!
//! 1. **The TUI never touched it.** The title has to be in the project. The log
//!    is not an acceptable answer, and this is the case with the teeth — a merge
//!    that sets aside a version nobody contested is not resolving a conflict, it
//!    is losing an update.
//! 2. **The TUI edited a version it had absorbed.** A supersession: the user saw
//!    that title and changed it, so it is legitimately gone and nothing is owed.
//!    The same rule `EditOwned` applies to the CLI's own overwrites.
//! 3. **The TUI was holding a changed copy when the CLI wrote its own.** A real
//!    race, and the only case where the recovery log is a correct home for one
//!    of the two versions.
//!
//! [`Cli::observe_tui_step`] tells them apart by reading the app either side of
//! every step — while the answer is still observable, which at quiesce it is
//! not. So the ambiguity the steering existed to avoid is now *decided* rather
//! than avoided, and the overlapping case is where the merge machinery is
//! actually exercised.
//!
//! **The weaker properties this replaces are worth naming**, because both would
//! read as green against a defect this one catches. "The title is somewhere, or
//! it is logged" is nearly unfalsifiable. "Exactly one side survives and the
//! other is in the log in an entry that names it" sounds much stronger and is
//! satisfied *precisely* by the `8f5f3ab` defect, which filed an uncontested
//! version as a conflict. Only asking what entitled the log to hold it separates
//! the two — see
//! [`a_version_nobody_contested_never_reaches_the_recovery_log`].
//!
//! **Tracks are still steered** ([`steer`]). C5's claim is about a track's
//! *state*, and an informed TUI shelve legitimately changes it, so tracks need
//! their own version of the three-case rule against a differently shaped claim.
//! Until they have one, a run where the TUI archives a track the CLI just
//! created is ambiguous rather than false.
//!
//! # How often the overlap actually fires, measured
//!
//! Over 384 generated cases, of 1041 task-surface steps: **985 ran before the
//! CLI owned any task at all**, 23 more where the TUI had not been handed one,
//! 33 where it held one — and 14 that landed on a claimed task, 5 superseding
//! and 9 racing. Roughly 3.6% of cases reach a real overlap.
//!
//! That is the schedule shape, not the aiming: an overlap needs a CLI add, a
//! commit, and a `Watch` before a TUI step can touch anything. Biasing it
//! further would mean re-weighting `arb_event`, which re-maps the random stream
//! and silently orphans every recorded seed — so `steer` aims one target in
//! three at a CLI-owned task instead, which costs no seeds, and the branches are
//! pinned by fixed cases rather than left to the search. Without that aiming the
//! same measurement was 4 races, 2 inbox touches and **not one supersession**.
//!
//! # Why C5 exists, and why counting titles was not enough
//!
//! C1 asks whether a title is **somewhere under `frame/`**. That is the right
//! question for a task and the wrong one for a track, because a track dropped
//! from `project.toml` leaves `tracks/<id>.md` sitting there with every title
//! in it. C1 is satisfied; the track and all its tasks have left the project.
//!
//! That is not hypothetical — it is the defect the config arc fixed. The TUI
//! held a `ProjectConfig` parsed at startup and wrote the whole file back from
//! it, so a `fr track new` from another process was erased by the next TUI
//! track operation. This suite ran throughout and saw nothing, which is why
//! that arc's pins are unit and `App`-level. **A property that cannot see a
//! defect in its own subject matter is worth extending, not working around.**
//!
//! So C5: *every track the CLI created or archived is still in the state the
//! CLI left it in, with its content where that state says it should be.*
//!
//! Both halves of that earn their place. **State, not mere existence**: a row
//! flipped back to `active` by a stale config write loses the track as surely
//! as a deleted row would, because its file is in `archive/_tracks/` and
//! `load_project` looks under `tracks/`. **Content where the state says**:
//! `load_project` skips a configured track whose file is missing, so a row
//! pointing at nothing loses the track just as completely — and an archived
//! row still *names* `tracks/<id>.md` while its file has moved, so the state is
//! what decides where to look.
//!
//! The oracle reads `project.toml` **off disk**, never `app.project.config`:
//! the in-memory config is the thing under test, and asking it whether the
//! project still holds a track is asking the defendant.
//!
//! `TrackArchive` is the second write shape and the reason the claim is about
//! state rather than existence: it is a config edit *and* a file move, in that
//! order, which is what `cmd_track_state_change` does and what the `.inflight`
//! marker exists to make recoverable. The commit here does both or claims
//! nothing; injecting a crash between them is `cli_integration.rs`'s job.
//!
//! **How often this actually fires, stated rather than assumed**: across 96
//! generated cases, roughly 20 create a track and 3–5 go on to archive one. The
//! archive needs a create before it in the same schedule, so it is the rarer
//! shape by construction. That is real coverage accumulating across runs, not
//! coverage on every run, and it is worth knowing which of the two it is.
//!
//! # Why the TUI is steered off the CLI's tracks
//!
//! The argument the task-level steering used to rest on, one level up and still
//! standing. `TrackArchive` and `TrackShelve` are legitimate operations that
//! take a track out of `tracks/` or out of the active set, so a run where the
//! TUI archives a track the CLI just created makes C5 **ambiguous** rather than
//! false — "the row is gone and that is correct" is exactly the answer a
//! two-answer oracle cannot distinguish from a lost update. [`steer`] therefore
//! excludes every track the CLI has acted on from every track-surface step.
//!
//! Giving tracks the three-case treatment tasks now have is the obvious next
//! step and deliberately not taken here: the claim is about a track's *state*
//! rather than a title's presence, so it needs its own notion of what the TUI
//! absorbed and its own reading of the config merge's conflicts.
//!
//! It cuts the other way too: the CLI only ever archives a track it created
//! itself, never one out of the fixture, for the same reason.
//!
//! # Acknowledgement, precisely
//!
//! A claim is recorded only where the writer had reason to believe the write
//! landed.
//!
//! - **CLI**: its commit phase completed without error.
//! - **TUI**: the affected `SaveTarget` is not in `app.unsaved`. A save that
//!   failed on lock contention is *not* an acknowledgement — it is the
//!   documented degraded path, and the retry and merge machinery is what has to
//!   make good on it. At quiesce, after `force_retry_unsaved`, anything still
//!   held is content that never reached disk.
//!
//! # What is modelled and what is real
//!
//! The CLI actor is a model of `lock_and_load`'s nine lines — try-lock, load,
//! `recover_pending` — and not a subprocess, because the three phases have to
//! be *separable*: the whole point is to run TUI steps between the CLI's load
//! and its write, and a subprocess cannot be paused there without sleeps. The
//! existing helper in `cli_integration.rs` pays 500ms to force one interleaving;
//! a few hundred generated schedules cannot.
//!
//! Everything else is real: the real `FileLock` on the real `.lock` file, real
//! `load_project`, real `ops::`, real `save_track`, real `App` driven through
//! real `handle_key`. **The fidelity risk is stated rather than hidden:** if a
//! handler ever stops going through `lock_and_load`, this suite will not
//! notice. That is `cli_integration.rs`'s job, and anything found here gets a
//! subprocess pin next to the two contention cases already there.
//!
//! # What it does not check
//!
//! P7's unowned-content claim (C4 in the design) is **not** asserted here. This
//! suite shares P9's fixture, which by design contains no stranded lines,
//! orphans or content indented past its metadata — so the check would pass
//! vacuously and read like coverage it is not. `conservation.rs` owns that
//! claim, over a fixture built for it.
//!
//! One action is held out of the generated set, and it is not a limitation of
//! the harness: **`TrackDelete`** removes a whole track file, taking the other
//! writer's tasks with it, and like an overlapping edit that has no right
//! answer. Nothing else is held out — `TrackArchive`, `TrackRename` and
//! `TrackShelve` all stay, because they move content *within* `frame/`, and
//! "somewhere under `frame/`" is exactly what C1 asks.
//!
//! # What it found
//!
//! Four defects, and only the first was predicted:
//!
//! 1. A save erased a concurrent write whenever the watcher had not caught up
//!    (`70c3a7e`). Four events, the shortest schedule this can generate.
//! 2. Every whole-track operation — archive, delete, prefix rename — rewrote
//!    `project.toml` and moved files with **no lock at all**, so it could land
//!    inside another process's read-modify-write (`cb022dc`).
//! 3. An archived track stayed in `app.project.tracks`, so renaming it wrote it
//!    back into `tracks/` beside the archived copy (`1da9c05`).
//! 4. The same, reached by moving a task in an archived track — which is why
//!    the fix drops the track from the project rather than guarding the two
//!    actions that happened to reach it (`1da9c05`).
//!
//! Two of those four need no second writer at all. That is worth stating: a
//! suite built to interleave two processes found its way to bugs one process
//! could hit, because *generating* sequences over the whole action set is doing
//! work independently of what the sequences were generated for.
//!
//! Three more since, each from a seed now pinned in the regressions file:
//!
//! 5. A title superseded inside one lock window was promoted to a claim the CLI
//!    had never written — a flaw in this suite's own oracle, not the product
//!    (`12250db`).
//! 6. `reorder_tracks` was not its own inverse across an inactive row, so
//!    moving a track past an archived one and undoing left the order wrong
//!    (`2ee3ba6`). Pre-existing, and P9's to state; P8 reached it first.
//! 7. A handler that took a changed file as ours updated the mtime but not the
//!    ancestor, so the *next* merge read the other writer's task as one both
//!    sides had added and discarded their newer version to the recovery log
//!    (`8f5f3ab`). The headline defect's shape, on the paths where a merge is
//!    avoided rather than run.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use proptest::prelude::*;

use frame::io::lock::FileLock;
use frame::io::project_io;
use frame::model::project::Project;
use frame::ops::ids::Mint;
use frame::ops::task_ops::{self, InsertPosition};
use frame::ops::{inbox_ops, recover};
use frame::tui::app::{App, Mode};

#[path = "support/tree_checks.rs"]
mod tree_checks;

#[path = "support/tui_steps.rs"]
mod tui_steps;

use tree_checks::{id_tally, present, unsettled};
use tui_steps::{ACTIONS, ActionKind, Step, apply_step, fixture, flush_and_save, live_task_ids};

// ---------------------------------------------------------------------------
// The schedule
// ---------------------------------------------------------------------------

/// One event in a generated interleaving.
///
/// The runner is **tolerant**, not validating: a `CliOp` with no open window is
/// a no-op, and so is a second `CliBegin`. That matters for shrinking — every
/// prefix of a schedule is itself a legal schedule, so proptest shrinks by
/// truncation without hitting rejections.
#[derive(Debug, Clone, Copy)]
enum Event {
    /// One semantic TUI action, through `handle_key`.
    Tui(Step),
    /// The CLI's `lock_and_load`: try-lock, load, recover.
    CliBegin,
    /// Mutate the CLI's loaded copy. No-op with no window open.
    CliOp(CliOp),
    /// Write what the CLI changed and release the lock.
    CliCommit,
    /// Deliver every path that changed since the last `Watch` to the TUI.
    Watch,
    /// The `R` key: retry every outstanding save now.
    Retry,
}

/// What the CLI writer does inside its window. Every one either creates content
/// with a unique title or edits content the CLI itself created, so a lost title
/// is unambiguous — the TUI is free to touch any of it, and what happens when it
/// does is [`Cli::observe_tui_step`]'s business.
#[derive(Debug, Clone, Copy)]
enum CliOp {
    /// `fr add` — a new task at the bottom of a track's backlog.
    AddTask { track: usize },
    /// `fr title` — retitle a task this actor added earlier.
    EditOwned { which: usize },
    /// `fr capture` — a new inbox item.
    Capture,
    /// `fr track new` — a whole new track: a file, a config row and a prefix.
    TrackNew,
    /// `fr track archive` — a config row edit *and* a file move, on a track
    /// this actor created. Two writes, in that order, as the command does them.
    TrackArchive { which: usize },
}

fn arb_event() -> impl Strategy<Value = Event> {
    // Weighted so most schedules are "the watcher was prompt", with a
    // meaningful tail where it lags. A schedule with no `Watch` at all is legal
    // and is the shape prediction 1 is about.
    prop_oneof![
        8 => arb_tui_step().prop_map(Event::Tui),
        3 => Just(Event::CliBegin),
        4 => prop_oneof![
            2 => (0usize..2).prop_map(|track| CliOp::AddTask { track }),
            1 => (0usize..8).prop_map(|which| CliOp::EditOwned { which }),
            1 => Just(CliOp::Capture),
            2 => Just(CliOp::TrackNew),
            2 => (0usize..4).prop_map(|which| CliOp::TrackArchive { which }),
        ].prop_map(Event::CliOp),
        3 => Just(Event::CliCommit),
        5 => Just(Event::Watch),
        1 => Just(Event::Retry),
    ]
}

/// A TUI step drawn from every action except the ones held out above.
fn arb_tui_step() -> impl Strategy<Value = Step> {
    let actions: Vec<ActionKind> = ACTIONS
        .iter()
        .copied()
        .filter(|a| *a != ActionKind::TrackDelete)
        .collect();
    (0..actions.len(), 0usize..64, 0u8..26).prop_map(move |(a, target, text)| Step {
        action: actions[a],
        target,
        text,
    })
}

// ---------------------------------------------------------------------------
// The CLI actor
// ---------------------------------------------------------------------------

/// The CLI's window: what `lock_and_load` returns, held open so TUI steps can
/// run inside it.
struct Window {
    lock: FileLock,
    project: Project,
    /// The config document, read under the lock alongside the project, exactly
    /// as every config-writing handler does. Held so a `TrackNew` can edit it
    /// and the commit can write it.
    doc: toml_edit::DocumentMut,
    /// Whether this window touched the config, so the commit writes it only
    /// when a command would have.
    dirty_config: bool,
    /// Which tracks this window changed, so the commit writes those and only
    /// those — as a command does.
    dirty_tracks: BTreeSet<String>,
    dirty_inbox: bool,
    /// Titles this window created, promoted to claims only once the commit
    /// completes without error.
    pending: Vec<Claim>,
    /// Titles this window retired, dropped from the claims on the same terms.
    retired: Vec<String>,
    /// Tracks this window created or restated, as (id, the state it left them
    /// in). Promoted to claims on the same terms as titles: only once the
    /// commit completes without error.
    pending_tracks: Vec<(String, String)>,
    /// Track files the commit owes a move to `archive/_tracks/`, recorded when
    /// the row was archived and performed *after* the config is written —
    /// `cmd_track_state_change`'s order.
    pending_archive_files: Vec<(String, String)>,
}

/// A title the CLI wrote, and where it put it.
#[derive(Debug, Clone)]
struct Claim {
    title: String,
    /// `None` for an inbox item.
    task_id: Option<String>,
    /// Whether the TUI has since changed this task from a copy that did not yet
    /// hold what the CLI committed — the one condition under which the recovery
    /// log is an acceptable place for this title to end up.
    ///
    /// Set by [`Cli::observe_tui_step`], which is where the three cases are told
    /// apart and why they have to be.
    at_risk: bool,
}

struct Cli {
    root: PathBuf,
    frame_dir: PathBuf,
    window: Option<Window>,
    /// Titles this actor believes reached disk. C1's left-hand side.
    claims: Vec<Claim>,
    /// Ids of tasks this actor owns, so the TUI can be steered away from them.
    owned_ids: Vec<String>,
    /// Tracks this actor has acted on, and the state it left each in. C5's
    /// left-hand side, and what the TUI is steered off at the track level.
    ///
    /// The claim is *the state I left it in*, not merely *it still exists*:
    /// a row flipped back to `active` by a stale config write loses the track
    /// as surely as a deleted row, since its file is in `archive/_tracks/` and
    /// `load_project` looks for it under `tracks/`.
    tracks_claimed: BTreeMap<String, String>,
    /// What the CLI last committed for each task it owns, as the task's own
    /// markdown lines. The yardstick for *informed*: a TUI copy that matches it
    /// has seen the CLI's write, so an edit from there supersedes rather than
    /// races.
    committed: BTreeMap<String, Vec<String>>,
    /// Tasks the TUI is holding a changed copy of — its own edit, not a version
    /// it absorbed. Read by [`Self::commit`] to decide whether a claim is born
    /// contested.
    tui_edited: BTreeSet<String>,
    /// Titles retired because the TUI knowingly superseded them. Kept only so a
    /// fixed case can assert it reached the branch it is named for — a test that
    /// passes because nothing happened is the failure mode these exist to avoid.
    retired_by_tui: Vec<String>,
    /// Distinguishes every generated title, so a lost one is unambiguous.
    seq: usize,
}

impl Cli {
    fn new(root: &Path) -> Self {
        Cli {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            window: None,
            claims: Vec::new(),
            owned_ids: Vec::new(),
            tracks_claimed: BTreeMap::new(),
            committed: BTreeMap::new(),
            tui_edited: BTreeSet::new(),
            retired_by_tui: Vec::new(),
            seq: 0,
        }
    }

    fn next_title(&mut self) -> String {
        self.seq += 1;
        format!("cli task {}", self.seq)
    }

    /// `lock_and_load`: lock first, then read, then finish any interrupted
    /// operation — in that order, which is the whole point of that function.
    ///
    /// The timeout is zero rather than `acquire_default`'s five seconds. In a
    /// single-threaded harness a blocking acquire against a lock this harness
    /// itself holds could only ever time out, and a try-lock that fails is
    /// exactly the real "another frame process is writing" outcome — the
    /// schedule can simply try again later.
    fn begin(&mut self) {
        if self.window.is_some() {
            return;
        }
        let Ok(lock) = FileLock::acquire(&self.frame_dir, Duration::from_millis(0)) else {
            return;
        };
        let Ok(mut project) = project_io::load_project(&self.root) else {
            return;
        };
        // Recovery rewrites files, so re-read when it did anything — otherwise
        // the command would write back over its own repair. `recover_under_lock`
        // does exactly this.
        if recover::recover_pending(&mut project).is_some() {
            let Ok(reloaded) = project_io::load_project(&self.root) else {
                return;
            };
            project = reloaded;
        }
        // Under the lock, like the project itself. Every config-writing
        // handler does exactly this: `lock_and_load`, then `read_config`.
        let Ok((_, doc)) = frame::io::config_io::read_config(&self.frame_dir) else {
            return;
        };
        self.window = Some(Window {
            lock,
            project,
            doc,
            dirty_config: false,
            dirty_tracks: BTreeSet::new(),
            dirty_inbox: false,
            pending: Vec::new(),
            retired: Vec::new(),
            pending_tracks: Vec::new(),
            pending_archive_files: Vec::new(),
        });
    }

    fn op(&mut self, op: CliOp) {
        let title = match op {
            CliOp::AddTask { .. } | CliOp::Capture => Some(self.next_title()),
            CliOp::EditOwned { .. } => Some(self.next_title()),
            CliOp::TrackNew | CliOp::TrackArchive { .. } => None,
        };
        let track_id = match op {
            CliOp::TrackNew => {
                self.seq += 1;
                Some(format!("clitrack{}", self.seq))
            }
            _ => None,
        };
        let claimed: Vec<String> = self.tracks_claimed.keys().cloned().collect();
        let owned = self.owned_ids.clone();
        let Some(window) = self.window.as_mut() else {
            return;
        };
        let frame_dir = window.project.frame_dir.clone();

        match op {
            CliOp::AddTask { track } => {
                let title = title.unwrap();
                let tracks = &window.project.config.tracks;
                if tracks.is_empty() {
                    return;
                }
                let track_id = tracks[track % tracks.len()].id.clone();
                let Some(prefix) = window.project.config.ids.prefixes.get(&track_id).cloned()
                else {
                    return;
                };
                let Some(entry) = window
                    .project
                    .tracks
                    .iter_mut()
                    .find(|(id, _)| *id == track_id)
                else {
                    return;
                };
                // The primary (null) namespace, which is what the fixture's
                // `.actor` pins and therefore what the TUI mints in too. Two
                // writers in *one* namespace is exactly what C2 is about;
                // `merge_simulation.rs` already covers distinct ones.
                let mint = Mint::new(&frame_dir, &track_id, &prefix, None);
                if let Ok(id) =
                    task_ops::add_task(&mut entry.1, title.clone(), InsertPosition::Bottom, mint)
                {
                    window.dirty_tracks.insert(track_id);
                    window.pending.push(Claim {
                        title,
                        task_id: Some(id),
                        at_risk: false,
                    });
                }
            }

            CliOp::EditOwned { which } => {
                if owned.is_empty() {
                    return;
                }
                let id = owned[which % owned.len()].clone();
                let title = title.unwrap();
                for (track_id, track) in window.project.tracks.iter_mut() {
                    let Some(old) = task_ops::find_task_in_track(track, &id) else {
                        continue;
                    };
                    let old_title = old.title.clone();
                    if task_ops::edit_title(track, &id, title.clone()).is_ok() {
                        window.dirty_tracks.insert(track_id.clone());
                        // A title this same window was going to claim, edited
                        // again before the commit, never reaches disk at all —
                        // the second edit overwrote it in memory. Claiming it
                        // would have the oracle accuse the other writer of
                        // losing something nobody ever wrote. Only titles from
                        // *earlier* windows are retired against `claims`;
                        // this one has to come back out of `pending`.
                        window.pending.retain(|c| c.title != old_title);
                        window.retired.push(old_title);
                        window.pending.push(Claim {
                            title,
                            task_id: Some(id),
                            at_risk: false,
                        });
                    }
                    break;
                }
            }

            CliOp::Capture => {
                let title = title.unwrap();
                let Some(inbox) = window.project.inbox.as_mut() else {
                    return;
                };
                inbox_ops::add_inbox_item(inbox, title.clone(), Vec::new(), None);
                window.dirty_inbox = true;
                window.pending.push(Claim {
                    title,
                    task_id: None,
                    at_risk: false,
                });
            }

            CliOp::TrackNew => {
                let track_id = track_id.unwrap();
                // The real thing, and the same call `cmd_track_new` makes.
                // `new_track` writes `tracks/<id>.md` itself and edits the
                // document; the row reaches disk when the window commits.
                let Ok(track) = frame::ops::track_ops::new_track(
                    &frame_dir,
                    &mut window.doc,
                    &mut window.project.config,
                    &track_id,
                    &format!("CLI Track {track_id}"),
                ) else {
                    return;
                };
                window.project.tracks.push((track_id.clone(), track));
                window.dirty_config = true;
                window.pending_tracks.push((track_id, "active".to_string()));
            }

            CliOp::TrackArchive { which } => {
                // Only a track this actor created, and only one still active.
                // Archiving anything else would be acting on a track the TUI is
                // also entitled to touch, which is what `steer` exists to keep
                // out of the oracle.
                //
                // Tracks this window created count too, alongside ones earlier
                // windows committed. Requiring a *previous* window made the op
                // fire in none of 96 runs — it needed a create, a commit, a new
                // window and then an archive, all in one schedule — so the arm
                // was dead code that read like coverage.
                let candidates: Vec<String> = window
                    .project
                    .config
                    .tracks
                    .iter()
                    .filter(|tc| {
                        tc.state == "active"
                            && (claimed.contains(&tc.id)
                                || window.pending_tracks.iter().any(|(id, _)| id == &tc.id))
                    })
                    .map(|tc| tc.id.clone())
                    .collect();
                if candidates.is_empty() {
                    return;
                }
                let track_id = candidates[which % candidates.len()].clone();
                let Some(file) = window
                    .project
                    .config
                    .tracks
                    .iter()
                    .find(|tc| tc.id == track_id)
                    .map(|tc| tc.file.clone())
                else {
                    return;
                };
                // An add earlier in this window was a *separate command* in
                // reality, and it wrote its track before this one ran. Model
                // that by flushing now, while the file is still at
                // `tracks/<id>.md` for the commit's move to pick up.
                //
                // Dropping it from `dirty_tracks` instead — which is what this
                // did — meant the content never reached any file while the
                // commit went on to promote its claim, and C1 then reported
                // frame for losing a task nothing had written. The suite's own
                // oracle, not the product, and the third time in this shape
                // after `12250db`: **the CLI actor claims what it wrote, so
                // every op that takes content out of the window has to say
                // where it went.**
                //
                // A failed flush leaves the track in `dirty_tracks` while the
                // retain below takes it out of `project.tracks`, so the commit
                // finds no content for it, fails, and claims nothing. That is
                // the conservative answer and it needs no extra bookkeeping.
                if window.dirty_tracks.contains(&track_id)
                    && let Some((_, track)) =
                        window.project.tracks.iter().find(|(id, _)| id == &track_id)
                    && project_io::save_track(&frame_dir, &file, track).is_ok()
                {
                    window.dirty_tracks.remove(&track_id);
                }
                if frame::ops::track_ops::archive_track(
                    &mut window.doc,
                    &mut window.project.config,
                    &track_id,
                )
                .is_err()
                {
                    return;
                }
                // Out of `tracks/` is out of the project, on this side too.
                window.project.tracks.retain(|(id, _)| id != &track_id);
                window.dirty_config = true;
                window
                    .pending_tracks
                    .push((track_id.clone(), "archived".to_string()));
                window.pending_archive_files.push((track_id, file));
            }
        }
    }

    /// Write what the window changed and release the lock.
    ///
    /// Claims are promoted only if every write succeeded — the acknowledgement
    /// rule from the module docs. A commit that failed halfway claims nothing,
    /// which is stricter than the real CLI (which would have written the first
    /// file) and never accuses the TUI of losing something the CLI never
    /// managed to write.
    fn commit(&mut self) {
        let Some(window) = self.window.take() else {
            return;
        };
        let mut ok = true;
        for track_id in &window.dirty_tracks {
            let Some(file) = track_file(&window.project, track_id) else {
                ok = false;
                continue;
            };
            let Some((_, track)) = window.project.tracks.iter().find(|(id, _)| id == track_id)
            else {
                ok = false;
                continue;
            };
            if project_io::save_track(&window.project.frame_dir, &file, track).is_err() {
                ok = false;
            }
        }
        if window.dirty_inbox
            && let Some(inbox) = window.project.inbox.as_ref()
            && project_io::save_inbox(&window.project.frame_dir, inbox).is_err()
        {
            ok = false;
        }
        // The config last, which is the order `cmd_track_new` writes in: the
        // track file already exists by the time its row does. The reverse
        // would leave a row naming a file that is not there, and
        // `load_project` drops such a track silently.
        if window.dirty_config
            && frame::io::config_io::write_config(&window.project.frame_dir, &window.doc).is_err()
        {
            ok = false;
        }
        // The file move comes after the config write, which is the order
        // `cmd_track_state_change` uses and the order the `.inflight` marker
        // exists to make recoverable. Interrupted between the two is a real
        // state and `cli_integration.rs` owns injecting it; here the commit
        // either does both or claims nothing.
        for (track_id, file) in &window.pending_archive_files {
            if frame::ops::track_ops::archive_track_file(&window.project.frame_dir, track_id, file)
                .is_err()
            {
                ok = false;
            }
        }
        drop(window.lock);

        if !ok {
            return;
        }
        for title in &window.retired {
            self.claims.retain(|c| &c.title != title);
        }
        for mut claim in window.pending {
            if let Some(id) = &claim.task_id {
                // Born contested: the TUI was already holding a changed copy of
                // this task when the CLI wrote its own version, so the merge
                // that follows has a genuine conflict to resolve and the
                // recovery log is an acceptable home for one of the two.
                claim.at_risk = self.tui_edited.contains(id);
                if !self.owned_ids.contains(id) {
                    self.owned_ids.push(id.clone());
                }
                // What the TUI has to be holding to count as informed about
                // this task. Taken from the copy that was just written, so it
                // is what a reader of the file would find.
                for (_, track) in &window.project.tracks {
                    if let Some(task) = task_ops::find_task_in_track(track, id) {
                        self.committed.insert(id.clone(), own_lines(task));
                        break;
                    }
                }
            }
            self.claims.push(claim);
        }
        for (track_id, state) in window.pending_tracks {
            self.tracks_claimed.insert(track_id, state);
        }
    }

    /// Record what one TUI step did to the tasks and inbox items the CLI has
    /// claimed, given the app's contents either side of it.
    ///
    /// # Three things a step can do, not two
    ///
    /// The first cut of this asked only whether the task changed across the
    /// step, and it was wrong. **A step can change the app's view of a task
    /// without the TUI having edited it at all**: the TUI refuses to edit a task
    /// whose file moved underneath it and reloads instead, so `before` and
    /// `after` differ while the user changed nothing. Classifying that as an
    /// edit marked claims contested that nobody had contested. So:
    ///
    /// - `before == after` — untouched.
    /// - `after` is what the CLI committed — the TUI **absorbed** their version.
    ///   The opposite of an edit: it is the session catching up, and it clears
    ///   any divergence rather than creating one.
    /// - otherwise — the TUI genuinely **changed** it.
    ///
    /// **Touched means any change to the task's own lines**, not just its title.
    /// A state change conflicts in `reconcile_track` exactly as a retitle does,
    /// and sends the CLI's version — title and all — to the log.
    ///
    /// # Why divergence is remembered rather than judged here
    ///
    /// Whether a claim may end up in the recovery log cannot be decided at the
    /// step, because it turns on what the CLI does *afterwards*. The shape that
    /// matters is: the TUI edits a task it had absorbed, a CLI window then
    /// writes its own version of that same task, and the merge at save time
    /// keeps ours and logs theirs. At the moment of the TUI's edit that was an
    /// ordinary informed edit; only the CLI's later commit makes it a race.
    ///
    /// So the step records *divergence* in [`Cli::tui_edited`], and
    /// [`Cli::commit`] reads it: a claim is born at risk when the TUI was
    /// already holding a changed copy of that task. That is the same
    /// acknowledgement discipline the rest of this actor uses — the claim is
    /// classified when it is made, from what was true then.
    ///
    /// Retirement still belongs here. A TUI edit to a task whose claimed version
    /// the session had absorbed is a **supersession**: the user saw that title
    /// and changed it, so it is legitimately gone and no assertion is owed —
    /// the same rule `EditOwned` applies to the CLI's own overwrites, one actor
    /// over.
    ///
    /// # The inbox has no divergence case, and that is not an omission
    ///
    /// An inbox item's identity *is* its content, so the TUI can only act on an
    /// item it is already holding: there is no version of one to be behind. Every
    /// TUI touch is therefore a supersession. That matches what `reconcile_inbox`
    /// does — it merges by multiset, keeps both versions of a double edit, and
    /// sets nothing aside — so an inbox claim never has the log to fall back on
    /// and must never need it.
    fn observe_tui_step(
        &mut self,
        before: &BTreeMap<String, Vec<String>>,
        after: &BTreeMap<String, Vec<String>>,
        inbox_before: &BTreeMap<String, usize>,
        inbox_after: &BTreeMap<String, usize>,
    ) {
        let mut retired: Vec<String> = Vec::new();
        let mut edited: Vec<String> = Vec::new();
        let mut absorbed: Vec<String> = Vec::new();

        for claim in &self.claims {
            match &claim.task_id {
                Some(id) => {
                    let (was, now) = (before.get(id), after.get(id));
                    if was == now {
                        continue;
                    }
                    if now == self.committed.get(id) {
                        absorbed.push(id.clone());
                        continue;
                    }
                    edited.push(id.clone());
                    // Editing from the version the CLI committed is a
                    // supersession, not a race.
                    if was == self.committed.get(id) {
                        retired.push(claim.title.clone());
                    }
                }
                None => {
                    // Fewer copies in memory than before means this step took
                    // one away — an edit, a delete or a triage.
                    let n = |m: &BTreeMap<String, usize>| m.get(&claim.title).copied().unwrap_or(0);
                    if n(inbox_after) < n(inbox_before) {
                        retired.push(claim.title.clone());
                    }
                }
            }
        }

        for id in absorbed {
            self.tui_edited.remove(&id);
        }
        self.tui_edited.extend(edited);
        self.retired_by_tui.extend(retired.iter().cloned());
        self.claims.retain(|c| !retired.contains(&c.title));
    }
}

/// A task's own markdown lines, excluding its subtasks.
///
/// The unit `ops::reconcile` compares sides by, and for the same reason: a
/// subtask is merged in its own right, so folding one into its parent's text
/// would report a change to the child as a change to the parent.
fn own_lines(task: &frame::model::task::Task) -> Vec<String> {
    let mut bare = task.clone();
    bare.subtasks.clear();
    frame::parse::serialize_tasks(std::slice::from_ref(&bare), 0)
}

/// What the TUI is holding for each task the CLI owns, keyed by id.
///
/// Read out of `app.project` rather than off disk on purpose: the question is
/// what the *session* believes, because that is what its next save writes and
/// what the merge offers as "ours".
fn tui_view(app: &App, cli: &Cli) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for id in &cli.owned_ids {
        for (_, track) in &app.project.tracks {
            if let Some(task) = task_ops::find_task_in_track(track, id) {
                out.insert(id.clone(), own_lines(task));
                break;
            }
        }
    }
    out
}

/// How many copies of each inbox title the TUI is holding.
///
/// A count rather than a set, because `reconcile_inbox` works in multisets and
/// two identical captures are two items, not one.
fn tui_inbox(app: &App) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(inbox) = &app.project.inbox {
        for item in &inbox.items {
            *out.entry(item.title.clone()).or_default() += 1;
        }
    }
    out
}

fn track_file(project: &Project, track_id: &str) -> Option<String> {
    project
        .config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.file.clone())
}

// ---------------------------------------------------------------------------
// Aiming the TUI
// ---------------------------------------------------------------------------

/// Re-aim a generated step so it never lands on a **track** the CLI owns, and
/// so a share of task steps land squarely on tasks it does.
///
/// Returns `None` when there is nothing left for it to act on, in which case the
/// step is skipped. `apply_step` resolves `target` modulo the number of live
/// candidates, so rewriting `target` to the index of a chosen one steers the
/// step without teaching the shared driver anything about this suite.
///
/// # Why tasks and inbox items are no longer steered away
///
/// They were, and the module docs used to argue they had to be: on one task
/// there is no right answer, so an oracle that accepts "gone from the project
/// but named in the log" cannot tell a resolved conflict from a lost update.
///
/// That holds only for an oracle that cannot see *which of the two* it is
/// looking at. [`Cli::observe_tui_step`] can — it reads the app either side of
/// every step and separates a supersession from a race — so the ambiguity the
/// steering existed to avoid is now decided rather than avoided.
///
/// # Why a share of them is aimed deliberately
///
/// Left to chance, overlap almost never happens. Reaching it needs a CLI add, a
/// commit, a `Watch` to put the task in the TUI's hands, and then a step landing
/// on that task out of the ten-odd live ones — and, for the stale case, a second
/// CLI window editing it with no `Watch` after. Measured over 384 generated
/// cases with no deliberate aiming: **four stale touches, two inbox touches, and
/// not one informed touch.** A property that reaches its own subject matter 1.5%
/// of the time and one of its three branches never is not covering it.
///
/// So one target in three is aimed at a CLI-owned task when the TUI is holding
/// one. The entropy comes from `step.target`, which the generator already
/// produces — **deliberately not from a new arm or a new weight in `arb_event`**,
/// because that re-maps the whole random stream and silently orphans every
/// recorded seed. Steering is the harness's own aiming mechanism and changing it
/// costs no seeds at all.
///
/// # Tracks stay steered
///
/// The same work has not been done for them. C5's claim is about a track's
/// *state*, and an informed TUI shelve legitimately changes that state, so it
/// needs its own version of the three-case rule against a differently shaped
/// claim. Until it has one, a run where the TUI archives a track the CLI just
/// created is still ambiguous rather than false.
fn steer(app: &App, step: &Step, cli: &Cli) -> Option<Step> {
    let mut step = *step;
    match step.action.surface() {
        tui_steps::Surface::Task => {
            if step.target.is_multiple_of(3) {
                let live = live_task_ids(app);
                // A CLI-owned task's descendants count as the TUI's own: subtask
                // ids are minted under the parent's, so `M-004.1` under a
                // CLI-owned `M-004` was added by whoever added the subtask.
                // Aiming is about the claimed task itself.
                let owned: Vec<usize> = live
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| cli.owned_ids.contains(id))
                    .map(|(i, _)| i)
                    .collect();
                if !owned.is_empty() {
                    step.target = owned[(step.target / 3) % owned.len()];
                }
            }
        }
        tui_steps::Surface::Inbox => {}
        tui_steps::Surface::Tracks => {
            // The rule the task-level steering used to follow, one level up.
            // `TrackArchive` and
            // `TrackShelve` are legitimate TUI operations that take a track out
            // of `tracks/` or out of the active set — so a run where the TUI
            // archives a track the CLI just created makes C5 *ambiguous*, not
            // false, exactly as two writers on one task make C1 ambiguous.
            // Steering keeps every failure a real one.
            //
            // **In the coordinates the action will resolve it in.** `target`
            // becomes `tracks_cursor`, and that indexes `App::tracks_view_order`
            // — active, then shelved, then archived — not `config.tracks`.
            // Filtering `config.tracks` and indexing that got the right answer
            // only while every track was active: once a step archived one, the
            // row moved to the end and everything below it shifted up, so
            // "index 1 among the unclaimed" pointed at the track after the one
            // meant, which in the CI seed pinned below was the CLI's own.
            // Steering aimed at `side` and shelved `clitrack1`, and C5 reported
            // frame for it.
            let allowed: Vec<usize> = app
                .tracks_view_order()
                .iter()
                .enumerate()
                .filter(|(_, id)| !cli.tracks_claimed.contains_key(**id))
                .map(|(i, _)| i)
                .collect();
            if allowed.is_empty() {
                return None;
            }
            step.target = allowed[step.target % allowed.len()];
        }
    }
    Some(step)
}

// ---------------------------------------------------------------------------
// The watcher, as a scheduling decision
// ---------------------------------------------------------------------------

/// Every file under `frame/` and its bytes, so a `Watch` can deliver exactly
/// what changed since the last one.
fn snapshot(frame_dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![frame_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(path, bytes);
            }
        }
    }
    out
}

/// Deliver every path whose bytes changed since the last delivery.
///
/// The TUI's *own* writes are delivered too, because the real watcher delivers
/// them — the mtime bookkeeping in `save_track_locked` is what is supposed to
/// make that harmless. And not scheduling a `Watch` is not an exotic
/// assumption: a CLI write followed by a keypress before the event loop polls
/// is sub-millisecond and entirely ordinary.
fn deliver(app: &mut App, frame_dir: &Path, last: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let now = snapshot(frame_dir);
    let changed: Vec<PathBuf> = now
        .iter()
        .filter(|(path, bytes)| last.get(*path) != Some(*bytes))
        .map(|(path, _)| path.clone())
        .collect();
    *last = now;
    if !changed.is_empty() {
        app.reload_changed_files(&changed);
    }
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

/// What the run left behind, and what each side says should be there.
struct Verdict {
    lost_by_cli: Vec<String>,
    lost_by_tui: Vec<String>,
    duplicate_ids: Vec<String>,
    unsettled: Option<String>,
    still_unsaved: Vec<String>,
    /// C5: tracks the CLI created or archived that are no longer in the state
    /// it left them in, or whose content is not where that state says.
    unconfigured_tracks: Vec<String>,
    /// C6: titles in the recovery log that no conflict entitles it to — the
    /// merge set a version aside on a task the TUI never raced it for.
    unjustified_setaside: Vec<String>,
    /// C6's other half: a title the log did keep, in an entry that does not name
    /// the task it came from. Content nobody can find is content nobody can put
    /// back.
    unidentified_setaside: Vec<String>,
    /// C7: a claimed title present more than once. Keeping both sides of a
    /// conflict as two tasks loses no text and still corrupts the project.
    duplicated_titles: Vec<String>,
    /// Claims that left the project and were found in the recovery log. Not a
    /// failure — `unjustified_setaside` is the failure — but the fixed cases
    /// assert on it so that a race they mean to provoke cannot quietly not
    /// happen and still pass.
    log_satisfied: Vec<String>,
    /// Titles the TUI knowingly superseded, from [`Cli::retired_by_tui`], for
    /// the same reason.
    retired_by_tui: Vec<String>,
}

fn judge(app: &App, frame_dir: &Path, cli: &Cli) -> Verdict {
    let (titles, _) = present(frame_dir);
    let text = tree_checks::all_text(frame_dir);
    // Not part of `all_text`, which collects `.md` only — so "in the project"
    // and "in the log" stay two separate questions, which is the whole point of
    // asking them separately.
    //
    // **`Write` entries only, and the distinction is load-bearing.** Frame logs
    // a deleted task's source too (`log_task_deletion`, category `Delete`), and
    // that is a different mechanism with a different licence: it records what a
    // user threw away on purpose, not a version a merge set aside. Accepting any
    // category let a deletion entry stand in for a conflict resolution, which is
    // precisely the conflation C6 exists to prevent — found by reverting the
    // retirement rule and watching C6 fire on a plain delete.
    let log: Vec<_> = frame::io::recovery::read_recovery_entries(frame_dir, None, None)
        .into_iter()
        .filter(|e| e.category == frame::io::recovery::RecoveryCategory::Write)
        .collect();

    let mut lost_by_cli = Vec::new();
    let mut unjustified_setaside = Vec::new();
    let mut unidentified_setaside = Vec::new();
    let mut log_satisfied = Vec::new();

    for claim in &cli.claims {
        if titles.contains(&claim.title) || text.contains(&claim.title) {
            continue;
        }
        // Gone from the project. Whether that is allowed turns entirely on what
        // the TUI did to this task, which `observe_tui_step` decided at the time
        // and recorded in `at_risk`.
        let Some(entry) = log
            .iter()
            .find(|e| e.body.contains(&claim.title) || e.description.contains(&claim.title))
        else {
            lost_by_cli.push(claim.title.clone());
            continue;
        };
        log_satisfied.push(claim.title.clone());
        if !claim.at_risk {
            // The log has it and nothing licensed the log to have it: no TUI
            // edit raced this task. A merge that sets aside a version nobody
            // contested is not resolving a conflict, it is losing an update —
            // the shape `8f5f3ab` had, which the old disjoint property could
            // only see as a plain loss and a looser one would call correct.
            unjustified_setaside.push(claim.title.clone());
            continue;
        }
        if let Some(id) = &claim.task_id
            && !frame::io::recovery::entry_names(entry, id)
        {
            unidentified_setaside.push(format!("{} (task {id})", claim.title));
        }
    }

    // C7. Titles rather than ids: `duplicate_ids` below already covers the same
    // id appearing twice, and a task resurrected under a *new* id by a merge on
    // the track it was moved out of would pass that and still leave the project
    // holding two of it.
    let mut title_tally: BTreeMap<String, usize> = BTreeMap::new();
    for task in tree_checks::all_tasks(frame_dir) {
        if !task.title.trim().is_empty() {
            *title_tally.entry(task.title.clone()).or_default() += 1;
        }
    }
    let duplicated_titles: Vec<String> = cli
        .claims
        .iter()
        // Tasks only. An inbox item is not in `all_tasks`, and duplicating one
        // is what `reconcile_inbox` does on purpose.
        .filter(|c| c.task_id.is_some())
        .filter(|c| title_tally.get(&c.title).is_some_and(|n| *n > 1))
        .map(|c| c.title.clone())
        .collect();

    // The TUI's side of C1: everything it is still holding in memory, with
    // nothing left in `unsaved`, must be on disk. Titles rather than ids
    // because an inbox item has no id.
    let mut in_memory: Vec<String> = Vec::new();
    for (_, track) in &app.project.tracks {
        for task in tree_checks::tasks_of(track) {
            if !task.title.trim().is_empty() {
                in_memory.push(task.title.clone());
            }
        }
    }
    if let Some(inbox) = &app.project.inbox {
        for item in &inbox.items {
            if !item.title.trim().is_empty() {
                in_memory.push(item.title.clone());
            }
        }
    }
    let lost_by_tui = in_memory
        .into_iter()
        .filter(|t| !titles.contains(t) && !text.contains(t))
        .collect();

    // C5, read off disk rather than out of `app.project.config` — the
    // in-memory config is the thing under test, and asking it whether the
    // project still holds a track would be asking the defendant.
    //
    // A row whose file is missing counts as gone, and that is not pedantry:
    // `load_project` skips a configured track whose file is absent, so the
    // track and every task in it leave the project just as completely as a
    // deleted row would take them.
    let unconfigured_tracks = match frame::io::config_io::read_config(frame_dir) {
        Ok((config, _)) => cli
            .tracks_claimed
            .iter()
            .filter(|(id, expected)| {
                let Some(tc) = config.tracks.iter().find(|tc| &tc.id == *id) else {
                    return true; // the row is gone
                };
                if &&tc.state != expected {
                    return true; // not the state the CLI left it in
                }
                // And the content is where that state says it is. An archived
                // track's file lives in `archive/_tracks/` while its row still
                // names `tracks/<id>.md`, so the state decides where to look.
                let path = if tc.state == "archived" {
                    frame_dir.join("archive/_tracks").join(format!("{id}.md"))
                } else {
                    frame_dir.join(&tc.file)
                };
                !path.exists()
            })
            .map(|(id, _)| id.clone())
            .collect(),
        // An unreadable config is a total loss of every track, not a reason to
        // report none: reading it is what `load_project` does first.
        Err(_) => cli.tracks_claimed.keys().cloned().collect(),
    };

    let mut seen = BTreeSet::new();
    let mut duplicate_ids = Vec::new();
    for id in id_tally(frame_dir) {
        if !seen.insert(id.clone()) {
            duplicate_ids.push(id);
        }
    }

    Verdict {
        lost_by_cli,
        lost_by_tui,
        duplicate_ids,
        unsettled: unsettled(frame_dir),
        still_unsaved: app.unsaved.keys().map(|t| t.label().to_string()).collect(),
        unconfigured_tracks,
        unjustified_setaside,
        unidentified_setaside,
        duplicated_titles,
        log_satisfied,
        retired_by_tui: cli.retired_by_tui.clone(),
    }
}

/// Run one schedule to quiesce and judge what it left behind.
///
/// Shared by the property and by the fixed cases below. A fixed case pins a
/// schedule that a generator change cannot orphan, which a `.proptest-regressions`
/// seed cannot do — a seed decodes through `arb_event` *as it is now*, so adding
/// an arm silently re-aims every line recorded before it.
///
/// `Err` is a step that left the app somewhere other than Navigate: in this
/// harness that sends the *next* step's keys to a handler nobody intended, so
/// the run is meaningless rather than failing.
fn run(schedule: &[Event]) -> Result<Verdict, String> {
    // Five seconds per contended acquisition would make this suite take hours.
    // Only the waiting is shortened; contention itself is real.
    frame::io::lock::cap_waits(Duration::from_millis(20));

    let tmp = fixture();
    let root = tmp.path();
    let frame_dir = root.join("frame");

    let project = project_io::load_project(root).expect("project loads");
    let mut app = App::new(project);
    let mut cli = Cli::new(root);
    let mut delivered = snapshot(&frame_dir);

    for event in schedule {
        match event {
            Event::Tui(step) => {
                if let Some(step) = steer(&app, step, &cli) {
                    // Read either side of the step, so what it did to a task the
                    // CLI owns can be classified while the answer is still
                    // observable. At quiesce it is far too late: the merge has
                    // run and every trace of who was holding what is gone.
                    let before = tui_view(&app, &cli);
                    let inbox_before = tui_inbox(&app);
                    apply_step(&mut app, &step);
                    if app.mode != Mode::Navigate {
                        return Err(format!("step {step:?} left the app in {:?}", app.mode));
                    }
                    let after = tui_view(&app, &cli);
                    let inbox_after = tui_inbox(&app);
                    cli.observe_tui_step(&before, &after, &inbox_before, &inbox_after);
                }
            }
            Event::CliBegin => cli.begin(),
            Event::CliOp(op) => cli.op(*op),
            Event::CliCommit => cli.commit(),
            Event::Watch => deliver(&mut app, &frame_dir, &mut delivered),
            Event::Retry => app.force_retry_unsaved(),
        }
    }

    // Quiesce, in the order the real thing would settle: the other process
    // finishes and lets go, the watcher catches up, the grace period drains,
    // the TUI is given every chance to write what it is holding, and one last
    // delivery lands.
    cli.commit();
    deliver(&mut app, &frame_dir, &mut delivered);
    flush_and_save(&mut app);
    app.force_retry_unsaved();
    deliver(&mut app, &frame_dir, &mut delivered);

    Ok(judge(&app, &frame_dir, &cli))
}

/// A window that adds a task to a track and then archives that track claimed
/// the task without ever writing it.
///
/// `fr add` and `fr track archive` are two commands, each with its own lock and
/// its own write, so the task reaches `tracks/<id>.md` before the archive moves
/// that file — and the title is under `frame/` either way. Modelling both as
/// one window let `CliOp::TrackArchive` drop the track from `dirty_tracks` with
/// the add still unwritten, and the commit promoted the claim regardless. C1
/// then reported frame for losing a task nothing had written.
///
/// Pinned as a fixed case rather than left to the seed that found it, because
/// the seed decodes through `arb_event` and the next arm added there would
/// re-aim it at something else entirely.
#[test]
fn a_track_archived_after_an_add_still_carries_the_task() {
    let schedule = [
        Event::CliBegin,
        Event::CliOp(CliOp::TrackNew),
        Event::CliCommit,
        Event::CliBegin,
        // Straight at the track the previous window created, which is the only
        // one `TrackArchive` will consider.
        Event::CliOp(CliOp::AddTask { track: 2 }),
        Event::CliOp(CliOp::TrackArchive { which: 0 }),
    ];
    let verdict = run(&schedule).expect("schedule runs");
    assert!(
        verdict.lost_by_cli.is_empty(),
        "a task added before its track was archived went with the file: {:?}",
        verdict.lost_by_cli
    );
    assert!(
        verdict.unconfigured_tracks.is_empty(),
        "{:?}",
        verdict.unconfigured_tracks
    );
}

/// Aim a task step at the first task the CLI owns.
///
/// `steer` re-aims a target when `target % 3 == 0`, picking
/// `owned[(target / 3) % owned.len()]`, so zero is "the CLI's first task" and
/// reads as that at every call site below.
fn at_cli_task(action: ActionKind) -> Event {
    Event::Tui(Step {
        action,
        target: 0,
        text: 0,
    })
}

/// **Informed supersession.** The TUI absorbed the CLI's task and then deleted
/// it. The title is gone from the project, nothing is in the recovery log, and
/// that is correct — the user saw that version and removed it.
///
/// This is the case the disjoint property could never express, and the reason
/// the oracle needs three answers rather than two. Without the retirement rule
/// in `observe_tui_step` the claim survives and C1 reports frame for losing a
/// task the user deliberately threw away — confirmed by reverting it.
#[test]
fn a_delete_of_a_task_the_tui_had_absorbed_retires_the_claim() {
    let verdict = run(&[
        Event::CliBegin,
        Event::CliOp(CliOp::AddTask { track: 0 }),
        Event::CliCommit,
        // The watcher catches up, so the TUI's copy *is* what the CLI wrote.
        Event::Watch,
        at_cli_task(ActionKind::DeleteTask),
    ])
    .expect("schedule runs");

    // Non-vacuity first: this case is worthless unless the informed branch
    // actually ran and the title actually left the project.
    assert_eq!(
        verdict.retired_by_tui,
        vec!["cli task 1".to_string()],
        "the informed branch did not fire, so the rest of this proves nothing"
    );
    assert!(
        verdict.lost_by_cli.is_empty(),
        "a task the TUI knowingly deleted is not a lost update: {:?}",
        verdict.lost_by_cli
    );
    assert!(
        verdict.unjustified_setaside.is_empty(),
        "nothing raced here, so nothing belongs in the log: {:?}",
        verdict.unjustified_setaside
    );
}

/// **A genuine race.** The TUI edits a task inside the window in which a CLI
/// command goes on to write its own version of that same task.
///
/// `reconcile_track` keeps ours and hands theirs to the recovery log, so the
/// CLI's newest title leaves the project — the documented resolution, not a
/// loss. The claim is allowed that answer *only* because the TUI was holding a
/// changed copy when the CLI committed. Had the TUI never touched the task, the
/// identical end state would be `unjustified_setaside` instead, which is the
/// shape `8f5f3ab` had and the distinction the whole three-case rule buys.
#[test]
fn a_stale_edit_sends_the_cli_version_to_the_recovery_log() {
    let verdict = run(&[
        Event::CliBegin,
        Event::CliOp(CliOp::AddTask { track: 0 }),
        Event::CliCommit,
        Event::Watch,
        // The second window takes the lock and holds it. The TUI's edit lands
        // *inside* that window — which is the only way a stale edit happens at
        // all, and is why the order here is not incidental: after the CLI has
        // written, the TUI sees the file moved and reloads instead of editing.
        // The documented degraded path is exactly this — a save that failed on
        // lock contention, followed by that process writing the file.
        Event::CliBegin,
        at_cli_task(ActionKind::EditTitle),
        Event::CliOp(CliOp::EditOwned { which: 0 }),
        Event::CliCommit,
    ])
    .expect("schedule runs");

    // Non-vacuity: the race has to have actually happened. `cli task 2` is the
    // retitle, and it must be gone from the project and found in the log —
    // otherwise this passes without ever reaching a conflict.
    assert_eq!(
        verdict.log_satisfied,
        vec!["cli task 2".to_string()],
        "no version reached the recovery log, so no conflict was provoked"
    );
    assert!(
        verdict.lost_by_cli.is_empty(),
        "the log is an acceptable home for a contested version: {:?}",
        verdict.lost_by_cli
    );
    assert!(
        verdict.unjustified_setaside.is_empty(),
        "this race is what entitles the log to hold it: {:?}",
        verdict.unjustified_setaside
    );
    assert!(
        verdict.unidentified_setaside.is_empty(),
        "and it has to say which task it came from: {:?}",
        verdict.unidentified_setaside
    );
    assert!(
        verdict.duplicated_titles.is_empty(),
        "one side survives, not both: {:?}",
        verdict.duplicated_titles
    );
}

/// **The inbox, where nothing is ever set aside.** A capture the TUI went on to
/// delete is retired like any informed touch — and unlike a track, there is no
/// stale case to fall back on, because an item's identity is its content and the
/// TUI can only act on one it already holds.
#[test]
fn an_inbox_capture_the_tui_deleted_is_not_a_lost_update() {
    let verdict = run(&[
        Event::CliBegin,
        Event::CliOp(CliOp::Capture),
        Event::CliCommit,
        Event::Watch,
        // The fixture has two items and the capture appends, so index 2 is the
        // CLI's. Inbox steps are not re-aimed, so this is the raw cursor.
        Event::Tui(Step {
            action: ActionKind::InboxDelete,
            target: 2,
            text: 0,
        }),
    ])
    .expect("schedule runs");

    assert_eq!(
        verdict.retired_by_tui,
        vec!["cli task 1".to_string()],
        "the inbox branch did not fire, so the rest of this proves nothing"
    );
    assert!(
        verdict.lost_by_cli.is_empty(),
        "an item the TUI triaged away is not a lost update: {:?}",
        verdict.lost_by_cli
    );
    assert!(
        verdict.unjustified_setaside.is_empty(),
        "and the inbox merge sets nothing aside: {:?}",
        verdict.unjustified_setaside
    );
}

/// **C6's teeth, stated deterministically.** A version the TUI never contested
/// must never end up in the recovery log.
///
/// This is the `8f5f3ab` shape: the TUI's save fails on the held lock, a handler
/// then takes the changed file into memory to catch up, and the CLI writes the
/// same task again. If adopting a file updates the mtime but not the ancestor,
/// the next merge meets that task absent from the ancestor and present on both
/// sides — "both added it differently" — keeps ours and files theirs as a
/// conflict. The merge does the right thing with wrong inputs, and **nobody was
/// ever in dispute**.
///
/// Every weaker oracle calls this correct. "The title is somewhere, or it is
/// logged" is satisfied. Even "exactly one side survives and the other is in the
/// log in an entry that names it" is satisfied — that is exactly what the buggy
/// merge produces. Only asking *what entitled the log to hold it* separates the
/// two, which is why `at_risk` exists and why it is set from what the TUI was
/// holding rather than from the end state.
///
/// **Fixed rather than left to the search.** With the defect reintroduced this
/// schedule reports it instantly; the property ran 768 generated cases and never
/// reached it. The seed that originally found it is one of the orphaned `found:`
/// lines in the regressions file, which is the rule in `CLAUDE.md` playing out
/// exactly as written.
#[test]
fn a_version_nobody_contested_never_reaches_the_recovery_log() {
    let verdict = run(&[
        // The CLI adds a task the TUI has never seen.
        Event::CliBegin,
        Event::CliOp(CliOp::AddTask { track: 0 }),
        Event::CliCommit,
        // A window opens and holds the lock, so the TUI's edit cannot save and
        // goes to `unsaved` — the designed degraded path. Catching up on the
        // changed file is what brings the CLI's task into memory.
        Event::CliBegin,
        Event::Tui(Step {
            action: ActionKind::EditTitle,
            target: 1,
            text: 0,
        }),
        // The same task, written again by the same actor that first wrote it.
        Event::CliOp(CliOp::EditOwned { which: 0 }),
        Event::CliCommit,
        // Anything that saves. The merge runs here.
        Event::Tui(Step {
            action: ActionKind::SetDone,
            target: 1,
            text: 0,
        }),
    ])
    .expect("schedule runs");

    assert!(
        verdict.unjustified_setaside.is_empty(),
        "the TUI never touched this task, so no merge may set its version aside: {:?}",
        verdict.unjustified_setaside
    );
    assert!(
        verdict.log_satisfied.is_empty(),
        "with nothing in dispute the log should be empty of claims entirely: {:?}",
        verdict.log_satisfied
    );
    assert!(verdict.lost_by_cli.is_empty(), "{:?}", verdict.lost_by_cli);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// P8: an interleaving of a TUI session and a CLI writer loses nothing
    /// either one acknowledged, and sets a version aside only where they raced.
    #[test]
    fn p8_two_writers_do_not_lose_acknowledged_work(
        // Long schedules on purpose. At 12 events this found the first two
        // defects and then read as green; the third and fourth needed a run
        // where a track could be archived *and then still be acted on*, which
        // is three or four track-level actions deep. The cost is a few seconds.
        schedule in prop::collection::vec(arb_event(), 1..28)
    ) {
        let verdict = match run(&schedule) {
            Ok(verdict) => verdict,
            Err(detail) => return Err(TestCaseError::fail(format!("{detail}\nschedule: {schedule:?}"))),
        };

        prop_assert!(
            verdict.still_unsaved.is_empty(),
            "the TUI is still holding unsaved work at quiesce: {:?}\nschedule: {schedule:?}",
            verdict.still_unsaved
        );
        prop_assert!(
            verdict.lost_by_cli.is_empty(),
            "the CLI wrote these and they are gone: {:?}\nschedule: {schedule:?}",
            verdict.lost_by_cli
        );
        prop_assert!(
            verdict.unconfigured_tracks.is_empty(),
            "the CLI left these tracks in a state the project no longer has: {:?}\nschedule: {schedule:?}",
            verdict.unconfigured_tracks
        );
        prop_assert!(
            verdict.unjustified_setaside.is_empty(),
            "the merge put these in the recovery log with no concurrent edit to justify it: {:?}\nschedule: {schedule:?}",
            verdict.unjustified_setaside
        );
        prop_assert!(
            verdict.unidentified_setaside.is_empty(),
            "these survive only in the recovery log, in an entry that does not name the task: {:?}\nschedule: {schedule:?}",
            verdict.unidentified_setaside
        );
        prop_assert!(
            verdict.duplicated_titles.is_empty(),
            "these titles are in the project more than once: {:?}\nschedule: {schedule:?}",
            verdict.duplicated_titles
        );
        prop_assert!(
            verdict.lost_by_tui.is_empty(),
            "the TUI believes these are in the project and they are not on disk: {:?}\nschedule: {schedule:?}",
            verdict.lost_by_tui
        );
        prop_assert!(
            verdict.duplicate_ids.is_empty(),
            "these ids were handed out twice: {:?}\nschedule: {schedule:?}",
            verdict.duplicate_ids
        );
        if let Some(detail) = verdict.unsettled {
            return Err(TestCaseError::fail(format!("{detail}\nschedule: {schedule:?}")));
        }
    }
}
