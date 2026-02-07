# Frame — Implementation Plan

This document covers the implementation architecture, project structure,
and phased task breakdown for Frame. The design specification
(`frame-design-v3_3.md`) is the authoritative reference for all behavior,
format, and interaction details.

---

## Implementation Architecture

### Language & Stack

**Rust.** Single binary, crossterm + ratatui for TUI, clap for CLI, toml
crate for config, regex crate for search.

Key dependencies:
- `ratatui` + `crossterm` — TUI rendering and terminal backend
- `clap` (derive) — CLI argument parsing
- `toml` / `toml_edit` — config read/write (toml_edit for round-tripping)
- `serde` + `serde_json` — JSON output, state persistence
- `regex` — search
- `notify` — file watching (for detecting external changes)
- `chrono` — dates

### Markdown Round-Tripping Strategy

This is the hardest technical problem in the project. The approach:

**Line-span preservation with selective rewrite.**

The parser reads the file and builds a task tree where every node retains
its original byte range (start line, end line) in the source file. When a
mutation occurs (e.g., change state from `[ ]` to `[x]`), only the
affected lines are rewritten. Everything else is emitted verbatim from the
original source.

Concretely:

1. **Parse phase**: Walk lines top-to-bottom. Recognize task lines by the
   `- [` prefix at the appropriate indent level. Build a tree of `Task`
   nodes, each carrying:
   - Parsed fields (state, id, title, tags, metadata, subtasks)
   - `source_range: Range<usize>` — line range in the original file
   - `dirty: bool` — whether this node has been modified

2. **Serialize phase**: Walk the tree. For each node:
   - If `dirty == false`: emit original source lines verbatim
   - If `dirty == true`: regenerate lines from parsed fields in canonical
     format
   - Non-task content (the `# Title`, `> description`, `## Backlog`
     headers, blank lines between sections) is stored as literal text
     nodes in the tree and always emitted verbatim.

3. **Canonical format**: When a dirty node is serialized, it uses the
   spec's format: 2-space indent per level, `- ` prefix for metadata,
   4-space indent for block content. This means the first time Frame
   touches a hand-edited task, it normalizes that task's formatting but
   leaves untouched tasks alone.

4. **Full rewrite on `fr clean`**: The clean command serializes everything
   in canonical format (marks all nodes dirty), which normalizes the
   entire file.

