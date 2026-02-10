# TUI Reference

Launch the TUI by running `fr` with no arguments.

## Views

### Track View

The default view. Shows a single track's tasks as an indented tree with expand/collapse. Switch between tracks with `1`-`9` or `Tab`/`Shift+Tab`.

### Tracks View

Overview of all tracks grouped by state (active, shelved, archived) with task count statistics. Switch to it with `0` or `` ` ``.

### Inbox View

Shows inbox items with numbered indices and tags. Switch to it with `i`.

### Recent View

Done tasks grouped by resolved date, with a tree structure for subtasks. Switch to it with `r`.

### Detail View

Full view of a single task showing all fields as navigable regions: Title, Tags, Added, Deps, Spec, Refs, Note, Subtasks. Open with `Enter` on a task in Track view or Recent view. A breadcrumb trail always shows the origin (track prefix or "Recent") and any parent tasks when drilling into subtasks.

## Modes

| Mode | Description |
|------|-------------|
| Navigate | Normal browsing, keybindings dispatch actions |
| Search | Typing a search query (`/` to enter, `Enter` to execute, `Esc` to cancel) |
| Edit | Inline text editing (task titles, tags, fields) |
| Move | Reordering tasks/tracks with `j`/`k`, confirm with `Enter`, cancel with `Esc` |
| Triage | Moving inbox items to tracks (select track, then position) |
| Confirm | Yes/no prompt (e.g., delete inbox item, archive track) |
| Select | Multi-select mode for bulk operations on tasks |
| Command | Command palette fuzzy search (`>` to open) |

## Keybindings

### Global (All Views, Navigate Mode)

| Key | Action |
|-----|--------|
| `1`-`9` | Switch to track by number |
| `0`, `` ` `` | Switch to Tracks view |
| `i` | Switch to Inbox view |
| `r` | Switch to Recent view |
| `Tab` | Next view |
| `Shift+Tab` | Previous view |
| `QQ` | Quit (press `Q` twice) |
| `Ctrl+Q` | Quit immediately |
| `?` | Toggle help overlay |
| `/` | Enter search mode |
| `n` | Next search match |
| `N` | Previous search match |
| `>` | Open command palette |
| `T` | Open tag color editor |
| `P` | Open project picker |
| `J` | Jump to task by ID |
| `z`, `u`, `Ctrl+Z`, `Super+Z` | Undo |
| `Z`, `Ctrl+Y`, `Ctrl+Shift+Z`, `Super+Shift+Z` | Redo |

### Track View — Navigate Mode

**Cursor movement:**

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top |
| `G`, `End` | Jump to bottom |
| `Alt+Up` | Jump to previous top-level task |
| `Alt+Down` | Jump to next top-level task |

**Expand/collapse:**

| Key | Action |
|-----|--------|
| `l`, `Right` | Expand task |
| `h`, `Left` | Collapse task / go to parent |

**State changes:**

| Key | Action |
|-----|--------|
| `Space` | Cycle state: todo -> active -> done -> todo |
| `o` | Set todo |
| `x` | Set done |
| `b` | Toggle blocked |
| `~` | Toggle parked |
| `c` | Toggle `#cc` tag |

**Task creation:**

| Key | Action |
|-----|--------|
| `a` | Add task at bottom of backlog |
| `=` | Append to end of group (top-level = bottom; subtask = end of siblings) |
| `-` | Insert after cursor (sibling at same level; type `-` again to outdent) |
| `p` | Add task at top of backlog |
| `A` | Add subtask under cursor |

**Insert behavior by cursor position:**

| Key | On top-level | On child | On sub-child |
|---|---|---|---|
| `a` | Append to end of backlog | ← same | ← same |
| `p` | Prepend to top of backlog | ← same | ← same |
| `-` | Insert top-level after current | Insert sibling after current | Insert sibling after current |
| `- -` | *(already top)* | Promote to top-level | Promote to child level |
| `- - -` | | *(already top)* | Promote to top-level |
| `=` | Same as `a` | Append to end of parent's children | Append to end of parent's children |
| `A` | Add child | Add sub-child | *(max depth)* |

