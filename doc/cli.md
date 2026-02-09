# CLI Reference

The Frame CLI binary is `fr`. Run with no arguments to launch the TUI.

**Global flags**:
- `--json` — output as JSON (on commands that support it)
- `-C <path>` / `--project-dir <path>` — run against a different project directory without changing the working directory

## Project Init

### `fr init`

Initialize a new Frame project in the current directory.

```
fr init [--name NAME] [--track ID NAME]... [--force]
```

| Flag | Description |
|------|-------------|
| `--name NAME` | Project name (default: directory name) |
| `--track ID NAME` | Create an initial track (repeatable) |
| `--force` | Reinitialize even if `frame/` already exists |

Creates `frame/` with `project.toml`, `inbox.md`, and any specified track files. If the directory is a git repository, adds `frame/.state.json` and `frame/.lock` to `.gitignore`.

## Reading Commands

### `fr list [TRACK]`

List tasks in a track.

```
fr list [TRACK] [--state STATE] [--tag TAG] [--all]
```

| Flag | Description |
|------|-------------|
| `TRACK` | Track ID (default: all active tracks) |
| `--state STATE` | Filter by state: `todo`, `active`, `blocked`, `done`, `parked` |
| `--tag TAG` | Filter by tag |
| `--all` | Include shelved and archived tracks |

Shows Backlog + Parked sections. Done section only shown when `--state done`.

### `fr show ID`

Show full details for a task, including metadata and subtasks.

```
fr show EFF-014
```

### `fr ready`

Show tasks that are ready to work on (todo state, no unresolved dependencies).

```
fr ready [--cc] [--track TRACK] [--tag TAG]
```

| Flag | Description |
|------|-------------|
| `--cc` | Show only `#cc`-tagged tasks on the cc-focus track |
| `--track TRACK` | Filter to specific track |
| `--tag TAG` | Filter by tag |

### `fr blocked`

Show all blocked tasks with their blocking dependencies.

```
fr blocked
```

### `fr search PATTERN`

Search tasks and inbox by regex pattern.

```
fr search PATTERN [--track TRACK]
```

| Flag | Description |
|------|-------------|
| `--track TRACK` | Limit to specific track |

Searches across all fields: ID, title, tags, notes, deps, refs, spec. Includes inbox items (title, tags, body) when no track filter is set.

### `fr inbox`

List inbox items (1-based numbering).

```
fr inbox
```

### `fr tracks`

List all tracks grouped by state (active, shelved, archived) with task counts.

### `fr stats`

Show aggregate task statistics across all active tracks in a tabular format.

```
fr stats [--all]
```

| Flag    | Description            |
|---------|------------------------|
| `--all` | Include shelved tracks |

### `fr recent`

Show recently completed tasks.

```
fr recent [--limit N]
```

| Flag | Description |
|------|-------------|
| `--limit N` | Maximum items (default: 20) |

### `fr deps ID`

Show the dependency tree for a task. Detects circular dependencies and missing references.

```
fr deps EFF-014
```

### `fr check`

Validate project integrity (read-only). Reports dangling dependencies, broken refs/specs, duplicate IDs, missing metadata, and format warnings.

## Task Creation

### `fr add TRACK TITLE`

Add a task to the bottom of a track's Backlog.

```
fr add TRACK TITLE [--after ID] [--found-from ID]
```

| Flag | Description |
|------|-------------|
| `--after ID` | Insert after this task instead of at bottom |
| `--found-from ID` | Add note "Found while working on ID" |

Auto-generates a task ID using the track's configured prefix.

### `fr push TRACK TITLE`

Add a task to the **top** of a track's Backlog.

```
fr push api "Fix authentication bug"
```

### `fr sub ID TITLE`

Add a subtask under an existing task.

```
fr sub EFF-014 "Handle edge case"
```

Auto-generates subtask ID in `PARENT.N` format (e.g., `EFF-014.3`).

### `fr inbox TEXT`

Add an item to the inbox.

```
fr inbox TEXT [--tag TAG]... [--note NOTE]
```

| Flag | Description |
|------|-------------|
| `--tag TAG` | Add tag (repeatable) |
| `--note NOTE` | Add note body |

## Task Modification

### `fr state ID STATE`

Change a task's state.

```
fr state EFF-014 active
```

States: `todo`, `active`, `blocked`, `done`, `parked`. Setting a top-level Backlog task to `done` moves it to the Done section immediately.

### `fr tag ID ACTION TAG`

Add or remove a tag.

```
fr tag EFF-014 add ready
fr tag EFF-014 rm ready
```

### `fr dep ID ACTION DEP_ID`

Add or remove a dependency.

```
fr dep EFF-015 add EFF-014
fr dep EFF-015 rm EFF-014
```

