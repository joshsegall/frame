# Feature: Repeat Last Action (`.`)

## Overview

Press `.` to repeat the last mutation on the cursor's current task.
Follows the vim convention: `.` replays the last change, cursor
movement is independent.

---

## Repeatable Actions

The following actions are recorded as the "last action" and can be
replayed with `.`:

| Action | What `.` repeats |
|--------|------------------|
| `Space` (cycle state) | Cycle state on current task |
| `x` (mark done) | Mark current task done |
| `b` (toggle blocked) | Set blocked on current task |
| `o` (set todo) | Set todo on current task |
| `~` (toggle parked) | Park current task |
| `t` (tag edit) | Apply same tag additions/removals |
| `d` (dep edit) | Apply same dep additions/removals |
| `e` (edit title) | Enter EDIT mode on current task's title |
| `@`/`d`/`n` (quick edit) | Enter EDIT mode on same region |

For tag and dep edits, the entire expression is stored. If you typed
`+cc +ready -design`, pressing `.` applies that same set of changes
to the next task.

For edit-mode entries (`e`, `@`, `n`), `.` reopens the editor
on the same region — it does not replay the typed text. The value of
repeating is skipping the "which region" decision, not automating
text input.

---

## Non-Repeatable Actions

These are **not** recorded and do not affect what `.` replays:

- Navigation (cursor movement, tab switching, expand/collapse)
- Add task (`a`, `o`, `p`, `A`)
- Move within track (`m`) or to track (`M`)
- Search (`/`) and jump (`J`)
- Undo/redo (`u`, `Ctrl+Y`)
- Triage (`Enter` in inbox)
- View switches (`i`, `r`, `0`)

`.` after a non-repeatable action replays whatever the last
repeatable action was.

---

## Cursor Independence

`.` acts on the cursor's current task and does **not** advance the
cursor. To repeat an action down a list:

```
x       ← mark done
j       ← move down
.       ← mark done
j       ← move down
.       ← mark done
```

This matches vim's model. Cursor movement and action replay are
separate concerns.

---

## SELECT Mode

`.` works in SELECT mode. It replays the last action on all selected
tasks, same as pressing the original key would.

If the last action was itself a bulk operation (e.g., bulk `+ready`),
`.` applies the same operation to the current selection. If there is
no selection, `.` applies to the cursor's task.

---

## Edge Cases

### No previous action

`.` with no recorded action is a no-op. No error, no feedback.

### Action becomes invalid

If the last action references something that no longer applies (e.g.,
removing a tag the current task doesn't have, or adding a dep that
already exists), the operation is silently skipped for that task —
same behavior as the original action.

### Undo interaction

A `.` replay is its own undo step. Pressing `u` after `.` undoes
the replayed action, not the original. Each `.` press creates an
independent undo entry.

### Cross-view

`.` is available in track view only. The stored action persists
across tab switches — repeat a tag change on one track, switch
tracks, `.` applies the same change on the new track.

---

## Implementation

### Storage

A single `last_action: Option<RepeatableAction>` on the app state.
Updated whenever a repeatable action completes successfully.

```rust
enum RepeatableAction {
    SetState(TaskState),
    CycleState,
    TagEdit(Vec<TagOp>),        // +cc, -design, etc.
    DepEdit(Vec<DepOp>),        // +EFF-014, -EFF-003, etc.
    EnterEdit(EditRegion),      // Title, Tags, Deps, Refs, Note
}
```

### Files touched

| File | Action |
|------|--------|
| `tui/app.rs` | Add `last_action` field |
| `tui/input/navigate.rs` | Record repeatable actions, handle `.` |
| `tui/undo.rs` | `.` creates independent undo step |

### Task breakdown

```
R1  Add RepeatableAction enum and storage
    - Define enum variants for each repeatable action type
    - Add Option<RepeatableAction> to app state
    - Record on successful completion of repeatable actions

R2  Implement . key handler
    - Dispatch stored action to current task (or selection)
    - Create independent undo step
    - No-op if no stored action
    - Silently skip invalid applications

R3  Ensure correct recording across contexts
    - Bulk operations record the action (not "bulk" itself)
    - Edit-mode entries record the region, not the text
    - Non-repeatable actions don't overwrite stored action
    - Action persists across tab switches
```
