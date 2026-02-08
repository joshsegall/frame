# Feature: Tag Color Editor (`T`)

## Overview

A popup for assigning colors to tags from a fixed palette. Accessible
from any view via `T`. Colors are written to `[ui.tag_colors]` in
`project.toml`.

Users who want colors outside the palette can edit `project.toml`
directly — the TUI reads arbitrary hex values, it just doesn't let
you type them in.

---

## Trigger

`T` (uppercase) from any view in NAVIGATE mode. Opens the tag color
popup as a floating overlay.

---

## Layout

### Tag list

```
 ┌─ Tag Colors ─────────────────────────────────┐
 │                                               │
 │  bug              ■ red                       │
 │  cc               ■ purple                    │
 │  cc-added         ■ purple                    │
 │  design           ■ cyan                      │
 │  migration          (none)                    │
 │  needs-input      ■ yellow                    │
 │  perf               (none)                    │
 │  ready            ■ green                     │
 │  research         ■ blue                      │
 │                                               │
 │                              Esc close        │
 └───────────────────────────────────────────────┘
```

- Tags listed alphabetically
- `■` swatch rendered in the tag's actual color
- Tags with no assigned color show `(none)` in dim text, no swatch
- Cursor highlight on the selected row
- Tag names rendered in their assigned color (or default text color
  if none) — this doubles as a live preview

### Tag sources

The list is the union of:
1. Tags currently used on any task across all tracks and inbox
2. Tags defined in `[ui.tag_colors]` in `project.toml` (even if not
   currently on any task)

Deduplicated. This ensures you can see and edit colors for tags you've
configured but temporarily removed from tasks, without losing the
color assignment.

### Palette picker

`Enter` on a tag opens the palette inline on that row:

```
 ┌─ Tag Colors ─────────────────────────────────┐
 │                                               │
 │  bug              ■ red                       │
 │  cc               ■ purple                    │
 │  cc-added         ■ purple                    │
 │  design           ■ cyan                      │
 │  migration        ■ ■ ■ ■ ■ ■ ■ ■ ■ ■  ×    │
 │  needs-input      ■ yellow                    │
 │  perf               (none)                    │
 │  ready            ■ green                     │
 │  research         ■ blue                      │
 │                                               │
 │    ←→ pick  Enter ✓  Bksp clear  Esc cancel   │
 └───────────────────────────────────────────────┘
```

The palette swatches replace the color label on the selected row. Each
`■` is rendered in its palette color. The currently selected swatch has
a bracket indicator or underline highlight. If the tag already has a
color, the picker opens with that swatch pre-selected.

---

## Palette

The fixed palette, drawn from Frame's accent colors:

| Label   | Hex       |
|---------|-----------|
| red     | `#FF4444` |
| yellow  | `#FFD700` |
| green   | `#44FF88` |
| cyan    | `#44DDFF` |
| blue    | `#4488FF` |
| purple  | `#CC66FF` |
| pink    | `#FB4196` |
| white   | `#FFFFFF` |
| dim     | `#5A5580` |
| text    | `#A09BFE` |

10 swatches — fits in a horizontal row. These are hardcoded in the
binary, not configurable. If users have overridden `[ui.colors]` in
their config, the palette still shows these canonical values (the
palette is for tag assignment, not a reflection of the current theme).

---

## Navigation

### Tag list mode

| Key              | Action                         |
|------------------|--------------------------------|
| `↑` / `k`       | Move cursor up                 |
| `↓` / `j`       | Move cursor down               |
| `Enter`          | Open palette picker on tag     |
| `Backspace`      | Clear color (set to none)      |
| `Esc`            | Close popup                    |

### Palette picker mode

| Key              | Action                         |
|------------------|--------------------------------|
| `←` / `h`       | Previous swatch                |
| `→` / `l`       | Next swatch                    |
| `Enter`          | Assign selected color          |
| `Backspace`      | Clear color (set to none)      |
| `Esc`            | Cancel, return to tag list     |

---

## Behavior

### Immediate save

Each color pick writes to `project.toml` immediately via `toml_edit`
(round-trip safe). The TUI's in-memory color map updates at the same
time, so all tag renders across all views reflect the change instantly.

