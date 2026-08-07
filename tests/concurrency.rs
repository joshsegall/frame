//! P8 — two writers over one project.
//!
//! > For a settled project, and any interleaving of a TUI session and a CLI
//! > writer whose edits touch **disjoint tasks**, at quiesce: every title
//! > either writer acknowledged writing is somewhere under `frame/`, no ID
//! > appears twice, and every file is settled.
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
//! # Why disjoint tasks
//!
//! Because it removes every judgement call from the oracle. When both writers
//! edit the same task there is no right answer — `reconcile` keeps ours and
//! writes theirs to the recovery log, which is defensible but means "the title
//! is gone from the project and that is correct". An oracle that has to accept
//! "gone, but mentioned in `.recovery.log`" cannot tell a documented conflict
//! resolution from a plain lost update.
//!
//! So the CLI actor only ever adds tasks with unique generated titles and edits
//! tasks it added itself; the TUI actor is steered away from anything the CLI
//! owns ([`steer`]). They still write **the same files** — which is the whole
//! point, since a track file is rewritten whole — but never the same task.
//! Under those conditions a merge should be clean every time, and any loss is a
//! real loss.
//!
//! A looser property that allows overlap and accepts the recovery-log escape
//! hatch is a reasonable follow-up. It is deliberately not first: it would find
//! the same defects with a weaker signal.
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
/// with a unique title or edits content the CLI itself created — see the
/// module docs on why disjointness is the price of an unambiguous oracle.
#[derive(Debug, Clone, Copy)]
enum CliOp {
    /// `fr add` — a new task at the bottom of a track's backlog.
    AddTask { track: usize },
    /// `fr title` — retitle a task this actor added earlier.
    EditOwned { which: usize },
    /// `fr capture` — a new inbox item.
    Capture,
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
    /// Which tracks this window changed, so the commit writes those and only
    /// those — as a command does.
    dirty_tracks: BTreeSet<String>,
    dirty_inbox: bool,
    /// Titles this window created, promoted to claims only once the commit
    /// completes without error.
    pending: Vec<Claim>,
    /// Titles this window retired, dropped from the claims on the same terms.
    retired: Vec<String>,
}

/// A title the CLI wrote, and where it put it.
#[derive(Debug, Clone)]
struct Claim {
    title: String,
    /// `None` for an inbox item.
    task_id: Option<String>,
}

struct Cli {
    root: PathBuf,
    frame_dir: PathBuf,
    window: Option<Window>,
    /// Titles this actor believes reached disk. C1's left-hand side.
    claims: Vec<Claim>,
    /// Ids of tasks this actor owns, so the TUI can be steered away from them.
    owned_ids: Vec<String>,
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
        self.window = Some(Window {
            lock,
            project,
            dirty_tracks: BTreeSet::new(),
            dirty_inbox: false,
            pending: Vec::new(),
            retired: Vec::new(),
        });
    }

    fn op(&mut self, op: CliOp) {
        let title = match op {
            CliOp::AddTask { .. } | CliOp::Capture => Some(self.next_title()),
            CliOp::EditOwned { .. } => Some(self.next_title()),
        };
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
                });
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
        drop(window.lock);

        if !ok {
            return;
        }
        for title in &window.retired {
            self.claims.retain(|c| &c.title != title);
        }
        for claim in window.pending {
            if let Some(id) = &claim.task_id
                && !self.owned_ids.contains(id)
            {
                self.owned_ids.push(id.clone());
            }
            self.claims.push(claim);
        }
    }

    /// Titles the CLI owns that live in the inbox, so a TUI step is not aimed
    /// at one.
    fn owned_inbox_titles(&self) -> BTreeSet<String> {
        self.claims
            .iter()
            .filter(|c| c.task_id.is_none())
            .map(|c| c.title.clone())
            .collect()
    }
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
// Keeping the two writers disjoint
// ---------------------------------------------------------------------------

/// Re-aim a generated step so it never lands on something the CLI owns.
///
/// Returns `None` when there is nothing left for it to act on, in which case
/// the step is skipped. This is the mechanism the whole property rests on:
/// `apply_step` resolves `target` modulo the number of live candidates, so
/// rewriting `target` to the index of an allowed one steers the step without
/// teaching the shared driver anything about this suite.
///
/// A CLI-owned task is excluded along with its descendants — the TUI's own
/// subtask ids are minted under the parent's, so `M-004.1` belongs to whoever
/// owns `M-004`.
fn steer(app: &App, step: &Step, cli: &Cli) -> Option<Step> {
    let mut step = *step;
    match step.action.surface() {
        tui_steps::Surface::Task => {
            let live = live_task_ids(app);
            let allowed: Vec<usize> = live
                .iter()
                .enumerate()
                .filter(|(_, id)| !cli.owned_ids.iter().any(|o| id_is_under(id, o)))
                .map(|(i, _)| i)
                .collect();
            if allowed.is_empty() {
                return None;
            }
            step.target = allowed[step.target % allowed.len()];
        }
        tui_steps::Surface::Inbox => {
            let owned = cli.owned_inbox_titles();
            let items = app.project.inbox.as_ref().map(|i| &i.items)?;
            let allowed: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, item)| !owned.contains(&item.title))
                .map(|(i, _)| i)
                .collect();
            if allowed.is_empty() {
                return None;
            }
            step.target = allowed[step.target % allowed.len()];
        }
        tui_steps::Surface::Tracks => {}
    }
    Some(step)
}

fn id_is_under(id: &str, owner: &str) -> bool {
    id == owner || id.starts_with(&format!("{owner}."))
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
}

fn judge(app: &App, frame_dir: &Path, cli: &Cli) -> Verdict {
    let (titles, _) = present(frame_dir);
    let text = tree_checks::all_text(frame_dir);

    let lost_by_cli = cli
        .claims
        .iter()
        .filter(|c| !titles.contains(&c.title) && !text.contains(&c.title))
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
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// P8: an interleaving of a TUI session and a CLI writer over disjoint
    /// tasks loses nothing either one acknowledged.
    #[test]
    fn p8_two_writers_do_not_lose_acknowledged_work(
        // Long schedules on purpose. At 12 events this found the first two
        // defects and then read as green; the third and fourth needed a run
        // where a track could be archived *and then still be acted on*, which
        // is three or four track-level actions deep. The cost is a few seconds.
        schedule in prop::collection::vec(arb_event(), 1..28)
    ) {
        // Five seconds per contended acquisition would make this suite take
        // hours. Only the waiting is shortened; contention itself is real.
        frame::io::lock::cap_waits(Duration::from_millis(20));

        let tmp = fixture();
        let root = tmp.path();
        let frame_dir = root.join("frame");

        let project = project_io::load_project(root).expect("project loads");
        let mut app = App::new(project);
        let mut cli = Cli::new(root);
        let mut delivered = snapshot(&frame_dir);

        for event in &schedule {
            match event {
                Event::Tui(step) => {
                    if let Some(step) = steer(&app, step, &cli) {
                        apply_step(&mut app, &step);
                        prop_assert!(
                            app.mode == Mode::Navigate,
                            "step {step:?} left the app in {:?}",
                            app.mode
                        );
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
        // finishes and lets go, the watcher catches up, the grace period
        // drains, the TUI is given every chance to write what it is holding,
        // and one last delivery lands.
        cli.commit();
        deliver(&mut app, &frame_dir, &mut delivered);
        flush_and_save(&mut app);
        app.force_retry_unsaved();
        deliver(&mut app, &frame_dir, &mut delivered);

        let verdict = judge(&app, &frame_dir, &cli);

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
