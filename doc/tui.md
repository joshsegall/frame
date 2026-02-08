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

Full view of a single task showing all fields as navigable regions: Title, Tags, Added, Deps, Spec, Refs, Note, Subtasks. Open with `Enter` on a task in Track view.

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
| `J` | Jump to task by ID |
| `z`, `u`, `Ctrl+Z` | Undo |
| `Z`, `Ctrl+Y`, `Ctrl+Shift+Z` | Redo |

### Track View — Navigate Mode

**Cursor movement:**

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top |
| `G`, `End` | Jump to bottom |

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
| `-` | Insert task after cursor |
| `p` | Add task at top of backlog |
| `A` | Add subtask under cursor |

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
| `Enter`, `l` | Open track in Track view |
| `a` | Add new track |
| `e` | Edit track name |
| `s` | Toggle shelve/activate |
| `D` | Archive or delete track (with confirmation) |
| `C` | Set cc-focus |
| `m` | Reorder track (enter move mode) |

### Inbox View — Navigate Mode

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top |
| `G`, `End` | Jump to bottom |
| `a` | Add new inbox item (bottom) |
| `-` | Insert item after cursor |
| `e` | Edit item title |
| `t` | Edit item tags |
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
| `l`, `Right`, `Enter` | Expand subtask tree |
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
| `Esc`, `Backspace` | Return to Track view |

### Edit Mode

| Key | Action |
|-----|--------|
| Characters | Insert text |
| `Left`, `Right` | Move cursor |
| `Alt+Left`, `Alt+Right` | Move by word |
| `Alt+b`, `Alt+f` | Move by word (readline) |
| `Home` | Jump to start of line |
| `Ctrl+E`, `End` | Jump to end of line |
| `Backspace` | Delete backward |
| `Alt+Backspace`, `Ctrl+Backspace` | Delete word backward |
| `Shift+Left/Right` | Extend selection |
| `Ctrl+A` | Select all |
| `Ctrl+C` | Copy selection |
| `Ctrl+X` | Cut selection |
| `Ctrl+V` | Paste |
| `Ctrl+Z` | Inline undo |
| `Ctrl+Y`, `Ctrl+Shift+Z` | Inline redo |
| `Enter` | Confirm edit |
| `Esc` | Cancel edit |

**With autocomplete dropdown visible:**

| Key | Action |
|-----|--------|
| `Up`, `Down` | Navigate suggestions |
| `Tab` | Accept suggestion |
| `Enter` | Accept suggestion and confirm edit |
| `Esc` | Dismiss dropdown |

**Multi-line note editing (Detail view):**

| Key | Action |
|-----|--------|
| `Enter` | Insert newline |
| `Tab` | Insert 4 spaces |
| `Up`, `Down` | Move between lines |
| `Esc` | Save and exit edit |

### Move Mode

| Key | Action |
|-----|--------|
| `j`, `Down` | Move item down |
| `k`, `Up` | Move item up |
| `g`, `Home` | Move to top |
| `G`, `End` | Move to bottom |
| `Enter` | Confirm position |
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

Circular dependencies marked with `↻`. Missing deps shown as `[?]`.

### Conflict Popup

Appears when an external file change conflicts with an in-progress edit. Shows the orphaned text. `Esc` to dismiss.

## Features

### Autocomplete

Activates automatically during editing:

- **Tag editing** — suggests tags from config `tag_colors`, `agent.default_tags`, and all existing tags
- **Dep editing** — suggests task IDs from all tracks
- **Spec/Ref editing** — suggests file paths from the project (up to 3 levels deep, filtered by `ref_extensions` and `ref_paths` config)

### Undo/Redo

Full undo/redo stack for all TUI mutations: state changes, title edits, task creation, moves, field edits, inbox operations, track management, and section moves.

- `z`, `u`, or `Ctrl+Z` — undo
- `Z`, `Ctrl+Y`, or `Ctrl+Shift+Z` — redo

External file changes insert a sync marker that blocks undo across the boundary.

Inline edit undo (`Ctrl+Z`/`Ctrl+Y` in Edit mode) operates within the current editing session separately from the main undo stack.

### File Watching

The TUI watches `frame/` for `.md` and `.toml` changes. Self-writes are detected and ignored. External changes trigger a reload; if an edit is in progress, the reload is queued until the edit completes.

### Filtering

Track view filtering via the `f` prefix key:

- `fa` active, `fo` todo, `fb` blocked, `fp` parked, `fr` ready
- `ft` filter by tag (with autocomplete)
- `f Space` clear state filter, `ff` clear all

Filtered tasks are shown with ancestor context rows (dimmed, non-selectable). The active filter is displayed in the tab bar.

### Multi-Select

`v` toggles individual task selection, `V` range-selects, `Ctrl+A` selects all. With tasks selected, bulk operations apply to all selected: state changes, tag/dep edits, move, cross-track move.

### Jump to Task (`J`)

Opens an ID search prompt with autocomplete showing all task IDs and titles. Enter jumps to the matching task, switching tracks if needed.

### Repeat Action (`.`)

Repeats the last repeatable action (state change, tag toggle, etc.) on the current task.

### Grace Period Moves

When a task is marked done in the TUI, it stays in Backlog for 5 seconds before moving to Done. During this period, undo cancels the move. The Recent view's reopen (`Space`) has a similar grace period in reverse.

### Clipboard

In Edit mode: `Ctrl+C` copies, `Ctrl+X` cuts, `Ctrl+V` pastes. Text selection via `Shift+arrows`.

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
