# Feature: Bulk Operations (SELECT Mode)

## Overview

Select multiple tasks in the TUI and apply an operation to all of them
at once. SELECT is a full mode — visually distinct, with its own status
row indicator and key hints. Existing action keys apply to the selection.

Primary use cases: bulk tagging, cross-track moves, and bulk state
changes to an absolute state.

---

## SELECT Mode

### Entering SELECT mode

`v` on any task in track view enters SELECT mode and marks the cursor's
task as selected. The status row shows `-- SELECT --` in the highlight
color.

### Toggle select (`v`)

While in SELECT mode, `v` toggles selection on the cursor's current
task. Move the cursor with ↑↓/jk, press `v` to add or remove tasks
from the selection.

### Range select (`V`)

`V` selects everything between the last selected task and the cursor,
inclusive. Operates on visible lines only — collapsed subtasks are not
included. If no task is selected yet, `V` enters SELECT mode and
behaves like `v`.

### Select all (`Cmd+A`)

Selects all visible tasks in the current track view.

### Exiting SELECT mode

- `Esc` — clears selection, returns to NAVIGATE mode
- Switching tabs or views — clears selection, returns to NAVIGATE mode

**Selection persists after successful operations.** Tagging, state
changes, dep edits, and moves do not clear the selection. This allows
chaining operations on the same set: select → tag → move → mark done.
Only `Esc` and tab/view switch clear the selection.

Cancelled operations (e.g., `Esc` from MOVE mode or the inline
editor) also preserve the selection — you cancelled the operation,
not the selection.

Drilling into a task detail view (`Enter`) and returning (`Esc`) does
**not** clear the selection. This lets you inspect a task before acting
on the group.

---

## Visual Treatment

### Selected rows

Selected rows have a **distinct background color** — a cool blue tone
at low lightness (`#1e4d5c` default), clearly different in hue from
the cursor's purple highlight. This covers the full row width.
Additionally, a `▌` bar in the highlight color appears on the left
edge as a redundant indicator for monochrome terminals and
accessibility.

### Cursor row

The cursor row keeps its normal highlight treatment (cursor background
color, bright white text) regardless of selection state. When the
cursor is on a selected row, the **cursor color wins** — the row
renders with the cursor's background, not the selection background.
The `▌` bar remains visible to indicate the row is part of the
selection.

### Example

```
▌● EFF-014 Implement effect inference    ready       ← selected
   ├ ○ .1 Add effect variables
   ├ ● .2 Unify effect rows              cc
   └ ○ .3 Test with nested closures
 ○ EFF-015 Effect handler opt pass       ready       ← cursor (not selected)
▌○ EFF-016 Error msgs for mismatches     ready       ← selected
  ○ EFF-017 Research: effect composition  research
▌○ EFF-018 Design doc: effect aliases    design       ← selected
```

Selected rows (EFF-014, EFF-016, EFF-018) show the `▌` bar and the
lifted background. The cursor (EFF-015) has its own distinct highlight.

---

## Status Row

```
-- SELECT --                                   3 selected   x/b/o/~ t d m Esc
```

- Left: mode indicator in highlight color
- Right of center: selection count (same position as search match count)
- Far right: key hints for available actions

---

## Bulk Actions

### State changes

| Key | Action           |
|-----|------------------|
| `x` | Mark all done    |
| `b` | Set all blocked  |
| `o` | Set all todo     |
| `~` | Park all         |

All set an absolute state. No cycling — `Space` is not available in
SELECT mode. Bulk marking as active (`>`) is not supported; do it
one at a time.

### Tagging (`t`)

`t` opens the inline editor (see below) with tag autocomplete. The
field accepts multiple tokens, each prefixed with `+` or `-`:

```
  tags: +cc +ready -design▊
```

Each `+tag` adds, each `-tag` removes. Bare tokens (no prefix) default
to `+`. Autocomplete activates on the current token being typed.

On confirm, all additions and removals apply to every selected task.
Tasks that already have a tag (for `+`) or don't have it (for `-`)
are silently skipped.

### Dependencies (`d`)

`d` opens the inline editor (see below) with task ID autocomplete.
Same multi-token `+`/`-` convention:

```
  deps: +EFF-014 -EFF-003▊
```

Each `+ID` adds a dep, each `-ID` removes one. Bare IDs default to
`+`. Autocomplete activates on the current token.