**Editing & actions:**

| Key | Action |
|-----|--------|
| `e` | Edit task title |
| `t` | Edit task tags |
| `Enter` | Open detail view |
| `m` | Enter move mode |
| `M` | Cross-track move |
| `C` | Set/clear cc-focus |
| `D` | Open dependency popup |
| `.` | Repeat last action |

**Filtering (prefix key `f`):**

| Key | Action |
|-----|--------|
| `fa` | Filter: active tasks |
| `fo` | Filter: todo tasks |
| `fb` | Filter: blocked tasks |
| `fp` | Filter: parked tasks |
| `fr` | Filter: ready tasks (todo/active, all deps resolved) |
| `ft` | Filter by tag (opens tag autocomplete) |
| `f Space` | Clear state filter |
| `ff` | Clear all filters |

**Multi-select:**

| Key | Action |
|-----|--------|
| `v` | Toggle selection on current task |
| `V` | Range select (from anchor to cursor) |
| `Ctrl+A` | Select all visible tasks |

With selection active (Select mode):

| Key | Action |
|-----|--------|
| `Space` | Cycle state on all selected |
| `x` | Set selected to done |
| `o` | Set selected to todo |
| `b` | Toggle blocked on selected |
| `~` | Toggle parked on selected |
| `t` | Bulk edit tags (`+tag -tag` syntax) |
| `d` | Bulk edit deps (`+ID -ID` syntax) |
| `m` | Bulk move selected tasks |
| `M` | Bulk cross-track move |
| `N` | Clear selection |
| `Esc` | Clear selection, return to Navigate |

### Tracks View — Navigate Mode

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top |
| `G`, `End` | Jump to bottom |
| `Enter` | Open track in Track view |
| `a`, `=` | Add new track (bottom of active list) |
| `-` | Insert new track after cursor |
| `p` | Add new track (top of active list) |
| `e` | Edit track name |
| `s` | Toggle shelve/activate |
| `X` | Archive or delete track (with confirmation) |
| `R` | Rename track prefix |
| `C` | Set cc-focus |
| `m` | Reorder track (enter move mode) |

### Inbox View — Navigate Mode

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top |
| `G`, `End` | Jump to bottom |
| `a`, `=` | Add new inbox item (bottom) |
| `-` | Insert item after cursor |
| `p` | Add new inbox item (top) |
| `e` | Edit item title |
| `t` | Edit item tags |
| `n` | Edit item note (inline multi-line editor) |
| `x` | Delete item (with confirmation) |
| `m` | Reorder item (enter move mode) |
| `Enter` | Triage item to a track |

### Recent View — Navigate Mode

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top |
| `G`, `End` | Jump to bottom |
| `Enter` | Open detail view |
| `l`, `Right` | Expand subtask tree |
| `h`, `Left` | Collapse subtask tree |
| `Space` | Reopen task (5s grace period, press again to cancel) |

### Detail View — Navigate Mode

**Region navigation:**

| Key | Action |
|-----|--------|
| `j`, `Down` | Next region / next subtask |
| `k`, `Up` | Previous region / previous subtask |
| `Tab` | Jump to next editable region |
| `Shift+Tab` | Jump to previous editable region |
| `g` | Jump to first region |
| `G` | Jump to last region |

**Editing:**

| Key | Action |
|-----|--------|
| `e`, `Enter` | Edit current region |
| `t` | Jump to Tags and edit |
| `@` | Jump to Refs and edit |
| `d` | Jump to Deps and edit |
| `n` | Jump to Note and edit |

**State changes (same as Track view):**

| Key | Action |
|-----|--------|
| `Space` | Cycle state |
| `o` | Set todo |
| `x` | Set done |
| `b` | Toggle blocked |
| `~` | Toggle parked |
| `M` | Cross-track move |

**Other:**

| Key | Action |
|-----|--------|
| `D` | Open dependency popup |
| `.` | Repeat last action |
| `Esc`, `Backspace` | Return to origin view (Track or Recent) |

### Edit Mode

