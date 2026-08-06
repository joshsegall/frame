---
name: managing-frame-tasks
description: >
  Manages tasks using the frame (`fr`) CLI, a markdown-based task tracker
  where `.md` files in a `frame/` directory are the source of truth. Use when
  the project contains a `frame/` directory, or when asked to create, modify,
  triage, query, or prioritize tasks, tracks, inbox items, or backlog order.
  Also use when asked to check what work is available, report progress, or
  file bugs and findings.
---

# Frame CLI for Agents

The `fr` CLI is the agent interface to frame. Humans use the TUI.
Tasks are markdown files inside a `frame/` directory in the repo.

---

## Concepts

### Project structure

Frame auto-discovers projects by walking up from the current directory
looking for a `frame/` directory (like git). Inside `frame/`:

- `project.toml` — config (tracks, ID prefixes, agent settings)
- `inbox.md` — unsorted capture (no IDs until triaged)
- `tracks/<id>.md` — ordered backlogs (e.g., `tracks/effects.md`)

Initialize with `fr init`.

### Tracks

Tracks are work streams. States: **active**, **shelved** (paused),
**archived** (finished). One active track can be **cc-focus** — tasks
here sort first in `fr ready --cc`.

### Task states

| Checkbox | State   | Meaning                         |
|----------|---------|---------------------------------|
| `[ ]`    | todo    | Not started                     |
| `[>]`    | active  | In progress                     |
| `[-]`    | blocked | Manually blocked (set by human) |
| `[x]`    | done    | Completed                       |
| `[~]`    | parked  | Deferred, not in active backlog |

### Metadata

- **ID** — track-prefixed (e.g., `EFF-014`). Subtasks use dots: `EFF-014.1`.
  An ID may carry a per-working-copy **actor token** before the number
  (`EFF-a14`, subtask `EFF-a14.b2`) so concurrent unsynced clones never collide.
  The token is part of the ID, not a decoration — never strip it. IDs stay
  copy-paste-stable: pass them verbatim to `--after`, `dep`, `show`, `state`,
  etc. `EFF-a14`, `EFF-14`, and `EFF-b14` are three distinct tasks.
- **Tags** — `#cc`, `#cc-added`, `#bug`, `#needs-input`, `#research`, `#design`
- **dep:** — IDs of blocking tasks
- **spec:** — paths to the docs this task implements
- **ref:** — paths to the files it touches
- **note:** — freeform text (can include code blocks)
- **added:** / **resolved:** — dates (auto-set)

`spec:` and `ref:` both hold **file paths relative to the project root**,
comma-separated, each optionally carrying a location: `doc/spec.md#section`,
`src/parser.rs:807`, `src/parser.rs:807-820`. Only the file is checked — the
location is kept and never validated. `fr check` reports a path with no file
behind it as an error, and `fr ref`/`fr spec` refuse to write one (pass
`--force` for a file you are about to create).

Paths are stored folded — `./sub/../real.md` is stored as `real.md` — and `add`,
`rm` and `set` match that way, so any spelling of a file reaches it and a list
never holds one file twice. The location suffix is part of the identity:
`rm src/parser.rs` does not remove `src/parser.rs:807`.

**Refs have to travel.** `add`/`set` refuse a path that escapes upward
(`../notes.md`), one that is absolute (`/etc/hosts`, and also
`/abs/path/into/the/project`), and one git is ignoring (`scratch/notes.md`) —
each resolves here and nowhere else. A file that is tracked despite an ignore
rule is fine, and outside a git repo nothing is checked. `rm` is never refused,
so an existing one can always be taken out.

Subtasks nest up to 3 levels. Position in the backlog *is* priority —
there are no priority fields.

A task appears in `fr ready` output when it is `todo`, not `blocked`,
and has no unresolved dependencies. Blocked state and dependencies are
independent: `blocked` is set manually by the human, while `dep:` tracks
explicit task-to-task dependencies. Either one prevents a task from
being ready.

### Working copies and actor tokens

Each working copy mints IDs in its own **actor namespace**, so two clones that
haven't synced never hand out the same number. The token is the letter before
the number (`EFF-a14`) — see Metadata above.

Git worktrees of one clone are **not** separate actors. A linked worktree
inherits its clone's token: nothing is written, and no new actor appears in the
registry. This is automatic and needs no setup.

**Do not run `fr actor claim`.** A clone's main working tree claims a token
automatically on its first mint; a linked worktree never claims at all, because
it inherits. Claiming one by hand in a worktree splits a single clone into two
actors — the exact failure the inheritance rule exists to prevent. If a mint
fails and asks you to claim a token, report that to the human instead of
resolving it yourself.