**What about code blocks in notes?** The parser tracks fenced code block
state (``` open/close) so it doesn't misinterpret lines inside code blocks
as tasks. The raw lines within a note block (including code fences) are
preserved as-is when the note hasn't been edited.

**Test strategy**: A corpus of test files with various edge cases. Parse →
serialize → diff against original. Any diff on unmodified nodes is a bug.

### Concurrency & File Coordination

**Architecture**: The TUI and CLI share no runtime — both are just
processes that read/write the same markdown files. Coordination is at the
filesystem level.

**Approach: File-level advisory locking + mtime-based reload.**

1. **Lock file**: `frame/.lock` — acquired (via `flock` / `fcntl`) before
   any write operation. Both TUI and CLI acquire the lock, perform the
   read-modify-write cycle, then release. This serializes all writes.

2. **TUI file watching**: The TUI uses `notify` to watch the `frame/`
   directory. When a file changes externally (CLI write, git checkout,
   manual edit), the TUI:
   - Re-reads the affected file(s)
   - Diffs the new task tree against its in-memory tree
   - Updates its internal state, preserving cursor position where possible
   - Pushes the external change onto the undo stack as a "sync" marker
     (see undo design below)

3. **No operation log needed**: The TUI doesn't need to know *what*
   changed semantically — it just diffs the before/after task trees. This
   is simpler and more robust than maintaining a separate change log.

4. **Conflict edge case**: If the TUI has unsaved EDIT mode changes in a
   text field and an external write arrives, the TUI should:
   - Queue the reload until the user exits EDIT mode
   - Then apply the reload, which usually merges cleanly (user edited
     task A, agent modified task B)
   - If the same task was modified externally, the user's in-progress
     edit text is shown in a **conflict popup** — a small overlay
     displaying the orphaned text with the option to copy all or
     select portions to the clipboard. The user can then re-enter
     EDIT mode and paste their work back in on top of the externally
     modified version. No user input is ever silently discarded.

### Undo Model

**Two-level undo:**

- **NAVIGATE mode**: Undo at the operation level. One `fr`-equivalent
  operation = one undo step. Examples: "mark EFF-014 done" is one step.
  "Move EFF-015 from position 2 to position 5" is one step. "Add task"
  is one step.

- **EDIT mode**: Standard text undo — character/word granularity, as
  expected in a text editor. The entire edit session (from entering EDIT
  to exiting) is also a single operation-level undo step in NAVIGATE mode.
  So: enter EDIT, type 50 characters, exit → in NAVIGATE mode, one `u`
  undoes the entire edit. Inside EDIT mode, Cmd+Z undoes character by
  character.

**Implementation**: An undo/redo stack of `Operation` values, where each
operation stores enough state to reverse it (old task state, old position,
old text content, etc.). Inverse operations are computed eagerly at
mutation time, not by diffing files. Redo is the standard complement:
undoing pushes the operation onto the redo stack, and any new mutation
clears the redo stack.

**Sync markers**: When an external file change is detected and reloaded,
a sync marker is pushed onto the undo stack. Undo cannot cross sync
markers — if you press `u` and the top of the stack is a sync marker,
nothing happens (or it beeps). This prevents the TUI from trying to undo
an agent's changes. Sync markers also clear the redo stack.

**Session-only**: The undo stack is not persisted. On TUI restart, the
stack is empty. Git serves as cross-session undo.

---

## Project Structure

```
frame/
├── Cargo.toml
├── src/
│   ├── main.rs                 # Entry point, dispatches CLI vs TUI
│   ├── lib.rs                  # Re-exports for testing
│   │
│   ├── model/                  # Core data types
│   │   ├── mod.rs
│   │   ├── task.rs             # Task, TaskState, Metadata, Tag
│   │   ├── track.rs            # Track, TrackState
│   │   ├── inbox.rs            # InboxItem
│   │   ├── project.rs          # Project (loaded state)
│   │   └── config.rs           # ProjectConfig (TOML mapping)
│   │
│   ├── parse/                  # Markdown ↔ data model
│   │   ├── mod.rs
│   │   ├── task_parser.rs      # Line-by-line task parser
│   │   ├── task_serializer.rs  # Task tree → markdown
│   │   ├── track_parser.rs     # Full track file (headers + sections)
│   │   ├── track_serializer.rs
│   │   ├── inbox_parser.rs     # Blank-line-separated inbox format
│   │   ├── inbox_serializer.rs
│   │   └── span.rs             # Source span tracking
│   │
│   ├── ops/                    # Business logic (shared by CLI + TUI)
│   │   ├── mod.rs
│   │   ├── task_ops.rs         # State changes, add, move, tag, dep, etc.
│   │   ├── track_ops.rs        # Track management
│   │   ├── inbox_ops.rs        # Inbox add, triage
│   │   ├── search.rs           # Regex search across tracks
│   │   ├── clean.rs            # fr clean logic
│   │   ├── check.rs            # Validation
│   │   ├── import.rs           # fr import
│   │   └── active_gen.rs       # ACTIVE.md generation
│   │
│   ├── io/                     # File system interaction
│   │   ├── mod.rs
│   │   ├── project_io.rs       # Discover project, load all files
│   │   ├── lock.rs             # Advisory file locking
│   │   ├── watcher.rs          # File change notification (notify)
│   │   └── state.rs            # .state.json read/write
│   │
│   ├── cli/                    # CLI interface
│   │   ├── mod.rs
│   │   ├── commands.rs         # Clap command definitions
│   │   ├── output.rs           # Human-readable + JSON formatting
│   │   └── handlers/           # One handler per command group
│   │       ├── list.rs
│   │       ├── show.rs
│   │       ├── ready.rs
│   │       ├── add.rs
│   │       ├── state.rs
│   │       ├── mv.rs
│   │       ├── tag.rs
│   │       ├── track.rs
│   │       ├── inbox.rs
│   │       ├── import.rs
│   │       └── ...
│   │
│   └── tui/                    # Terminal UI
│       ├── mod.rs
│       ├── app.rs              # App state, event loop, mode management
│       ├── event.rs            # Event handling (key → action dispatch)
│       ├── undo.rs             # Undo stack
│       ├── render/             # ratatui rendering
│       │   ├── mod.rs
│       │   ├── tab_bar.rs
│       │   ├── track_view.rs
│       │   ├── detail_view.rs
│       │   ├── inbox_view.rs
│       │   ├── recent_view.rs
│       │   ├── tracks_view.rs
│       │   ├── status_row.rs
│       │   ├── help_overlay.rs
│       │   └── autocomplete.rs
│       ├── input/              # Mode-specific input handling
│       │   ├── mod.rs
│       │   ├── navigate.rs
│       │   ├── edit.rs
│       │   ├── move_mode.rs
│       │   ├── search.rs
│       │   └── triage.rs
│       └── text_editor.rs      # Shared text editing logic
│
└── tests/
    ├── parse/
    │   ├── round_trip.rs       # Parse → serialize → diff
    │   ├── task_parse.rs       # Unit tests for task parsing
    │   ├── track_parse.rs
    │   ├── inbox_parse.rs
    │   └── edge_cases.rs       # Code blocks, deep nesting, etc.
    ├── ops/
    │   ├── state_transitions.rs
    │   ├── move_ops.rs
    │   ├── id_assignment.rs
    │   └── clean.rs
    ├── cli/
    │   └── integration.rs      # Run fr commands against test projects
    └── fixtures/
        ├── simple_track.md
        ├── complex_track.md
        ├── inbox.md
        ├── project.toml
        └── ...
```

---

## Implementation Phases — Concrete Task Breakdown

Each phase builds on the previous. CC should complete all tasks in a phase
before moving to the next. Tasks within a phase can be done in the listed
order.

### Phase 1: Data Model, Parsing, and File I/O

**Goal**: Read and write Frame project files with perfect round-tripping.

```
1.1  Define core data types in model/
     - TaskState enum (Todo, Active, Blocked, Done, Parked)
     - Task struct (state, id, title, tags, metadata map, subtasks vec,
       source_range, dirty flag)
     - Metadata enum (Dep, Ref, Spec, Note, Added, Resolved)
     - Track struct (name, description, backlog, parked, done, source text)
     - TrackConfig (id, name, state, file path, prefix)
     - InboxItem struct (title, body, tags)
     - ProjectConfig struct (mirrors project.toml)
     - Project struct (config, tracks, inbox)

1.2  Implement task line parser (parse/task_parser.rs)
     - Parse a single task line: checkbox, state, optional ID, title, tags
     - Parse metadata lines (key: value and key: block forms)
     - Parse subtasks recursively (indent-based)
     - Track source line ranges on every node
     - Handle code fences inside note blocks (don't misparse as tasks)
     - Enforce 3-level nesting limit

1.3  Implement track file parser (parse/track_parser.rs)
     - Parse the full track file structure: # Title, > description,
       ## Backlog, ## Parked, ## Done sections
     - Preserve non-task lines as literal text nodes
     - Delegate task parsing to task_parser
     - Return Track with all sections populated

1.4  Implement task serializer (parse/task_serializer.rs)
     - Serialize a task tree back to markdown
     - Respect dirty flag: verbatim for clean nodes, canonical for dirty
     - Canonical format: 2-space indent, comma-separated deps/refs
     - Handle note blocks with code fences

1.5  Implement track file serializer (parse/track_serializer.rs)
     - Reassemble full track file from Track struct
     - Literal text nodes emitted verbatim
     - Task sections use task_serializer

1.6  Implement inbox parser and serializer (parse/inbox_*.rs)
     - Parse blank-line-separated items
     - Each item: title line (with optional tags), body lines
     - Round-trip: preserve body text formatting

1.7  Implement project discovery and loading (io/project_io.rs)
     - Walk up from CWD looking for frame/ directory
     - Read project.toml via toml crate
     - Load all track files, inbox
     - Return fully populated Project

1.8  Implement TOML config read/write (model/config.rs + io/)
     - Use toml_edit for round-trip-safe config editing
     - Read: deserialize to ProjectConfig
     - Write: update specific fields without reformatting

1.9  Write round-trip test suite (tests/parse/)
     - Corpus of test fixture files
     - For each: parse → serialize → assert identical to input
     - Edge cases: code blocks in notes, deep nesting, multiple deps,
       tasks with no metadata, tasks with all metadata, empty sections

1.10 Implement advisory file locking (io/lock.rs)
     - Acquire/release flock on frame/.lock
     - Timeout with error message if lock held too long
     - Used by every write path (CLI command, TUI mutation)
```

### Phase 2: Core Operations Layer

**Goal**: All business logic that both CLI and TUI share.

```
2.1  Task state transitions (ops/task_ops.rs)
     - cycle_state(task): todo→active→done→todo
     - set_blocked(task): any→blocked, blocked→todo
     - set_parked(task): any→parked, parked→todo
     - set_done(task): any→done (adds resolved date)
     - set_state(task, state): direct set (for CLI)
     - All transitions mark task dirty and handle resolved/added dates

2.2  Task CRUD operations (ops/task_ops.rs)
     - add_task(track, title, position): append/prepend/after
     - add_subtask(parent_id, title)
     - edit_title(task_id, new_title)
     - delete → mark done + tag #wontdo (no hard delete)
     - Auto-assign ID on creation
     - Auto-set added date

2.3  Task metadata operations (ops/task_ops.rs)
     - add_tag, remove_tag
     - add_dep, remove_dep (validates target ID exists)
     - set_note (replace note content)
     - add_ref, set_spec

2.4  Move operations (ops/task_ops.rs)
     - move_task(id, new_position) — within same track
     - move_task_to_track(id, target_track, position) — cross-track,
       reassigns ID, updates deps across all tracks
     - Renumber subtask IDs on reparenting

2.5  Track operations (ops/track_ops.rs)
     - new_track(id, name): create file, add to config
     - shelve, activate, archive track
     - reorder active tracks
     - set cc-focus

2.6  Inbox operations (ops/inbox_ops.rs)
     - add_inbox_item(title, tags, body)
     - triage(index, track, position): remove from inbox, add to track
       with auto-assigned ID and carried-over tags/body→note

2.7  Search (ops/search.rs)
     - Regex search across task titles, notes, tags
     - Scoped to track or all tracks
     - Returns list of (track, task, match spans)

2.8  fr clean (ops/clean.rs)
     - Assign IDs to tasks missing them
     - Assign added dates where missing
     - Archive done tasks past threshold (250 lines) to per-track archive
     - Validate deps (flag dangling)
     - Validate file refs (flag broken paths)
     - State suggestions (all subtasks done → suggest parent done)
     - Generate ACTIVE.md

2.9  fr check (ops/check.rs)
     - Validate all deps resolve to existing task IDs
     - Validate all spec/ref paths exist on disk
     - Report format issues
     - Return structured results (for --json)

2.10 Import (ops/import.rs)
     - Parse a markdown file as a list of tasks
     - Insert into target track at specified position
     - Auto-assign IDs, auto-set dates
```

### Phase 3: CLI

**Goal**: Complete `fr` command-line interface.

```
3.1  Set up clap command structure (cli/commands.rs)
     - Top-level subcommands: list, show, ready, blocked, search, inbox,
       tracks, stats, check, clean, add, push, sub, state, tag, dep,
       note, ref, spec, mv, track, import, title, recent, triage
     - Global flags: --json
     - Wire up main.rs dispatch

3.2  Implement read commands
     - fr list [track] [--state X] [--tag X] [--all] [--json]
     - fr show <id> [--json]
     - fr ready [--cc] [--track X] [--tag X] [--json]
     - fr blocked [--json]
     - fr search <pattern> [--track X]
     - fr inbox [--json]
     - fr tracks [--json]
     - fr stats [--json]
     - fr recent [--limit N]
     - fr deps <id>
     - fr check

3.3  Implement write commands
     - fr add <track> "title" [--after <id>] [--found-from <id>]
     - fr push <track> "title"
     - fr sub <id> "title"
     - fr inbox "text" [--tag X] [--note "body"]
     - fr state <id> <state>
     - fr tag <id> add|rm <tag>
     - fr dep <id> add|rm <dep-id>
     - fr note <id> "text"
     - fr ref <id> <path>
     - fr spec <id> <path>#<section>
     - fr title <id> "new title"
     - fr mv <id> <pos>|--top|--after <id>|--track <track>
     - fr triage <index> --track <track> [--top|--bottom|--after <id>]

3.4  Implement track management commands
     - fr track new <id> "name"
     - fr track shelve|activate|archive <id>
     - fr track mv <id> <position>
     - fr track cc-focus <id>

3.5  Implement maintenance commands
     - fr clean [--dry-run]
     - fr import <file> --track <track> [--top] [--after <id>]

3.6  Human-readable and JSON output formatting (cli/output.rs)
     - Consistent formatting for all read commands
     - --json produces machine-readable output matching spec examples

3.7  Integration tests (tests/cli/)
     - Set up temp project directories with fixtures
     - Run fr commands as subprocess
     - Assert file contents and stdout
```

### Phase 4: TUI — Core Views and Navigation

**Goal**: Read-only TUI that displays all views with navigation.

```
4.1  Terminal setup and app skeleton (tui/app.rs)
     - Initialize crossterm raw mode, alternate screen
     - Set up ratatui terminal with crossterm backend
     - Event loop: poll for key events, render on change
     - App state: current view, mode, loaded project
     - Graceful shutdown on Ctrl+Q / Cmd+Q

4.2  Color palette and theming
     - Define color constants from spec as defaults
     - Read overrides from [ui.colors] and [ui.tag_colors] in project.toml
     - Fall back to global config, then to hardcoded defaults
     - Not a full theme engine — just the colors listed in the spec's TOML

4.3  Tab bar rendering (tui/render/tab_bar.rs)
     - Active track tabs with names
     - Tracks view tab (▸)
     - Inbox tab with count (🔥N)
     - Recent tab (✓)
     - Highlight current tab
     - Separator line below

4.4  Track view rendering (tui/render/track_view.rs)
     - Render task tree with indentation and tree lines (├ └)
     - State symbols (○ ◐ ⊘ ✓ ◇) with correct colors
     - Collapse/expand indicators (▸ ▾)
     - Abbreviated subtask IDs (.1, .2, .2.1)
     - Tags rendered in foreground color
     - Cursor highlight (current line in bright white)
     - Parked section with separator
     - Default collapse: all collapsed except first task expanded 1 level
     - Scrolling for long lists

4.5  Cursor navigation (tui/input/navigate.rs)
     - ↑↓ / jk: move cursor through visible items
     - ←/h: collapse current node (or move to parent)
     - →/l: expand current node (or move to first child)
     - Cmd+↑/g: jump to top
     - Cmd+↓/G: jump to bottom
     - Track expand/collapse state in app state

4.6  Tab switching
     - 1-9: switch to active track N
     - Tab/Shift+Tab: next/prev track
     - i: inbox view
     - r: recent view
     - 0/`: tracks view

4.7  Tracks view rendering (tui/render/tracks_view.rs)
     - Full-screen list of all tracks
     - Grouped by state (Active, Shelved, Archived)
     - Stats per track (count of each state)
     - cc-focus indicator (★cc)

4.8  Status row rendering (tui/render/status_row.rs)
     - Empty in NAVIGATE mode
     - Mode indicator in highlight color for other modes
     - Right-aligned context hints

4.9  Help overlay (tui/render/help_overlay.rs)
     - Toggle with ?
     - Semi-transparent overlay showing key bindings
     - Context-sensitive (different content per view)

4.10 UI state persistence (io/state.rs)
     - Read .state.json on startup
     - Write on every state change (debounced, ~200ms)
     - Persist: view, active_track, cursor per track, expanded nodes,
       scroll offset, last search

4.11 SEARCH mode (tui/input/search.rs)
     - / enters search mode
     - Prompt in status row with cursor
     - Real-time highlighting of matches in content area
     - Enter: execute, jump to first match
     - Esc: cancel
     - n/N: next/prev match (in NAVIGATE mode after search)
     - Scope follows current view
```

### Phase 5: TUI — Task Actions and MOVE Mode

**Goal**: Full mutating interactions in track view.

```
5.1  State changes
     - Space: cycle state (todo→active→done→todo)
     - x: mark done
     - b: toggle blocked
     - ~: toggle parked
     - Visual feedback: item updates in-place immediately
     - Write to file (via ops layer + file lock)

5.2  Add task
     - a: append to bottom of backlog, enter EDIT mode for title
     - o: insert after current task, enter EDIT mode
     - p: push to top of backlog, enter EDIT mode
     - A: add subtask to selected task, enter EDIT mode

5.3  MOVE mode (tui/input/move_mode.rs)
     - m: enter MOVE mode on selected task
     - ↑↓: physically reorder in list (real-time reflow)
     - Cmd+↑/g: move to top
     - Cmd+↓/G: move to bottom
     - Enter: confirm
     - Esc: cancel (restore original position)
     - Also works in tracks view for reordering tracks
     - Status row shows "-- MOVE --" with hints

5.4  Inline title editing
     - e: enters EDIT mode on selected task's title
     - Single-line text editor in-place
     - Enter confirms, Esc cancels
     - Status row shows "-- EDIT --"

5.5  Undo/redo system (tui/undo.rs)
     - Operation stack with inverse operations
     - Redo stack (cleared on new mutation, cleared on sync marker)
     - u / Ctrl+Z in NAVIGATE mode: undo last operation
     - Ctrl+Y / Ctrl+Shift+Z in NAVIGATE mode: redo
     - Sync markers for external file changes
     - Text-level undo/redo within EDIT mode (separate from operation stack)

5.6  File watcher integration (io/watcher.rs)
     - Watch frame/ directory for changes
     - On external change: reload affected files, diff, update state
     - Queue reload if in EDIT mode
     - Push sync marker to undo stack

5.7  Conflict popup (tui/render/conflict_popup.rs)
     - Shown when an external change conflicts with in-progress edit text
     - Displays the orphaned edit text in a scrollable overlay
     - User can select/copy text to clipboard, then dismiss
     - Never silently discards user input
```

### Phase 6: TUI — Detail View and Full Editing

**Goal**: Rich task detail view with per-region editing.

```
6.1  Detail view rendering (tui/render/detail_view.rs)
     - Structured document layout per spec
     - Regions: title, tags, added, deps, spec, refs, note, subtasks
     - Dep display with inline state symbols
     - Code block rendering in notes
     - Tags in foreground color

6.2  Region-based navigation
     - ↑↓: move between regions
     - Tab/Shift+Tab: jump to next/prev editable region
     - Region highlighting (subtle indicator of current region)
     - Esc: back to track view

6.3  Text editor component (tui/text_editor.rs)
     - Shared component used by all EDIT mode interactions
     - Single-line mode: ←→, Opt+←→, Cmd+←→, backspace, Opt+bksp,
       clipboard, Enter confirms, Esc cancels
     - Multi-line mode: same plus Enter=newline, Tab=4 spaces,
       Esc finishes (saves)
     - Text selection:
       - Shift+←/→: extend selection by character
       - Shift+Opt+←/→: extend selection by word
       - Shift+Cmd+←/→: extend selection to start/end of line
       - Shift+↑/↓: extend selection by line (multiline only)
       - Cmd+A: select all within current field
       - Any non-shift movement collapses selection to cursor
       - Typing with active selection replaces selected text
       - Cmd+C copies selection, Cmd+X cuts, Cmd+V replaces selection
       - Selected text rendered with inverted/highlight background
     - Character-level undo within edit session

6.4  EDIT mode in detail view (tui/input/edit.rs)
     - e/Enter on any region: enter EDIT mode
     - Title: single-line editor
     - Tags: single-line with tag autocomplete
     - Deps: single-line with task ID autocomplete
     - Spec/Ref: single-line with file path autocomplete
     - Note: multi-line editor, expandable area
     - #/@/d/n shortcuts: jump to region + enter EDIT

6.5  Autocomplete component (tui/render/autocomplete.rs)
     - Floating dropdown below the edit cursor
     - ↑↓ to navigate, Enter to select, Esc to dismiss
     - Typing filters entries
     - Tag autocomplete: known tags from config + existing tags in project
     - Task ID autocomplete: all task IDs across tracks
     - File path autocomplete: walk project directory
```

### Phase 7: TUI — Inbox, Recent, Polish

**Goal**: Complete all views and polish for daily use.

```
7.1  Inbox view rendering (tui/render/inbox_view.rs)
     - Sequential numbering
     - Title + tags + body text (dimmed)
     - Cursor navigation

7.2  Inbox interactions (tui/input/triage.rs)
     - a: add new item (inline multi-line editor)
     - e: edit selected item
     - #: edit tags
     - x: delete with confirmation prompt
     - m: MOVE mode to reorder
     - Enter: begin triage flow

7.3  Triage flow
     - Step 1: track selection autocomplete
     - Step 2: position selection (t/b/a)
     - Creates task in target track, removes from inbox
     - Cursor advances to next item

7.4  Recent view rendering (tui/render/recent_view.rs)
     - Reverse-chronological list of done tasks
     - Grouped by date (resolved date)
     - Show track origin
     - Space or Enter: reopen (set state back to todo)

7.5  File watcher for auto-clean
     - Detect external file modifications (mtime change)
     - Run clean logic on reload
     - Subtle indicator when files were normalized

7.6  Responsive layout
     - Handle terminal resize events
     - Graceful degradation at narrow widths
     - Truncate long titles with ellipsis
     - Scroll for deep nesting

7.7  Edge cases and polish
     - Empty states (no tasks, no tracks, empty inbox)
     - Error display (file read errors, lock contention)
     - Very long notes (scrollable in detail view)
     - Unicode handling in titles and notes
     - Ctrl+C doesn't crash (catch signal, clean shutdown)
```

### Phase 8: Agent SKILL File

**Goal**: A SKILL.md file that CC (or any coding agent) can read to
understand how to use `fr` effectively.

```
8.1  Write SKILL.md for fr CLI usage
     - Overview of Frame concepts (tracks, tasks, states, tags)
     - Common workflows: pick up work, report progress, file findings
     - Command reference with examples
     - Conventions: when to use #cc-added, how to structure subtasks,
       when to use fr inbox vs fr add
     - Example session: agent picks up task, creates subtasks,
       marks progress, files discovered issues
```
---

## Testing Strategy

- **Unit tests**: Parser, serializer, state transitions, ID assignment,
  each in isolation with small inputs.
- **Round-trip tests**: Parse → serialize → diff for a corpus of fixture
  files. This is the most important test suite.
- **Integration tests**: CLI commands against temp project directories.
  Assert file contents and command output.
- **TUI tests**: Not automated initially. Manual testing against the
  fixture project. Consider snapshot testing (render to string buffer)
  for regression once views stabilize.

---

## Build & Run

```bash
# Development
cargo build
cargo test
cargo run -- list          # CLI mode
cargo run -- tui           # TUI mode (or just `fr` with no subcommand?)

# Install
cargo install --path .
fr list                    # CLI
fr                         # TUI (no subcommand = launch TUI)
```

**Binary name**: `fr`. No subcommand → launch TUI. Any subcommand →
CLI mode. This means `fr` with no args opens the TUI, `fr list` runs
the CLI. Simple and ergonomic.

---

## Open Questions (Low Priority, Decide During Implementation)

1. Should `fr` with no subcommand launch TUI, or require `fr tui`?
   Recommendation: no subcommand = TUI.
2. Should ACTIVE.md be gitignored or committed? Recommendation: committed
   (it's useful for anyone reading the repo).
3. Should `fr clean` run automatically before every read command, or only
   when mtime differs? Recommendation: mtime-based, not every time.
