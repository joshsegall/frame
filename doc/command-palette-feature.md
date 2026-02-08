# Feature: Command Palette

## Overview

A fuzzy-searchable action launcher accessible from any NAVIGATE context.
The palette surfaces all available actions for the current view, shows
their keybindings, and executes them on selection. It teaches keybindings
while providing an alternative invocation path.

---

## Trigger

`>` in NAVIGATE mode (any view). Not available from EDIT, MOVE, SEARCH,
or SELECT modes.

`>` is Frame's identity character — the play triangle in the `[>]` logo.
Pressing it means "go, execute, do something." It pairs naturally with
`?` (help) on adjacent keys (Shift+. and Shift+/): `?` for "what can I
do?" (passive reference), `>` for "let me do it" (active launcher).

`:` is deliberately reserved for a potential future command-line mode
(vim ex-style typed commands).

---

## Interaction Model

### Opening

Press `>` in NAVIGATE mode. A floating overlay appears in the content
area. The overlay consists of:

1. **Input line** — top of the overlay, with `>` prompt in highlight
   color and a text cursor. The user types here to filter.
2. **Separator** — a horizontal rule below the input line.
3. **Results list** — below the separator, showing matching actions.
   Max 10 visible entries, scrollable if more match.
4. **Footer** — result count, separated from results by a blank line.

The overlay is rendered on top of the content area. No background
dimming (consistent with the help overlay behavior).

### Layout

```
  ┌──────────────────────────────────────────┐
  │ > mark d▌                                │
  │──────────────────────────────────────────│
  │ ▸ Mark done                            x │
  │   Mark done (#wontdo)                    │
  │   Mark done (#duplicate)                 │
  │   Edit dependencies                    d │
  │                                          │
  │   4 of 36 actions                        │
  └──────────────────────────────────────────┘
```

### Filtering

