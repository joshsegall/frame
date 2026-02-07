# Feature: `fr init`

## Overview

Bootstrap a new Frame project in the current directory.

---

## Command

```
fr init [--name "Project Name"] [--track <id> "name"]...
```

### Flags

| Flag | Description |
|------|-------------|
| `--name "Name"` | Set project name. Default: current directory name. |
| `--track <id> "name"` | Create an initial track. Repeatable. |

### Examples

```bash
# Minimal — infer name from directory
fr init

# Named project
fr init --name "Lace Compiler"

# With initial tracks
fr init --track effects "Effect System" --track infra "Infrastructure"
```

---

## What It Creates

```
<cwd>/
  frame/
    project.toml          # From template, with tracks if specified
    inbox.md              # Empty inbox
    tracks/               # Track files if --track used, empty dir otherwise
    archive/              # Empty, ready for done archiving
```

`.state.json` is NOT created by init — the TUI creates it on first launch.

---

## Prefix Generation

Track IDs are mapped to uppercase prefixes for task numbering.

**Rules:**
1. Take the **last hyphen-separated segment** of the track id
2. Uppercase the first **3 characters**
3. If the segment is shorter than 3 chars, use what's available (uppercase)

| Track ID | Last Segment | Prefix |
|----------|-------------|--------|
| `effects` | `effects` | `EFF` |
| `compiler-infra` | `infra` | `INF` |
| `unique-types` | `types` | `TYP` |
| `core` | `core` | `COR` |
| `ui` | `ui` | `UI` |
| `modules` | `modules` | `MOD` |
| `error-handling` | `handling` | `HAN` |

**Collision handling:** If two tracks generate the same prefix (e.g.
`type-inference` and `unique-types` both → `TYP`), append characters
from the earlier segment(s) until unique. In the example:
`type-inference` → `TYP`, `unique-types` → `UTY`. The second track
detected gets the modified prefix.

Users can always edit `[ids.prefixes]` in `project.toml` after init.
The generated prefixes are a starting point, not a constraint.

---

## Template: `project.toml`

The template is a static string embedded in the binary. Comments serve
as inline documentation for users editing the file later.

```toml
[project]
name = "{name}"

[agent]
cc_focus = ""
default_tags = ["cc-added"]

[clean]
auto_clean = true
done_threshold = 250
archive_per_track = true

# Directories to search for spec: and ref: path validation and autocomplete.
# Paths are relative to the project root (parent of frame/).
ref_paths = ["doc", "spec", "docs", "design", "papers"]

# --- Tracks ---
# Add tracks with [[tracks]] entries, or use: fr track new <id> "name"
#
# [[tracks]]
# id = "example"
# name = "Example Track"
# state = "active"
# file = "tracks/example.md"

# --- ID Prefixes ---
# Map track IDs to uppercase prefixes for task numbering.
# Auto-generated when tracks are created. Edit freely.
#
# [ids.prefixes]
# example = "EXA"

# --- UI Customization ---
# Uncomment and edit to override defaults.

[ui]
# # search only thse directories for references and spec autocomplete
ref_paths = ["doc", "spec", "docs", "design", "papers"]

# # include only these file extensions in refs and spec autocomplete
# ref_extensions = ["md", "txt", "rst", "pdf", "toml", "yaml"]
# show_key_hints = false
# tag_style = "foreground"        # "foreground" or "pill"
#
# [ui.colors]
# background = "#0C001B"
# text = "#A09BFE"
# text_bright = "#FFFFFF"
# highlight = "#FB4196"
# dim = "#5A5580"
# red = "#FF4444"
# yellow = "#FFD700"
# green = "#44FF88"
# cyan = "#44DDFF"
#
# [ui.tag_colors]
# research = "#4488FF"
# design = "#44DDFF"
# ready = "#44FF88"
# bug = "#FF4444"
# cc = "#CC66FF"
# cc-added = "#CC66FF"
# needs-input = "#FFD700"
```

When `--track` flags are provided, the track entries and prefix entries
are emitted uncommented in their respective sections, replacing the
example comments.

---

## Template: `inbox.md`

```markdown
# Inbox
```

---

## Template: Track Files

When `--track` is used, each track gets a file at `tracks/<id>.md`:

```markdown
# {Track Name}

> 

## Backlog

## Parked

## Done
```

The empty `> ` line prompts the user to add a description.

---

## Edge Cases

### Already initialized

If `frame/` exists in the current directory:

```
Error: Frame project already exists in ./frame/
```

Exit code 1. No `--force` flag. Delete `frame/` manually to reinit.

### Nested project

If a parent directory contains a Frame project (would be discovered
by the normal walk-up logic):

```
Note: parent project found at ../../frame/
Creating new project in ./frame/
```

Print the note to stderr, then proceed normally. Nested projects are
legitimate (monorepo with sub-projects).

### Directory name inference

`--name` default is the current directory's name, with hyphens
converted to spaces and title-cased: `my-cool-project` → `My Cool Project`.

If the directory name is unhelpful (e.g. `/tmp`), the user should
pass `--name`.

### Track ID validation

Track IDs must be lowercase alphanumeric with hyphens. No spaces, no
uppercase, no underscores. Validated on input:

```
Error: invalid track id "My Track" — use lowercase with hyphens (e.g. "my-track")
```

---

## Implementation

### Files touched

| File | Action |
|------|--------|
| `cli/commands.rs` | Add `Init` subcommand to clap |
| `cli/handlers/init.rs` | New handler |
| `main.rs` | Handle `init` before project discovery |

### Key implementation notes

1. **Pre-discovery dispatch**: `fr init` must be handled *before* the
   normal project discovery in `main.rs`. All other commands discover
   the project first, then dispatch. `init` is special — it creates
   the project.

2. **Template as const string**: The `project.toml` template lives as
   a `const &str` or `include_str!` in the init handler. The `{name}`
   placeholder is replaced with `str::replace`. Track and prefix
   sections are appended when `--track` flags are provided.

3. **Shared prefix logic**: The prefix generation function should live
   in `model/config.rs` or `ops/track_ops.rs` so `fr track new` can
   reuse it. Signature: `fn generate_prefix(track_id: &str, existing: &[String]) -> String`

4. **No locking needed**: Init creates new files in a new directory.
   No existing project state to coordinate with.

### Task breakdown

```
I1  Add Init subcommand to clap
    - Subcommand with --name and repeatable --track args
    - Handle in main.rs before project discovery

I2  Implement prefix generation
    - fn generate_prefix(track_id, existing_prefixes) -> String
    - Last-segment, first-3-chars, uppercase rule
    - Collision resolution by prepending chars from earlier segments
    - Unit tests for all cases in the table above + collisions
    - Place in shared location for fr track new reuse

I3  Implement init handler
    - Create frame/, tracks/, archive/ directories
    - Render project.toml from template with name substitution
    - If --track: append track entries and prefix entries, create
      track files, create tracks/ dir
    - Create inbox.md
    - Validate: frame/ doesn't already exist
    - Warn: parent project detected
    - Validate: track IDs are lowercase-hyphen format

I4  Update fr track new to use shared prefix generation
    - Replace any existing prefix logic with the shared function
    - Ensure collision detection considers existing prefixes in config
```
