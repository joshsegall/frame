# Board View

Design spec for the board view — a kanban-style cross-track view showing tasks in Ready, In Progress, and Done columns.

## Overview

The board view projects tasks from all active tracks into three columns based on workflow state. It's designed as a companion to the agent workflow: by default it shows what CC can pick up, what CC is working on, and what was recently completed.

It is a **view** (like Tracks, Inbox, Recent), not a track tab. It operates on the same underlying data and undo stack as every other view.

## Columns

### Ready

Tasks that are available to work on. A task appears here when:

- State is `todo`
- Not blocked (no `[-]` state)
- All dependencies resolved (all dep targets are `done`)

In **CC mode** (default): additionally filtered to tasks with the `#cc` tag.

Order: grouped by track (active track tab order), position order within each track. A subtle track header separates groups (track name, dimmed).

### In Progress

Tasks with `active` state (`[>]`).

In **CC mode**: filtered to `#cc`-tagged active tasks.

Order: same as Ready — grouped by track, position order within track.

### Done

Tasks with `done` state (`[x]`), filtered to those resolved within the last N days (configured by `board_done_days`, default 7).

In **CC mode**: additionally filtered to tasks with the `#cc` tag (or `#cc-added`).

Order: reverse chronological by `resolved` date. No track grouping — recency is the organizing principle. Track is apparent from the color-coded ID prefix.

## Mode Toggle

The board has two modes:

| Mode | Ready column | In Progress column | Done column |
|------|-------------|-------------------|-------------|
| **CC** (default) | `#cc` + ready | `#cc` + active | `#cc` + last N days |
| **All** | All ready | All active | All last N days |

Toggle with `c`. The current mode is shown as a label in the board header: `mode: cc` or `mode: all`. The toggle applies consistently to all three columns.

The mode setting persists in `.state.json` (`board_mode: "cc" | "all"`).

## Layout

### Three-column layout

```
┌─ Ready (3) ──────┬─ In Progress (1) ┬─ Done (5) ────────┐
│ effects          │ effects          │ EFF-012 Effect-    │
│ EFF-015 Effect   │ EFF-014 Implemen │ aware DCE          │
│ handler opt pass │ t effect inferen │ INFRA-009 Add span │
│ EFF-016 Error    │ ce for closures  │ tracking to HIR    │
│ msgs for         │                  │ nodes              │
│ mismatches       │                  │ EFF-003 Implement  │
│                  │                  │ effect handler     │
│ infra            │                  │ desugaring         │
│ INFRA-022 Fix    │                  │                    │
│ linker flags     │                  │                    │
└──────────────────┴──────────────────┴────────────────────┘
```

Columns share available width equally (⅓ each). Each column has:

- **Header**: column name + task count, highlighted per column identity
- **Separator**: horizontal line below header
- **Scrollable body**: independent vertical scroll per column

### Card rendering

Each card shows ID and title on one line, wrapped:

```
EFF-015 Effect handler opt pass
```

ID + title on the same line, soft-wrapped to column width, capped at 4 visual lines total. Truncated with `…` if it still overflows. The ID is color-coded by track prefix color (derived from `[ui.tag_colors]` or auto-assigned).

```
EFF-015 Effect handler opt pass
EFF-016 Error msgs for mismatches
```

No blank lines between cards — consecutive lines, high density. Track headers provide visual breaks.

Track headers in Ready and In Progress columns appear as dimmed, non-selectable separator lines (similar to section separators in track view).

### Narrow width handling

Below 80 columns total width: hide the Done column entirely. Show a hint in the header area: `→ Done (N)`. The remaining two columns split the width equally.

Below 50 columns: single-column mode. Show only the focused column (whichever the cursor is in). Column headers become navigable tabs. `h`/`l` switches the visible column.

## Navigation

### Column navigation

| Key | Action |
|-----|--------|
| `h`, `Left` | Move cursor to previous column |
| `l`, `Right` | Move cursor to next column |

When moving between columns, the cursor lands on the nearest row to the current vertical position (preserving approximate scroll position).

### Within-column navigation