Fuzzy substring matching. Typing "mv" matches "Move task", "Move to
track". Typing "done" matches "Mark done", "Mark done (#wontdo)".
Matched characters are rendered in the highlight color within the
result text.

Matching is case-insensitive. The algorithm scores by:
1. Prefix match (query matches start of a word) — highest
2. Consecutive character match — high
3. Scattered character match — lower

The list re-sorts by score on every keystroke. When the input is empty,
actions are shown in their default order (grouped by category, most
common first).

### Navigation

| Key        | Action                                           |
|------------|--------------------------------------------------|
| typing     | Filter results                                   |
| `↑` / `↓` | Move selection in results list                   |
| `Enter`    | Execute selected action                          |
| `Esc`      | Close palette, return to NAVIGATE                |
| `Backspace`| Delete character (closes if input empty after `>`) |

The first result is auto-selected. Arrow keys move the highlight.
Typing always goes to the input — there's no separate "focus" between
input and list. The input always has focus, arrows just move the
selection indicator.

### Execution

When the user presses `Enter`:

1. The palette closes immediately.
2. The selected action fires.
3. If the action requires follow-up input (e.g., "Move to track" needs
   a track selection), it **chains into the existing UI flow** for that
   action. The user experiences the same interaction as if they'd pressed
   the direct keybinding.

Example flows:

- Select "Mark done" → palette closes, task marked done. Same as `x`.
- Select "Move task" → palette closes, enters MOVE mode. Same as `m`.
- Select "Move to track" → palette closes, enters track selection flow.
  Same as `M`.
- Select "Edit tags" → palette closes, enters EDIT mode on tags with
  autocomplete. Same as `t`.
- Select "Add subtask" → palette closes, enters EDIT mode for new
  subtask title. Same as `A`.
- Select "Switch to Inbox" → palette closes, switches to inbox view.
  Same as `i`.
- Select "Filter: active only" → palette closes, applies filter.
  Same as `fa`.

The palette is purely a launcher. It translates a selection into the
same action dispatch that a direct keybinding would trigger.

---

## Mode

The command palette introduces a new mode: **COMMAND**.

The feature is called "command palette" in documentation and conversation,
but the mode indicator is `-- COMMAND --` — shorter and more natural
alongside EDIT, MOVE, SEARCH.

### Modes Summary (updated)

| Mode       | Indicator          | How to enter  | How to exit              |
|------------|--------------------|---------------|--------------------------|
| NAVIGATE   | (none)             | Default       | —                        |
| EDIT       | `-- EDIT --`       | `e`/`Enter`   | `Esc` or `Enter`         |
| MOVE       | `-- MOVE --`       | `m`           | `Enter` or `Esc`         |
| SEARCH     | `/pattern▌`        | `/`           | `Enter` or `Esc`         |
| SELECT     | `-- SELECT --`     | `v`           | `Esc` or action          |
| COMMAND    | `-- COMMAND --`    | `>`           | `Enter` (exec) or `Esc`  |

### Status Row

```
-- COMMAND --                                        ↑↓ navigate  Enter ✓  Esc ✗
```

The input lives in the overlay, not the status row. This avoids
redundancy and is consistent with how the help overlay doesn't use the
status row.

### Input Handling

COMMAND mode captures all keyboard input:

- Printable characters → append to filter input
- `Backspace` → delete from filter (if empty after `>`, close palette)
- `↑`/`↓` → move selection
- `Enter` → execute selected action, exit COMMAND mode
- `Esc` → close palette, return to NAVIGATE

`h`/`j`/`k`/`l` type into the filter (they're printable characters),
not navigation. This is consistent with SEARCH mode where `/` captures
all input for the pattern.

---

## Relationship to `?` Help Overlay

The `?` help overlay remains as a **static reference**. The palette is
the **interactive** counterpart. They serve different needs:

- `?` is for scanning — "let me see all the keys at a glance."
- `>` is for doing — "I know what I want, let me find and execute it."

No changes to the `?` overlay are needed.

---

## Action Registry

Actions are registered centrally with metadata:

```rust
struct PaletteAction {
    id: &'static str,
    label: &'static str,
    shortcut: Option<&'static str>,
    contexts: &'static [ViewContext],
    category: ActionCategory,
}
```

### Contexts

```rust
enum ViewContext {
    TrackView,
    DetailView,
    InboxView,
    RecentView,
    TracksView,
    Global,          // Available in all views
}
```

An action appears in the palette only if its `contexts` list includes
the current view. `Global` actions always appear.

### Categories

For default ordering when no filter text is entered:

```rust
enum ActionCategory {
    State,       // State changes
    Create,      // Add task, subtask, inbox item
    Edit,        // Edit title, tags, deps, note, ref
    Move,        // Move, reorder
    Filter,      // Filter commands
    Select,      // Select mode operations
    Navigate,    // Switch views, open detail, back
    Search,      // Search, jump-to-task
    Manage,      // Track management
    System,      // Undo, redo, quit
}
```

Within each category, actions are ordered by expected frequency.

---

## Full Action List

### Global (all views)

| Label                    | Shortcut    | Category  |
|--------------------------|-------------|-----------|
| Switch to track: {name}  | `1`–`9`    | Navigate  |
| Next track               | `Tab`       | Navigate  |
| Open Inbox               | `i`         | Navigate  |
| Open Recent              | `r`         | Navigate  |
| Open Tracks              | `0`         | Navigate  |
| Search                   | `/`         | Search    |
| Jump to task by ID       | `J`         | Search    |
| Toggle help              | `?`         | Navigate  |
| Undo                     | `z` / `u`   | System    |
| Redo                     | `Z`         | System    |
| Quit                     | `QQ`        | System    |

"Switch to track: {name}" entries are generated dynamically — one per
active track. They filter naturally: typing "eff" matches "Switch to
track: Effect System".

Note: `QQ` is a two-key sequence. The palette executes quit directly
on selection — the palette selection itself serves as the confirmation
gate.

### Track View

| Label                           | Shortcut  | Category |
|---------------------------------|-----------|----------|
| Cycle state                     | `Space`   | State    |
| Set todo                        | `o`       | State    |
| Mark done                       | `x`       | State    |
| Set blocked                     | `b`       | State    |
| Set parked                      | `~`       | State    |
| Toggle cc tag                   | `c`       | State    |
| Mark done (#wontdo)             |           | State    |
| Mark done (#duplicate)          |           | State    |
| Add task (bottom)               | `a`       | Create   |
| Insert after cursor             | `-`       | Create   |
| Push to top                     | `p`       | Create   |
| Add subtask                     | `A`       | Create   |
| Edit title                      | `e`       | Edit     |
| Edit tags                       | `t`       | Edit     |
| Move task                       | `m`       | Move     |
| Move to track                   | `M`       | Move     |
| Move to top                     |           | Move     |
| Move to bottom                  |           | Move     |
| Filter: active only             | `fa`      | Filter   |
| Filter: todo only               | `fo`      | Filter   |
| Filter: blocked only            | `fb`      | Filter   |
| Filter: ready (deps met)        | `fr`      | Filter   |
| Filter: by tag                  | `ft`      | Filter   |
| Clear state filter              | `f Space` | Filter   |
| Clear all filters               | `ff`      | Filter   |
| Toggle select                   | `v`       | Select   |
| Range select                    | `V`       | Select   |
| Select all                      | `Ctrl+A`  | Select   |
| Select none                     | `N`       | Select   |
| Open detail                     | `Enter`   | Navigate |
| Collapse all                    |           | Navigate |
| Expand all                      |           | Navigate |
| Set cc-focus                    | `C`       | Manage   |
| Repeat last action              | `.`       | System   |

**Compound actions** ("Mark done (#wontdo)", "Mark done (#duplicate)")
add the tag and set done in one step. No direct keybinding — they exist
only in the palette.

**"Move to top"** and **"Move to bottom"** skip MOVE mode and execute
directly.

**Filter actions** chain appropriately: "Filter: by tag" opens the tag
filter input (same as `ft`).

**Select actions** that enter SELECT mode transition to SELECT after
the palette closes.

### Detail View

| Label                    | Shortcut      | Category |
|--------------------------|---------------|----------|
| Cycle state              | `Space`       | State    |
| Mark done                | `x`           | State    |
| Set blocked              | `b`           | State    |
| Set parked               | `~`           | State    |
| Edit region              | `e` / `Enter` | Edit     |
| Edit tags                | `t`           | Edit     |
| Edit refs                | `@`           | Edit     |
| Edit dependencies        | `d`           | Edit     |
| Edit note                | `n`           | Edit     |
| Move to track            | `M`           | Move     |
| Jump to task by ID       | `J`           | Search   |
| Back to track            | `Esc`         | Navigate |

### Inbox View

| Label                    | Shortcut | Category |
|--------------------------|----------|----------|
| Add item (bottom)        | `a`      | Create   |
| Insert after cursor      | `-`      | Create   |
| Edit title               | `e`      | Edit     |
| Edit tags                | `t`      | Edit     |
| Delete item              | `x`      | State    |
| Begin triage             | `Enter`  | Move     |
| Move item                | `m`      | Move     |

### Recent View

| Label                    | Shortcut      | Category |
|--------------------------|---------------|----------|
| Reopen as todo           | `Space`       | State    |
| Toggle expand            | `Enter`       | Navigate |

### Tracks View

| Label                    | Shortcut | Category |
|--------------------------|----------|----------|
| Open track               | `Enter`  | Navigate |
| Add new track            | `a`      | Create   |
| Edit track name          | `e`      | Edit     |
| Shelve / activate        | `s`      | Manage   |
| Archive / delete         | `D`      | Manage   |
| Reorder track            | `m`      | Move     |
| Set cc-focus             | `C`      | Manage   |

---

## Rendering

### Color Intent

The implementer should use Frame's existing color palette and match
these intentions. Do not hardcode specific hex values — follow the
established semantic color assignments.

| Element                    | Intent                                 |
|----------------------------|----------------------------------------|
| Border                     | Structural / deemphasized              |
| `>` prompt                 | Emphasis — highlight/accent color      |
| Filter text + cursor       | Primary readable text (bright)         |
| Selected row indicator `▸` | Emphasis — highlight/accent color      |
| Selected row text          | Primary readable text (bright)         |
| Unselected row text        | Standard text color                    |
| Shortcut column            | Secondary/structural — deemphasized    |
| Matched chars in results   | Emphasis — highlight/accent color      |
| Footer (result count)      | Secondary/structural — deemphasized    |
| "No matching actions" msg  | Standard text color (legible, not dim) |

### Overlay Structure

```
┌──────────────────────────────────────────┐
│ > filter text▌                           │
│──────────────────────────────────────────│
│ ▸ Cycle state                      Space │
│   Mark done                            x │
│   Set blocked                          b │
│   Set parked                           ~ │
│   Add task (bottom)                    a │
│   Insert after cursor                  - │
│   Edit title                           e │
│   Edit tags                            t │
│                                          │
│   8 of 36 actions                        │
└──────────────────────────────────────────┘
```

- Top separator (between input and results): horizontal rule — keeps.
- Bottom separator (above footer): blank line — lighter visual weight.
- No background dimming (consistent with help overlay).

### Sizing

- **Width**: content area width minus 4 (2 chars padding each side).
  Max 60 characters inner width — palette doesn't stretch endlessly on
  wide terminals.
- **Height**: input + separator + up to 10 results + blank line +
  footer = max 14 rows. Shrinks to fit fewer results. Minimum: input +
  separator + 1 result + blank line + footer = 5 rows.
- **Position**: horizontally centered in content area. Vertically:
  top edge at row 3 of the content area.

### Empty State

```
┌──────────────────────────────────────────┐
│ > xyzzy▌                                 │
│──────────────────────────────────────────│
│                                          │
│            No matching actions           │
│                                          │
│                                          │
│   0 of 36 actions                        │
└──────────────────────────────────────────┘
```

"No matching actions" in standard text color, centered.

---

## Fuzzy Matching Algorithm

Simple scored fuzzy match. The action list is small (< 50 items) so
performance is irrelevant.

```
fn fuzzy_score(query: &str, target: &str) -> Option<(i32, Vec<usize>)>
```

Returns `None` if no match, or `Some((score, matched_indices))`.

1. Both query and target are lowercased for comparison.
2. Walk through query characters. For each, find the next occurrence in
   target after the previous match position. If any character can't be
   found, return `None`.
3. Score bonuses:
   - +10 for each character matching at start of a word (after space,
     hyphen, or at position 0)
   - +5 for consecutive matched characters
   - +3 for each match in the first half of the target
   - -1 for each gap character between matches
4. Sort by score descending, then alphabetically as tiebreaker.

The `matched_indices` vector drives highlight rendering — each matched
position in the target string gets the highlight color.

**Shortcut matching**: The fuzzy matcher runs against the combined string
`label + " " + shortcut`, not just the label. This way typing "x" finds
"Mark done" (via the `x` shortcut), and typing "fa" finds "Filter:
active only" (via both the label and the `fa` shortcut). Matched
characters are highlighted wherever they fall — in the label, in the
shortcut, or both. The `matched_indices` are mapped back to their
respective display positions during rendering.

---

## Implementation

### Files touched

| File | Action |
|------|--------|
| `tui/app.rs` | Add `Command` to Mode enum, handle mode transitions |
| `tui/input/command.rs` | New — input handling for COMMAND mode |
| `tui/render/command_palette.rs` | New — overlay rendering |
| `tui/command_actions.rs` | New — action registry, context filtering |
| `tui/event.rs` | Route key events to command input handler |
| `tui/render/status_row.rs` | Add COMMAND mode indicator |

### Key implementation notes

1. **Action dispatch reuse**: The palette doesn't implement any actions
   itself. It resolves a selection to an action ID, then calls the same
   dispatch function that the keybinding handler uses. Adding a new
   action to the palette is just a registry entry, not new logic.

2. **Dynamic actions**: "Switch to track: {name}" entries are generated
   at palette-open time by iterating active tracks. Rebuilt each time
   the palette opens.

3. **Compound actions**: "Mark done (#wontdo)" fires two operations:
   `tag add wontdo` then `set_done`. Registered as an action sequence.

4. **Multi-key shortcuts display**: Filter shortcuts (`fa`, `fo`, etc.)
   and `QQ` display as multi-character strings in the shortcut column.
   This is display-only — the palette executes the full action in one
   step on `Enter`.

5. **Rendering layer**: The palette renders in the same overlay layer
   as help and conflict popups. Only one overlay active at a time
   (opening palette closes help, and vice versa).

6. **No persistence**: Filter text clears on close. Selection resets
   to first item on open. No state persisted to `.state.json`.

### Task breakdown

```
CP1  Define action registry and context system
     - PaletteAction struct: id, label, shortcut, contexts, category
     - ViewContext and ActionCategory enums
     - Static registry of all actions
     - fn available_actions(context: ViewContext) -> Vec<&PaletteAction>
     - Dynamic action generation for track switching
     - Unit tests: correct actions for each context

CP2  Implement fuzzy matching
     - fn fuzzy_score(query, target) -> Option<(score, matched_indices)>
     - Case-insensitive, word-boundary bonus, consecutive bonus, gap penalty
     - fn filter_actions(query, actions) -> Vec<(PaletteAction, score, indices)>
     - Unit tests: scoring, ordering, edge cases, empty query

CP3  Implement command mode input handler
     - Add Command variant to Mode enum
     - Key routing: printable → filter, backspace, arrows, enter, esc
     - Track filter text, selected index, filtered results
     - On Enter: resolve action, dispatch, exit mode
     - On Esc: exit mode, no action
     - Backspace on empty input closes palette

CP4  Implement palette overlay rendering
     - Border, input line with > prompt, separator, results, footer
     - Selection indicator (▸) on current item
     - Highlight matched characters in results
     - Right-aligned shortcut column
     - Variable height based on result count
     - Empty state ("No matching actions")
     - Use existing overlay rendering patterns (help overlay)

CP5  Wire up action dispatch
     - Map action IDs to existing dispatch functions
     - Compound actions (wontdo, duplicate) as action sequences
     - Dynamic track-switching actions
     - Actions with follow-up chain into existing UI flows
     - Select mode actions transition to SELECT after palette closes

CP6  Add > keybinding and mode transitions
     - > in NAVIGATE → COMMAND mode
     - COMMAND → NAVIGATE on Esc
     - COMMAND → (action's target mode) on Enter
     - Status row: -- COMMAND --
     - Ensure mutual exclusivity with other overlays
```

---

## Future Extensions

Out of scope for initial implementation:

- **CLI commands in palette**: `fr clean`, `fr check`, `fr stats` as
  palette actions with results displayed in a temporary overlay.
- **Frecency sorting**: Track which actions the user selects most often
  and boost their score. Persist in `.state.json`.
- **Custom aliases**: User-defined short names for action sequences
  in `project.toml`.