`fr actor` with no subcommand shows the current token and which tier it came
from. `fr info` shows it too, alongside the ID frontier.

### Tag conventions

| Tag           | Meaning                                 |
|---------------|-----------------------------------------|
| `#cc`         | Agent can take this autonomously        |
| `#cc-added`   | Filed by the agent                      |
| `#bug`        | Something broken                        |
| `#needs-input`| Needs human judgment to proceed         |
| `#research`   | Exploratory investigation               |
| `#design`     | Producing a design doc or spec          |

---

## Workflows

### Pick up work

```bash
# Check for cc-tagged tasks across all active tracks (focus track first)
fr ready --cc

# Or see all unblocked tasks across active tracks
fr ready

# Filter by track
fr ready --track effects

# Read task details and deps before starting
fr show EFF-014 --context
fr deps EFF-014

# Claim it
fr state EFF-014 active
```

### Report progress

```bash
fr state EFF-014.1 done
fr note EFF-014 "Row unification needs special handling for polymorphic effects"
fr ref EFF-014 add src/effects/infer.rs src/effects/solve.rs:142
fr state EFF-014 done
```

### File findings

```bash
# Don't know which track → inbox
fr inbox "Parser crashes on empty effect block" --tag bug

# Know the track → add directly
fr add effects "Handle empty effect blocks in parser" --found-from EFF-014

# Break work into subtasks
fr sub EFF-014 "Add effect variables to closure types"
fr sub EFF-014 "Unify effect rows during inference"
fr sub EFF-014 "Test with nested closures"
```

### When stuck

- **`fr ready --cc` returns nothing** → Check the `cc_only` field in
  `fr ready --cc --json` output.
  - If `cc_only` is `true`: Do not broaden your search. Run `fr blocked`
    to see if you can unblock a `#cc` task. If not, stop and ask the
    human for direction.
  - If `cc_only` is `false`: Run `fr ready` to see all unblocked tasks
    across active tracks. Pick up the highest-priority task you can handle.
    If still nothing, check `fr blocked` or ask the human.
- **Task is blocked** → Check `fr deps <id>` to see what's blocking it.
  If the blocker is something you can do, pick that up first. If it needs
  human input, tag the blocker `#needs-input` and move on.
- **A write is rejected because the track is shelved** → The track is paused
  work, not a bad ID. Usually a stale `--track` argument: check `fr tracks` and
  pick an active one. Don't run `fr track activate` to work around it — that's
  a human decision about what work is in flight.
- **A mint fails asking you to claim an actor token** → Report it to the human.
  Do not run `fr actor claim`; see Working copies and actor tokens.
- **Unsure which track** → Use `fr inbox` and let the human triage it.
- **Task is too large** → Break it down with `fr sub` before starting.
- **Conflicting or unclear spec** → Add a note with `fr note` explaining
  the ambiguity, tag `#needs-input`, and pick up different work.

---

## Command Reference

### Global flags

| Flag | Description |
|------|-------------|
| `--json` | Machine-parseable output — see Conventions |
| `-C <path>` | Run against a different project directory |
| `--version` | Print version and the build's commit |

### Project init

| Command | Description |
|---------|-------------|
| `fr init` | Initialize a new frame project in the current directory |
| `fr init --name "name"` | Set project name (default: directory name) |
| `fr init --track <id> "name"` | Create an initial track (repeatable) |

### Reading

