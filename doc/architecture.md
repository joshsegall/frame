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

**Conservation rule**: parsing may not consume a non-blank line without recording it somewhere in the model. A line the parser cannot attribute to any task — mis-indented prose, a metadata key without its colon, a fragment left by a merge — is carried verbatim and re-emitted where it was found, on both the verbatim and the canonical path.

**Which task carries it depends on where it sits**, and the two cases want opposite anchors:

- **Between tasks**, at or above the metadata indent: held on the *next* task at that level as `leading_lines`, emitted ahead of it. A line here is only droppable when another task follows it at the same level, so a successor always exists. `fr check` reports it as `stranded_line`.
- **Inside a task**, indented past that task's own metadata: held on the task it sits *under* as `trailing_lines`, emitted after that task's own lines and before its subtasks. `fr check` reports it as `stranded_line_under`.

The split exists because one field for both put the second case on a task at a different nesting level — content inside a subtask ended up carried by that subtask's parent's *sibling*, since that was the nearest level with a successor to hang it on. Nothing recorded where it really sat, so a section move relocated the task and left the line behind, where it was re-read as part of a neighbouring task's note and destroyed by the next ordinary edit of that task. Anchoring to the owner is what makes it travel: a section move, an archive and a cross-track move all carry the task, and the line goes with it at the same relative position, so it parses back the same way. Found by P7 (`tests/conservation.rs`), which is the property that watches operation sequences rather than a single parse.

An over-deep *task* line is not stranded content: it is read at its real indent and recorded at the enclosing depth, so nesting past `MAX_DEPTH` is flattened rather than dropped.

One consequence worth knowing: `trailing_lines` must be emitted after all of a task's metadata, because metadata following stranded content is not collected onto the task. So a note written in **block** form ends at the same indent the stranded run sits at and absorbs it, after which the content is note text and an edit to that note replaces it. That is visible — it renders inside the note of the task the user named — and is the licensed case, as distinct from absorption by a *neighbouring* task's note, which was the defect.

**Why**: a consumed-but-unrecorded line is invisible in a way nothing else can catch. It is absent from the model, so no view shows it and `fr check` cannot see it, and the next write of the file deletes it — and because filling in one task's date rewrites a whole track, the deletion surfaces in a file the user never edited. Parse property P5 (`tests/parse_properties.rs`) states the rule directly: parse, write back untouched, and every non-blank line must return byte for byte. P7 (`tests/conservation.rs`) states it where it actually bit — across a *sequence of operations*, since `fr clean` rewriting a whole track is what turned a parser bug into a line vanishing from a file nobody had edited.

**Boundary rule**: The task parser stops at blank lines. The track parser handles inter-section blank lines as trailing content on the preceding section or leading content on the next header. Getting this boundary wrong causes blank lines to accumulate or disappear on repeated save cycles.

A blank at the *end* of a stranded run is dropped by both anchors. Inside a run it is content — it keeps two stranded paragraphs from being glued together — but a blank between the run and the task below separates the two and belongs to neither. The anchors have to agree about this, because which one claims a given line depends only on that line's indent: when they disagreed, one write emitted the blank and the next re-read the run under the other anchor and dropped it, so a single edit changed the file twice. P6's fixpoint check is what states that.

**One format, one parser.** Every file shape under `frame/` has exactly one parse/serialize pair, and reading a file with the wrong one is a real and recurring bug rather than a hypothetical. A done-task archive (`archive/<track>.md`) is a bare task list under an `# Archive` heading with no `## Section` headers; an archived whole track (`archive/_tracks/<track>.md`) is a track file moved there intact. Reading the first as a track finds no sections, so a repair or a rename "succeeds" having changed nothing; reading the second as a task list stops at its first section header, so a rewrite deletes every section below. Both have happened, in `ops/fix.rs`, `ops/track_ops.rs` and `fr actor merge` respectively. `parse_archive`/`serialize_archive` exist so there is one answer, and it carries the header, the tail below the last task, and the file's line ending — the three things four hand-rolled readers each dropped.

