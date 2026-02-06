# Frame â€” Design Specification v3.3

## Overview

Frame is a file-based task tracker with a terminal UI. Tasks are stored as
markdown files in a project directory. The TUI provides visual management.
The CLI provides programmatic access for coding agents.

**Key properties:**
- Markdown files are the source of truth
- Position in a list is priority (no priority fields)
- Tracks organize concurrent work streams
- The TUI is the primary human interface; the CLI is for agents
- All writes go through `fr` â€” humans via TUI, agents via CLI
- Git is the history and audit trail

---

## Concepts

### Project

A root directory containing a `frame/` subdirectory. Frame auto-discovers
the project by walking up from the current directory (like git).

```
my-project/
  frame/
    project.toml
    inbox.md
    .state.json               # TUI state (gitignored)
    tracks/
      effects.md
      unique-types.md
      modules.md
      compiler-infra.md
    archive/
      effects.md              # Per-track archive of done tasks
      unique-types.md
      ...
      _tracks/                # Fully archived (finished) tracks
        bootstrap.md
```

### Track

A work stream with its own ordered backlog. Three states:

| State        | Meaning                     | Visibility            |
|--------------|-----------------------------|-----------------------|
| **active**   | Current work stream         | Tab bar               |
| **shelved**  | Paused, will return to      | Tracks view           |
| **archived** | Finished, reference only    | Tracks view (toggle)  |

Active tracks are ordered â€” first is highest-priority work stream. One
active track can be designated **cc-focus** (where CC looks for work).

### Task

A unit of work. Can be a single line or a richly annotated item with
subtasks, notes, code snippets, dependencies, tags, and file references.

### Tags

Freeform labels for categorization. Conventional tags:

| Tag            | Meaning                                  |
|----------------|------------------------------------------|
| `research`     | Exploratory investigation                |
| `design`       | Producing a design doc or spec           |
| `ready`        | Ready to execute (implementation clear)  |
| `bug`          | Something broken                         |
| `cc`           | CC can take this and run with it         |
| `cc-added`     | Filed by CC                              |
| `needs-input`  | Needs human judgment to proceed          |

Tags are displayed as colored text (foreground color only, no background
pill by default â€” see UI notes).

---

## Task Format

### Grammar

```
task        = checkbox id? title tags? NL metadata* subtask*
checkbox    = "- [" state "] "
state       = " " | "x" | "-" | ">" | "~"
id          = "`" TRACK_PREFIX "-" NUMBER "`" " "
title       = <text to end of line, before tags>
tags        = " #" tag (" #" tag)*
tag         = <word, no spaces>
metadata    = INDENT "- " key ":" " " value NL
            | INDENT "- " key ":" NL block
key         = "dep" | "ref" | "spec" | "note"
            | "added" | "resolved"
value       = <text to end of line>
block       = (INDENT INDENT <text> NL)+
            | INDENT INDENT "```" lang? NL (<text> NL)* INDENT INDENT "```" NL
subtask     = INDENT task
```

**States:**

| Char | State   | Symbol | Color                 |
|------|---------|--------|-----------------------|
| ` `  | todo    | `â—‹`    | muted (text color)    |
| `>`  | active  | `â—`    | highlight (pink)      |
| `-`  | blocked | `âŠ˜`    | red                   |
| `x`  | done    | `âœ“`    | dim                   |
| `~`  | parked  | `â—‡`    | yellow                |

**Automatic metadata:**

- `added:` â€” date task was created (YYYY-MM-DD), auto-set on creation
- `resolved:` â€” date task was marked done, auto-set on state â†’ done

### Examples

**Minimal:**

```markdown
- [ ] Fix parser crash on empty blocks
```

**With ID, tags, and auto-metadata:**

```markdown
- [ ] `EFF-003` Implement effect handler desugaring #ready #cc
  - added: 2025-05-14
```

**Full example with multiline notes and code:**

```markdown
- [>] `EFF-014` Implement effect inference for closures #ready
  - added: 2025-05-10
  - dep: EFF-003
  - spec: doc/spec/effects.md#closure-effects
  - ref: doc/design/effect-handlers-v2.md
  - note:
    Found while working on EFF-002.

    The desugaring needs to handle three cases:
    1. Simple perform with no resumption
    2. Perform with single-shot resumption
    3. Perform with multi-shot resumption (if we support it)

    See the Koka paper for approach:
    ```lace
    handle(e) { ... } with {
      op(x, resume) -> resume(x + 1)
    }
    // desugars to match on effect tag
    ```
  - [ ] `EFF-014.1` Add effect variables to closure types
    - added: 2025-05-10
  - [>] `EFF-014.2` Unify effect rows during inference #cc
    - added: 2025-05-11
    - note: Row unification is the hard part here
    - [ ] `EFF-014.2.1` Handle row polymorphism
    - [ ] `EFF-014.2.2` Implement row simplification
  - [ ] `EFF-014.3` Test with nested closures
```