| Key | Action |
|-----|--------|
| `j`, `Down` | Move cursor down |
| `k`, `Up` | Move cursor up |
| `g`, `Home` | Jump to top of column |
| `G`, `End` | Jump to bottom of column |

Cursor skips track header separators (non-selectable, same as context rows in filtered track view).

### Scrolling

Each column scrolls independently. The cursor drives scroll for its column — moving past the visible area scrolls that column. Other columns remain at their current scroll position.

## Actions

### State changes

Same keybindings as track view, same behavior:

| Key | Action |
|-----|--------|
| `Space` | Cycle state: todo → active → done → todo |
| `o` | Set todo |
| `x` | Set done |
| `b` | Toggle blocked |
| `~` | Toggle parked |

State changes trigger the same 5-second grace period as track view. A task marked done stays in its current column for 5 seconds before moving to Done. Undo during the grace period cancels the move.

When a task's state changes such that it no longer belongs in its current column (e.g., marked active → moves from Ready to In Progress), the card animates out after the grace period and appears in the target column.

If a state change causes a task to leave the board entirely (e.g., marked blocked or parked), it disappears after the grace period.

### Detail view

| Key | Action |
|-----|--------|
| `Enter` | Open detail view for selected task |
| `Esc` | Return to board view (from detail) |

The detail view works identically to entering from track view. The breadcrumb shows the track origin. `Esc` returns to the board with cursor position preserved.

### Other actions

| Key | Action |
|-----|--------|
| `c` | Toggle CC/All mode |
| `M` | Cross-track move |
| `D` | Open dependency popup |
| `e` | Edit task title (inline) |
| `t` | Edit task tags (inline) |
| `J` | Jump to task by ID |
| `.` | Repeat last action |
| `?` | Help overlay (board-specific bindings) |
| `>` | Command palette |

`m` (move/reorder) is **not available** in board view — position is determined by backlog order in the source track. To reprioritize, switch to the track view.

Multi-select (`v`, `V`) is **not available** in board view in v1. Tasks span multiple tracks, and bulk operations across tracks in a columnar layout adds complexity without clear benefit yet.

## Global keybindings

The board view integrates with existing global navigation:

| Key | Action |
|-----|--------|
| `K` | Switch to board view (from any view) |
| `1`-`9` | Switch to track by number (leaves board) |
| `0`, `` ` `` | Switch to Tracks view |
| `i` | Inbox view |
| `r` | Recent view |
| `Tab` | Next view (board is in the rotation) |
| `Shift+Tab` | Previous view |

### View rotation order

The board view slots into the view cycle between Tracks and the first track tab:

`Tracks → Board → Track 1 → Track 2 → … → Inbox → Recent → Tracks`

## Data model

### View enum

Add `Board` variant to the `View` enum:

```
View::Board
```

No parameters needed — the board always shows all active tracks.

### Board state

New struct on `App`:

```rust
struct BoardState {
    /// Which column the cursor is in
    focus_column: BoardColumn,
    /// Cursor index within each column (independent)
    cursor: [usize; 3],
    /// Scroll offset for each column (independent)
    scroll: [usize; 3],
    /// CC mode or All mode
    mode: BoardMode,
}

enum BoardColumn {
    Ready,    // 0
    InProgress, // 1
    Done,     // 2
}

enum BoardMode {
    Cc,
    All,
}
```

Board state persists in `.state.json`:

```json
{
  "board": {
    "mode": "cc",
    "focus_column": 0
  }
}
```

Cursor and scroll positions are ephemeral (not persisted) — they depend on task state which changes frequently.

### Flat item list per column

Each column builds its own `Vec<BoardItem>`:

```rust
enum BoardItem {
    TrackHeader { track_id: String, track_name: String },
    Task { track_id: String, task_id: String, title: String, id_display: String },
}
```

These lists are rebuilt on every data change (same trigger as `build_flat_items()` in track view). The board reads from the same `Project` data — no separate data path.

### Ready/In Progress task collection

```
for each active track (in tab order):
    for each top-level backlog task (in position order):
        if task matches column criteria:
            add TrackHeader (if first task from this track)
            add Task
