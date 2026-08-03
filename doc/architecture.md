# Architecture

Developer reference for frame's internal design. Each section explains a design decision, why it was made, and what would break without it.

## Module Overview

```
src/
  model/    Data types: Task, Track, Inbox, ProjectConfig, Project
  parse/    Markdown parser + serializer pairs (task, track, inbox)
  io/       Project discovery, file locking, config I/O, UI state, file watcher, project registry, ID frontier, in-flight marker
  ops/      Business logic: task CRUD, ID minting, track management, inbox, search, clean, check, fix, recover, import
  cli/      CLI interface (clap commands, handlers, JSON/human output)
  tui/      TUI interface: app state, undo, command palette, input handling, rendering
```

The dependency flow is strictly: `model` ← `parse` ← `io` ← `ops` ← `cli`/`tui`. The `cli` and `tui` modules are siblings that share `ops` but never import each other.

## Selective Rewrite (Parser Design)

The parser/serializer system is designed so that **parse-then-serialize is byte-identical when nothing changes**. This is the most important architectural invariant.

Each parsed `Task` stores:
- `source_lines: Range<usize>` — original line span in the file
- `source_text: Vec<String>` — the task's **own** lines only (task line + metadata), **excluding** subtask lines
- `dirty: bool` — whether the task was modified after parsing

On serialization: clean tasks (`dirty == false`) emit `source_text` verbatim. Dirty tasks regenerate in canonical format. Subtasks are **always** recursed independently — this is what makes selective rewrite work.

**Why**: Editing subtask B never reformats parent A or sibling C. Users' hand-written formatting (extra spaces, custom line breaks in notes) is preserved exactly. Without this, every save would reformat the entire file.

**Conservation rule**: parsing may not consume a non-blank line without recording it somewhere in the model. A line the parser cannot attribute to any task — mis-indented prose, a metadata key without its colon, a fragment left by a merge — is held on the *next* task at that level as `leading_lines` and re-emitted verbatim ahead of it, on both the verbatim and the canonical path. Attaching to the following task rather than the preceding one is deliberate: a line is only droppable when another task follows it at the same level, so a successor always exists, and one field covers every position. `fr check` reports these as `stranded_line`.

**Why**: a consumed-but-unrecorded line is invisible in a way nothing else can catch. It is absent from the model, so no view shows it and `fr check` cannot see it, and the next write of the file deletes it — and because filling in one task's date rewrites a whole track, the deletion surfaces in a file the user never edited. Parse property P5 (`tests/parse_properties.rs`) states the rule directly: parse, write back untouched, and every non-blank line must return byte for byte.

**Boundary rule**: The task parser stops at blank lines. The track parser handles inter-section blank lines as trailing content on the preceding section or leading content on the next header. Getting this boundary wrong causes blank lines to accumulate or disappear on repeated save cycles.

**Code**: `src/parse/task_parser.rs`, `src/parse/task_serializer.rs`, `src/parse/track_parser.rs`, `src/parse/track_serializer.rs`

## Task ID System

Task IDs use a prefix-per-track mapping defined in `[ids.prefixes]` in `project.toml`. Each track maps to a prefix string (e.g., `eng = "E"`), and IDs auto-increment within that prefix (E1, E2, ...).