| Key | Action |
|-----|--------|
| Characters | Insert text |
| `Left`, `Right` | Move cursor |
| `Alt+Left`, `Alt+Right` | Move by word |
| `Alt+b`, `Alt+f` | Move by word (readline) |
| `Home`, `Ctrl+A` | Jump to start of line |
| `Ctrl+E`, `End` | Jump to end of line |
| `Backspace` | Delete backward |
| `Alt+Backspace`, `Ctrl+Backspace` | Delete word backward |
| `Ctrl+U` | Kill to start of line |
| `Shift+Left/Right` | Extend selection |
| `Ctrl+C`, `Super+C` | Copy selection |
| `Ctrl+X`, `Super+X` | Cut selection |
| `Ctrl+V`, `Super+V` | Paste |
| `Ctrl+Z`, `Super+Z` | Inline undo |
| `Ctrl+Y`, `Ctrl+Shift+Z`, `Super+Shift+Z` | Inline redo |
| `Enter` | Confirm edit |
| `Esc` | Cancel edit |

**With autocomplete dropdown visible:**

| Key | Action |
|-----|--------|
| `Up`, `Down` | Navigate suggestions |
| `Tab` | Accept suggestion |
| `Enter` | Accept suggestion and confirm edit |
| `Esc` | Dismiss dropdown |

**Multi-line note editing (Detail view, Inbox view):**

| Key | Action |
|-----|--------|
| `Enter` | Insert newline |
| `Tab` | Insert 4 spaces |
| `Up`, `Down` | Move between lines |
| `Alt+Up`, `Alt+Down` | Jump between paragraphs |
| `Esc` | Save and exit edit |

### Move Mode

| Key | Action |
|-----|--------|
| `j`, `Down` | Move item down (same depth, crosses parent boundaries) |
| `k`, `Up` | Move item up (same depth, crosses parent boundaries) |
| `h`, `Left` | Outdent: promote to parent's level |
| `l`, `Right` | Indent: make child of sibling above |
| `g`, `Home` | Move to top |
| `G`, `End` | Move to bottom |
| `Enter` | Confirm position (re-keys IDs if depth/parent changed) |
| `Esc` | Cancel and restore |

### Search Mode

| Key | Action |
|-----|--------|
| Characters | Type search query (regex) |
| `Enter` | Execute search |
| `Esc` | Cancel search |
| `Up` | Previous search history |
| `Down` | Next search history |

### Triage Mode

**Step 1 — Select track:**

| Key | Action |
|-----|--------|
| Characters | Filter track list |
| `Up`, `Down` | Navigate tracks |
| `Enter` | Select track |
| `Esc` | Cancel triage |

**Step 2 — Select position:**

| Key | Action |
|-----|--------|
| `t` | Insert at top |
| `b`, `Enter` | Insert at bottom |
| `Up`, `Down` | Navigate options |
| `Esc` | Cancel |

### Confirm Mode

| Key | Action |
|-----|--------|
| `y` | Confirm action |
| `n`, `Esc` | Cancel |

### Command Palette

| Key | Action |
|-----|--------|
| Characters | Filter actions |
| `Up`, `Down` | Navigate actions |
| `Enter` | Execute selected action |
| `Backspace` | Delete filter char (or close if empty) |
| `Esc` | Close palette |

Actions are context-sensitive — the available set depends on the current view (Track, Detail, Tracks, Inbox, Recent). Uses fuzzy matching: type any part of an action name to filter. Each action shows its keyboard shortcut, making the palette useful for discovering keybindings.

Some actions are **palette-only** (no direct key binding):