Adding validates the dependency task exists.

### `fr note ID TEXT`

Set a task's note (replaces existing).

```
fr note EFF-014 "Found while working on closures"
```

### `fr ref ID PATH`

Add a file reference.

```
fr ref EFF-014 doc/design/effects.md
```

### `fr spec ID PATH`

Set the spec reference.

```
fr spec EFF-014 doc/spec.md#closure-effects
```

### `fr title ID TITLE`

Change a task's title.

```
fr title EFF-014 "New title text"
```

### `fr mv ID`

Move a task (reorder within track or cross-track).

```
fr mv ID [POSITION] [--top] [--after ID] [--track TRACK]
```

| Flag | Description |
|------|-------------|
| `POSITION` | 0-indexed position in backlog |
| `--top` | Move to top of backlog |
| `--after ID` | Move after this task |
| `--track TRACK` | Move to a different track (cross-track) |

Cross-track moves rewrite the task's ID prefix to match the target track and update all dependency references in other tracks.

### `fr triage INDEX --track TRACK`

Move an inbox item to a track, converting it to a task.

```
fr triage INDEX --track TRACK [--top] [--bottom] [--after ID]
```

| Flag | Description |
|------|-------------|
| `INDEX` | Inbox item number (**1-based**) |
| `--track TRACK` | Target track (required) |
| `--top` | Insert at top of backlog |
| `--bottom` | Insert at bottom (default) |
| `--after ID` | Insert after this task |

## Track Management

### `fr track new ID NAME`

Create a new track.

```
fr track new api "API Layer"
```

Creates the `.md` file, adds to `project.toml`, generates an ID prefix.

### `fr track shelve ID`

Set track state to `shelved` (hidden from default listings).

### `fr track activate ID`

Set track state to `active`.

### `fr track archive ID`

Set track state to `archived` and move file to `frame/archive/`.

### `fr track delete ID`

Delete an empty track (no tasks, no archive files). Non-empty tracks must be archived instead.

### `fr track mv ID POSITION`

Reorder a track to a new position (0-indexed among active tracks).

### `fr track cc-focus ID`

Set the cc-focus track (used by `fr ready --cc`).

### `fr track rename ID`

Rename a track's name, ID, or task prefix.

```
fr track rename ID [--name NAME] [--new-id NEW_ID] [--prefix PREFIX] [--dry-run] [--yes]
```

| Flag | Description |
|------|-------------|
| `--name NAME` | New display name |
| `--new-id NEW_ID` | New track ID |
| `--prefix PREFIX` | New task ID prefix (bulk-rewrites all task IDs and cross-track dep references) |
| `--dry-run` | Preview changes without writing |
| `-y`, `--yes` | Auto-confirm prefix rename |

At least one of `--name`, `--new-id`, or `--prefix` is required. Flags can be combined.

## Maintenance

### `fr clean`

Run project maintenance.

```
fr clean [--dry-run]
```

Actions performed:
- Assign missing task IDs
- Add missing `added` dates
- Resolve duplicate IDs
- Archive done tasks exceeding the threshold
- Report dangling dependencies and broken refs
- Suggest actions (e.g., "all subtasks done — consider marking done")
- Generate `ACTIVE.md` summary

### `fr import FILE --track TRACK`

Import tasks from a markdown file into a track.

```
fr import tasks.md --track api [--top] [--after ID]
```

| Flag | Description |
|------|-------------|
| `--track TRACK` | Target track (required) |
| `--top` | Insert at top of backlog |
| `--after ID` | Insert after this task |

Parses checkbox tasks from the file, auto-assigns IDs, preserves existing metadata. Supports up to 3-level nesting.

## Project Registry

Frame maintains a global project registry at `~/.config/frame/projects.toml` (or `$XDG_CONFIG_HOME/frame/projects.toml`). Projects register automatically when you run `fr init`, use `fr` in a project directory, or add them explicitly.

### `fr projects`

List registered projects sorted by most recently accessed via CLI.

```
fr projects
```

Output includes project name, path (abbreviated with `~`), and relative time since last access. Missing projects (directory no longer exists) show `(not found)`.

### `fr projects add PATH`

Register a project by path. The path must contain a `frame/project.toml`.

```
fr projects add ../api-server
```

Relative paths are resolved to absolute.

### `fr projects remove NAME_OR_PATH`

Remove a project from the registry by name or path. This only removes the registry entry — no files are deleted.

```
fr projects remove design-system
```

If the name is ambiguous (multiple projects share the same name), specify by path instead.

### The `-C` Flag

Run any Frame command against a different project directory:

```
fr -C ~/code/api-server tasks
fr -C ~/code/api-server add bugs "Fix auth bug"
```

The `-C` flag also triggers auto-registration if the target project isn't already in the registry.
