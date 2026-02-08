# Frame — Multi-Project Support

## Overview

Frame becomes multi-project aware through a global project registry
stored at `~/.config/frame/projects.toml`. Projects register
automatically through normal usage. The TUI gains a project picker
overlay accessible via `Shift+P`, and the CLI gains a `fr projects`
subcommand family for registry management.

The design preserves Frame's single-project-at-a-time mental model.
Switching projects is a clean load — no split views, no tabs, no
ambient cross-project state. The picker is a fast overlay for jumping
between projects, not a dashboard.

---

## Project Registry

### Location

`~/.config/frame/projects.toml`

Created automatically on first use. Frame follows the XDG Base
Directory Specification — if `$XDG_CONFIG_HOME` is set, the file
lives at `$XDG_CONFIG_HOME/frame/projects.toml`. Using `~/.config`
is the established convention for CLI tools across Linux and macOS
(gh, git, docker, kubectl, etc.).

### Format

```toml
[[projects]]
name = "frame"
path = "/Users/josh/code/frame"
last_accessed_tui = 2025-02-08T14:30:00Z
last_accessed_cli = 2025-02-08T15:45:00Z

[[projects]]
name = "api-server"
path = "/Users/josh/code/api-server"
last_accessed_tui = 2025-02-07T09:15:00Z
last_accessed_cli = 2025-02-08T12:00:00Z
```

The `name` field is read from the project's `project.toml` at
registration time and cached here.

**Timestamps are split by interface:**

- `last_accessed_tui` — updates when the TUI opens or switches to
  this project. Used for sort order in the TUI project picker.
- `last_accessed_cli` — updates on any CLI command targeting this
  project. Used for sort order in `fr projects list`.

Each interface sorts by its own recency. The TUI timestamp
updates only on project load or switch — not while a project is
open. The CLI timestamp updates on any CLI command targeting the
project.

### Registration

Projects register automatically through three paths:

1. **`fr init`** — registers the project immediately after
   scaffolding.
2. **`fr` in a project directory** — if the current directory
   contains a `project.toml` and isn't already registered, Frame
   adds it to the registry before launching.
3. **`fr projects add <path>`** — explicit manual registration.

Duplicate paths are silently ignored. Registration never blocks
the user — it happens in the background and failures are
non-fatal.

---

## TUI: Project Picker

### Entry Points

| Context                         | Behavior                                |
|---------------------------------|-----------------------------------------|
| `fr` outside any project        | Project picker on launch                |
| `fr` inside a project           | Opens the project normally              |
| `Shift+P` inside the TUI       | Opens picker overlay                    |

When launched outside a project with no registered projects,
Frame displays a message:

```
No projects registered.

Run `fr init` in a project directory to get started,
or `fr projects add <path>` to register an existing project.
```

### Picker Overlay

The picker appears as a centered popup overlay on top of the
current view, consistent with Frame's existing popup patterns
(e.g., conflict resolution, triage).

```
┌─ projects ──────────────────────────┐
│                                     │
│ ▶ frame           ~/code/frame      │
│   api-server      ~/code/api        │
│   docs-site       ~/code/docs       │
│   design-system   (not found)       │
│                                     │
│  sorted by: recent                  │
│  ↑↓/jk navigate  Enter open        │
│  X remove  s sort  Esc close        │
└─────────────────────────────────────┘
```

**Layout:**
- Left column: project name (from `project.toml`), left-justified
- Right column: path (abbreviated with `~`), left-justified
- Cursor indicator: `▶` on the selected row
- Current project (if any) indicated with highlight color
- Missing projects shown in dim text with `(not found)`
- Sort indicator and key hints at the bottom of the popup

**Interactions:**

| Key         | Action                                        |
|-------------|-----------------------------------------------|
| `↑` / `↓`  | Navigate project list                         |
| `j` / `k`  | Navigate project list (vim-style)             |
| `Enter`    | Open selected project (clean load)            |
| `X`         | Remove selected project from registry         |
| `s`         | Toggle sort: recent (default) ↔ alphabetical  |
| `Esc`       | Close picker, return to current project        |