- **Subtask IDs**: `PARENT.N` format, up to 3 levels deep (e.g., `E5.2.1`)
- **Actor-token namespaces**: Each ID segment carries an optional actor token (`E-a14`; the primary clone's `null` namespace is the bare `E-014`). New numbers are minted by max-scanning **within one namespace**, so two unsynced clones never collide — each scans only its own. `src/model/task_id.rs` owns the grammar and the per-namespace scan/construct primitives.
- **Cross-track move**: Rewrites the moved task's ID (and all subtask IDs) to the target track's prefix, then scans **all** tracks to update dep references pointing to old IDs
- **Reparent (depth/parent change)**: When a task changes parent via `h`/`l` in TUI move mode or `--promote`/`--parent` on CLI, all IDs in the subtree are re-keyed to match the new parent structure (e.g., `E5.2` → `E10` when promoted to top-level). ID re-keying happens on confirm (`Enter`), not during live preview.
- **Re-keying mints in the mover's namespace**: Both re-keying paths above re-mint the new segments in the **mover's** actor-token namespace (scanning the target in that namespace), not the original creator's. *Why the mover, not the creator:* only the mover writes the mover's namespace, so the max-scan can't collide with another clone's concurrent mints — minting into a different clone's namespace from a copy that can't see its other writes is exactly the unsynced-collision the design prevents. Creator provenance is already lost when a cross-track move changes the prefix, so nothing extra is given up. These are explicit actions, so an unclaimed clone auto-claims a token first and aborts cleanly (no partial mutation) if the frontier is empty. The undo/redo paths recover the mover's token from the stored new ID's leaf segment (`TaskId::leaf_token`).
- **Comparison is on the canonical form**: Every ID compare, lookup, and `HashMap`/`HashSet` key uses `TaskId`'s canonical text (its `Display`/`Deref`), so namespaces are distinguished by construction — `E-a14`, `E-14`, and `E-b14` are three different ids. `fr check`'s duplicate detection therefore reports only genuine same-namespace collisions (`E-a14` minted twice) and never treats two namespaces as a dup; a post-merge collision rides the existing duplicate-ID report at its existing severity (no token-specific check path). Dep resolution, jump-to-task, `--after`/`--parent`/`--track` target lookup, prefix rename, and `abbreviated_id` all slice or whole-string-compare, carrying tokens through untouched.
- **Collision detection**: Checked at track creation and ID/prefix rename time to prevent duplicate prefixes

**Why**: Prefixes make task IDs globally unique and immediately identify which track a task belongs to. The rewrite-on-move rule preserves this invariant; the per-namespace mint rule keeps it collision-free across unsynced clones.

**Code**: `src/ops/task_ops.rs` (ID assignment, cross-track move, reparent), `src/ops/track_ops.rs` (prefix management), `src/io/config_io.rs` (config mutations)

## ID Frontier (Durable Mint)

A max-scan alone is not a frontier: it moves **backwards** whenever the live maximum drops. `fr clean` archiving a done task, `fr delete`, or a second git worktree whose working copy hasn't merged the other's new tasks all lower it, and the next mint reissues a number that is already spoken for. The worktree case is the sharp one — worktrees of one clone inherit a single actor token, so they mint in the same namespace from different views.

So a mint takes `max(scan floor, recorded frontier) + 1`:

- **Scan floor** — what this working copy can see: the live track *plus* `archive/<track>.md` and `archive/_tracks/<track>.md`, so archiving a task never frees its number.
- **Recorded frontier** — a durable record of every number handed out, in a file shared by all worktrees of the clone: `<git-common-dir>/frame-ids.toml` (inside git — the same path from every linked worktree, and uncommittable by construction) or `frame/.ids.toml` (outside git, where no worktrees exist to coordinate with). Keyed by `(project, prefix, namespace)`, so it never grows with the number of tasks.

The record is written **before** the task is, so a number is spoken for from the instant it's handed out. Numbers are never reused and gaps are expected, so an abandoned mint costs nothing — no leases, no expiry, no reclaiming.

**Single chokepoint**: `ops::ids::Mint` — every mint path (add, triage, import, cross-track move, promote, `fr clean`'s ID assignment and duplicate resolution) goes through it. `Mint::scan_only` opts out for callers holding a bare `Track` with no project on disk (tests). Two allocators stay outside: child IDs (`PARENT.N`, numbered per parent) and `fr actor merge`, which renumbers a whole namespace in bulk into *another* clone's namespace, whose frontier is a different file.

**Child IDs are detected, not prevented.** Two worktrees of one clone adding a subtask to the same parent still mint the same `PARENT.N`. Covering that would mean a frontier entry *per parent* — a store that grows with the number of tasks, against the bound above, holding entries that are dead as soon as the other worktree's task merges in — or numbering children from the top-level counter, which costs the `.1 .2 .3` reading of a file people hand-edit. Neither is worth it, because the two collisions differ in kind: a reissued **top-level** number has two legitimate holders and no machine answer for which should move (hence `IdReissuedAfterArchive`, reported with no repair), while a **child** number means nothing outside its parent, so renumbering it is mechanical. So: `fr check` reports the collision as a `DuplicateId` error — the duplicate scan has always recursed into subtasks — and `fr clean` resolves it by renumbering the later copy *under its own parent*, which is the allocator split in `ops::clean`. A third warning, `ChildIdNotUnderParent`, catches a subtask whose ID escaped its parent anyway (`M-020` under `M-003`, which is what clean itself produced before the split), repairable by `fr check --fix`.

**Failure handling**: the store is regenerable cache and every degraded path lands on the old scan-only behavior, never a wrong answer or a failed mint. Absent → empty. Unparsable → moved aside to `.bak`, empty. Unwritable, or lock contention past 5s → mint from the floor alone. Writes are temp-file + rename under a **separate, never-removed** lock file (`frame-ids.lock`): deleting the lock would let a waiter hold the lock on an unlinked inode while a newcomer locks a fresh file.

**`fr check` integration**: read-only reports covering what the live-tracks-only duplicate check can't see. Two of them are ID collisions involving an archive, kept separate because they are different problems with different repairs: a **live task holding an archived task's number** (reissued — renumber the live task) versus **one ID appearing twice inside the archives with no live task** (duplicated history from a non-idempotent archive append — delete the extra copies; nothing was reissued). Conflating them produces a warning that names the same file twice and prescribes renumbering a task that doesn't exist. The other two are store health: an **unreadable store**, and a leftover **`frame-ids.toml.bak`** meaning the frontier was reset once, so numbers from that window may have been reissued. None of these mutate the store — a mint resets an unreadable one, but check leaves it in place so the warning names a file still worth inspecting. `fr check --fix` deletes the leftover `.bak` (it is a stale artifact, and the warning exists only to say so) but still leaves an *unreadable store* alone, for the same reason check does. `fr info` shows the recorded frontier per prefix and the store's path.

**Code**: `src/io/ids.rs` (store, locking, recovery, health probes), `src/ops/ids.rs` (`Mint`, floor computation, the child-ID rationale), `src/ops/check.rs` (`check_archived_id_collisions`, `check_id_frontier`), `src/ops/clean.rs` (`next_child_id_under`), `src/ops/fix.rs` (`apply_renumber_subtask`)

## TUI State Model

The TUI has two orthogonal state axes:

**Mode** — what the user is currently doing:
`Navigate` | `Search` | `Edit` | `Move` | `Triage` | `Confirm` | `Select` | `Command`

Only one mode is active at a time. Each mode captures different keys and renders different UI chrome (status row, overlays). Mode transitions are explicit — entering Edit stores the edit target, exiting Edit commits or discards.

**View** — what the user is looking at:
`Track(index)` | `Tracks` | `Inbox` | `Recent` | `Detail { track_id, task_id }` | `Search`

View determines which renderer draws the main area and which input handler processes keys (in Navigate mode). Views are independent of modes — you can be in Search mode while on any view.

**Project Search**: The `Search` view displays project-wide search results grouped by source (active tracks in tab order, inbox, archives). Results are stored in `SearchResults` (items, groups, cursor, scroll) and rendered by `render/search_view.rs`. The search prompt reuses `Mode::Search` with a `project_search_active` flag to distinguish from view search. After jumping to a result with Enter, Esc returns to the search results; pressing Esc again restores the pre-search view.

**FlatItem flattening**: The task tree is flattened into a `Vec<FlatItem>` for rendering. Each `FlatItem::Task` carries depth, tree-line ancestry info (`ancestor_last: Vec<bool>`), expand/collapse state, and an `is_context` flag for filtered ancestor rows. This flat list is the single source of truth for cursor position, scroll offset, and rendering.

**Persistence**: Per-track cursor/scroll/expanded-set is saved to `.state.json` (debounced, every 5 keystrokes). Filters, selections, and ephemeral mode state are not persisted.

**Code**: `src/tui/app.rs` (App struct, Mode, View, FlatItem, build_flat_items), `src/io/state.rs` (.state.json I/O)

## Undo System

Every mutating TUI action pushes an `Operation` onto the undo stack. Each operation stores enough data to fully reverse: old/new values, task/track IDs, position indices.

**Operation variants** (grouped by domain):
- *Tasks*: StateChange, TitleEdit, TaskAdd, SubtaskAdd, TaskMove, FieldEdit, SectionMove, Reopen, CrossTrackMove
- *Inbox*: InboxAdd, InboxDelete, InboxTitleEdit, InboxTagsEdit, InboxMove, InboxTriage
- *Tracks*: TrackAdd, TrackNameEdit, TrackShelve, TrackArchive, TrackDelete, TrackCcFocus, TrackMove

**Navigation on undo**: `UndoNavTarget` specifies where the UI should navigate after undo/redo — switching to the affected track/view and placing the cursor on the affected task. This prevents "undo happened somewhere offscreen" confusion.

**Sync markers**: When an external file change is detected and reloaded, a `SyncMarker` is pushed. Undo cannot cross a sync marker, preventing the user from undoing someone else's (or another tool's) edits.

**Why**: Without sync markers, undoing after an external reload could silently revert changes the user didn't make and can't see.

**Code**: `src/tui/undo.rs` (Operation enum, UndoStack), undo dispatch in `src/tui/input/mod.rs`

## File Watching & Conflict Resolution

`FrameWatcher` uses the `notify` crate to watch `frame/` for `.md` and `.toml` changes, ignoring `.lock` and `.state.json`.

**Self-write detection**: The `App` maintains a `write_gen` counter, incremented on each save. The watcher checks whether a detected change matches the current generation and skips it. Without this, every TUI save would trigger a redundant reload.

**Deferred reload**: If the user is in Edit or Move mode when an external change arrives, the reload is queued in `pending_reload` and applied when the mode exits. This prevents the edit buffer from being yanked away mid-keystroke.

**Conflict popup**: If a deferred reload discovers that the task being edited was deleted or moved externally, a conflict popup displays the orphaned edit text so the user can copy it. The edit is discarded (there's no merge).

**Unsaved files are merged, not replaced**: `App::unsaved` records files whose in-memory content did not reach disk. A reload of a file in that set does *not* overwrite it — both sides then hold content that exists nowhere else, and the usual reason a save failed is another `fr` holding the lock, which is the same process that goes on to write the file. So the collision is likely, not exotic.

Such a reload runs a three-way merge (`ops::reconcile`, shared with the merge driver — see below) against `App::baselines` — the last content known to be on disk for that file, recorded at load and after each successful save, kept as text and parsed only when a merge actually runs. Tasks are matched by ID, so:

- an addition on either side survives;
- a change to a task the other side did not touch is taken;
- subtasks merge independently of their parent;
- an edit beats a delete, in both directions (a delete is trivially repeatable, an edit is not);
- a task both sides changed differently keeps the in-memory version, with the other written to the recovery log.

Not attempted: task ordering (a task follows the side it came from) and subtask reparenting (a subtask's ID extends its parent's, so a move renumbers it and reads as an addition plus a deletion). Without a baseline the merge cannot run, and the fallback is to keep the in-memory version whole and log the incoming one.

**The inbox merges by content, not identity.** Inbox items have no IDs, so there is nothing stable to match on. `reconcile_inbox` treats the two sides as multisets and takes the standard three-way count (`ours + theirs - base`, floored at zero), which expresses exactly what the inbox is used for — captures and removals. An edit then reads as a removal plus a capture, which is correct in both directions, and a *double* edit keeps both versions rather than choosing: a duplicate in a capture list costs one triage keystroke, while a dropped capture is unrecoverable. Because nothing is ever set aside, the inbox merge reports no conflicts and writes nothing to the recovery log. Tracks cannot work this way — duplicating a task duplicates an ID, which `fr check` reports as an error.

The mtime is deliberately not refreshed on this path: `track_changed_on_disk` reads it to decide whether memory and disk have diverged, and after a merge they have.

**Code**: `src/io/watcher.rs` (FrameWatcher), `src/tui/render/conflict_popup.rs`, `src/ops/reconcile.rs`

## Merging Under Version Control

`ops::reconcile` has two callers, not one. The TUI reload above is the first; `fr merge` — registered as a git merge driver — is the second. Both hand it an ancestor and two sides and get back a merged track; only where the three versions come from differs.

**Why a driver is necessary rather than nice.** `fr done` *relocates* a task from `## Backlog` to `## Done`. A line-based merge sees a deletion in one region and an insertion in another and cannot know they are the same task, so it conflicts — and any resolution that keeps both hunks produces two tasks with one ID, one `[ ]` and one `[x]`. The file still looks plausible and `fr show` disagrees with it. Worse, once git has written `<<<<<<<` into the file it is no longer valid frame markdown, so every tool that could diagnose the damage is broken too, and what follows is hand-editing line ranges in a file whose structure is already wrong. That is how a `## Parked` header gets deleted.

**Why matching by ID is sound across branches.** Actor tokens namespace mints per working copy, and the durable frontier (`io::ids`) lives in the git common directory and is shared by every worktree of a clone. Two branches therefore cannot mint the same top-level number for different tasks, so a key present on both sides always denotes the same task. That is what lets the driver reuse `reconcile` unchanged, with no renumbering step. The one uncovered case is child numbers (`BAC-153.2`, see `ops::ids`), which `fr check` reports as a duplicate and `fr clean` renumbers under the parent.

**No conflict markers, ever.** On conflict the driver writes a file that still parses, keeps our version, writes theirs to the recovery log, and exits non-zero so the VCS halts. Because the file carries no markers, staging the path would otherwise commit our side and silently discard theirs — so the conflicted task also gets a `conflict:` metadata line, which `fr check` reports as an error until `fr merge --resolve <ID>` clears it. That marker is the only durable record that a decision is outstanding.

**Git readiness is one surface with one owner.** Three things must be true for a frame project in git: `.gitignore` covers working-copy-local files, `.gitattributes` routes frame markdown to the driver, and `.git/config` registers the driver. `fr git setup` does all three and is idempotent; `fr init` calls it. `fr check --fix` deliberately does none of it and points there instead — it used to add the `.gitignore` pattern and nothing else, which left nobody able to predict which part `--fix` would repair.

The third piece is the awkward one: `.git/config` is per-clone and cannot be committed. A teammate who clones a correctly-configured project gets the attributes but not the driver, and silently falls back to text merges. That is why `fr check` warns about an unregistered driver — it is the only thing that tells a fresh clone to run setup. Both git checks are no-ops outside a repo, or when `git` cannot be run.

**Code**: `src/ops/merge_files.rs`, `src/ops/git_setup.rs`, `src/cli/handlers/merge.rs`, `src/cli/handlers/git.rs`

## Done Task Lifecycle

Done tasks have a grace period to prevent accidental section moves:

1. **TUI**: When a task's state changes, it stays where it is and a `PendingMove { from, to }` is created with a 5-second deadline. `to` comes from `canonical_section` (below), so one shape covers every direction. The event loop's 250ms poll calls `flush_expired_pending_moves()` to execute moves whose deadline has passed.
2. **Cancel**: Pressing undo during the grace period cancels the pending move and reverts the state change — the task never leaves Backlog.
3. **Immediate flush**: View changes and quit flush all pending moves immediately (no dangling state).
4. **CLI**: `fr state ID done` moves immediately with no grace period (non-interactive, no undo).
5. **Reopen**: Space in Recent view schedules a Done → Backlog move with the same 5s grace, allowing cancel by pressing Space again.

**Undo entries**: `PendingMove::push_undo` says whether flushing records its own `Operation::SectionMove`. It is not derivable from `from`/`to` — it depends on what the scheduler already recorded. `Operation::Reopen` restores the task to the Done section at its original index itself, so the Recent-view reopen needs nothing more; `Operation::StateChange` restores state and the resolved date and leaves the task where it sits, so every move scheduled alongside one needs an entry. Getting this wrong is invisible until someone presses undo: un-parking used to skip the entry and undo left a `[~]` task in the Backlog.

**Leaving Done**: the resolved date is put back for the grace period, because the task is still in the Done section and both the Done column and the Recent view sort on it. `execute_pending_move` strips it when the move fires.

**Subtree unity**: `move_task_between_sections()` moves the entire subtask tree together. Only top-level tasks in a section can be moved — subtasks don't move independently.

### The section policy

`task_ops::canonical_section(state) -> SectionKind` is the single statement of which section a top-level task belongs in: Parked → `## Parked`, Done → `## Done`, everything else → `## Backlog`. `reconcile_task_section` pairs it with `top_level_section` and moves the task if the two differ.

**It is a total function from state, not a list of transitions, and that is the point.** Three places used to decide this independently, and the two written as `from → to` enumerations both missed the same cell: neither `cmd_state` nor the TUI listed Done → Parked, so parking a completed task left it in `## Done` wearing `[~]`. Asking "where does this state belong" has no cases to forget.

`fr check` guards the invariant as `task_in_wrong_section`, and `--fix` repairs it with the same move. The warning is **top-level only**: a subtask has no section of its own, and reporting one is both a false positive and — if a repair acted on it — a way to tear a subtask out of its parent.

Note what this means for testing: `tests/parity.rs` could not catch that, because it compares the CLI to the TUI and both were wrong *identically*. Agreement is not correctness. The behaviour is pinned instead by `state_change_moves_a_task_to_the_section_its_state_calls_for` in `tests/cli_integration.rs`, which sweeps every (state × starting section) pair against the known-right answer.

The TUI applies the same policy but defers the move by 5 seconds (above), and adds undo entries and board column pins that the CLI has no need for. *What* section is shared; *when* to move and *what else* to do are the surface's own business.

### Multi-file writes: add before remove

Single-file writes are atomic — `recovery::atomic_write` is temp-file + rename, so a crash leaves either the old file or the new one. The exposure is operations that are only complete after *two or more* files are written, where an interruption in between leaves the project half-updated.

**The rule: whichever write creates must run before the write that destroys.** An interruption then leaves the work duplicated, never missing. That direction matters more than it looks: a duplicate is visible, and often self-healing or repairable, while a task that exists nowhere is indistinguishable from one that never existed — no check can detect it and no repair can recover it. `fr clean` archives before it drains Done, `fr triage` writes the track before it rewrites the inbox, and `fr mv --track` writes the target before the source.

`fr mv --track` used to do the opposite, and lost the task outright when the target write failed. Its recovery-log fallback covered a *failing write* but not a process death, which is the window the ordering exists for. Fixed, and pinned by the crash-injection tests below.

**Ordering prevents loss; it does not make the result visible.** After a cut cross-track move the task is in both tracks under *different* IDs, and after a cut archival the config says archived while the file is still in `tracks/`. Both report `✓ project is valid`, and neither is detectable in principle: two tasks with different IDs in two tracks is a legitimate shape, and so is a config entry whose file has not been read yet. The only thing that knows something went wrong is the process that was doing it.

So it writes that down first. `io::inflight` records the operation's **intent** in `frame/.inflight` before the first write; `commit()` removes it after the last. `Drop` removes the file only if committed, so an early `?`, a panic and a kill all converge on the same observable state — which matters, because the gap between "write returned an error" and "process died" is exactly where the old recovery-log mitigation failed.

**Recovery rolls forward, automatically.** `ops::recover` runs on every write command, under the lock the command already takes, and completes the remaining steps: drop the stale source copy, finish the file move, retire the token, remove the triaged inbox item. Rolling *back* would need undo records amounting to a copy of the prior state, which git already holds; rolling forward finishes an intent the user already expressed. Handing it to a human is the worse option, not the safer one — "delete whichever copy is wrong" invites deleting the right one.

What remains is derived by inspecting current state, not from a step log, so nothing has to be written mid-operation to track progress. Every destructive step is gated on a precondition (the target copy really is there; the task really did land). When one fails — a hand edit, a `git checkout` in between — recovery changes nothing, reports it, and leaves the marker so `fr check` keeps saying so until `fr check --fix --yes` acknowledges it. Every outcome goes to the recovery log, including the ones that did nothing: an automatic decision is only defensible if it leaves a trail.

The marker is a **breadcrumb, not a mutex** — no command refuses to run because one exists. `fr clean` is excluded deliberately: its interrupted state is self-healing, and `auto_clean` runs it on every TUI file reload, so a marker per run would be churn with no signal in it.

**The TUI holds one lock per operation, not per save.** `App::save_track_logged` takes the lock for a single write; `App::save_batch_logged` takes it once for several. Saving a cross-track move's two tracks one at a time would take and release the lock between them, leaving a window another process can write into — the ordering would be correct but not atomic. The fallible `save_track` / `save_inbox` are private, and the inner writes never acquire, so a batch can hold one lock across all of them (`FileLock` is not re-entrant; an inner re-acquire would deadlock). A failed save is written to the recovery log and not surfaced in the UI: mid-flow a transient error toast is noise the user cannot act on, while `fr recovery` and `fr check` surface it where they can.

**Verified, not assumed.** `io::fault` (debug builds only) fails a write selected by path — `FRAME_FAIL_WRITE=tracks/b.md` — so a test can cut one step of a real sequence and inspect what survived. The tests assert on files on disk rather than on the recovery log, because the log only catches a write that returns an error and would be skipped by an abrupt death; disk state is what survives either. Covered: cross-track move (both windows), track archival, `fr actor merge` with the registry write cut, and `fr check --fix` partway through its plan. Each asserts the work survives and that re-running converges.

**Archival is idempotent, and archive-first**: `fr clean` appends the batch to `archive/<track>.md` and *only then* removes those tasks from the track, so a failure between the two writes can never lose a task. The cost of that ordering is that the losing state — archived, but still in Done — is reachable (a crash, or a `git checkout`/`reset` reverting the track file). So the append skips any task whose ID the archive already holds, and drains it from Done regardless: leaving it there would make every future `fr clean` retry the same batch. The live copy that gets dropped *should* be identical to the archived one, but if it was edited after the first write it goes to the recovery log rather than vanishing. Without this, a lost track update meant the next clean appended the batch a second time — which is how a real project ended up with 20 tasks recorded twice, caught later by `fr check`'s duplicate-archive warning.

**Code**: `src/tui/app.rs` (PendingMove, PendingMoveKind, flush_expired_pending_moves), `src/ops/task_ops.rs` (move_task_between_sections, is_top_level_in_section), `src/ops/clean.rs` (archive_done_tasks, archived_task_ids)

## Filtering & Ancestor Context

Track views support state filters and tag filters, applied globally across all tracks.

**State filters**: Active, Todo, Blocked, Parked, Ready. "Ready" has special semantics: the task must be todo or active **and** all its deps must be in done state (resolved). This matches the CLI's `fr ready` command.

**Tag filter**: Matches any task that has the specified tag.

**Ancestor context rows**: When a nested task matches the filter but its parent doesn't, the parent appears as a dimmed, non-selectable "context" row (`FlatItem::Task { is_context: true }`). This preserves the tree structure so users can see where matching tasks live.

**`apply_filter()`** post-processes the flat item list: marks matching tasks, inserts ancestor context rows, and removes non-matching leaves. Cursor movement (`skip_non_selectable()`) skips context rows and separators.

**Code**: `src/tui/app.rs` (FilterState, StateFilter, apply_filter, task_matches_filter, has_unresolved_deps), `src/tui/render/track_view.rs` (dimmed rendering for context rows)

## Dependency Popup & Inverse Index

The dep popup (`D` key) shows a task's dependency graph in both directions: "blocked by" (upstream deps) and "blocking" (downstream dependents).

**Inverse index**: `build_dep_index(project) -> HashMap<String, Vec<String>>` scans every task across all tracks and maps each dep target to the list of tasks that depend on it. This is rebuilt on project reload.

**Tree walk**: The popup recursively expands deps (and inverse deps), tracking visited task IDs to detect cycles. Circular references are marked with `↻` and not expanded further. Dangling refs (deps pointing to non-existent tasks) show `[?]` in red.

**Navigation**: Enter on a task in the popup jumps to that task (cross-track if needed), closing the popup. This makes the dep popup a navigation tool, not just a display.

**Code**: `src/tui/app.rs` (DepPopupState, DepPopupEntry, build_dep_index), `src/tui/render/dep_popup.rs`

## Multi-Select & Bulk Operations

Selection is a `HashSet<String>` of task IDs on the App. Entering Select mode (`v`) toggles individual tasks; `V` starts a range selection from an anchor point.

**Stand-in row**: During bulk move, selected tasks are collapsed into a single `FlatItem::BulkMoveStandin { count }` row ("━ N tasks ━") that the user positions with j/k. On confirm, all selected tasks are inserted at that position.

**Bulk editing**: Bulk tag and dep edits use additive/subtractive syntax: `+tag -tag` for tags, `+ID -ID` for deps. This is parsed at confirm time and applied to each selected task individually.

**Selection persistence**: The selection set persists across individual operations until explicitly cleared (Esc in Select mode, or switching views). This allows chaining: select tasks, bulk move, then bulk tag.

**Code**: `src/tui/app.rs` (selection: HashSet, range_anchor), `src/tui/input/mod.rs` (select mode handlers)

## Project Registry

Frame maintains a global project registry at `~/.config/frame/projects.toml` (or `$XDG_CONFIG_HOME/frame/projects.toml`). Each entry records a project name, absolute path, and separate `last_accessed_tui`/`last_accessed_cli` timestamps.

**Auto-registration**: Projects are registered automatically on `fr init`, when the CLI loads a project, and when the TUI launches. The corresponding timestamp is touched on each access, keeping the "most recently used" ordering current.

**Path-based internal API**: All registry functions (`read_registry_from`, `write_registry_to`, `register_project_in`, `remove_project_from`) take an explicit file path rather than computing it from env vars internally. Convenience wrappers (`read_registry()`, `register_project()`, etc.) call `registry_path()` and delegate. This allows unit tests to use temp file paths directly, avoiding `set_var` race conditions in parallel test execution (which is unsafe in Rust 2024 edition).

**TUI project switching**: The project picker replaces the entire `App` state (`*app = App::new(project)`) rather than selectively updating fields. This ensures all derived state (flat items, filter state, undo stack) is cleanly reset.

**Code**: `src/io/registry.rs`

## Track Name & Config Sync

Track names exist in two places that must stay synchronized:
1. `project.toml` — the `[[tracks]]` table has a `name` field
2. The track's `.md` file — the `# Title` line in the first `TrackNode::Literal`

All mutations (rename, create, delete) must update both. Config edits use `toml_edit::DocumentMut` to preserve comments, formatting, and key ordering in `project.toml`.

**File locking**: Unix `flock()` on `frame/.lock` prevents concurrent CLI and TUI writes to the same project. The lock is acquired before any mutation and released on drop. The TUI holds the lock for the duration of each save operation (not the entire session).

**Code**: `src/io/config_io.rs` (TOML mutations), `src/io/lock.rs` (FileLock), `src/model/track.rs` (TrackNode::Literal)

## Recovery Log

Frame includes a recovery system to prevent silent data loss. An append-only markdown log at `frame/.recovery.log` captures data that Frame couldn't save normally.

**What gets logged:**
- **Parser drops** — unrecognized lines in `inbox.md` that the parser can't parse
- **Write failures** — when `atomic_write()` fails, the intended content is preserved in the log
- **Conflict dismissals** — TUI conflict popup text is saved before being cleared
- **Cross-track move failures** — if the target track write fails after the source was already saved

**Atomic writes**: All file mutations use `NamedTempFile` + rename (`atomic_write()`) to prevent partial writes. The recovery log itself uses `O_APPEND` for concurrent-safe appends.

**Size management**: When the log exceeds 1MB, a non-blocking inline trim removes entries older than 30 days. Users can also run `fr recovery prune` manually.

**`fr check` integration**: Reports `#lost` tagged tasks and recovery log summary (entry count + oldest timestamp).

**Code**: `src/io/recovery.rs` (core module), `src/ops/check.rs` (lost task detection)
