# Frame Concepts

Frame is a markdown-based task tracker. `.md` files are the source of truth — you can edit them by hand or through the CLI/TUI.

## Projects

A Frame project is any directory tree containing a `frame/` subdirectory. Running `fr init` creates one:

```
myproject/
  frame/
    project.toml      # project configuration
    inbox.md           # inbox items
    tracks/
      api.md           # one file per track
      ui.md
    archive/           # archived data
      effects.md       # done-task archives (from fr clean)
      _tracks/         # archived whole track files
        old-track.md
    .lock              # advisory lock file
    .state.json        # TUI state (cursor, scroll, expanded)
```

Project discovery walks up from the current directory until it finds a `frame/` folder.

## Tracks

A track is a unit of work (a feature, workstream, or area). Each track has:

- **id** — short identifier used in commands (e.g., `api`, `effects`)
- **name** — human-readable display name (e.g., "API Layer", "Effect System")
- **state** — one of `active`, `shelved`, or `archived`
- **file** — path to the markdown file (e.g., `tracks/api.md`)

Track states:

| State | Meaning |
|-------|---------|
| `active` | Shown in tabs, included in listings |
| `shelved` | Hidden from default views, preserved for later |
| `archived` | Moved to `frame/archive/`, read-only |

Each track file has three sections: **Backlog** (todo/active/blocked tasks), **Parked** (intentionally paused), and **Done** (completed).

## Tasks

Tasks are markdown checkboxes with structured metadata. Each task has a **state**:

| State | Checkbox | Meaning |
|-------|----------|---------|
| Todo | `- [ ]` | Not started |
| Active | `- [>]` | In progress |
| Blocked | `- [-]` | Waiting on dependencies |
| Done | `- [x]` | Completed |
| Parked | `- [~]` | Intentionally paused |

Tasks can nest up to 3 levels deep (top-level, subtask, sub-subtask). Each indentation level uses 2 spaces.

### Task IDs

Tasks get unique IDs based on their track's configured prefix:

```
EFF-001          # top-level task
EFF-001.1        # subtask
EFF-001.1.2      # sub-subtask
```

The prefix mapping (e.g., `effects` -> `EFF`) is configured in `project.toml` under `[ids.prefixes]`.

### Tags

Tags are `#word` tokens at the end of a task line:

```
- [>] `EFF-014` Implement effect inference #ready #cc
```

Tags are stored without the `#` prefix internally.

## Metadata

Tasks can have metadata lines indented below the task line:

| Field | Format | Description |
|-------|--------|-------------|
| `added` | `added: 2025-05-14` | Date the task was created |
| `resolved` | `resolved: 2025-05-14` | Date the task was completed |
| `dep` | `dep: EFF-003, INFRA-007` | Task dependencies (comma-separated IDs) |
| `ref` | `ref: doc/design.md, src/lib.rs` | File references (comma-separated paths) |
| `spec` | `spec: doc/spec.md#section` | Spec file with optional anchor |
| `note` | `note: Free text` | Note (single-line or multi-line block) |

## Inbox

The inbox (`frame/inbox.md`) is a quick-capture bucket for ideas that haven't been assigned to a track yet. Inbox items have a title, optional tags, and optional body text — but no ID, state, or metadata.

**Triage** moves an inbox item into a track, converting it to a proper task with an auto-assigned ID.

## Done Lifecycle

When a task is marked done:

- **TUI**: The task stays in Backlog for a 5-second grace period (undo-able), then moves to the Done section automatically.
- **CLI**: `fr state ID done` moves top-level Backlog tasks to the Done section immediately.

When a top-level task moves between sections (Backlog <-> Done), its entire subtask tree moves with it. Subtasks cannot be moved between sections independently — only top-level tasks trigger section moves.

When the Done section exceeds the configured threshold (default: 250 tasks), `fr clean` archives the oldest tasks to a per-track archive file in `frame/archive/`.

## Configuration

`project.toml` has these sections:

### `[project]`

```toml
[project]
name = "My Project"
```

### `[[tracks]]`

Array of track definitions:

```toml
[[tracks]]
id = "effects"
name = "Effect System"
state = "active"
file = "tracks/effects.md"
```

### `[ids.prefixes]`

Maps track IDs to task ID prefixes:

```toml
[ids.prefixes]
effects = "EFF"
infra = "INF"
```

### `[agent]`

Settings for AI agent integration:

```toml
[agent]
cc_focus = "effects"       # track for `fr ready --cc`
default_tags = ["cc"]        # tags always shown in autocomplete (even if no tasks use them yet)
```

### `[clean]`

Auto-clean and archival settings:

```toml
[clean]
auto_clean = true          # run clean after file reload in TUI (default: true)
done_threshold = 250       # max done tasks per track before archiving (default: 250)
archive_per_track = true   # separate archive file per track (default: true)
```

### `[ui]`

TUI display settings:

```toml
[ui]
kitty_keyboard = true      # Kitty keyboard protocol (default: true, set false if terminal has issues)
ref_extensions = ["md"]    # file extensions for ref/spec autocomplete (empty = all)
ref_paths = ["doc", "spec", "docs", "design", "papers"]  # directories for ref/spec autocomplete (empty = whole project)

[ui.tag_colors]
bug = "#FF4444"
design = "#44DDFF"

[ui.colors]
# custom state/UI color overrides (hex values)
```
