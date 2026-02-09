# Architecture

Developer reference for frame's internal design. Each section explains a design decision, why it was made, and what would break without it.

## Module Overview

```
src/
  model/    Data types: Task, Track, Inbox, ProjectConfig, Project
  parse/    Markdown parser + serializer pairs (task, track, inbox)
  io/       Project discovery, file locking, config I/O, UI state, file watcher, project registry
  ops/      Business logic: task CRUD, track management, inbox, search, clean, check, import
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

**Boundary rule**: The task parser stops at blank lines. The track parser handles inter-section blank lines as trailing content on the preceding section or leading content on the next header. Getting this boundary wrong causes blank lines to accumulate or disappear on repeated save cycles.

**Code**: `src/parse/task_parser.rs`, `src/parse/task_serializer.rs`, `src/parse/track_parser.rs`, `src/parse/track_serializer.rs`

## Task ID System

Task IDs use a prefix-per-track mapping defined in `[ids.prefixes]` in `project.toml`. Each track maps to a prefix string (e.g., `eng = "E"`), and IDs auto-increment within that prefix (E1, E2, ...).

- **Subtask IDs**: `PARENT.N` format, up to 3 levels deep (e.g., `E5.2.1`)
- **Cross-track move**: Rewrites the moved task's ID (and all subtask IDs) to the target track's prefix, then scans **all** tracks to update dep references pointing to old IDs
- **Collision detection**: Checked at track creation and ID/prefix rename time to prevent duplicate prefixes

**Why**: Prefixes make task IDs globally unique and immediately identify which track a task belongs to. The rewrite-on-move rule preserves this invariant.

**Code**: `src/ops/task_ops.rs` (ID assignment, cross-track move), `src/ops/track_ops.rs` (prefix management), `src/io/config_io.rs` (config mutations)

## TUI State Model

The TUI has two orthogonal state axes:

**Mode** — what the user is currently doing:
`Navigate` | `Search` | `Edit` | `Move` | `Triage` | `Confirm` | `Select` | `Command`

Only one mode is active at a time. Each mode captures different keys and renders different UI chrome (status row, overlays). Mode transitions are explicit — entering Edit stores the edit target, exiting Edit commits or discards.

**View** — what the user is looking at:
`Track(index)` | `Tracks` | `Inbox` | `Recent` | `Detail { track_id, task_id }`

View determines which renderer draws the main area and which input handler processes keys (in Navigate mode). Views are independent of modes — you can be in Search mode while on any view.

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

**Code**: `src/io/watcher.rs` (FrameWatcher), `src/tui/render/conflict_popup.rs`

## Done Task Lifecycle

Done tasks have a grace period to prevent accidental section moves:

1. **TUI**: When a task is marked done (Space/x), it stays in the Backlog section with a `PendingMove::ToDone` created with a 5-second deadline. The event loop's 250ms poll calls `flush_expired_pending_moves()` to execute moves whose deadline has passed.
2. **Cancel**: Pressing undo during the grace period cancels the pending move and reverts the state change — the task never leaves Backlog.
3. **Immediate flush**: View changes and quit flush all pending moves immediately (no dangling state).
4. **CLI**: `fr state ID done` moves immediately with no grace period (non-interactive, no undo).
5. **Reopen**: Space in Recent view creates a reverse `PendingMove::ToBacklog` with the same 5s grace, allowing cancel by pressing Space again.

**Subtree unity**: `move_task_between_sections()` moves the entire subtask tree together. Only top-level tasks in a section can be moved — subtasks don't move independently.

**Code**: `src/tui/app.rs` (PendingMove, PendingMoveKind, flush_expired_pending_moves), `src/ops/task_ops.rs` (move_task_between_sections, is_top_level_in_section)

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