### Inline Editor Placement

Both `t` (tag) and `d` (dep) open a single-line text field **below
the cursor row**, inserted inline — no popup border. The field pushes
content down temporarily, like inserting a line. A label on the left
indicates the field type:

```
▌○ EFF-016 Error msgs for mismatches     ready       ← cursor
  tags: +cc +ready -design▊                          ← inline editor
  ○ EFF-017 Research: effect composition  research
```

```
▌○ EFF-016 Error msgs for mismatches     ready       ← cursor
  deps: +EFF-014 -EFF-003▊                           ← inline editor
  ○ EFF-017 Research: effect composition  research
```

Autocomplete dropdown appears below the editor field, filtering on
the current token. `Enter` confirms all operations, `Esc` cancels.

### Move within track (`m`)

`m` enters MOVE mode with the selection as a group. The selected tasks
are **collapsed into a single stand-in row** at the cursor position.
All other tasks remain visible, giving full context for placement.

```
 ○ EFF-015 Effect handler opt pass       ready
 ▌━━━ 3 tasks ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━    ← stand-in (cursor)
 ⊘ EFF-012 Effect-aware DCE              ready
 ○ EFF-016 Error msgs for mismatches     ready
```

The stand-in row uses the highlight color and selection background.

- ↑↓ moves the stand-in through the list. Non-selected tasks reflow
  around it in real time
- Maintain at least **3 rows of context** above and below at viewport
  edges, scrolling as needed
- `Enter` confirms — the stand-in expands into the placed tasks,
  preserving their original relative order
- `Esc` cancels — all tasks return to original positions

If selected tasks were at positions 2, 5, and 8, the stand-in
represents them as a unit. On confirm, they fill in at the stand-in's
position in their original relative order (2, 5, 8 → contiguous).

Status row during bulk move:

```
-- MOVE (3) --                                       ↑↓ move  Enter ✔  Esc ✘
```

### Move to track (`M`)

`M` in SELECT mode triggers the cross-track move flow (same UI as
single-task `M`): track autocomplete dropdown appears at the cursor
row. On confirm, all selected tasks move to the target track. IDs
are reassigned, deps updated across all tracks.

---

## Subtree Behavior

Selecting a parent task does **not** implicitly select its subtasks.
The state change or tag applies to the parent only.

Exception: **move**. Moving a parent always brings its subtasks along.
This is standard Frame behavior, not SELECT-specific.

To bulk-operate on subtasks, expand the parent and select them
explicitly.

---

## Undo

A bulk operation is a **single undo step**. Pressing `u` after
returning to NAVIGATE mode reverses the entire bulk action at once.

---

## Cross-Section Selection

Selection can span the Backlog and Parked sections. Operations apply
regardless of section. Tasks move between sections as their new state
dictates (e.g., `~` on a backlog task moves it to Parked).

---

## Scope

### Track view only

SELECT mode is available in **track view** only. `v` and `V` are
no-ops in inbox, recent, and tracks views. Those views have their
own interaction patterns.

### Selection clears on

- `Esc` in SELECT mode (explicit clear)
- Tab/view switch (different track or view type)

### Selection persists across

- All successful bulk operations (tag, state, dep, move)
- Cancelled sub-operations (Esc from MOVE, Esc from inline editor)
- Detail view drill-in (`Enter`) and return (`Esc` from detail)
- Scrolling
- Collapse/expand of other nodes

---

## Edge Cases

### Empty selection

If the last selected task is deselected with `v` (selection becomes
empty), exit SELECT mode automatically and return to NAVIGATE.

### Collapsed parent in range select

`V` range select operates on visible lines only. Collapsed subtasks
are not included. Expand the parent first to include them.

### Selected task modified by external change

If a file watcher reload changes a selected task's state or removes
it, remove it from the selection. If the selection becomes empty,
exit SELECT mode. Show the sync marker in the undo stack as usual.

### Very large selections

No artificial limit on selection size. The status row count and the
visual indicators scale naturally. Performance is bounded by the
ops layer (each task is a single mutation call).

---

## CLI Equivalent

No bulk CLI commands. The CLI operates on single tasks by ID. Use
shell loops for scripted bulk operations:

```bash
for id in EFF-014 EFF-016 EFF-018; do
  fr tag "$id" add ready
done
```

---

## Colors

