# Feature: Track Prefix Rename (`P`)

## Overview

Rename a track's ID prefix from the Tracks view. Updates all task IDs,
subtask IDs, and dependency references across the entire project. Uses
the existing CLI rename operation under the hood.

---

## Trigger

`P` (uppercase) on a track in the Tracks view, NAVIGATE mode.

Not available from track view or detail view — prefix changes are
heavyweight track management operations and belong in the Tracks view
alongside other track management keys.

---

## Flow

### Step 1: Edit prefix

`P` on a track opens an inline single-line editor on the prefix column
(or adjacent to the track name) with the current prefix pre-filled and
selected:

```
 ┌───────────────────────────────────────────────┐
 │                                               │
 │                  todo act blk done park        │
 │ Active          [ ] [>] [-]  [x]  [~]         │
 │  1  Effect System   8   3   1   12    2        │
 │     Prefix: [EFF]                              │
 │  2  Unique Types    5   1   0    3    0        │
 │  3  Compiler Infra  3   2   0    8    1  ★cc   │
 │                                               │
 └───────────────────────────────────────────────┘
```

Standard text editing keys. Input is auto-uppercased as you type —
press `e` and it appears as `E`. The prefix is validated on the fly:
- Must be alphanumeric (letters and numbers only, auto-uppercased)
- Must not duplicate an existing prefix
- Must not be empty

Invalid input shows a subtle inline error (e.g., dim red text below
the editor: "prefix already in use" or "uppercase letters and numbers
only").

`Enter` proceeds to confirmation. `Esc` cancels.

### Step 2: Confirmation

A small confirmation popup showing the blast radius:

```
 ┌─ Rename Prefix ──────────────────────────────┐
 │                                               │
 │  EFF → FX                                     │
 │                                               │
 │  This will rename:                            │
 │    23 task IDs in Effect System                │
 │     8 dep references across 3 other tracks    │
 │                                               │
 │  This cannot be undone. Use git to revert.    │
 │                                               │
 │                    Enter confirm  Esc cancel   │
 └───────────────────────────────────────────────┘
```

The counts are computed by scanning the project before executing:
- **Task IDs**: count of all tasks (including subtasks) in the target
  track that carry the old prefix
- **Dep references**: count of `dep:` entries across all *other* tracks
  that reference IDs with the old prefix, plus the number of tracks
  those deps span

The "cannot be undone" line is important — it sets expectations clearly.

`Enter` executes the rename. `Esc` cancels and returns to the prefix
editor (not all the way back to Tracks view, so the user can try a
different prefix without starting over).

### Step 3: Execute

The rename operation:
1. Updates `[ids.prefixes]` in `project.toml`
2. Renames all task IDs in the track file (top-level and subtasks)
3. Updates all dep references across all track files
4. Pushes a **sync marker** onto the undo stack (no undo)
5. Reloads the project state

The user returns to Tracks view. The track now shows the new prefix
in any visible context.

---

## Validation

| Rule | Behavior |
|------|----------|
| Empty prefix | Error: "prefix cannot be empty" |
| Lowercase input | Auto-uppercased (no error) |
| Non-alphanumeric chars | Error: "letters and numbers only" |
| Duplicate prefix | Error: "prefix already used by {track name}" |
| Same as current | No error, Enter is a no-op (no confirmation needed) |

Validation runs on every keystroke. The `Enter` key is inert while
validation fails — no need for an explicit error popup, the inline
message is enough.

**CLI and config normalization:** The CLI also silently uppercases
prefix input. If `project.toml` contains a lowercase prefix (hand-
edited), it is normalized to uppercase on the next write (`fr clean`
or any operation that touches the config).

---

## Undo

Prefix rename pushes a **sync marker** onto the undo stack. `u` will
not reverse it. The confirmation popup states "This cannot be undone.
Use git to revert." This matches the principle that rare, heavyweight
config changes use git as the undo mechanism rather than the in-memory
undo stack.

The sync marker also clears the redo stack, as with external file
changes.

---

## Edge Cases

### Track with no tasks

If the track has zero tasks, the rename is trivial — just update the
prefix in `project.toml`. The confirmation still shows:

```
  0 task IDs in Example Track
  0 dep references across other tracks
```

No speedbump skip — keep the flow consistent.

### Archive files

If the track has archived done tasks in `archive/<track-id>.md`, those
task IDs must also be renamed. The blast radius count should include
archived tasks.

### Subtask IDs

Subtask IDs like `EFF-014.1`, `EFF-014.2.1` all carry the parent's
prefix. The rename updates them all. The count includes subtasks.

---

## Implementation

### Files touched

| File | Action |
|------|--------|
| `tui/input/tracks_view.rs` | Handle `P` key, launch prefix editor |
| `tui/render/tracks_view.rs` | Render inline prefix editor |
| `tui/render/prefix_confirm.rs` | New — confirmation popup with blast radius |
| `tui/app.rs` | Add prefix rename state (editing, confirming, new value) |
| `ops/track_ops.rs` | Blast radius computation (count affected IDs and deps) |

The actual rename execution reuses the existing CLI rename operation
in the ops layer.

### Task breakdown

```
P1  Blast radius computation
    - fn prefix_rename_impact(project, track_id, new_prefix)
      -> { task_count, dep_ref_count, affected_track_count }
    - Count all task/subtask IDs with old prefix in target track
    - Count all dep references to old-prefix IDs across other tracks
    - Include archived tasks in count
    - Unit tests: no tasks, subtasks, cross-track deps, archives

P2  Inline prefix editor in Tracks view
    - P key on selected track opens single-line editor
    - Pre-filled with current prefix, text selected
    - Live validation: uppercase alphanumeric, no duplicates, non-empty
    - Inline error display below editor when invalid
    - Enter proceeds to confirmation (only when valid)
    - Esc cancels back to Tracks view

P3  Confirmation popup
    - Floating popup showing old → new prefix
    - Blast radius counts from P1
    - "Cannot be undone" warning
    - Enter confirms, Esc returns to editor
    - Render in standard popup style (border, centered)

P4  Wire up execution
    - On confirm: call existing rename operation from ops layer
    - Push sync marker to undo stack
    - Reload project state
    - Return to Tracks view with updated prefix
```
