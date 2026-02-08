# Cross-Track Move — Spec Additions

---

## TUI: Cross-Track Move (`M`)

`M` (uppercase) moves the selected task to a different track. Available
in both track view and detail view. Lowercase `m` remains local
reorder; uppercase `M` is cross-track move.

### Flow

The flow is identical to inbox triage: press `M`, and the same
autocomplete popup appears below the task row to select a target track.
After track selection, the same position dialog appears (top, bottom,
cancel). The current track is excluded from the track list. All
non-archived tracks are available as targets, including shelved tracks.

In detail view, the autocomplete popup appears below the title line
for visual consistency with the track view placement.

After confirming, the task is removed from the source track and
inserted in the target track. The task receives a new ID with the
target track's prefix (e.g. `EFF-014` → `INF-019`). All `dep:`
references to the old ID are updated across all tracks.

`Esc` at any step cancels and returns to normal navigation.

### Behavior details

**Subtasks**: Moving a parent task moves its entire subtree. All
subtask IDs are renumbered with the new prefix recursively
(`EFF-014.2` → `INF-019.2`, `EFF-014.2.1` → `INF-019.2.1`).

**Moving a subtask**: Moving a subtask out of its parent promotes it
to a top-level task in the target track. It receives a new top-level
ID (not a dotted ID). Its children (if any) are renumbered as children
of the new top-level ID.

**Cursor after move**: In track view, the cursor advances to the next
task in the source track (or previous if the moved task was last). In
detail view, the view closes and returns to the track view with the
cursor on the next task.

**Tags, metadata, notes**: Everything carries over unchanged. Only
the ID changes. `dep:`, `ref:`, `spec:`, `note:`, `added:` are all
preserved. The `added:` date is not reset — the task isn't new, it
just moved.

**Dep updates**: Any task in any track that has a `dep:` referencing
the old ID gets updated to the new ID. This is the same logic used
by `fr mv --track` in the CLI.

### Undo

One undo step. The operation stores:
- Source track, original position, original ID
- Target track, new position, new ID
- List of dep references that were updated (old ID → new ID)

Undo moves the task back to its original position in the source track,
restores the original ID (and subtask IDs), and reverts all dep
reference updates. This is a multi-file write but the inverse is
fully deterministic.

---

## Key Binding Additions

### Task Actions (NAVIGATE mode, track view)

Add to the existing table:

| Key | Action                                           |
|-----|--------------------------------------------------|
| `M` | Move task to another track (cross-track move)    |

### Detail View (NAVIGATE mode)

Add to the existing table:

| Key | Action                                           |
|-----|--------------------------------------------------|
| `M` | Move task to another track (cross-track move)    |

---

## Implementation Plan Additions

### Phase 2 additions

The cross-track move logic already exists in the plan as `move_task_to_track`
in `ops/task_ops.rs` (task 2.4). No new ops code needed — the TUI flow
calls the same operation.

### Phase 5 additions

```
5.8  Cross-track move flow (tui/input/navigate.rs)
     - M: enter cross-track move flow
     - Reuse triage autocomplete and position dialog components
     - Autocomplete positioned below task row (track view) or below
       title line (detail view)
     - Target list: all non-archived tracks except current
     - Execute via move_task_to_track from ops layer
     - Cursor management after move
```

### Undo stack additions (Phase 5.5)

New `Operation` variant:

```
TaskCrossTrackMove {
    task_id_old, task_id_new,
    source_track, source_position,
    target_track, target_position,
    dep_updates: Vec<(track_id, task_id, old_dep, new_dep)>
}
  → undo: move task back, restore old ID + subtask IDs, revert dep updates
```

---

## Design Decisions Log Additions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Cross-track move key | `M` (uppercase) | Heavyweight sibling of `m` (local reorder); distinct action, distinct key |
| Cross-track move flow | Same as triage (autocomplete + position dialog) | One interaction pattern to learn; reuses existing components |
| Cross-track move targets | All non-archived tracks including shelved | Task may belong to a paused work stream |
| Cross-track move in detail view | Supported, popup under title line | Natural to decide "this belongs elsewhere" while reading details |