```

Ready criteria: `todo` state, not blocked, all deps resolved, and (in CC mode) has `#cc` tag.  
In Progress criteria: `active` state, and (in CC mode) has `#cc` tag.

### Done task collection

```
collect all top-level done tasks across all active tracks
    where resolved date >= (today - board_done_days)
    and (in CC mode) has #cc or #cc-added tag
sort by resolved date descending
```

**Archive interaction**: Done tasks that have been archived by `fr clean` are not shown on the board. The board only reads from active track files, not archive files. This is consistent with the track view Done section behavior.

## Configuration

### `[ui]` section in `project.toml`

```toml
[ui]
board_done_days = 7    # days of done tasks to show (default: 7, 0 = hide done column)
```

Setting `board_done_days = 0` hides the Done column entirely (two-column layout, Ready + In Progress split 50/50).

## File watching

The board view responds to file changes identically to track view:

- Self-writes detected and ignored
- External changes trigger rebuild of all three column lists
- Edit/move mode defers reload
- Sync marker pushed to undo stack on reload

Since the board reads from all active tracks, any track file change triggers a board rebuild.

## Undo

All state changes from the board push to the same undo stack. `UndoNavTarget` navigates to the affected task — which may require switching to track view if the undo operation doesn't map cleanly to a board column (e.g., undoing a block operation on a task that's no longer on the board).

When undoing from the board view: if the affected task is visible on the board, cursor moves to it (switching columns if needed). If not visible (e.g., it was parked), switch to the source track view and navigate there.

## Search

`/` enters search mode. Matches highlight across all three columns simultaneously. `n`/`N` cycles through matches across columns (left to right, top to bottom within each column).

## Filtering

The global tag filter (`ft`) applies to the board view — narrowing all columns to tasks with the matching tag. State filters are not applicable (the columns are the state filter).

The CC/All mode toggle is independent of the tag filter. They compose: CC mode + tag filter `#bug` shows only `#cc #bug` tasks in Ready/In Progress.

## Empty states

- **Ready column empty**: "No ready tasks" (in CC mode: "No #cc tasks ready — press c for all")
- **In Progress column empty**: "Nothing active" (in CC mode: "No #cc tasks active")
- **Done column empty**: "No tasks completed in last N days" (in CC mode: "No #cc tasks completed in last N days")
- **All columns empty**: "No tasks across active tracks" (unlikely but handled)

## Scope boundaries (not in v1)

- **Subtask expansion on board**: Cards show top-level tasks only. No expand/collapse. Drill into detail view to see subtasks.
- **Multi-select**: Not supported on the board in v1.
- **Drag-and-drop reordering**: No reorder within columns. Position is controlled in track view.
- **Custom column definitions**: Fixed three columns. No user-defined columns.
- **Swimlanes by track**: The grouping within Ready/In Progress serves this purpose without full swimlane rendering.

## Implementation plan

### Phase 1: Core board rendering

1. Add `View::Board` to the view enum
2. Add `BoardState` to `App`
3. Implement column data collection (ready, in progress, done)
4. Implement board renderer (three-column layout, card rendering, headers)
5. Add `K` keybinding to switch to board view
6. Add board to view rotation cycle (`Tab`/`Shift+Tab`)

### Phase 2: Navigation and interaction

7. Column navigation (`h`/`l`)
8. Within-column navigation (`j`/`k`/`g`/`G`)
9. Independent column scrolling
10. State change keybindings with grace period
11. Detail view drill-down (`Enter`) and return (`Esc`)

### Phase 3: Mode toggle and config

12. CC/All mode toggle (`c`)
13. Persist board mode in `.state.json`
14. `board_done_days` config option
15. Narrow-width layout (hide Done below 80 cols, single-column below 50)

### Phase 4: Integration

16. Search highlighting across columns
17. Tag filter support
18. File watching / board rebuild on data change
19. Undo navigation from board context
20. Help overlay with board-specific bindings
21. Command palette board-specific actions
22. Empty state messages

### Dependencies

- No new crates required
- No parser changes
- No file format changes
- No CLI changes (board is TUI-only)