**Removing a project:**
Pressing `X` on a project shows a brief inline confirmation
(same pattern as existing destructive actions in Frame). This
removes the entry from the registry — it does not delete any
files on disk.

**Missing projects:**
If a registered project's directory no longer exists, the entry
remains in the picker but renders in dim text (`#5A5580`) with
`(not found)` replacing the path. The user can still select it
(Frame shows an error) or remove it with `X`.

### Launching Outside a Project

When `fr` launches outside any project directory, the picker
renders as a popup overlay over a blank background — same
component, same layout, same interactions. The only difference
is that `Esc` is not available (there's nothing to go back to).
`q` quits Frame from this screen.

### Switching Behavior

Opening a project from the picker is a clean load — equivalent
to quitting and running `fr` in that project's directory. No
additional TUI state carries over between projects beyond what
Frame already persists. Whatever session state Frame already
preserves (cursor position, selected track) is loaded from the
target project as usual.

The `last_accessed_tui` timestamp updates when the project opens.

---

## CLI: `fr projects`

### Subcommands

**`fr projects`** (alias: `fr projects list`)

Lists registered projects. Default sort: most recently accessed
via CLI (`last_accessed_cli`).

```
$ fr projects
  frame           ~/code/frame           2 min ago
  api-server      ~/code/api             yesterday
  docs-site       ~/code/docs            3 days ago
  design-system   (not found)            1 week ago
```

**`fr projects add <path>`**

Registers a project by path. The path must contain a valid
`project.toml`. Relative paths are resolved to absolute.

```
$ fr projects add ../api-server
Added: api-server (/Users/josh/code/api-server)
```

```
$ fr projects add ../not-a-project
Error: no project.toml found at /Users/josh/code/not-a-project
```

**`fr projects remove <name-or-path>`**

Removes a project from the registry by name or path.

```
$ fr projects remove design-system
Removed: design-system
```

### The `-C` Flag

`fr -C <path> <command>` runs any Frame command against a
different project directory without changing the working
directory. This is the primary mechanism for agents to interact
with multiple projects.

```
$ fr -C ~/code/api-server tasks
$ fr -C ~/code/api-server add "Fix auth bug" --track bugs
```

The `-C` flag also triggers auto-registration — if the target
directory has a `project.toml` and isn't registered, it gets
added to the registry.

---

## Colors

The picker follows Frame's established palette:

| Element                | Color                          |
|------------------------|--------------------------------|
| Popup border           | `#A09BFE` (Text)               |
| Header text            | `#A09BFE` (Text)               |
| Project name           | `#FFFFFF` (Text Bright)        |
| Path                   | `#5A5580` (Dim)                |
| Selected row name      | `#FFFFFF` (Text Bright)        |
| Selected row cursor    | `#FB4196` (Highlight)          |
| Current project name   | `#FB4196` (Highlight)          |
| Missing project        | `#5A5580` (Dim) for both cols  |
| Sort indicator         | `#5A5580` (Dim)                |
| Key hints              | `#A09BFE` (Text)               |

---

## Edge Cases

**Name collisions:** Two projects can share a name (e.g., both
called "api"). The path column disambiguates visually. The CLI
`remove` command matches name first; if ambiguous, it asks the
user to specify by path instead.

**Corrupted registry:** If `projects.toml` can't be parsed,
Frame backs it up as `projects.toml.bak` and starts with an
empty registry. A warning is printed to stderr.

**Concurrent access:** Multiple Frame instances may update the
registry simultaneously (e.g., two terminals). Frame reads the
file, merges changes (add-only or update `last_accessed_*`), and
writes atomically. Removes are immediate — no merge needed.

**Large registries:** The picker doesn't paginate. If someone
registers 100+ projects, the list scrolls. This is an unlikely
edge case and pagination can be added later if needed.

---

## Documentation Updates

When implementing this feature, the following files need to be
updated:

- **SKILLS.md** — add multi-project commands and keybindings
- **README.md** — mention multi-project support if the README
  covers feature overview or getting started
- **`doc/` directory** — update relevant docs (CLI reference,
  TUI keybindings, configuration) with registry location,
  `fr projects` subcommands, `Shift+P` picker, and `-C` flag
