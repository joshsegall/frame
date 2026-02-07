# Frame UI — Selection & Search Highlight Spec

## Overview

This spec defines the colors and rendering for three UI elements:
1. Active tab indicator (tab bar)
2. Selected row highlight (track view, inbox, etc.)
3. Search match highlighting

All colors are solid 24-bit RGB values (`Color::Rgb`). No alpha/transparency
exists in ratatui — these are pre-blended against the base background
`#0C001B`.

---

## 1. Active Tab

The currently selected tab in the tab bar.

- **Background**: `#1A0A20` (translucent pink tint, pre-blended)
- **Foreground (text)**: `#FFFFFF` (white, bold)
- **Bottom border**: Render a `▁` (U+2581, LOWER ONE EIGHTH BLOCK) character
  row beneath the tab text in `#FB4196` (highlight pink), OR use
  ratatui's `Modifier::UNDERLINED` with underline color set to `#FB4196`
  if the terminal supports colored underlines (most modern terminals do
  via `SetUnderlineColor`). Prefer the underline approach.

**Inactive tabs** remain as-is:
- **Background**: transparent (same as tab bar background)
- **Foreground**: `#A09BFE` (text color)
- **No underline**

---

## 2. Selected Row

The row under the cursor in any list view (track view, inbox, tracks view,
recent view).

Two visual elements combine:

### Left border accent
- Render a `▎` (U+258E, LEFT ONE QUARTER BLOCK) character in the leftmost
  column of the selected row.
- Color: `#FB4196` (highlight pink) foreground, row background as bg.
- This replaces whatever whitespace/indent character would normally occupy
  that column.

### Row background tint
- **Background**: `#2E0E2B` (deep magenta tint, pre-blended against #0C001B)
- Applied to every cell in the selected row.

### Text colors on selected row
- **Task title**: `#FFFFFF` (white, bold)
- **Task ID**: `#DAB8F0` (light lavender — brighter than the normal #A09BFE
  so IDs remain readable against the tinted background)
- **State symbols**: use their normal colors (todo=#A09BFE, active=#FB4196,
  blocked=#FF4444, parked=#FFD700, done=#5A5580)
- **Tags**: use their normal tag colors
- **Dim/secondary text** (match count, metadata hints): `#8A7EAA`

### Non-selected rows
No change from current spec:
- **Background**: `#0C001B` (base background)
- **Task title**: `#FFFFFF`
- **Task ID**: `#A09BFE`
- No left border character

---

## 3. Search Match Highlighting

Inline highlighting of regex matches within task text (titles, notes, tags,
IDs) when a search is active.

- **Background**: `#0D7377` (teal)
- **Foreground**: `#FFFFFF` (white)
- Applied per-character span over whatever text is matched.
- Overrides the normal foreground color of the matched text (whether it's
  a title, ID, tag, etc.).
- On the selected row: search highlight background `#0D7377` still applies
  (it should be visually distinct from the row selection background
  `#2E0E2B`). The teal is bright enough to stand out.

---

## Color Reference Table

| Element                  | Foreground  | Background  | Notes              |
|--------------------------|-------------|-------------|--------------------|
| Active tab text          | `#FFFFFF`   | `#1A0A20`   | Bold               |
| Active tab underline     | `#FB4196`   | —           | Colored underline  |
| Inactive tab text        | `#A09BFE`   | transparent | —                  |
| Selected row bg          | —           | `#2E0E2B`   | Full row           |
| Selected row left border | `#FB4196`   | `#2E0E2B`   | ▎ character        |
| Selected row title       | `#FFFFFF`   | `#2E0E2B`   | Bold               |
| Selected row ID          | `#DAB8F0`   | `#2E0E2B`   | —                  |
| Selected row secondary   | `#8A7EAA`   | `#2E0E2B`   | —                  |
| Search match             | `#FFFFFF`   | `#0D7377`   | Per-character span |
| Normal row title         | `#FFFFFF`   | `#0C001B`   | —                  |
| Normal row ID            | `#A09BFE`   | `#0C001B`   | —                  |

---

## Implementation Notes

- **Colored underlines**: crossterm supports `SetUnderlineColor` (CSI 58;2;r;g;b m).
  ratatui exposes this via `Style::underline_color()`. Check that this is
  available in the ratatui version being used. If not, fall back to the
  `▁` block character approach for the tab indicator.

- **Left border character**: The `▎` is a single-width Unicode character.
  It occupies one cell. Account for this in column layout — the content
  of the selected row shifts right by one character compared to non-selected
  rows, OR reserve that column for all rows (space for non-selected, `▎`
  for selected) so content alignment stays consistent. **Prefer the
  reserved column approach** to avoid content jumping when moving the cursor.

- **Config integration**: These new colors should be added to the
  `[ui.colors]` section of `project.toml`:
  ```toml
  [ui.colors]
  selection_bg = "#2E0E2B"
  selection_border = "#FB4196"    # same as highlight by default
  selection_id = "#DAB8F0"
  search_match_bg = "#0D7377"
  search_match_fg = "#FFFFFF"
  tab_active_bg = "#1A0A20"
  ```
  All optional — defaults to the values in this spec.
