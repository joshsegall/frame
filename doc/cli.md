# CLI Reference

The frame CLI binary is `fr`. Run with no arguments to launch the TUI.

**Global flags**:
- `--json` — output as JSON (on commands that support it)
- `-C <path>` / `--project-dir <path>` — run against a different project directory without changing the working directory
- `-V` / `--version` — version plus the commit the binary was built from (`fr 0.1.6 (ad763b0)`); omits the commit when the build didn't come from a git checkout

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

Creates `frame/` with `project.toml`, `inbox.md`, and any specified track files. If the directory is a git repository, adds `frame/.*` to `.gitignore` — one pattern covering every working-copy-local file, present and future. See [`fr check`](#fr-check) for what those are and why they must not be committed.

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

Validate project integrity. Read-only unless `--fix` is passed. Reports dangling dependencies, broken refs/specs, duplicate IDs, missing metadata, and format warnings. Also flags actor issues: this clone's token drifting from `actors.toml`, and **multiple active tokens sharing one provenance name** (a sign a machine has accumulated tokens — e.g. a git-worktree-per-session workflow — with a suggested `fr actor merge` to collapse them).

It flags **ID collisions involving an archive**, which nothing else catches — the duplicate-ID *error* and `fr clean`'s duplicate resolution both compare live tracks only. These are two different problems and are reported separately:

- a **live task holding an archived task's ID**: the number was reissued after the original was archived (possible before the [ID frontier](architecture.md#id-frontier-durable-mint) became durable). Renumber the live task by hand.
- **one ID appearing more than once inside the archives**, with no live task involved: the same task's history was written twice, not a number handed out twice. Delete the extra copies. This came from an archive append that wasn't idempotent (now fixed), so it only appears on pre-existing data or a hand-edited archive.

Both are warnings rather than errors: there is no automatic repair, and they fire on data that predates the fixes. It also reports an **unreadable ID frontier store** (the next mint resets it and falls back to scanning, which can't see another worktree's uncommitted tasks) and a leftover `frame-ids.toml.bak`, which means the frontier *was* reset at some point and numbers minted in that window may have been reissued. Deleting the `.bak` clears that one.

It flags **working-copy-local frame files leaking into git** — `frame/.state.json`, `frame/.lock`, `frame/.recovery.log`, `frame/.actor`, `frame/.inflight`, and (for projects outside git, where the frontier store is working-copy-local) `frame/.ids.toml` and `frame/.ids.lock`. Committing these leaks machine-local state into shared history; the append-only recovery log also conflicts on every merge that touches it.

`fr init` covers them with a single `.gitignore` pattern, `frame/.*`, rather than an entry each. Enumeration can't cover a file that doesn't exist yet — a project created before an entry was added never got that line, and had to be told about it after the fact — whereas the pattern covers the next one automatically. **This depends on a rule: nothing under `frame/` that needs to be committed may start with a dot.** That is already the convention (`actors.toml` is the one deliberately shared machine-relevant file, and is deliberately not a dotfile); if a committed dotfile ever becomes necessary, a `!frame/.foo` line after the pattern is the escape hatch. The pattern covers dotfiles directly inside `frame/`, not nested ones.

Check reports a file that git already **tracks** (needs `git rm --cached <path>` as well as a `.gitignore` line — ignore rules don't apply to files already in the index) and one that exists but **isn't ignored** (the next `git add -A` commits it). Projects outside git are skipped.

It reports an **interrupted operation**: a multi-file operation that started and did not finish, recorded in `frame/.inflight`. Normally the next write command completes it and clears the marker, so seeing this means either nothing has been written since, or recovery declined to act because a precondition no longer held. See [Multi-file writes](architecture.md) and `fr recovery` for the detail.

Finally, it warns about **task notes and inbox item bodies that leave a code fence open**. Frame itself parses these correctly — a note's extent is set by [indentation, not fence state](format.md#metadata) — but an unclosed fence makes every markdown renderer downstream (GitHub, editor previews) swallow the rest of the file into a code block. The warning names the offending opener, e.g. ` ```rust `. Fence balance follows CommonMark, so a fence carrying an info string cannot close a block: ` ```lace ` / ` ```rust ` / ` ``` ` is balanced and does *not* warn.

#### `fr check --fix`

```
fr check --fix [--dry-run] [--yes]
```

Applies the repairs check would otherwise only describe. Bare `fr check` never writes — the repair path is only reached with `--fix`.

The plan is exactly what check reported: one warning in, at most one repair out. Six findings are repairable:

| Finding | Repair | Deletes? |
|---|---|---|
| unclosed note fence | append a closing fence to the note | no |
| unclosed inbox fence | append a closing fence to the body | no |
| local file not ignored | add `frame/.*` to `.gitignore` | no |
| duplicate archived ID | drop the extra copies, keeping one | **yes** |
| leftover `frame-ids.toml.bak` | delete the stale backup | **yes** |
| interrupted operation recovery declined | clear the `.inflight` marker | **yes** |

An **interrupted operation** (`frame/.inflight`) is normally not repaired here at all — the next write command completes it automatically and clears the marker. The repair above exists only for the case where recovery declined to act because a precondition no longer held, so the marker would otherwise stand forever with no way to acknowledge it.

Everything else check reports is left alone, because it has no repair that is safe to apply without a decision — renumbering a reissued ID rewrites something other work may reference, a `ref:` can be legitimately absent on the current branch, `fr actor merge` renumbers a whole namespace, and a `#lost` tag exists precisely to be read by a human. A local file git already **tracks** is only half-repairable: the `.gitignore` pattern is added, but `git rm --cached` is yours to run. However many local files are reported, the repair is one line — the pattern covers all of them, so a project that predates it migrates in one step rather than acquiring entries one incident at a time.

**Confirmation.** Repairs that delete prompt once before anything is written; `--yes` skips the prompt. Declining cancels the whole run, additive repairs included — a run applies its entire plan or none of it. With stdin closed (CI, an agent) the prompt reads nothing, which is not `y`, so the run cancels: pass `--yes` to mean it. Removed archive copies go to the [recovery log](#fr-recovery) before deletion, so a duplicate that was hand-edited after the first write is recoverable.

`--dry-run` prints the plan and writes nothing. Repairs are idempotent — a second `--fix` reports `nothing to repair`. `--json` reports `planned`, `applied`, `skipped`, and the `remaining` check result, re-read from disk after the write.

**This is not `fr clean`.** Clean handles what frame expects to do for you as work proceeds — minting IDs, filling `added:`/`resolved:` dates, resolving duplicate IDs, archiving finished work, reconciling sections — and it runs *unattended*, after every file reload in the TUI when `auto_clean` is on. So it may only do what is correct with nobody watching. `--fix` repairs damage: states that should never have arisen, where a human should have read the diagnosis first. That is the line, and it is not about how destructive a repair is — clean already archives and renumbers.

### `fr info`

Show project identity at a glance (read-only — never claims a token):

| Field      | Description                                                        |
|------------|--------------------------------------------------------------------|
| `version`  | `fr` crate version, plus the commit the binary was built from       |
| `project`  | project name from `project.toml`                                   |
| `frame_dir`| absolute path to the discovered `frame/` directory                 |
| `actor`    | this clone's token — the literal token, `primary` (null), or `unclaimed` |
| `tracks`   | count of active tracks                                             |
| `frontier` | last ID number handed out per prefix in this clone's namespace, and the [frontier store](architecture.md#id-frontier-durable-mint) it came from |

```
fr info [--json]
```

`version` shows the build's short commit in parentheses (`0.1.6 (ad763b0)`), as does `fr --version`. Nothing else does: the `--help` banner and the JSON `version` field stay on the bare crate version, so anything parsing a version string doesn't have to cope with a suffix — the JSON reports the commit separately in `commit` (`null` when the binary wasn't built from a git checkout).

`frontier` is normally invisible; it's what to look at when a minted ID isn't the number you expected. It reads `none recorded` before this clone has minted anything, and `unreadable` when the store is corrupt (which `fr check` explains).

With `--json`, the `actor` field distinguishes all three states for machine consumers: a literal token string (`"a"`), `"null"` for the primary clone, and JSON `null` when unclaimed. The JSON object also includes `shelved_tracks` and `archived_tracks` counts, and an `id_frontier` object (`path`, `state`, `namespace`, and `recorded` as a prefix → number map).

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

Auto-generates a task ID using the track's configured prefix, minted in this working copy's [actor-token namespace](concepts.md#minting-in-a-token-namespace) (the primary clone mints bare numbers like `EFF-14`; a clone with token `a` mints `EFF-a1`). The **first mint in an unclaimed clone auto-claims** a token and announces it once on stderr. A linked git worktree inherits its clone's token instead, and never auto-claims: if the clone has no token at all, the mint fails with a message pointing at `fr actor claim` (in the main working tree) or `fr actor claim --local` (here).

A [shelved](concepts.md#tracks) track rejects new tasks: `fr add`, `fr push`, `fr sub`, `fr import`, `fr triage`, and `fr mv --track` into it fail with a message pointing to `fr track activate`. Re-activate the track first.

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

States: `todo`, `active`, `blocked`, `done`, `parked`. Setting a top-level Backlog task to `done` moves it to the Done section immediately. Marking a task `active` is rejected when its track is [shelved](concepts.md#tracks) (re-activate the track first with `fr track activate`); other transitions on a shelved track's tasks are allowed.

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

Run project maintenance — the work frame expects to do for you as tasks come and go.

```
fr clean [--dry-run]
```

Actions performed:
- Assign missing task IDs
- Add missing `added` dates
- Add missing `resolved` dates to done tasks
- Resolve duplicate IDs
- Archive done tasks exceeding the threshold
- Move top-level tasks into the section matching their state
- Report dangling dependencies and broken refs
- Suggest actions (e.g., "all subtasks done — consider marking done")

**Clean runs unattended**, not only when you ask: with `auto_clean` on (the default) the TUI runs it after every file reload. So everything above must be correct with nobody watching and no output read — that constraint is what decides whether a repair belongs here or behind [`fr check --fix`](#fr-check---fix), which is invoked deliberately after a diagnosis has been read. Destructiveness is not the line: clean already archives tasks and renumbers IDs.

Missing `resolved:` dates are filled *after* archival, deliberately. Archive retention ranks done tasks by that date and treats a missing one as oldest, so stamping it earlier in the run would make the oldest task look like the newest completion — retained over genuinely recent work, and surfacing at the top of `fr recent`.

IDs assigned or reassigned by a real (non-`--dry-run`) clean are minted in this clone's [actor-token namespace](concepts.md#minting-in-a-token-namespace), auto-claiming a token on first use. Archival and thresholds key on task state and `resolved:` dates, not ID structure, so they are unaffected by the token. A `--dry-run` previews without claiming a token or writing anything.

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

### `fr projects prune`

Remove every registry entry whose project directory no longer exists (the same `(not found)` entries shown by `fr projects`). Useful for clearing out stale entries left behind by deleted or temporary projects.

```
fr projects prune            # remove all not-found entries
fr projects prune --dry-run  # list what would be removed, change nothing
```

Add `--json` for machine-readable output (an array of `{name, path}`). Only registry entries are removed — no project files are touched.

### The `-C` Flag

Run any frame command against a different project directory:

```
fr -C ~/code/api-server tasks
fr -C ~/code/api-server add bugs "Fix auth bug"
```

The `-C` flag also triggers auto-registration if the target project isn't already in the registry.

## Actor Tokens

Each working copy (git clone) holds one **actor token**, recorded in the committed `frame/actors.toml` registry and the gitignored `frame/.actor` file. A clone's linked git worktrees share its token — via the clone-wide `<git-common-dir>/frame-actor`, or by inheriting the main working tree's `frame/.actor` (which is where the primary's `null` lives). See [concepts.md](concepts.md#actors) for the model. Every minted ID carries its minter's token, so separate clones never collide; worktrees of one clone share a token and are kept apart by the [ID frontier](architecture.md#id-frontier-durable-mint) instead.

### `fr actor`

Show this working copy's token and status. `null` is displayed as "primary (untokened)". Also shows where the token came from — a local `frame/.actor`, the clone-wide shared token, or inherited from the main working tree (naming it) when run in a linked worktree. Warns if the token isn't recorded in the registry, and prints a notice when the never-used frontier is nearly empty. In an unclaimed *worktree* it explains that a mint there won't auto-claim, and gives both ways out.

```
fr actor
fr actor --json
```

### `fr actor claim [--name NAME] [--local]`

Auto-claim a token from the frontier (a random pick from the first few never-used safe letters, to scatter concurrent claims). Writes the clone-wide **shared** token (`<git-common-dir>/frame-actor`, inherited by every worktree) and a registry row. Fails when no unused tokens remain, pointing you to `fr actor set` to reclaim a retired token or claim a custom multi-character one.

```
fr actor claim
fr actor claim --name josh-laptop
fr actor claim --local     # claim only for this worktree (frame/.actor)
```

`--name` sets the registry provenance (default: the machine hostname). `--local` writes this worktree's `frame/.actor` instead of the shared token, so this one working copy diverges onto its own token — use it to run a worktree as a genuinely distinct, concurrent actor.

A clone-wide (non-`--local`) claim also **removes this working copy's local `frame/.actor`**, if it had one, and says so — the local file wins resolution, so leaving it would make the claim a silent no-op here. When run from a linked worktree, it warns if the *main* working tree still has a local override that shadows the new shared token there.

### `fr actor set TOKEN [--name NAME] [--local]`

Claim a specific token. Accepts a single safe letter (`a–z` minus `i`, `l`, `o`), a multi-character token (`aa`, `foo`), or `null`. Reclaims a retired token by flipping it back to active. Refuses a token that another working copy already holds (retire it there first, or pick another). Idempotent if this clone already holds the token. Writes the clone-wide shared token by default; `--local` (and always `null`) writes this worktree's `frame/.actor`.

```
fr actor set b             # set the shared token every worktree inherits
fr actor set b --local     # override just this worktree
fr actor set null          # record this clone as the primary (always local)
fr actor set team-ci --name ci-runner
```

As with `fr actor claim`, a non-`--local` `set` clears any local `frame/.actor` that would shadow the new clone-wide token.

`fr actor set null` is also the migration entry point: running it in a project that predates actor tokens creates `frame/actors.toml`.

### `fr actor retire TOKEN`

Tombstone a token (`state = retired`). It leaves the auto-assignment frontier but stays in the registry and can be reclaimed later with `fr actor set TOKEN`. If you retire your own clone's token, frame warns you to claim a new one.

```
fr actor retire b
```

### `fr actor merge FROM... --into TOKEN`

Collapse one or more actor namespaces into a single target token, renumbering every id minted by a `FROM` token into `--into`'s sequence and retiring the source tokens. Use it when a machine has accumulated several tokens (e.g. a git-worktree-per-session workflow, where each fresh working copy auto-claims its own token) and you want them back down to one.

The remap is **per-segment**: only segments minted by a `FROM` token change; a subtask minted by a third actor is preserved. Merging `d` and `f` into `b` turns `SEC-d1` into `SEC-b2` and `SEC-d1.a3` into `SEC-b2.a3` (actor `a`'s child is kept), never `SEC-b2.b1`. Numbers continue after the highest existing `--into` number, so no id collides. Ids across all tracks **and archives** are renumbered, and `dep:` references are rewritten to match.

`--into` must be an existing, active token. Source tokens are retired (tombstoned, reclaimable) on success. The full `OLD → NEW` mapping is printed (and available with `--json`).

```
fr actor merge d f --into b            # merge tokens d and f into b
fr actor merge d f --into b --dry-run  # preview the remap, write nothing
fr actor merge d f --into b --rewrite-notes  # also rewrite id mentions in note/spec/ref prose
```

By default, id mentions inside note/spec/ref **prose** are reported but not changed. `--rewrite-notes` rewrites them too, while skipping anything that looks like a git citation (e.g. `fix(SEC-d1)` or an id next to a commit sha), since those quote immutable history.

After a merge, any *other* working copy still holding a retired source token should be re-pointed — run `fr actor set <into>` there, or delete its `frame/.actor` so it inherits the shared token on next mint.

### `fr actor list`

List all tokens with state and provenance. The current clone's token is marked with `*`.

```
fr actor list
fr actor list --json
```
