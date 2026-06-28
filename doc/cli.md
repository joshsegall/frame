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

Searches across all fields: ID, title, tags, notes, deps, refs, spec. Includes inbox items (title, tags, body) when no track filter is set. Archived tasks (`frame/archive/*.md` files created by `fr clean`) are always included; archive results are prefixed with `[archive:track_id]`.

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

### `fr info`

Show project identity at a glance (read-only — never claims a token):

| Field      | Description                                                        |
|------------|--------------------------------------------------------------------|
| `version`  | `fr` crate version                                                 |
| `project`  | project name from `project.toml`                                   |
| `frame_dir`| absolute path to the discovered `frame/` directory                 |
| `actor`    | this clone's token — the literal token, `primary` (null), or `unclaimed` |
| `tracks`   | count of active tracks                                             |

```
fr info [--json]
```

With `--json`, the `actor` field distinguishes all three states for machine consumers: a literal token string (`"a"`), `"null"` for the primary clone, and JSON `null` when unclaimed. The JSON object also includes `shelved_tracks` and `archived_tracks` counts.

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

Auto-generates a task ID using the track's configured prefix, minted in this working copy's [actor-token namespace](concepts.md#minting-in-a-token-namespace) (the primary clone mints bare numbers like `EFF-14`; a clone with token `a` mints `EFF-a1`). The **first mint in an unclaimed clone auto-claims** a token and announces it once on stderr.

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

Auto-generates a subtask ID in `PARENT.N` format. The new last segment carries this clone's [actor token](concepts.md#minting-in-a-token-namespace) (e.g. clone `b` adds `EFF-014.b1`); the parent's segments are preserved. As with `fr add`, the first mint in an unclaimed clone auto-claims a token.

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

### `fr done ID`

Mark a task done (shortcut for `fr state ID done`).

```
fr done EFF-014
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

The re-minted ID segments are created in **this clone's** [actor-token namespace](concepts.md#minting-in-a-token-namespace) — the *mover's* namespace, not the original creator's — by scanning the target in that namespace (e.g. clone `c` moving `EFF-a14` into track INF produces `INF-c1`, and a moved subtree re-keys to `INF-c1.c1`, `INF-c1.c2`). This is the collision-free rule: only the mover writes its own namespace, so the re-mint can't clash with another clone's concurrent work. As with `fr add`, the first such move in an unclaimed clone auto-claims a token; if no token can be claimed the move aborts with the `fr actor set …` routing message and changes nothing. Because a cross-track move changes the ID prefix, the original creator's namespace is not preserved across the move.

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

Promoting an inbox item mints a new task ID in this clone's [actor-token namespace](concepts.md#minting-in-a-token-namespace) (auto-claiming a token on the first mint in an unclaimed clone).

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

IDs assigned or reassigned by a real (non-`--dry-run`) clean are minted in this clone's [actor-token namespace](concepts.md#minting-in-a-token-namespace), auto-claiming a token on first use. Archival, thresholds and `ACTIVE.md` key on task state and `resolved:` dates, not ID structure, so they are unaffected by the token. A `--dry-run` previews without claiming a token or writing anything.

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

Parses checkbox tasks from the file, auto-assigns IDs, preserves existing metadata. Supports up to 3-level nesting. Assigned IDs are minted in this clone's [actor-token namespace](concepts.md#minting-in-a-token-namespace), auto-claiming a token on the first mint in an unclaimed clone.

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

## Actor Tokens

Each working copy (git clone) holds one **actor token**, recorded in the committed `frame/actors.toml` registry and the gitignored `frame/.actor` file. See [concepts.md](concepts.md#actors) for the model. Tokens are managed today but not yet used in minted IDs.

### `fr actor`

Show this working copy's token and status. `null` is displayed as "primary (untokened)". Warns if the `.actor` token isn't recorded in the registry, and prints a notice when the never-used frontier is nearly empty.

```
fr actor
fr actor --json
```

### `fr actor claim [--name NAME]`

Auto-claim a token from the frontier (a random pick from the first few never-used safe letters, to scatter concurrent claims). Writes `.actor` and a registry row. Fails when no unused tokens remain, pointing you to `fr actor set` to reclaim a retired token or claim a custom multi-character one.

```
fr actor claim
fr actor claim --name josh-laptop
```

`--name` sets the registry provenance (default: the machine hostname).

### `fr actor set TOKEN [--name NAME]`

Claim a specific token. Accepts a single safe letter (`a–z` minus `i`, `l`, `o`), a multi-character token (`aa`, `foo`), or `null`. Reclaims a retired token by flipping it back to active. Refuses a token that another working copy already holds (retire it there first, or pick another). Idempotent if this clone already holds the token.

```
fr actor set b
fr actor set null          # record this clone as the primary
fr actor set team-ci --name ci-runner
```

`fr actor set null` is also the migration entry point: running it in a project that predates actor tokens creates `frame/actors.toml`.

### `fr actor retire TOKEN`

Tombstone a token (`state = retired`). It leaves the auto-assignment frontier but stays in the registry and can be reclaimed later with `fr actor set TOKEN`. If you retire your own clone's token, frame warns you to claim a new one.

```
fr actor retire b
```

### `fr actor list`

List all tokens with state and provenance. The current clone's token is marked with `*`.

```
fr actor list
fr actor list --json
```