| Command | Description |
|---------|-------------|
| `fr ready` | Unblocked todo tasks across active tracks |
| `fr ready --cc` | cc-tagged tasks across all active tracks (focus track first) |
| `fr ready --track <id>` | Unblocked tasks on a specific track |
| `fr ready --tag <tag>` | Unblocked tasks with a specific tag |
| `fr list [track]` | List tasks (all active tracks, or one track) |
| `fr list --state <state>` | Filter by state (todo/active/blocked/done/parked) |
| `fr list --tag <tag>` | Filter by tag |
| `fr list --all` | Include shelved and archived tracks |
| `fr show <id>` | Full task details |
| `fr show <id> --context` | Task details with ancestor context |
| `fr search <pattern>` | Regex search across tasks, inbox, and archives |
| `fr search <pattern> --track <id>` | Search within one track |
| `fr deps <id>` | Dependency tree for a task |
| `fr blocked` | Blocked tasks and their blockers |
| `fr tracks` | All tracks with stats |
| `fr stats` | Task count summary for active tracks |
| `fr stats --all` | Include shelved tracks in stats |
| `fr recent` | Recently completed tasks |
| `fr recent --limit <n>` | Limit results (default: 20) |
| `fr inbox` | List inbox items |
| `fr check` | Validate project integrity — read-only; see Maintenance for what it covers |
| `fr check --fix` | **Human's call** — applies repairs. Report findings instead of fixing them |
| `fr info` | Project identity: version and build commit, name, frame dir, actor token, ID frontier, track count (report which clone you're on) |
| `fr actor` | Show this working copy's actor token and which tier it resolved from |

### Creating tasks

| Command | Description |
|---------|-------------|
| `fr add <track> "title"` | Add task to bottom of track's backlog |
| `fr add <track> "title" --after <id>` | Insert after a specific task |
| `fr add <track> "title" --found-from <id>` | Add with discovery context note |
| `fr push <track> "title"` | Add task to top of track's backlog |
| `fr sub <id> "title"` | Add a subtask under a parent task |
| `fr inbox "text"` | Capture to inbox |
| `fr inbox "text" --tag bug` | Capture with tag |
| `fr inbox "text" --note "details"` | Capture with body text |

A **shelved** track does not accept new tasks. `add`, `push`, `sub`, `import`,
`triage`, and `mv --track` all reject one, pointing at `fr track activate`. A
stale `--track` argument is the usual cause — check `fr tracks` before assuming
the track ID is wrong.

### Modifying tasks

| Command | Description |
|---------|-------------|
| `fr state <id> <state>` | Change state. Setting a backlog task to `done` moves it to Done immediately |
| `fr start <id>` | Shortcut for `state <id> active` (rejected if the track is shelved) |
| `fr done <id>` | Shortcut for `state <id> done` |
| `fr tag <id> add <tag>` | Add a tag |
| `fr tag <id> rm <tag>` | Remove a tag |
| `fr dep <id> add <dep-id>` | Add a dependency |
| `fr dep <id> rm <dep-id>` | Remove a dependency |
| `fr note <id> "text"` | Set task note |
| `fr ref <id> add <path>...` | Add file references (`src/x.rs`, `src/x.rs:807`) |
| `fr ref <id> rm <path>...` | Remove file references |
| `fr ref <id> set <path>...` | Replace the whole ref list |
| `fr spec <id> add\|rm\|set <path>...` | Same three actions for `spec:` |
| `fr title <id> "new title"` | Change task title |
| `fr mv <id> --top` | Move task to top of its section |
| `fr mv <id> --after <id>` | Move after another task |
| `fr mv <id> <position>` | Move to numeric position (0-indexed) |
| `fr mv <id> --track <track>` | Move to different track (rewrites ID prefix, updates deps) |
| `fr mv <id> --promote` | Promote subtask to top-level (re-keys IDs) |
| `fr mv <id> --parent <id>` | Reparent under another task (re-keys IDs) |

`fr mv` operates on a top-level task in whichever section holds it — Backlog,
Parked, or Done — not just the Backlog. A cross-track move lands the task in the
*same* section on the target, so a done task stays done and keeps its
`resolved:` date rather than being reopened into the backlog.

### Triage & import

| Command | Description |
|---------|-------------|
| `fr triage <index> --track <id>` | Move inbox item to a track (1-based index) |
| `fr triage <index> --track <id> --top` | Triage to top of backlog |
| `fr triage <index> --track <id> --after <id>` | Triage after a specific task |
| `fr import <file.md> --track <id>` | Import tasks from a markdown file |
| `fr import <file.md> --track <id> --top` | Import at top of backlog |

### Track management

| Command | Description |
|---------|-------------|
| `fr track new <id> "name"` | Create a new track |
| `fr track shelve <id>` | Shelve (pause) a track |
| `fr track activate <id>` | Activate a shelved track |
| `fr track archive <id>` | Archive a finished track |
| `fr track mv <id> <position>` | Reorder active tracks (0-indexed) |
| `fr track cc-focus <id>` | Set the cc-focus track |
| `fr track cc-focus --clear` | Clear the cc-focus setting |
| `fr track rename <id> --name "name"` | Rename display name |
| `fr track rename <id> --new-id <new-id>` | Change track ID (moves file) |
| `fr track rename <id> --prefix <PREFIX> --yes` | Bulk-rewrite task ID prefixes |
| `fr track rename <id> --prefix <PREFIX> --dry-run` | Preview prefix rename |
| `fr track delete <id>` | Delete an empty track |

### Multi-project

| Command | Description |
|---------|-------------|
| `fr projects` | List registered projects (sorted by last access) |
| `fr projects add <path>` | Register a project by path |
| `fr projects remove <name_or_path>` | Remove from registry |
| `fr -C <path> <command>` | Run command against a different project |

Projects auto-register on `fr init` or first use. Registry:
`~/.config/frame/projects.toml`.

### Actor tokens

Only `fr actor` (no subcommand) is safe to run unprompted — it just reports.
Everything below writes identity state shared across the clone, and is the
**human's** call. See Working copies and actor tokens above.

| Command | Description |
|---------|-------------|
| `fr actor` | Show the current token and which tier it resolved from |
| `fr actor list` | All tokens with state and provenance |
| `fr actor claim` | Auto-claim a token — **do not run**; working copies claim on first mint |
| `fr actor set <token>` | Claim a specific token (`--local` for this worktree only) |
| `fr actor retire <token>` | Tombstone a token (stays reclaimable) |
| `fr actor merge <from>... --into <t>` | Human repair for accumulated tokens: renumbers IDs into one namespace and retires the sources. Preview with `--dry-run` |

### Maintenance

| Command | Description |
|---------|-------------|
| `fr clean` | Assign IDs/dates, archive done tasks, reconcile sections, validate |
| `fr clean --dry-run` | Preview what clean would do |
| `fr check` | Read-only validation: deps, refs, duplicate and reissued IDs, unclosed code fences, actor-registry drift, local files leaking into git, ID-frontier health, interrupted operations, `#lost` tasks, recovery log |
| `fr check --fix` | Applies repairs — **do not run unprompted**, see below |
| `fr delete <ids>...` | **Permanently** delete tasks (`--yes` skips the prompt) |
| `fr recovery` | View recovery log entries (most recent first) |
| `fr recovery prune [--all]` | Remove old recovery log entries |
| `fr recovery path` | Print path to recovery log file |

`fr delete` is permanent and has no undo on the CLI. Prefer `fr state <id>
parked` for work that is being set aside, or ask the human. Delete only when
explicitly told to.

`fr check` is read-only and safe to run any time. **`fr check --fix` is not** —
it rewrites task notes, edits `.gitignore`, and (with `--yes`) deletes duplicate
archive entries. Report what `fr check` found and let the human decide. `fr
clean` is the routine maintenance you *can* run: it assigns IDs and dates,
archives finished work, and reconciles sections.

---

## Conventions

### Structured output

Append `--json` to any read command for machine-parseable output.
Always use `--json` when parsing output programmatically — human
formats are for display only and may change.

### Agent-filed tasks

Always tag tasks you create with `#cc-added` so the human knows an agent
filed them. This is not automatic — you must include it explicitly.
Include the tag inline in the title (it gets parsed out automatically):

```bash
fr add effects "Handle empty effect blocks #cc-added" --found-from EFF-014
```

### `fr inbox` vs `fr add`

- **`fr inbox`** — unsure which track, or quick note needing human triage
- **`fr add <track>`** — you know the track
- **`--found-from <id>`** — discovered while working on another task

### Subtask context

Always use `--context` when showing a task. Parent tasks often contain
specs, notes, and dependencies that explain why the subtask exists. In
`--json` mode, ancestor context is included automatically.

### Subtask structure

Use `fr sub` to break work down. Subtasks get dotted IDs automatically
(`EFF-014.1`, `EFF-014.2`). Mark each done individually.

### Maintenance

Run `fr clean` periodically and `fr check` for read-only validation.

---

## Common Mistakes

- **`fr set` does not exist.** The command is `fr state <id> <state>`. There is no `set` subcommand.

---

## Example Session

```bash
# 1. Check for available work
fr ready --cc
# → [infra] [ ] INFRA-015 Add span tracking to HIR nodes #cc

# 2. Read details
fr show INFRA-015 --context

# 3. Claim it
fr state INFRA-015 active

# 4. Break into subtasks
fr sub INFRA-015 "Add Span field to HirNode struct"
fr sub INFRA-015 "Thread spans through lowering pass"
fr sub INFRA-015 "Update error reporting to use spans"

# 5. Work through subtasks
fr state INFRA-015.1 done
fr ref INFRA-015 add src/hir/mod.rs

fr state INFRA-015.2 done
fr note INFRA-015 "Lowering pass now preserves source spans from AST"

# 6. Discover a bug while working
fr inbox "Span merging loses column info for multi-line expressions" --tag bug

# 7. Finish and close
fr state INFRA-015.3 done
fr state INFRA-015 done

# 8. Maintenance
fr clean
```