### Inbox Format

Inbox items are separated by blank lines (a blank line before each `-`).
This allows multiline notes naturally:

```markdown
# Inbox

- Parser crashes on empty effect block #bug
  Saw this when testing with empty `handle {}` blocks.
  Stack trace points to parser/effect.rs line 142.

- Think about whether `perform` should be an expression or statement
  #design
  If it's an expression, we get composability:
  ```lace
  let x = perform Ask() + 1
  ```
  But it makes the effect type more complex.

- CC found bug in module resolution for re-exported effect types
  #cc-added #bug

- Read the Koka paper on named handlers #research

- Unique type inference interacts with effect handlers somehow
  #research
  Noticed this while working on EFF-014, not sure of implications
  yet â€” could be a big deal.
```

Inbox items have no IDs. The first line after `-` is the title. Subsequent
indented lines (before the next blank-line-separated item) are the body
(notes/description). Tags can appear on the title line.

### File References

- **`spec:`** â€” specification this task implements
- **`ref:`** â€” any related file (design docs, tests, papers, links)

### Cross-track dependencies

```markdown
# In tracks/effects.md:
- [ ] `EFF-010` Effect-aware module imports
  - dep: MOD-012

# In tracks/modules.md:
- [ ] `MOD-012` Support qualified effect imports
```

---

## Track File Structure

```markdown
# Effect System

> Design and implement the algebraic effect system for Lace.

## Backlog

- [>] `EFF-014` Implement effect inference for closures #ready
  - added: 2025-05-10
  - dep: EFF-003
  - spec: doc/spec/effects.md#closure-effects
  - [ ] `EFF-014.1` Add effect variables to closure types
  - [>] `EFF-014.2` Unify effect rows during inference #cc
  - [ ] `EFF-014.3` Test with nested closures
- [ ] `EFF-015` Effect handler optimization pass #ready
  - dep: EFF-014
- [-] `EFF-012` Effect-aware dead code elimination #ready
  - dep: EFF-014, INFRA-003
- [ ] `EFF-016` Error messages for effect mismatches #ready
- [ ] `EFF-017` Research: algebraic effect composition #research
- [ ] `EFF-018` Design doc: effect aliases #design

## Parked

- [~] `EFF-020` Higher-order effect handlers #research

## Done

- [x] `EFF-003` Implement effect handler desugaring #ready
  - resolved: 2025-05-14
- [x] `EFF-002` Parse effect declarations #ready
  - resolved: 2025-05-12
- [x] `EFF-001` Define effect syntax #ready
  - resolved: 2025-05-08
```

**Backlog**: ordered by position. **Parked**: unordered, deferred.
**Done**: reverse-chronological. Archived past threshold (250 lines)
to `archive/<track-id>.md`.

---

## Configuration

### `project.toml`

```toml
[project]
name = "lace"

[agent]
cc_focus = "infra"
default_tags = ["cc-added"]

[[tracks]]
id = "effects"
name = "Effect System"
state = "active"
file = "tracks/effects.md"

[[tracks]]
id = "unique"
name = "Unique Types"
state = "active"
file = "tracks/unique-types.md"

[[tracks]]
id = "infra"
name = "Compiler Infrastructure"
state = "active"
file = "tracks/compiler-infra.md"

[[tracks]]
id = "modules"
name = "Module System"
state = "shelved"
file = "tracks/modules.md"

[[tracks]]
id = "bootstrap"
name = "Bootstrap"
state = "archived"
file = "archive/_tracks/bootstrap.md"

[clean]
auto_clean = true
done_threshold = 250
archive_per_track = true

[ids.prefixes]
effects = "EFF"
unique-types = "UNQ"
modules = "MOD"
infra = "INFRA"

[ui]
show_key_hints = false        # Toggle with ?
tag_style = "foreground"      # "foreground" (colored text) or "pill" (bg color)

[ui.colors]
background = "#0C001B"
text = "#A09BFE"
text_bright = "#FFFFFF"
highlight = "#FB4196"
dim = "#5A5580"
red = "#FF4444"
yellow = "#FFD700"
green = "#44FF88"
cyan = "#44DDFF"

[ui.tag_colors]
research = "#4488FF"
design = "#44DDFF"
ready = "#44FF88"
bug = "#FF4444"
cc = "#CC66FF"
cc-added = "#CC66FF"
needs-input = "#FFD700"
```