**Code**: `src/parse/task_parser.rs`, `src/parse/task_serializer.rs`, `src/parse/track_parser.rs`, `src/parse/track_serializer.rs`, `src/parse/archive_parser.rs`, `src/parse/archive_serializer.rs`

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

**Persistence**: Per-track cursor/scroll/expanded-set is saved to `.state.json` (debounced, every 5 keystrokes). Filters, selections, the active view search, and ephemeral mode state are not persisted — search *history* is. `UiState` must not gain `#[serde(deny_unknown_fields)]`: state files written by older versions carry keys that have since been dropped.

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

**The watcher is a freshness feature, not a correctness dependency.** It used to be both, and that was a defect (P8's headline, `tests/concurrency.rs`). `save_track_locked` wrote unconditionally, so an external write was only safe if the watcher had already delivered it — and the gap between another `fr` writing and the event loop polling that event is sub-millisecond and entirely ordinary, six of the ~30 mutating key handlers checked the mtime, and `FrameWatcher::start` can fail outright and leave the TUI running without a watcher at all. A save now compares the file against the ancestor *under the lock* and merges when they differ, by the same route a reload does. What the watcher buys is seeing another process's work without pressing a key; what it no longer buys is not losing it.

Absorbing rather than refusing is a decision, not a default: a keystroke can quietly pull in another process's edits, which is what a reload already does with the same machinery. Refusing would leave the file unwritten and route it to `unsaved`, where the retry meets the identical question a moment later with the user waiting on it.

**An absorbed version becomes the ancestor.** Whichever way a merge went, that version has been dealt with, so it is what memory and disk last agreed on — on the reload path *and* the save path, and on the plain replace path where no merge ran at all. Leaving the old ancestor in place makes the next merge meet the same external change a second time, and by then our copy contains it, so the merge reads it as a task both sides edited and files the other version in the recovery log as a conflict nobody was in dispute over.

The ancestor recorded at load is the file's own bytes, not a re-serialization of what was parsed out of them. Those agree for a settled file and differ for one that merely round-trips, since a clean record is emitted from its `source_text` verbatim — and every difference reads as somebody else's write.

Such a reload runs a three-way merge (`ops::reconcile`, shared with the merge driver — see below) against `App::baselines` — the last content known to be on disk for that file, recorded at load and after each successful save, kept as text and parsed only when a merge actually runs. Tasks are matched by ID, so:

- an addition on either side survives;
- a change to a task the other side did not touch is taken;
- subtasks merge independently of their parent;
- an edit beats a delete, in both directions (a delete is trivially repeatable, an edit is not);
- a task both sides changed differently keeps the in-memory version, with the other written to the recovery log.

Not attempted: task ordering (a task follows the side it came from) and subtask reparenting (a subtask's ID extends its parent's, so a move renumbers it and reads as an addition plus a deletion). Without a baseline the merge cannot run, and the fallback is to keep the in-memory version whole and log the incoming one.

**The inbox merges by content, not identity.** Inbox items have no IDs, so there is nothing stable to match on. `reconcile_inbox` treats the two sides as multisets and takes the standard three-way count (`ours + theirs - base`, floored at zero), which expresses exactly what the inbox is used for — captures and removals. An edit then reads as a removal plus a capture, which is correct in both directions, and a *double* edit keeps both versions rather than choosing: a duplicate in a capture list costs one triage keystroke, while a dropped capture is unrecoverable. Because nothing is ever set aside, the inbox merge reports no conflicts and writes nothing to the recovery log. Tracks cannot work this way — duplicating a task duplicates an ID, which `fr check` reports as an error.

The mtime is deliberately not refreshed on this path: `track_changed_on_disk` reads it to decide whether memory and disk have diverged, and after a merge they have.

**`project.toml` merges as an edit applied to the document on disk.** The TUI held a `ProjectConfig` parsed at startup and wrote the whole file back from it, so a track another process added was erased — and because `toml::to_string_pretty` cannot emit a comment, so was every line of documentation in the file, on every track operation, contended or not. A fresh `fr init` project went from 107 lines to 51 the first time a track was shelved.

So `reconcile_config` is the third merge and the only one that does not return a merged value: it applies our `base → ours` delta to **their** `toml_edit` document, in place, and the caller writes that. `project.toml` is a file a person reads, and `ProjectConfig` models neither its comments nor any key it does not know — a merged struct would produce a correct config and destroy the file. The merged struct comes back out by re-parsing what was written, so memory and disk cannot drift.

The delta from the ancestor to memory is exactly the operation the user just performed. That is what lets this live on the save path instead of every config mutation having to edit a document itself, as the CLI's handlers do.

It covers the four regions the TUI can change — `tracks`, `ids.prefixes`, `agent.cc_focus`, `ui.tag_colors` — and nothing else, so `project.name`, `clean`, `recovery` and the rest of `ui` are whatever is on disk. Tracks match on id and merge field by field, so a rename by us and a shelve by them both survive. Two policies differ from the track merge, both for the same reason — a config row has to agree with the *file* it names:

- **We changed a track they removed: theirs wins**, the opposite of edit-beats-delete for a task. Keeping ours would resurrect a row pointing at a file they have already archived or deleted.
- **Both added the same id: ours wins**, because by then `tracks/<id>.md` is ours.

Order is reapplied only when our side actually moved something; rewriting it unconditionally would have a shelve in the TUI silently undo another process's `fr track mv`.

**Adopting a config is not a re-init.** The reload path used to skip `project.toml` entirely as "would need full re-init" — which would throw away the undo stack, every track's view state, and anything sitting in `unsaved`. `App::adopt_config` instead parses in tracks that appeared, drops tracks that went away after flushing any unsaved work for them, and re-resolves `View::Track` by track id rather than by index. Loading someone else's track is a passive load and does not mint.

**An operation that writes a file validates against disk, not against the snapshot.** Creating a track wrote `tracks/<id>.md` unconditionally after checking for a duplicate id in the in-memory config, so a track another process had created was overwritten with an empty template and every task in it destroyed — damage no merge can undo, since the file is gone before the config write happens. `App::track_id_taken_on_disk` is asked under the lock, and the operation refuses. Refusing is right there and wrong on the save path: nothing has been written yet, and the user is at the prompt that asked for the name.

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

A cut `fr track rename --new-id` is the exception that proves it, and the reason `check_track_roster` exists. There the half-applied state *is* detectable — config names a file that is not there, and a file in `tracks/` answers to nobody — and it is worth detecting, because `load_project` skips a track whose file is missing, so the track and every task in it silently leave the project. The marker recovers it automatically; the check catches the same state arriving any other way, which a marker cannot: a merge that took one side's `project.toml` and the other's file layout, a manual `mv`, a partial checkout.

So it writes that down first. `io::inflight` records the operation's **intent** in `frame/.inflight` before the first write; `commit()` removes it after the last. `Drop` removes the file only if committed, so an early `?`, a panic and a kill all converge on the same observable state — which matters, because the gap between "write returned an error" and "process died" is exactly where the old recovery-log mitigation failed.

**Recovery rolls forward, automatically.** `ops::recover` runs on every write command, under the lock the command already takes, and completes the remaining steps: drop the stale source copy, finish the file move, retire the token, remove the triaged inbox item, finish the rename and the config entry that goes with it. Rolling *back* would need undo records amounting to a copy of the prior state, which git already holds; rolling forward finishes an intent the user already expressed. Handing it to a human is the worse option, not the safer one — "delete whichever copy is wrong" invites deleting the right one.

What remains is derived by inspecting current state, not from a step log, so nothing has to be written mid-operation to track progress. Every destructive step is gated on a precondition (the target copy really is there; the task really did land). When one fails — a hand edit, a `git checkout` in between — recovery changes nothing, reports it, and leaves the marker so `fr check` keeps saying so until `fr check --fix --yes` acknowledges it. Every outcome goes to the recovery log, including the ones that did nothing: an automatic decision is only defensible if it leaves a trail.

The marker is a **breadcrumb, not a mutex** — no command refuses to run because one exists. `fr clean` is excluded deliberately: its interrupted state is self-healing, and `auto_clean` runs it on every TUI file reload, so a marker per run would be churn with no signal in it.

**Three locks, in a fixed order, and the two inner ones never reach outward.** Frame holds more than one lock at a time — a write command takes the project lock, then mints under the ID-frontier lock, then records a failure under the recovery-log lock — so the acquisition order has to be stated rather than assumed:

```
frame/.lock                  (project)
  ├── <frontier>.lock        (ops::ids mint)
  └── <recovery log>.lock    (log_recovery)
```

The hierarchy is a tree, not a cycle, and what keeps it one is that **neither leaf acquires anything**. `io::recovery` takes only its own lock, and never loads a project or touches the frontier; `io::ids` never writes to the recovery log. So there is no path that takes an inner lock and then reaches for the project lock, which is the shape a deadlock would need. Two commands hold *only* a leaf: `fr merge` (deliberately no project lock — acquiring one mid-rebase would block on or deadlock against the `fr` that invoked it) and `fr recovery prune`. A single-lock holder cannot be half of a cycle.

Every acquisition is also timeout-bounded — 5s project, 5s frontier, 2s recovery — and **both leaves degrade rather than fail**: a mint that cannot take the frontier lock falls back to scanning, and an append that cannot take the log lock appends anyway and warns. So even a future ordering mistake produces a bounded stall and a degraded write, never a hang. That is deliberate: these are error paths, and a recovery log that blocks the thing it is trying to record is worse than one that races.

`FileLock` is not re-entrant, so no path may take the same lock twice — an inner re-acquire against the same file fails even within one process, since `flock` excludes across file descriptions.

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

**A lock file is never unlinked.** `flock` is on the open file description, not on the path, so removing the file on release leaves any waiter holding a descriptor on an unlinked inode — which it goes on to lock — while the next process finds nothing at the path, creates a fresh file, and locks that. Two writers, one "lock", exactly when there is contention to serialize. The project lock used to unlink; the id-frontier and recovery-log locks never did, on the grounds that only a `rename(2)`-replaced file was exposed. The race needs no rename: a release with a waiter is enough. An empty `frame/.lock` left behind carries no state — its presence never means "locked" — so there is no stale lock to clear.

**A change that is not one file's worth takes one lock for all of it.** Archiving a track writes `project.toml` and moves the track file; deleting one unlinks a file and rewrites the config; renaming a prefix rewrites an archive, the config, the renamed track and every track with a `dep:` into it. `App::with_project_lock` runs the whole change under one lock, and when the lock cannot be had it runs **none** of it. That is the opposite of what a track save does on contention — keep the content in memory, retry with backoff, dump to `.rescue/` at exit — and the difference is the point: an unlinked file cannot wait in memory for a later attempt, and there is no half of "archive this track" worth keeping. `FileLock` is not re-entrant, so `App::lock_held` makes the saves inside such a change write under the lock in hand rather than deadlocking against their own session.

**`project.toml` is a save target like any other.** It takes the lock, records a failure instead of discarding it, retries on the timer, is named in the unsaved indicator, and gets a copy in `.rescue/` at exit. It is the one target that is never *merged* — there is no three-way merge for TOML here — so under the lock it is last writer wins.

**A CLI command reads the project *after* taking the lock, never before.** Waiting for the lock is the ordinary case, not an exotic one — another `fr` holds it for as long as its own write takes, and we block up to five seconds. A project read before that wait is a pre-write copy of files the other process is about to change, so saving it back erases whatever landed: silently, with no recovery entry, and most likely when contention is highest. `handlers::lock_and_load` discovers the root, locks, then loads, and returns the lock alongside the project so no caller can reintroduce the ordering. This is the CLI's answer to the collision the TUI handles with a baseline and `ops::reconcile` — a command that reads once and writes once needs no merge, only the right order.

**Code**: `src/io/config_io.rs` (TOML mutations), `src/io/lock.rs` (FileLock), `src/model/track.rs` (TrackNode::Literal), `src/cli/handlers/mod.rs` (`lock_and_load`)

## Recovery Log

Frame includes a recovery system to prevent silent data loss. An append-only markdown log at `frame/.recovery.log` captures data that Frame couldn't save normally.

**What gets logged:**
- **Parser drops** — unrecognized lines in `inbox.md` that the parser can't parse
- **Write failures** — when `atomic_write()` fails, the intended content is preserved in the log
- **Conflict dismissals** — TUI conflict popup text is saved before being cleared
- **Cross-track move failures** — if the target track write fails after the source was already saved

**Atomic writes**: All file mutations use `NamedTempFile` + rename (`atomic_write()`) to prevent partial writes. The recovery log itself uses `O_APPEND` for appends and `atomic_write()` for the two operations that *shrink* it.

**The log is the one file with no backstop of its own**, so it gets the same discipline as everything it protects. Both places that shrink it — the inline trim and `fr recovery prune` — rewrite by temp-file + rename, because truncating in place puts the whole log at risk of an interruption between the truncate and the write, and this is the file holding content that reached nowhere else. Shrinking it is also the one operation with nothing to fall back on: a failed track write logs its content here, and a failed write *of this file* has nowhere left to go. So a failure leaves the log exactly as it was.

**Its lock is a separate file, never removed, named beside the log** (`.recovery.log` → `.recovery.lock`) — the discipline `FileLock::acquire_at` documents and `io::ids` already relies on. Locking the log itself would let a waiter hold the lock on an inode that a rename has since unlinked, while a newcomer locks the fresh file: two writers, one "lock". The lock is *derived* from the log rather than being a fixed name for the same reason: two processes that resolve different logs must not share one lock. Appends take the lock too, since an append racing a rename lands on the unlinked inode and is gone; an append that cannot get the lock proceeds anyway and warns, because a refused append loses the entry for certain while a raced one only might. Reads take no lock — rename gives them either the old file or the new one, never a torn mix.

**The log is shared by every git worktree of a clone**, at `<git-common-dir>/frame-recovery.log`; outside git it stays at `frame/.recovery.log`. Same mechanism as the ID frontier and the shared actor token, and for a sharper reason than either: a per-worktree log is *ephemeral*. `git worktree remove` deletes gitignored files silently — exit 0, no prompt — so a log inside a worktree is the only copy of something sitting in a directory git will delete on request. Short of deletion it is merely invisible from the worktree next door, which is how a conflict entry written by the main working tree came to be reported as never written at all. Nothing under `.git/` can be committed, so this costs no repo clutter and needs no `.gitignore` entry.

**Every entry carries `Origin:`**, the absolute frame directory it was written from, stamped centrally in `log_recovery_inner` rather than at the 32 construction sites. It goes first, before the entry's own fields, because those fields are relative to a frame directory the entry otherwise never names — `Target: tracks/main.md` identifies nothing once one log serves several working copies. A log left in a working copy by an older frame is read alongside the shared one and absorbed by the next write: shared-first, unlink-second, so an interruption leaves a duplicate rather than a hole.

**Size triggers housekeeping; age decides what goes.** Outgrowing `max_size` (default 5MB) is what makes an append *consider* trimming, and the trim removes only entries older than `prune_age_days` (default 30) — under the lock the append already holds, since `FileLock` is not re-entrant. So a log full of recent entries grows past its limit and loses nothing, which is the right way round: the newest entries are the ones still worth having, and it means the size setting cannot be turned into a data-loss knob by accident. Both are configurable under `[recovery]`, along with the log's location; `fr recovery prune` runs the same rule on demand.

**Paths in the log are absolute.** A recovery entry, and any message about one, is read from somewhere other than where it was written — another worktree, another day, a different working directory. `fr recovery path` prints where the log actually resolved to, and `fr merge` names the log it wrote to rather than assuming the reader can find it. Project-relative paths are for messages about the project you are standing in.

**The `.rescue/` dump** (`App::dump_unsaved`, written at exit for work that never reached disk) is atomic for the same reason: a half-written rescue file is worse than none, because it looks like the thing you lost.

**`fr check` integration**: Reports `#lost` tagged tasks and recovery log summary (entry count + oldest timestamp).

**Code**: `src/io/recovery.rs` (core module), `src/ops/check.rs` (lost task detection)