Add to the configurable color set:

| Name             | Default     | Usage                          |
|------------------|-------------|--------------------------------|
| `selection_bg`   | `#0F1A3D`   | Background for selected rows   |

In `project.toml`:

```toml
[ui.colors]
selection_bg = "#0F1A3D"
```

---

## Implementation

### Files touched

| File | Action |
|------|--------|
| `tui/app.rs` | Add `selection: HashSet<TaskId>`, SELECT mode |
| `tui/input/navigate.rs` | `v`/`V`/`Cmd+A` handlers, selection-aware dispatch |
| `tui/input/move_mode.rs` | Group move logic for bulk selections |
| `tui/render/track_view.rs` | Selected row background + `▌` indicator |
| `tui/render/status_row.rs` | SELECT mode indicator, count, hints |
| `tui/undo.rs` | Batch undo for bulk operations |
| `model/config.rs` | `selection_bg` color config |

### Key implementation notes

1. **Selection is a `HashSet<TaskId>`** on the app state. SELECT mode
   is active when the set is non-empty. Mode dispatch checks this.

2. **Bulk actions reuse the ops layer.** Each bulk action loops over
   the selection and calls the same `ops/task_ops` functions as single
   actions. The only addition is undo batching.

3. **Undo batching.** Collect all inverse operations into a single
   `BulkOperation(Vec<Operation>)` variant on the undo stack. Undo
   replays the inverses in reverse order.

4. **Range select needs the visible item list.** `V` operates on
   rendered line order (accounting for collapse). The render pass
   already computes this — expose it for input handling.

5. **Stand-in row for bulk move.** On entering MOVE mode with a
   selection, remove selected tasks from the rendered list and insert
   a single stand-in row. Track original positions for cancel/undo.
   On confirm, expand stand-in into the placed tasks at that position.

6. **Multi-token `+`/`-` editor.** Shared by both tag and dep inline
   editors. The field tokenizes on spaces. Each token is parsed for
   an optional `+`/`-` prefix. Autocomplete matches against the
   current token (after prefix). No prefix defaults to `+`.

7. **Selection persists after operations.** Do not clear the HashSet
   after applying a bulk action. Only clear on explicit `Esc` or
   tab/view switch. This includes cancelled sub-operations (e.g.,
   `Esc` from MOVE mode returns to SELECT with selection intact).

### Task breakdown

```
B1  Add SELECT mode and selection state
    - HashSet<TaskId> on app state
    - v enters SELECT mode and toggles current task
    - V range select on visible items
    - Cmd+A select all visible
    - Esc clears selection, exits mode
    - Auto-exit when selection becomes empty
    - Auto-clear on tab/view switch
    - Preserve across detail drill-in/out

B2  Render selection visuals
    - Selected row background color (selection_bg)
    - ▌ bar in highlight color on left edge
    - Cursor distinct from selection
    - Cursor + selected row combined treatment
    - Add selection_bg to configurable colors

B3  SELECT mode status row
    - "-- SELECT --" in highlight color, left position
    - "N selected" right-justified, same position as search count
    - Key hints far right: x/b/o/~ t d m Esc

B4  Bulk state changes
    - x/b/o/~ dispatch to ops layer for each selected task
    - Batch into single BulkOperation undo step
    - Selection persists after apply

B5  Bulk tagging
    - t opens inline editor below cursor row with "tags:" label
    - Multi-token field: +tag adds, -tag removes, bare = add
    - Autocomplete on current token after prefix
    - Apply all additions/removals to all selected, skip no-ops
    - Single undo step, selection persists

B6  Bulk dependency edit
    - d opens inline editor below cursor row with "deps:" label
    - Multi-token field: +ID adds, -ID removes, bare = add
    - Autocomplete on current token after prefix
    - Apply to all selected, skip no-ops
    - Single undo step, selection persists

B7  Bulk move within track
    - m collapses selected into stand-in row
    - Stand-in shows "N tasks" in highlight color
    - ↑↓ moves stand-in, non-selected tasks reflow
    - 3-row context margin at viewport edges
    - Enter expands stand-in into placed tasks, selection persists
    - Esc restores original positions, selection persists
    - Single undo step

B8  Bulk move to track
    - M triggers track autocomplete at cursor row
    - Move all selected to target track
    - Reassign IDs, update deps
    - Single undo step
```
