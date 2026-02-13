# CLI Reference

The frame CLI binary is `fr`. Run with no arguments to launch the TUI.

**Global flags**:
- `--json` — output as JSON (on commands that support it)
- `-C <path>` / `--project-dir <path>` — run against a different project directory without changing the working directory

## Project Init

### `fr init`

Initialize a new frame project in the current directory.

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
fr show ID [--context]
```

| Flag | Description |
|------|-------------|
| `--context` | Include ancestor context (parent chain, root-first) |

With `--context`, each ancestor is shown with a `── Parent ──` separator and all its fields, followed by the target task with a `── Task ──` separator. Useful for subtasks whose parent tasks contain specs, notes, or dependencies that explain the subtask's purpose.

In JSON mode (`--json`), an `ancestors` array is always included regardless of `--context`. The array is ordered root-first and is empty for top-level tasks.

### `fr ready`

Show tasks that are ready to work on (todo state, no unresolved dependencies).

```
fr ready [--cc] [--track TRACK] [--tag TAG]
```

| Flag | Description |
|------|-------------|
| `--cc` | Show `#cc`-tagged tasks across all active tracks (focus track first) |
| `--track TRACK` | Filter to specific track |
| `--tag TAG` | Filter by tag |

With `--cc --json`, the output includes `focus_track` (may be `null` if unset) and `cc_only` fields so agents can determine whether to broaden their search when no `#cc` tasks are available.

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

List all tracks grouped by state (active, shelved, archived) with metadata (id, prefix, file, cc-focus).

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

### `fr start ID`

Start a task (shortcut for `fr state ID active`).

```
fr start EFF-014
```

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

### `fr delete ID...`

Permanently delete one or more tasks.

```
fr delete ID... [--yes]
```

| Flag | Description |
|------|-------------|
| `ID...` | One or more task IDs to delete |
| `--yes` | Skip confirmation prompt |

Deleted tasks are logged to the recovery log before removal. The entire subtask tree is deleted with the task.

### `fr mv ID`

Move a task (reorder within track, cross-track, or reparent).

```
fr mv ID [POSITION] [--top] [--after ID] [--track TRACK] [--promote] [--parent ID]
```

| Flag | Description |
|------|-------------|
| `POSITION` | 0-indexed position in backlog |
| `--top` | Move to top of backlog |
| `--after ID` | Move after this task |
| `--track TRACK` | Move to a different track (cross-track) |
| `--promote` | Promote subtask to top-level (placed after former parent by default) |
| `--parent ID` | Reparent under the given task (becomes last child) |

Cross-track moves rewrite the task's ID prefix to match the target track. Reparenting (`--promote` or `--parent`) re-keys the task and all descendant IDs to match the new parent structure. Both operations update all dependency references across tracks.

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

### `fr track cc-focus [ID] [--clear]`

Set or clear the cc-focus track. The cc-focus track is optional — when set, its tasks sort first in `fr ready --cc` output. Use `--clear` to remove the setting.

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

### `fr recovery`

View the recovery log (most recent entries first).

```
fr recovery [--limit N] [--since ISO-8601] [--json]
```

| Flag | Description |
|------|-------------|
| `--limit N` | Show at most N entries (default: 10) |
| `--since TIMESTAMP` | Only show entries after this ISO-8601 timestamp |
| `--json` | Output as JSON array |

### `fr recovery prune`

Remove old entries from the recovery log.

```
fr recovery prune [--before TIMESTAMP] [--all]
```

| Flag | Description |
|------|-------------|
| `--before TIMESTAMP` | Remove entries older than this timestamp (default: 30 days ago) |
| `--all` | Remove all entries |

### `fr recovery path`

Print the absolute path to the recovery log file.

```
fr recovery path
```

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

Run any frame command against a different project directory:

```
fr -C ~/code/api-server tasks
fr -C ~/code/api-server add bugs "Fix auth bug"
```

The `-C` flag also triggers auto-registration if the target project isn't already in the registry.