### Custom hex colors

If a tag has a color in `[ui.tag_colors]` that doesn't match any
palette swatch (e.g., a hand-edited hex value like `#FF8844`), it
still shows a swatch rendered in that color, with the hex value as
the label:

```
  migration        ■ #FF8844
```

When the palette picker opens on such a tag, no swatch is pre-selected.
Picking a palette color replaces the custom hex. `Esc` keeps the
custom value unchanged. This ensures custom hex users aren't surprised
by losing their color — they only lose it by deliberately picking a
replacement.

### Clearing a color

`Backspace` on a tag (either in tag list mode or palette picker mode)
removes the entry from `[ui.tag_colors]`. The tag reverts to the
default text color. If the tag was one of the conventional tags with
a default color in the hardcoded defaults (e.g., `bug` → red), clearing
reverts to that default rather than to no color.

### Undo

Color changes are **not** on the TUI undo stack. They're config changes,
not task mutations. Git covers reverting config. This matches the
principle that undo is for task operations.

### Empty state

If no tags exist anywhere in the project and none are in config:

```
 ┌─ Tag Colors ─────────────────────────────────┐
 │                                               │
 │  No tags in project.                          │
 │  Add tags to tasks with t in the TUI           │
 │  or --tag in the CLI.                         │
 │                                               │
 │                              Esc close        │
 └───────────────────────────────────────────────┘
```

---

## Sizing

- **Width**: 50% of terminal width, minimum 36 columns, maximum 60
- **Height**: content-sized up to 70% of terminal height, scrollable
  if many tags
- **Position**: centered

Narrower than the dep popup since the content is simpler — just tag
names and color labels.

---

## Implementation

### Files touched

| File | Action |
|------|--------|
| `tui/render/tag_color_popup.rs` | New — popup rendering, tag list, palette picker |
| `tui/input/navigate.rs` | Handle `T` key globally (all views) |
| `tui/app.rs` | Add popup state (open, selected tag, picker mode, cursor) |
| `model/config.rs` | Helper to collect all tags from project + config |
| `io/project_io.rs` | Write tag color changes to project.toml via toml_edit |

### Task breakdown

```
T1  Collect all project tags
    - Scan all tasks across all tracks + inbox for tags
    - Merge with keys from [ui.tag_colors] in config
    - Deduplicate and sort alphabetically
    - Return Vec<(String, Option<Color>)>

T2  Implement tag list rendering
    - Floating popup with border, title "Tag Colors"
    - Alphabetical tag list with color swatches
    - Tags rendered in their assigned color
    - (none) in dim for unassigned tags
    - Cursor highlight on selected row
    - Scrolling for long lists
    - Empty state message
    - Hint bar: "Enter pick  Bksp clear  Esc close"

T3  Implement palette picker
    - Inline horizontal swatch row replacing color label
    - 10 swatches in accent colors
    - Bracket/highlight indicator on selected swatch
    - Pre-select current color if tag already has one
    - ←→ navigation between swatches
    - Enter assigns, Backspace clears, Esc cancels
    - Hint bar updates: "←→ pick  Enter ✓  Bksp clear  Esc cancel"

T4  Implement config write
    - On color pick: write to [ui.tag_colors] in project.toml
    - Use toml_edit for round-trip safe editing
    - On clear: remove key from [ui.tag_colors]
    - Update in-memory color map immediately
    - Handle edge case: [ui.tag_colors] section doesn't exist yet
      (create it)

T5  Wire up T key binding
    - T in NAVIGATE mode from any view opens popup
    - Popup captures all input when open
    - State in app.rs: open flag, tag list, selected index,
      picker mode (bool), picker cursor
```

### Notes

- The popup is a config editor, not a task mutation — no file locking
  needed (only `project.toml` is touched, not track files).
- `toml_edit` preserves comments and formatting in `project.toml`,
  so existing user comments in the config aren't destroyed.
- If `[ui.tag_colors]` doesn't exist in the file, the first color
  assignment creates the section. Subsequent assignments append to it.