| Action | View | Description |
|--------|------|-------------|
| Mark done (#wontdo) | Track | Add `#wontdo` tag and mark task done |
| Mark done (#duplicate) | Track | Add `#duplicate` tag and mark task done |
| Collapse all | Track | Collapse all expanded tasks |
| Expand all | Track | Expand all tasks with children |

## Overlays

### Help Overlay (`?`)

Context-sensitive keybinding reference. Scrollable with `j`/`k`, `g`/`G` to jump. Close with `?` or `Esc`.

### Tag Color Editor (`T`)

Popup for assigning colors to tags from a 10-swatch palette.

- `j`/`k` — navigate tags
- `Enter` — open color picker
- `Backspace` — clear tag color
- `Esc` — close

In the color picker:
- `h`/`l` — navigate swatches
- `Enter` — assign color
- `Backspace` — clear color
- `Esc` — close picker

### Dependency Popup (`D`)

Shows upstream (blocked by) and downstream (blocking) dependencies in an expandable tree.

- `j`/`k` — navigate entries
- `h`/`l` — collapse/expand
- `g`/`G` — jump to first/last
- `Enter` — jump to task (cross-track)
- `Esc` — close

The popup has two sections: **Blocked by** (tasks this task depends on) and **Blocking** (tasks that depend on this task). When a section has 1-2 entries, they are auto-expanded one level. With 3+ entries, they start collapsed. Empty sections show "(nothing)".

Circular dependencies marked with `↻`. Missing deps shown as `[?]`.

### Project Picker (`P`)

Switch between registered frame projects without leaving the TUI. If `fr` is launched outside any project, the picker opens automatically.

- `j`/`k` — navigate project list
- `Enter` — switch to selected project
- `s` — toggle sort (recent vs. alphabetical)
- `X` — remove project from registry (press twice to confirm)
- `Esc` — close

Projects are listed with their name and abbreviated path. The current project is highlighted. Missing projects (directory no longer exists) are shown dimmed with "(not found)".

### Prefix Rename (`R` in Tracks View)

3-step flow for renaming a track's ID prefix (e.g., `EFF` to `FX`):

1. **Edit**: Inline editor opens pre-filled with the current prefix, text selected. Type the new prefix (auto-uppercased). Live validation shows errors for empty, non-alphanumeric, or duplicate prefixes. `Enter` to proceed, `Esc` to cancel.
2. **Confirm**: Popup shows old/new prefix, blast radius (task ID count, dep references across tracks), and a warning that the operation cannot be undone. `Enter` to confirm, `Esc` to go back to editing.
3. **Execute**: Renames all task IDs and subtask IDs in the track, updates dep references across all other tracks, renames IDs in the archive file, and updates `project.toml`. Inserts an undo sync marker (no undo — use git to revert).

### Conflict Popup

Appears when an external file change conflicts with an in-progress edit. Shows the orphaned text. `Esc` to dismiss.

## Features

### Autocomplete

Activates automatically during editing:

- **Tag editing** — suggests tags from config `tag_colors`, `ui.default_tags`, and all existing tags
- **Dep editing** — suggests task IDs from all tracks
- **Spec/Ref editing** — suggests file paths from the project (up to 3 levels deep, filtered by `ref_extensions` and `ref_paths` config)

The dropdown appears immediately when entering edit mode on an eligible field — no keypress is needed to trigger it. It also activates during inline tag editing (`t` in Track view), filter tag selection (`ft`), and jump-to-task (`J`).

Filtering is case-insensitive substring matching. Word extraction depends on the field: tag autocomplete matches the word after the last space; dep autocomplete matches after the last comma or space; file path autocomplete matches after the last space.

`Tab` accepts the selected suggestion and stays in edit mode. `Enter` accepts and confirms the edit. `Esc` dismisses the dropdown but stays in edit mode.

### Undo/Redo

Full undo/redo stack for all TUI mutations: state changes, title edits, task creation, moves, field edits, inbox operations, track management, and section moves.

- `z`, `u`, or `Ctrl+Z` — undo
- `Z`, `Ctrl+Y`, or `Ctrl+Shift+Z` — redo

Undo navigates to the affected item — switching views and tracks if needed — and briefly highlights it.

External file changes insert a sync marker that blocks undo across the boundary. The undo stack starts empty on each launch, so there is nothing to undo from a previous session. When a sync marker is inserted, the redo stack is cleared permanently.

Inline edit undo (`Ctrl+Z`/`Ctrl+Y` in Edit mode) operates within the current editing session separately from the main undo stack.

### File Watching

The TUI watches `frame/` for `.md` and `.toml` changes. Self-writes are detected and ignored. External changes trigger a reload; if an edit is in progress, the reload is queued until the edit completes.

Reload is deferred during both Edit and Move modes, applied when the mode exits. After reload, if `auto_clean` is enabled (default: true), frame automatically assigns missing IDs/dates and archives excess done tasks. This can cause visible changes to the file that weren't made by the user. Each reload inserts an undo sync marker, which clears the redo stack.

### Filtering

Track view filtering via the `f` prefix key:

- `fa` active, `fo` todo, `fb` blocked, `fp` parked, `fr` ready
- `ft` filter by tag (with autocomplete)
- `f Space` clear state filter, `ff` clear all

State and tag filters are independent — you can have both active simultaneously. Both must match (AND logic). `f Space` clears only the state filter (tag stays). `ff` clears both.

The filter applies globally across all tracks, not per-track. Switching tracks keeps the same filter active. Filters are session-only — not persisted to `.state.json`, cleared on restart.

"Ready" means: state is todo or active, AND all dependency tasks are done.

When a filter removes the task under the cursor, the cursor moves to the nearest matching task. Ancestor context rows appear when a nested task matches but its parent doesn't — the parent is shown dimmed to preserve tree context. Context rows are skipped during cursor navigation. When no tasks match, the track shows "no matching tasks".

The active filter indicator appears right-aligned in the tab separator (e.g., "filter: ready #bug"). Filters have no undo — they are view-only, not mutations.

### Multi-Select

`v` toggles individual task selection, `V` range-selects, `Ctrl+A` selects all. With tasks selected, bulk operations apply to all selected: state changes, tag/dep edits, move, cross-track move.

First `v` press enters Select mode and toggles the current task. Subsequent `v` presses toggle without mode change. `V` sets an anchor at the current position; moving the cursor and pressing `V` again selects all tasks in the range. Context rows (from filtering) and section separators are excluded from selection.

Selection persists when opening Detail view (`Enter`) and returning (`Esc`). Selection is cleared when switching to a different view (Inbox, Recent, Tracks). Multi-select is only available in Track view. If the last selected task is deselected via `v`, the mode returns to Navigate automatically.

### Jump to Task (`J`)

Opens an ID search prompt with autocomplete showing all task IDs and titles. Enter jumps to the matching task, switching tracks if needed.

### Repeat Action (`.`)

Repeats the last repeatable action (state change, tag toggle, etc.) on the current task.

### Grace Period Moves

When a task is marked done in the TUI, it stays in Backlog for 5 seconds before moving to Done. During this period, undo cancels the move. The Recent view's reopen (`Space`) has a similar grace period in reverse.

The 5-second timer only counts down in Navigate mode. Entering Search, Edit, or Move mode pauses it. Switching views (e.g., Track to Inbox) or quitting immediately flushes all pending moves — tasks move to their target section without waiting.

The entire subtask tree moves as a unit. Subtasks cannot be moved between sections independently — only top-level tasks trigger section moves.

### Clipboard

In Edit mode: `Ctrl+C`/`Super+C` copies, `Ctrl+X`/`Super+X` cuts, `Ctrl+V`/`Super+V` pastes. Text selection via `Shift+arrows`.

## Keyboard Protocol

Frame uses the Kitty keyboard protocol by default for unambiguous key event reporting. This matters most for modified keys like `Ctrl+Shift+Z` (redo) and distinguishing `Tab` from `Ctrl+I`. If your terminal doesn't support it, frame falls back gracefully — most keybindings still work, but a few modified-key combos may not register. Set `kitty_keyboard = false` in `project.toml` if you see input issues.

## Configuration

UI-relevant settings in `project.toml`:

```toml
[ui]
kitty_keyboard = true      # enhanced keyboard protocol (disable if terminal has issues)
ref_extensions = ["md"]    # file types for ref/spec autocomplete
ref_paths = ["doc"]        # directories for ref/spec autocomplete

[ui.tag_colors]
bug = "#FF4444"
design = "#44DDFF"

[ui.colors]
# state/UI color overrides
```

The TUI persists cursor positions, scroll offsets, and expanded task state in `frame/.state.json` (auto-saved, not meant for manual editing).
