# Filtering — Spec Additions

---

## TUI: Track View Filtering

Filtering narrows the visible task list in track view by hiding
non-matching tasks. Filters are fast toggles via the `f` prefix key,
composable (state + tag), and visually indicated in the tab separator
line. Navigation, search, and all task actions work normally on the
filtered set.

### Filter Keys (NAVIGATE mode, track view)

All filters use the `f` prefix followed by a second key. The `f` key
alone does nothing — it waits for the second keypress. If the second
key doesn't match a filter command, the sequence is ignored.

| Keys       | Action                                          |
|------------|-------------------------------------------------|
| `fa`       | Filter to active tasks only                     |
| `fo`       | Filter to open (todo) tasks only                |
| `fb`       | Filter to blocked tasks only                    |
| `fp`       | Filter to parked tasks only                     |
| `fr`       | Filter to ready tasks (todo/active, deps met)   |
| `ft`       | Filter by tag (autocomplete popup)              |
| `f Space`  | Clear state/ready filter (keep tag filter)      |
| `ff`       | Clear all filters                               |

### Filter Dimensions

Two independent filter dimensions that can be combined:

**State filter** — one of: active, todo, blocked, parked, ready. Only
one state filter can be active at a time. Setting a new state filter
replaces the previous one. `f Space` clears the state filter.

**Tag filter** — one tag at a time. `ft` opens the tag autocomplete
popup (same component used for tag editing). Select a tag to filter.
Pressing `ft` again replaces the tag filter with a new selection.
Tag filter is cleared by `ff` or by pressing `ft` and then `Esc`.

Combining: `fr` then `ft bug` shows ready tasks tagged `#bug`. State
and tag filters are independent — `f Space` clears only the state
filter, leaving the tag filter in place. `ff` clears everything.

### Ready Filter Semantics

`fr` applies the same logic as `fr ready` in the CLI:
- Task state is `todo` or `active`
- AND the task has no deps, or all deps are in state `done`

Subtasks are shown nested under qualifying parents. A parent with
some incomplete subtasks still appears if the parent itself qualifies
as ready (its own deps are met). This matches the CLI behavior.

### What Filtering Does to the Task List

Non-matching tasks are **hidden** — removed from the visible list
entirely, not dimmed or grayed out. The exception is **ancestor
context**: when a subtask matches the filter but its parent does not,
the parent is shown as a dimmed, non-selectable context row so the
hierarchy remains legible. Context rows cannot be selected, and
cursor movement skips over them.

Example: filter to blocked (`fb`) when only `EFF-014.2` is blocked:

```
 ○ EFF-014 Implement effect inference        ← dimmed context, not selectable
   ⊘ .2 Unify effect rows              cc    ← matches, selectable
```

Tasks that don't match and have no matching descendants are hidden
completely.

### Visual Indicator

When any filter is active, the filter state is displayed on the tab
separator line, right-aligned with the screen, with one space buffer
before the screen on the right:

```
 Effects │ Unique │ Infra │ ▸ │ *5 │ ✓ │
─────────┴────────┴───────┴───┴────┴───┴────────── filter: ready #bug
```

No filter active (default):
```
 Effects │ Unique │ Infra │ ▸ │ *5 │ ✓ │
─────────┴────────┴───────┴───┴────┴───┴───────────────────────────────
```

State filter only:
```
─────────┴────────┴───────┴───┴────┴───┴──────────── filter: blocked
```

Tag filter only:
```
─────────┴────────┴───────┴───┴────┴───┴────────────────── filter: #cc
```

"filter:" is rendered in dim text. State names and tag names are
rendered in their associated colors (e.g. blocked in red, `#bug` in
red, `#cc` in purple).

The filter indicator is **only shown in track view**. When switching
to inbox, recent, or tracks view, the indicator is hidden. It
reappears when returning to track view if a filter is still active.

### Empty State

If a filter matches no tasks, a message of "no matching tasks" is
shown right-justified on the line below the filter indicator, 
using the same styling as the zero-results search message:

```
 Effects │ Unique │ Infra │ ▸ │ *5 │ ✓ │
─────────┴────────┴───────┴───┴────┴───┴──────────── filter: blocked
                                                  no matching tasks
```

### Behavior Details

**Scope**: Track view only. Filter keys are ignored in inbox view,
recent view, and tracks view. The filter indicator is hidden in
non-track views.

**Global filter state**: One filter applies across all tracks. When
switching between track tabs, the same filter remains active. This
keeps mental state simple — "I'm looking at blocked tasks" applies
everywhere, not just one track.

**Filtered navigation**: `↑↓`/`jk` move through visible (matching)
tasks only. `g`/`G` jump to top/bottom of the filtered set. Context
rows (dimmed ancestors) are skipped by cursor movement.

**Search within filters**: `/` search operates on the filtered set.
Matches only highlight among visible tasks. `n`/`N` cycle through
visible matches.

**Task actions on filtered set**: All task actions work normally on
the selected (visible) task. If a state change causes a task to no
longer match the filter (e.g. marking an active task done while
filtered to active), the task disappears from view and the cursor
advances to the next visible task.

**Expand/collapse**: Subtree expand/collapse works within the filtered
set. If a parent matches but none of its children match, expanding
the parent shows nothing (the parent appears as a leaf).

**Persistence**: Filters are ephemeral. They are not persisted to
`.state.json` and reset on TUI restart. Filters are "I'm looking for
something right now" tools, not view configuration.

### Undo

Filters are view-only operations — they don't mutate data and don't
go on the undo stack. `ff` clears filters; there's no need to undo
a filter.

---

## Implementation Plan Additions

### Phase 4 additions

```
4.12 Track view filtering (tui/input/navigate.rs, tui/render/track_view.rs)
     - f prefix key: wait for second keypress, dispatch filter command
     - State filter: active, todo, blocked, parked, ready
     - Tag filter: reuse tag autocomplete component
     - Global filter state in app state (not per-track, not persisted)
     - track_view.rs: compute visible task set from filter
       - Hide non-matching tasks entirely
       - Show dimmed ancestor context for matching children
       - Context rows are non-selectable, skipped by cursor
     - tab_bar.rs: render filter indicator on separator line
       (track view only, hidden in other views)
     - Empty state: "no matching tasks" right-justified below indicator
     - Ready filter: reuse ready logic from ops layer
     - Cursor management when task leaves filtered set after state change
```

---

## Design Decisions Log Additions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| `#ready` tag | Removed | Readiness is structural (deps met + state); avoid confusion with ready filter |
| Filter interaction | `f` prefix key + second key | Fast, no mode switch, mnemonic |
| Filter dimensions | State (one) + tag (one), composable | Covers useful cases without complexity |
| Filter scope | Track view only | Other views have their own structure |
| Filter indicator | Tab separator line, right-aligned | Uses dead space; status row stays free |
| Filter indicator visibility | Track view only, hidden in other views | Don't show irrelevant chrome |
| Filter state | Global (all tracks share one filter) | One mental model; simpler than per-track |
| Non-matching tasks | Hidden, not dimmed | Clean filtered view; ancestors shown as dimmed context |
| Filter persistence | Session-only, not in `.state.json` | Ephemeral tool, not view config |
| Empty filter state | "no matching tasks" below indicator | Reuse search-style messaging; consistent placement |
