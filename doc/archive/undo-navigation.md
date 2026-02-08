# Undo Navigation — Design Addendum

Supplements the Undo Model in `frame-implementation-plan.md` §4.

## Problem

The undo stack is global — all operations across all views go on one
stack. If the user edits a task's note in detail view, returns to
track view, and presses `u`, the note edit is undone but the user
can't see what changed. This is confusing and feels like nothing
happened (or worse, like something broke silently).

## Solution: Undo Navigates to Context

**Single global stack, but every undo/redo navigates the UI to show
the affected item.**

Each `Operation` on the undo stack must store a **navigation target**:

```rust
struct UndoNavigation {
    track_id: String,         // which track the task lives in
    task_id: String,           // which task was affected
    open_detail: bool,         // whether to open detail view
    detail_region: Option<DetailRegion>,  // which region to highlight
}
```

## Behavior

When `u` is pressed:

1. Pop the operation, apply the inverse (as before).
2. Read the operation's navigation target.
3. Switch to the target track tab (if not already there).
4. Move the cursor to the target task.
5. If `open_detail` is true, open the detail view and scroll to
   the relevant region.
6. Briefly flash/highlight the affected area (200-300ms) so the
   user sees what changed.

Redo (`Ctrl+Y`) follows the same navigation logic.

## When to set `open_detail: true`

Only for operations that affect task internals not visible from the
track view:

- Note edits → open detail, region = Note
- Dep add/remove → open detail, region = Deps
- Ref/spec changes → open detail, region = Refs
- Tag changes → **false** (tags are visible in track view)
- State changes → **false**
- Move operations → **false**
- Add/remove task → **false**
- Title edits → **false** (title visible in track view)

## Edge Cases

### 1. Task no longer exists
The task was added, then undo removes it. Navigation target is gone.
**→ Navigate to the track, place cursor at the position where the
task was (or nearest existing task). Do not open detail view.**

### 2. Task moved to a different track since the operation
The undo itself may move it back (e.g., undoing a cross-track move).
**→ Navigate to wherever the task ends up after the undo is applied.**

### 3. User is in EDIT mode when pressing `u`
`u` in EDIT mode is a regular character insert, not undo. `Cmd+Z` in
EDIT mode is text-level undo within the editor. **→ No change needed.
Operation-level undo only fires in NAVIGATE mode, which means the
user has already exited the detail view.**

### 4. Undo of an inbox operation
Inbox items don't have task IDs. Store the inbox index instead.
**→ Switch to inbox view, place cursor at the affected index.**

### 5. Undo of a track management operation
E.g., undoing `track shelve`. **→ Switch to tracks view, cursor on
the affected track.**

### 6. Multiple rapid undos
Each undo navigates. If the user holds `u` and undoes 5 operations
across 3 tracks, the view jumps with each one. This is correct —
it shows the user what's being undone. The flash duration should be
short enough (200ms) that rapid undos don't feel sluggish.

### 7. Undo at sync marker
No change — undo is blocked at sync markers as before. No navigation
occurs.

## Implementation Notes

- `UndoNavigation` is computed at mutation time, same as the inverse
  operation. It captures the state *after* the forward operation
  (i.e., where the task is now), because that's where we need to
  navigate when undoing.
- The flash/highlight is cosmetic polish — implement navigation first,
  add the flash later. A simple approach: set a `flash_target: Option<(String, Instant)>`
  on the app state, clear it after the duration, and render the
  target with the highlight color while it's set.
- Navigation should not push anything onto the undo stack — it's a
  side effect of undo, not an operation.
