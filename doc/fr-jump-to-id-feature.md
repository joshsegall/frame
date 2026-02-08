# Feature: Jump to Task (`J`)

## Overview

Jump directly to any task by ID from any track view. Switches tracks
and expands collapsed parents as needed.

---

## Interaction

1. Press `J` in NAVIGATE mode (track view)
2. Status row shows a jump prompt with task ID autocomplete:

```
jump: EFF-01▊                                              Esc cancel
```

3. Autocomplete dropdown shows matching task IDs across all active
   tracks, filtered as you type. Matches are partial — typing `14`
   matches `EFF-014`, `INFRA-014`, etc. Typing `EFF-` narrows to
   that track's prefix.

4. `Enter` (or selecting from autocomplete) jumps to the task:
   - If the task is on the current track: cursor moves to it
   - If the task is on a different active track: switch to that tab,
     cursor lands on the task
   - If the task is a subtask: parent is expanded (recursively if
     needed), cursor lands on the subtask

5. `Esc` cancels and returns to the previous position and view.

---

## Autocomplete

Reuses the existing task ID autocomplete component. The candidate list
is drawn from all **active** tracks, not just the current view. Each
entry shows the full ID and title for context:

```
  EFF-014  Implement effect inference for closures
  EFF-014.1  Add effect variables to closure types
  EFF-014.2  Unify effect rows during inference
```

Subtask IDs are included. Done tasks within active tracks are included
(they may be in the Done section and not yet archived).

---

## Scope

### Available everywhere

`J` works from **any view** — track, inbox, recent, or tracks view.
It always jumps to a task in a track view. If invoked from a non-track
view, the jump switches to the target track's tab.

### Active tracks only

Autocomplete candidates are drawn from **active tracks only**. Shelved
and archived tracks are excluded to avoid loading potentially large
files that aren't part of the current workflow.

To find tasks in shelved or archived tracks, use `fr show <id>` in
the CLI, or activate/unshelve the track first.

### SELECT mode

Works from SELECT mode without clearing the selection, consistent
with detail drill-in behavior. Jump to check a dependency, come back,
continue operating on your selection.

---

## Key Binding

| Key | Action                                    |
|-----|-------------------------------------------|
| `J` | Open jump prompt (any view, NAVIGATE mode) |

Added to the **Navigation** key binding table in the main spec
(available globally, not just track view).

---

## Implementation

### Files touched

| File | Action |
|------|--------|
| `tui/input/navigate.rs` | Handle `J`, open jump prompt |
| `tui/render/status_row.rs` | Render jump prompt |
| `tui/render/autocomplete.rs` | Supply active-track ID candidates |
| `tui/app.rs` | Execute jump (track switch, expand, cursor) |

### Key notes

1. **Active-track candidate list.** The autocomplete component scopes
   candidates to active tracks only. No lazy-loading of shelved or
   archived track files. Add a `scope` parameter to the autocomplete
   source.

2. **Track switch on jump.** Reuse the existing tab-switch logic. Set
   active track, restore or initialize cursor/scroll state for that
   track, then override cursor to the target task. If jumping from a
   non-track view (inbox, recent, tracks), switch to track view first.

3. **Recursive expand.** If jumping to `EFF-014.2.1`, expand `EFF-014`
   then expand `EFF-014.2`. Walk up the parent chain, expanding each.

### Task breakdown

```
J1  Add J key handler and jump prompt
    - J opens single-line editor in status row with "jump:" label
    - Available in all views (track, inbox, recent, tracks)
    - Esc cancels, Enter confirms
    - Prompt styling matches search prompt

J2  Active-track autocomplete candidates
    - Add "all active tracks" scope to autocomplete component
    - Candidates: all task IDs + titles across active tracks
    - Include subtasks, include done tasks (within active tracks)

J3  Execute jump
    - Same-track: move cursor
    - Cross-track: switch tab, then move cursor
    - From non-track view: switch to track view first
    - Subtask: expand parent chain recursively
    - Preserve SELECT mode selection if active
```
