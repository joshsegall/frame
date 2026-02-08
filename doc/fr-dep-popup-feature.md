# Feature: Dep Popup (`D`)

## Overview

A floating overlay showing a task's dependency relationships in both
directions: what blocks it (upstream) and what it blocks (downstream).
Triggered by `D` on any task from track view or detail view.

---

## Trigger

`D` (uppercase) on the cursor task. Works in:
- Track view
- Detail view

If the task has no deps and nothing depends on it, the popup still opens
with both sections showing `(nothing)`.

---

## Layout

```
 ┌─ Dependencies: EFF-012 ──────────────────────┐
 │                                               │
 │  Blocked by                                   │
 │  ▾ [>] EFF-014  Effect inference    Effects   │
 │      [x] EFF-003  Handler desugar   Effects ✓ │
 │    [ ] INFRA-003  Span tracking     Infra     │
 │                                               │
 │  Blocking                                     │
 │    (nothing)                                  │
 │                                               │
 │                    ←→ expand   Esc close       │
 └───────────────────────────────────────────────┘
```

For a task that others depend on:

```
 ┌─ Dependencies: EFF-014 ──────────────────────┐
 │                                               │
 │  Blocked by                                   │
 │    [x] EFF-003  Handler desugaring  Effects ✓ │
 │                                               │
 │  Blocking                                     │
 │    [-] EFF-012  Effect-aware DCE    Effects   │
 │    [ ] EFF-015  Handler opt pass    Effects   │
 │    [ ] UNQ-008  Unique effect refs  Unique    │
 │                                               │
 │                    ←→ expand   Esc close       │
 └───────────────────────────────────────────────┘
```

### Sections

- **Blocked by** — tasks this task lists in its `dep:` metadata
  (upstream). Must be done before this task is ready.
- **Blocking** — tasks across all tracks that list this task in their
  `dep:` metadata (downstream). Finishing this task unblocks them.

Either section shows `(nothing)` if empty.

### Entry format

Each entry shows:
- Expand/collapse indicator (▸/▾) if the entry has further deps or
  blocking relationships to explore. No indicator for leaf nodes.
- Checkbox state in standard colors (`[ ]`, `[>]`, `[-]`, `[x]`, `[~]`)
- Task ID
- Title (truncated to fit popup width)
- Track name (right-aligned, dimmed if same track as root task)
- ✓ suffix for done deps

---

## Expand / Collapse

Both sections support expand/collapse using the standard `←`/`→`
(`h`/`l`) pattern. Expanding a dep in "Blocked by" shows what that
dep itself depends on. Expanding a dep in "Blocking" shows what that
downstream task is also blocking.

### Auto-expand on open

- **1–2 direct entries** in a section → auto-expanded one level
- **3+ direct entries** → all start collapsed

This means the common case (one or two blockers) immediately shows you
the full chain. Complex cases start compact for scanning first.

### Cycle detection

Circular dependencies show as:

```
  ▾ [>] EFF-014  Effect inference    Effects
      ↻ EFF-012  (circular)
```

Dimmed, not expandable. Serves as an inline diagnostic — if you see
circular deps, something needs fixing.

### Dangling refs

Missing task IDs show as:

```
  [?] MOD-099  (not found)
```

`[?]` in the red color, not expandable.

---

## Navigation

| Key              | Action                                    |
|------------------|-------------------------------------------|
| `↑` / `k`       | Move cursor up through entries            |
| `↓` / `j`       | Move cursor down through entries          |
| `→` / `l`       | Expand selected entry                     |
| `←` / `h`       | Collapse selected (or move to parent)     |
| `Enter`          | Jump to task (closes popup, cross-track)  |
| `Esc`            | Close popup                               |

Cursor moves freely between both sections. `Enter` closes the popup and
navigates to the selected task, switching tracks if needed.

---

## Sizing

- **Width**: 60% of terminal width, minimum 40 columns, maximum 80
- **Height**: content-sized up to 70% of terminal height; scrolls
  internally if content overflows
- **Position**: centered horizontally, vertically centered or slightly
  above center

---

## Implementation

### Inverse dependency index

The "Blocking" section requires scanning all tasks across all tracks to
find which ones reference the current task in their `dep:` list. This
should be built as an index at project load time:

```
HashMap<TaskId, Vec<TaskId>>
```

Maps each task ID to the list of tasks that depend on it. Rebuilt on
file reload (external changes, CLI writes, etc.).

### Files touched

| File | Action |
|------|--------|
| `tui/render/dep_popup.rs` | New — popup rendering and layout |
| `tui/input/navigate.rs` | Handle `D` key in track view |
| `tui/input/detail.rs` | Handle `D` key in detail view (if separate) |
| `tui/app.rs` | Add popup state (open/closed, root task, expand state, cursor) |
| `ops/` or `model/project.rs` | Inverse dep index construction and rebuild |

### Task breakdown

```
D1  Build inverse dependency index
    - fn build_dep_index(project) -> HashMap<TaskId, Vec<TaskId>>
    - Scan all tasks across all tracks, collect reverse mappings
    - Rebuild on project reload
    - Unit tests: basic deps, cross-track deps, no deps, circular deps

D2  Implement dep popup rendering
    - Floating popup with border, title "Dependencies: <ID>"
    - Two sections with "Blocked by" / "Blocking" headers
    - Entry rendering: state, ID, title (truncated), track name
    - Expand/collapse indicators (▸/▾) for entries with children
    - Auto-expand logic: 1–2 entries expanded, 3+ collapsed
    - Cycle detection (↻ marker), dangling ref display ([?])
    - Done deps dimmed with ✓ suffix
    - Content scrolling when overflow
    - Sizing: 60% width (40–80), content-height up to 70%
    - Hint bar at bottom: "←→ expand   Esc close"

D3  Implement dep popup navigation
    - Cursor movement (↑↓/jk) across entries in both sections
    - Expand/collapse (←→/hl) with recursive depth
    - Enter: resolve target task ID, close popup, navigate to task
      (switch track tab if cross-track, set cursor to task)
    - Esc: close popup, return to previous view

D4  Wire up D key binding
    - Track view NAVIGATE mode: D opens dep popup for cursor task
    - Detail view NAVIGATE mode: D opens dep popup for focused task
    - Popup state in app.rs (open flag, root task ID, expand map, cursor)
    - Key events routed to popup when open (popup captures input)
```

### Notes

- The popup is **read-only** — no mutations, no undo considerations.
- Expand/collapse state is **ephemeral** — not persisted. Auto-expand
  runs fresh each time the popup opens.
- Track names use dim color when same as root task's track, normal
  text color when cross-track, to draw attention to cross-track deps.