### Global config

`~/.config/frame/config.toml`:

```toml
[ui]
show_key_hints = false
tag_style = "foreground"
```

---

## TUI Design

### Design Principles

- **Narrow-column optimized**: 60-80 columns (half/third screen width),
  full screen height
- **Vim-like minimalism**: no chrome, no box-drawing on main views. Blank
  lines for breathing room. Dense information.
- **Modal**: modes displayed in the bottom row (like vim's `-- INSERT --`).
  Modes: NAVIGATE (default, no indicator), EDIT, MOVE, SEARCH.
- **Key hints off by default**: `?` toggles a help overlay.
- **Dark aesthetic**: deep blue-purple background, white task text, colored
  tags, pink highlights.

### Color Palette

```
Background:     #0C001B    (deep blue-black)
Text:           #A09BFE    (soft purple â€” IDs, metadata, structure)
Text bright:    #FFFFFF    (white â€” task titles, selected items)
Highlight:      #FB4196    (hot pink â€” active state, selection, mode labels)
Dim:            #5A5580    (muted purple â€” done items, tree lines)
Red:            #FF4444    (blocked state, bug tag)
Yellow:         #FFD700    (parked state, needs-input tag)
Green:          #44FF88    (ready tag)
Cyan:           #44DDFF    (design tag, spec refs)
Purple:         #CC66FF    (cc, cc-added tags)
Blue:           #4488FF    (research tag)
```

**Tag rendering**: Tags use **foreground color only** by default (colored
text, no background). This keeps the display clean and avoids visual noise.
The `pill` style (colored background with dark text) is available as an
option in config if preferred â€” worth prototyping both and deciding.

### Layout

Three regions:

1. **Tab bar** (row 1): Track tabs + view tabs
2. **Content area** (rows 2 to N-1): Current view, full width and height
3. **Status row** (last row): Mode indicator + search prompt (vim-style)

```
 Effects â”‚ Unique â”‚ Infra â”‚ â–¸ â”‚ ðŸ“¥5 â”‚ âœ“ â”‚
â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
â–¸â— EFF-014 Implement effect inference    ready
   â”œ â—‹ .1 Add effect variables
   â”œ â— .2 Unify effect rows              cc
   â”” â—‹ .3 Test with nested closures
 â—‹ EFF-015 Effect handler opt pass       ready
 âŠ˜ EFF-012 Effect-aware DCE              ready
 â—‹ EFF-016 Error msgs for mismatches     ready
 â—‹ EFF-017 Research: effect composition  research
 â—‹ EFF-018 Design doc: effect aliases    design

 â”€â”€ Parked â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
 â—‡ EFF-020 Higher-order handlers         research

```

The last row is normally empty (no mode indicator in NAVIGATE mode).

When a mode is active:

```
-- MOVE --                                           â†‘â†“ move  Enter âœ“  Esc âœ—
```

```
-- EDIT --                                                           Esc done
```

```
/handler regexâ–Œ                                                n/N next/prev
```

Mode labels are rendered in the highlight color, similar to vim's
`-- INSERT --`.

### Subtree Collapse

**Collapsed by default.** When you navigate to a track, all subtrees are
collapsed, showing only top-level tasks. Exception: the **first task**
(highest priority) is expanded one level deep (showing its immediate
children, but not their children).

| Symbol | Meaning                     |
|--------|-----------------------------|
| `â–¸`    | Collapsed, has children      |
| `â–¾`    | Expanded                     |
| (none) | Leaf node (no children)      |

`â†’` or `l` expands the selected node. `â†` or `h` collapses it (or moves
to parent if already collapsed). Expand/collapse state is persisted (see
UI State Persistence below).

```
â–¾â— EFF-014 Implement effect inference    ready
   â”œ â—‹ .1 Add effect variables
   â”œ â— .2 Unify effect rows              cc
   â”” â—‹ .3 Test with nested closures
â–¸â—‹ EFF-015 Effect handler opt pass       ready
 âŠ˜ EFF-012 Effect-aware DCE              ready
 â—‹ EFF-016 Error msgs for mismatches     ready
 â—‹ EFF-017 Research: effect composition  research
 â—‹ EFF-018 Design doc: effect aliases    design
```

### Views

**Track view** (default, tabs `1`-`9`):
Ordered backlog with collapsible hierarchy. Primary interaction view.

**Inbox view** (`i`):
The quick-capture queue. Items have no IDs and no ordering semantics â€”
they're unsorted until triaged into a track.

```
 Effects â”‚ Unique â”‚ Infra â”‚ â–¸ â”‚ ðŸ“¥5 â”‚ âœ“ â”‚
â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

 1  Parser crashes on empty effect block         bug
    Saw this when testing with empty `handle {}`
    blocks. Stack trace points to
    parser/effect.rs line 142.

 2  Should `perform` be expression or statement
    design
    If it's an expression, we get composability:
    ```lace
    let x = perform Ask() + 1
    ```

 3  Module resolution bug with re-exported       bug
    effect types                           cc-added

 4  Read Koka paper on named handlers    research

 5  Unique types interact with effect    research
    handlers somehow

```

Each item shows its sequential number (for reference, not a persistent
ID), title, tags, and body text (if any). Body text is shown indented
below the title, dimmed slightly for visual hierarchy.

**Inbox-specific keys (NAVIGATE mode):**

| Key     | Action                                       |
|---------|----------------------------------------------|
| `a`     | Add new inbox item (opens inline editor)     |
| `e`     | Edit selected item (title + body, EDIT mode) |
| `#`     | Edit tags on selected item                   |
| `Enter` | Begin triage: send to a track                |
| `x`     | Delete item (with confirmation)              |
| `m`     | Enter MOVE mode (reorder inbox items)        |
| `/`     | Search within inbox                          |

Standard navigation (`â†‘â†“`, `g`/`G`, `Cmd+â†‘/â†“`) works as in other views.

**Adding items (`a`):**

Opens an inline editor at the bottom of the inbox list. Type the title
on the first line. Press `Enter` to add body text (subsequent lines).
`Esc` finishes and saves the item. Tags can be added inline with `#tag`
in the title, or via `#` after creation.

**Editing items (`e`):**

Enters EDIT mode on the selected item. The full item (title + body) is
editable as a text block. Standard text editing keys apply. `Esc`
finishes editing.

**Triage flow (`Enter`):**

Triaging moves an inbox item into a track as a proper task with an
auto-assigned ID.

1. Press `Enter` on an inbox item
2. **Track selection**: autocomplete dropdown of active tracks appears.
   Type to filter, `â†‘â†“` to navigate, `Enter` to select.
3. **Position selection**: choose where in the track backlog to insert:
   - `t` â€” top (highest priority)
   - `b` â€” bottom (lowest priority, default)
   - `a` â€” after a specific task (autocomplete for task ID)
4. Item is removed from inbox, added to the track with an ID and
   `added:` date. Tags carry over.

`Esc` at any step cancels triage and returns to the inbox.

**Bulk triage:** There is no special bulk mode. Triage items one at a
time â€” the flow is fast enough (3 keystrokes for the common case:
`Enter`, select track, `Enter` for default bottom position). After
triaging, the cursor advances to the next item automatically.

**Recent view** (`r`):
Completed tasks, reverse-chronological, grouped by date. Can reopen.

**Tracks view** (`0` or `` ` ``):
Full-screen listing of all tracks. Active, shelved, archived sections.
Manage track state, reorder, set cc-focus. Move mode (`m`) reorders
tracks here.

```
 Effects â”‚ Unique â”‚ Infra â”‚ â–¸ â”‚ ðŸ“¥5 â”‚ âœ“ â”‚
â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

 Active
  1  Effect System              3â— 1âŠ˜ 8â—‹ 2â—‡ 12âœ“
  2  Unique Types               1â— 0âŠ˜ 5â—‹ 0â—‡  3âœ“
  3  Compiler Infrastructure    2â— 0âŠ˜ 3â—‹ 1â—‡  8âœ“  â˜…cc

 Shelved
  4  Module System              0â— 1âŠ˜ 3â—‹ 0â—‡  6âœ“

 Archived
  5  Bootstrap                  0â— 0âŠ˜ 0â—‹ 0â—‡ 15âœ“
```

### Detail View

`Enter` on a task opens the detail view, replacing the content area. It
renders the task as a **structured document** â€” continuous text you navigate
freely, with semantically meaningful regions.

```
 Effects â”‚ Unique â”‚ Infra â”‚ â–¸ â”‚ ðŸ“¥5 â”‚ âœ“ â”‚
â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

 â— EFF-014 Implement effect inference for closures
 ready

 added: 2025-05-10
 dep: EFF-003 âœ“, MOD-007 â—‹
 spec: doc/spec/effects.md#closure-effects
 ref: doc/design/effect-handlers-v2.md

 note:
 Found while working on EFF-002.

 The desugaring needs to handle three cases:
 1. Simple perform with no resumption
 2. Perform with single-shot resumption
 3. Perform with multi-shot resumption (if we support it)

 See the Koka paper for approach:
 ```lace
 handle(e) { ... } with {
   op(x, resume) -> resume(x + 1)
 }
 ```

 Subtasks
   â—‹ .1 Add effect variables to closure types
   â— .2 Unify effect rows                        cc
     â—‹ .2.1 Handle row polymorphism
     â—‹ .2.2 Row simplification
   â—‹ .3 Test with nested closures

```

**Two modes in detail view:**

**NAVIGATE mode** (default, no mode indicator):
- `â†‘â†“` moves between regions (title, tags, dep, spec, ref, note,
  subtasks)
- `Tab` jumps to next editable region, `Shift+Tab` to previous
- `e` or `Enter` enters EDIT mode on the current region
- `#`, `@`, `d`, `n` shortcut to the relevant region and enter EDIT mode
- `Space` cycles state
- `Esc` returns to track view

**EDIT mode** (bottom row shows `-- EDIT --`):
- Cursor is within the field's text content
- Standard text editing keys apply (arrows, Opt+arrow, Cmd+arrow,
  clipboard, backspace, etc.)
- `Tab` inserts 4 spaces (in note fields) or is ignored (in single-line
  fields)
- `Enter` confirms and exits EDIT mode (single-line fields)
- `Esc` exits EDIT mode back to NAVIGATE (multiline fields)
  - For single-line fields, `Esc` cancels the edit
- Context-appropriate autocomplete activates:
  - `dep:` field â†’ task ID autocomplete
  - `spec:` / `ref:` fields â†’ file path autocomplete
  - Tags line â†’ tag autocomplete
  - `note:` â†’ freeform text, no autocomplete

For multiline fields (note), the editing area expands as needed.
`Esc` finishes editing and returns to NAVIGATE mode. Content is saved
on exit (not on cancel â€” there is no cancel for multiline, since you
can always undo).

**Adding new fields from NAVIGATE mode:**
- `@` â†’ adds a new `ref:` line (or `spec:` if none exists), enters EDIT
- `d` â†’ focuses the `dep:` line, enters EDIT with autocomplete
- `n` â†’ focuses the `note:` block (creates it if missing), enters EDIT
- `#` â†’ focuses the tags line, enters EDIT with autocomplete

These work from **anywhere** in the detail view â€” you don't navigate to
the region first.

**Dep display**: Dependencies show current state inline:
`EFF-003 âœ“` (done), `MOD-007 â—‹` (todo), `INFRA-003 âŠ˜` (blocked).

---

## Modes Summary

Frame uses vim-style modes, indicated in the bottom status row:

| Mode       | Indicator          | How to enter       | How to exit        |
|------------|--------------------|--------------------|--------------------|
| NAVIGATE   | (none)             | Default            | â€”                  |
| EDIT       | `-- EDIT --`       | `e`/`Enter` on region | `Esc` or `Enter` (single-line) |
| MOVE       | `-- MOVE --`       | `m` on a task      | `Enter` (confirm) or `Esc` (cancel) |
| SEARCH     | `/patternâ–Œ`        | `/`                | `Enter` (execute) or `Esc` (cancel) |

The mode indicator is rendered in the **highlight color** in the bottom-left
of the status row. The right side of the status row shows context-sensitive
hints for the current mode.

NAVIGATE mode shows nothing in the status row (clean, like vim's normal mode).

### Mode Details

**MOVE mode** â€” available in both track view (reorder tasks) and tracks
view (reorder tracks):

```
-- MOVE --                                           â†‘â†“ move  Enter âœ“  Esc âœ—
```

The selected item is highlighted in the highlight color. Arrow keys
physically move the item in the list. The list reflows in real time.
`Enter` confirms the new position. `Esc` cancels and restores the
original position.

**SEARCH mode** â€” the search prompt appears in the status row (bottom of
screen, like vim):

```
/handlerâ–Œ                                                      n/N next/prev
```

Type regex pattern. Matches highlight in the content area above in real
time as you type. `Enter` executes the search â€” cursor moves to first
match, the prompt closes, and you can use `n`/`N` to cycle through
matches. `Esc` clears the search and returns to normal view.

Search scope follows context:
- Track view â†’ search within current track
- Tracks view â†’ search across all tracks
- Inbox â†’ search inbox
- Recent â†’ search recent

---

## Key Bindings

### Principles

- Single keys for common actions
- Arrow keys for navigation (vim hjkl also available)
- `Cmd+C/V/X/Z` for clipboard and undo (standard Mac shortcuts)
- Number keys `1`-`9` for tab switching
- Lowercase for views and common actions
- Uppercase for heavier actions
- `Esc` exits any mode or context
- `#` for tags, `@` for refs, `d` for deps, `n` for notes
- `Cmd+Q` to quit
- `?` toggles help overlay

### Navigation (NAVIGATE mode, all views)

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `â†‘` / `k`             | Move cursor up                      |
| `â†“` / `j`             | Move cursor down                    |
| `â†` / `h`             | Collapse / go to parent             |
| `â†’` / `l`             | Expand / go to first child          |
| `Cmd+â†‘` / `g`         | Top of list                         |
| `Cmd+â†“` / `G`         | Bottom of list                      |
| `Enter`                | Open detail / drill in              |
| `Esc`                  | Back / close / cancel               |

### Tab/View Switching (NAVIGATE mode)

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `1`-`9`                | Switch to active track N            |
| `Tab` / `Shift+Tab`   | Next / previous track tab           |
| `i`                    | Inbox view                          |
| `r`                    | Recent completed view               |
| `0` or `` ` ``        | Tracks view                         |
| `/`                    | Enter SEARCH mode                   |
| `?`                    | Toggle help overlay                 |
| `Cmd+Q`               | Quit                                |

### Task Actions (NAVIGATE mode, track view)

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `a`                    | Add task (append to bottom)         |
| `o`                    | Insert task after current           |
| `p`                    | Push task to top of backlog         |
| `A`                    | Add subtask to selected task        |
| `Space`                | Cycle state: todo â†’ active â†’ done   |
| `x`                    | Mark done (direct)                  |
| `b`                    | Toggle blocked                      |
| `~`                    | Toggle parked                       |
| `m`                    | Enter MOVE mode                     |
| `Cmd+Z` / `u`         | Undo                                |

### Quick Edit (NAVIGATE mode, track view)

| Key   | Action                                         |
|-------|-------------------------------------------------|
| `e`   | Edit task title inline (enters EDIT mode)       |
| `#`   | Edit tags (EDIT mode + autocomplete)            |
| `@`   | Add file/spec ref (EDIT mode + path autocomplete)|
| `d`   | Edit dependencies (EDIT mode + ID autocomplete) |
| `n`   | Add/edit note (EDIT mode)                       |

### Detail View (NAVIGATE mode)

| Key            | Action                                  |
|----------------|-----------------------------------------|
| `â†‘â†“`          | Move between regions                    |
| `Tab`          | Jump to next editable region            |
| `Shift+Tab`   | Previous editable region                |
| `e` / `Enter` | Enter EDIT mode on current region       |
| `#`            | Jump to tags + EDIT mode                |
| `@`            | Add/edit ref + EDIT mode                |
| `d`            | Jump to deps + EDIT mode                |
| `n`            | Jump to note + EDIT mode                |
| `Space`        | Cycle state                             |
| `Esc`          | Back to track view                      |

### Text Editing (EDIT mode)

Standard text editing in all text fields (titles, track names, notes,
single-line metadata):

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `â†` / `â†’`             | Move cursor                         |
| `Opt+â†` / `Opt+â†’`     | Move by word                        |
| `Cmd+â†` / `Cmd+â†’`     | Start / end of line                 |
| `Backspace`            | Delete backward                     |
| `Opt+Backspace`        | Delete word backward                |
| `Cmd+C/V/X`           | Copy / paste / cut                  |
| `Cmd+Z`               | Undo                                |
| `Enter`                | Confirm (single-line) / newline (multi) |
| `Tab`                  | 4 spaces (multiline) / ignored (single-line) |
| `Esc`                  | Exit EDIT mode (cancel single-line, finish multi) |


### Text Selection (EDIT mode)

Standard shift-movement selection in all text fields:

| Key                          | Action                            |
|------------------------------|-----------------------------------|
| `Shift+←` / `Shift+→`       | Extend selection by character     |
| `Shift+Opt+←` / `Shift+Opt+→` | Extend selection by word       |
| `Shift+Cmd+←` / `Shift+Cmd+→` | Extend selection to start/end of line |
| `Shift+↑` / `Shift+↓`       | Extend selection by line (multiline only) |
| `Cmd+A`                      | Select all within current field   |

Any non-shift movement collapses the selection to the cursor position.
Typing with an active selection replaces the selected text. `Cmd+C`
copies the selection, `Cmd+X` cuts it, `Cmd+V` replaces it with the
clipboard. Selected text is rendered with an inverted/highlight background.

### MOVE Mode

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `â†‘` / `k`             | Move item up                        |
| `â†“` / `j`             | Move item down                      |
| `Cmd+â†‘` / `g`         | Move to top                         |
| `Cmd+â†“` / `G`         | Move to bottom                      |
| `Enter`                | Confirm new position                |
| `Esc`                  | Cancel, restore original            |

Works in **track view** (reorder tasks) and **tracks view** (reorder
tracks).

### SEARCH Mode

| Key                    | Action                              |
|------------------------|-------------------------------------|
| typing                 | Filter/highlight matches in real time |
| `Enter`                | Execute, move to first match        |
| `Esc`                  | Cancel, clear search                |
| `n` (after execute)    | Next match                          |
| `N` (after execute)    | Previous match                      |

### Autocomplete (within EDIT mode)

| Key                    | Action                              |
|------------------------|-------------------------------------|
| `â†‘` / `â†“`             | Navigate entries                    |
| `Enter`                | Select entry                        |
| `Esc`                  | Cancel autocomplete                 |
| typing                 | Filter entries                      |

---

## UI State Persistence

Frame saves TUI state to `frame/.state.json` (gitignored) so that
restarting the TUI picks up exactly where you left off.

**Persisted state:**

| State                    | What's saved                          |
|--------------------------|---------------------------------------|
| Active view              | Which view is showing (track/inbox/recent/tracks) |
| Selected track           | Which track tab is active             |
| Cursor position          | Selected task in each track           |
| Expand/collapse          | Which nodes are expanded, per track   |
| Scroll position          | Viewport offset, per track            |
| Search state             | Last search pattern (for `n`/`N`)     |

State is written on every change (debounced to avoid excessive I/O)
and read on startup. If `.state.json` is missing or corrupt, Frame
starts with defaults (first track, first task, collapsed).

The file is local-only (gitignored) since UI state is per-machine
and per-user.

```json
{
  "view": "track",
  "active_track": "effects",
  "tracks": {
    "effects": {
      "cursor": "EFF-014",
      "expanded": ["EFF-014", "EFF-014.2"],
      "scroll_offset": 0
    },
    "infra": {
      "cursor": "INFRA-015",
      "expanded": [],
      "scroll_offset": 0
    }
  },
  "last_search": "handler"
}
```

---

## Agent Interface (CLI)

### Reading

```
fr ready                              # Unblocked tasks, all active tracks
fr ready --cc                         # cc-tagged tasks on cc-focus track
fr ready --track effects
fr ready --tag ready
fr ready --json

fr list [track]
fr list --state active|todo|blocked|done|parked
fr list --tag research
fr list --all
fr list --json

fr show <id>
fr show <id> --json

fr tracks
fr tracks --json

fr inbox
fr inbox --json

fr search <pattern>                   # Regex, all tracks
fr search <pattern> --track X

fr deps <id>                          # Dependency tree
fr blocked                            # Blocked tasks and blockers
fr recent
fr recent --limit 20

fr stats
fr stats --json

fr check                              # Validate deps, refs, format
```

### Writing

```
# Capture
fr inbox "quick note"
fr inbox "bug found" --tag bug
fr inbox "longer note" --note "additional details here"

# Create tasks
fr add <track> "title"
fr push <track> "title"
fr add <track> "title" --after <id>
fr add <track> "title" --found-from <id>
fr sub <id> "title"

# Modify
fr mv <id> <position>
fr mv <id> --top
fr mv <id> --after <other-id>
fr mv <id> --track <track>

fr state <id> todo|active|blocked|done|parked
fr tag <id> add <tag>
fr tag <id> rm <tag>
fr dep <id> add <dep-id>
fr dep <id> rm <dep-id>
fr note <id> "text"
fr ref <id> <filepath>
fr spec <id> <filepath>#<section>
fr title <id> "new title"

# Triage inbox items
fr triage <index> --track <track>
fr triage <index> --track <track> --top
fr triage <index> --track <track> --after <id>

# Bulk import
fr import <file.md> --track <track>
fr import <file.md> --track <track> --top
fr import <file.md> --track <track> --after <id>

# Track management
fr track new <id> "name"
fr track shelve <id>
fr track activate <id>
fr track archive <id>
fr track mv <id> <position>
fr track cc-focus <id>

# Maintenance
fr clean
fr clean --dry-run
fr check
```

### `fr ready --cc`

```json
{
  "focus_track": "infra",
  "tasks": [
    {
      "id": "INFRA-015",
      "title": "Add span tracking to HIR nodes",
      "state": "todo",
      "tags": ["ready", "cc"],
      "deps": [],
      "spec": "doc/spec/hir.md#spans",
      "added": "2025-05-12",
      "subtasks": [...]
    }
  ]
}
```

### `fr import`

Imports a markdown file of tasks into a track:

```bash
fr import /tmp/effects-tasks.md --track effects
fr import /tmp/effects-tasks.md --track effects --top
```

IDs auto-assigned. `added:` dates auto-set. Supports the workflow
where CC or Claude produces a task breakdown as markdown, then imports.

---

## `fr clean`

Normalizes files and performs maintenance. Runs automatically on read
when externally-modified files are detected (via mtime).

1. **Format normalization**: indentation, spacing, list markers
2. **ID assignment**: assign IDs to tasks missing them
3. **Date assignment**: add `added:` where missing (uses current date)
4. **Done archiving**: move completed tasks past threshold to per-track
   archive
5. **Dependency validation**: flag dangling references
6. **File reference validation**: flag broken spec/ref paths
7. **State suggestions**: all subtasks done â†’ suggest parent done;
   dep not done â†’ suggest blocked
8. **ACTIVE.md generation**: read-only convenience file for CC orientation

---

## Design Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| State encoding | Checkbox characters `[>][-][~]` | Scannable in raw text; TUI is primary interface |
| Priority | Position-based | Explicit ordering; reorder = reprioritize |
| Track organization | TOML manifest + separate files | Independent editing, clear ordering |
| Agent writes | CLI only | Canonical formatting, no parse drift |
| Detail view | Structured document, not form | Feels like text editing; free cursor movement |
| Modes | Vim-style (NAVIGATE/EDIT/MOVE/SEARCH) | Consistent with editor muscle memory |
| Mode indicator | Bottom row, highlight color | Matches vim convention |
| Search | Bottom row, regex default, scoped | Vim-style, context-aware |
| Tab in EDIT mode | 4 spaces (multiline) / ignored (single) | Avoids clash with region-jump in NAVIGATE |
| Subtree collapse | Collapsed by default, first task expanded | Dense display; progressive disclosure |
| Quit | Cmd+Q | Safe, prevents accidental quit |
| Key hints | Hidden by default | Clean for experienced users; `?` for help |
| Tags | Foreground color text (pill style optional) | Less visual noise; configurable |
| `cc` tag | Short form of `cc-ok` | Concise |
| `ready` tag | Replaces `implement` | Clearer: means "ready to execute" |
| `needs-input` tag | Replaces `josh-only` | Name-agnostic |
| Inbox separator | Blank line before `-` | Natural, matches existing TODO list habit |
| Dates | Auto-managed `added:` and `resolved:` | No manual entry; enables Recent view |
| External editor | Not supported (TUI only) | Keep workflow in-app for now |
| Move mode | Available in track + tracks views | Consistent reordering experience |
| UI state | Persisted to `.state.json` (gitignored) | Resume exactly where you left off |
| Undo | Session-only, unlimited | Git for cross-session undo |
| State transitions | Space cycles todo→active→done; b/~ toggle | Simple, predictable |
| Metadata format | Comma-separated on one line | Parser accepts both; canonical output is clean |
| `found:` metadata | Removed; use note field instead | Simpler grammar; note is flexible enough |
| Task deletion | No hard delete; `#wontdo`/`#duplicate` + done | Preserves audit trail via git |
| Text selection | Shift+movement in EDIT mode | Standard editor behavior |
| Edit conflicts | Popup with orphaned text, copy to clipboard | Never silently discard user input |
| Nesting limit | 3 levels max | Keeps parser/TUI manageable |
