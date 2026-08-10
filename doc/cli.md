# CLI Reference

The frame CLI binary is `fr`. Run with no arguments to launch the TUI.

**Global flags**:
- `--json` — output as JSON. Every command has a JSON surface except `fr merge`, whose interface is an exit status a VCS driver reads
- `-C <path>` / `--project-dir <path>` — run against a different project directory without changing the working directory
- `-V` / `--version` — version plus the commit the binary was built from (`fr 0.1.6 (ad763b0)`); omits the commit when the build didn't come from a git checkout

### `--json` on a command that writes

A write command reports what it did, in the same shapes the read commands use — so `fr add --json` returns exactly what `fr show --json` would for the task it created, rather than a second task shape to learn:

```json
{ "command": "add", "changed": true, "track": "main",
  "tasks": [ { "id": "MAI-042", "title": "write the parser", "state": "todo", "added": "2026-08-09" } ] }
```

`command` names the subcommand, so a consumer piping several can tell them apart. `tasks` is a list because `delete` and `import` act on several; for `delete` it is the snapshot taken *before* the deletion, since afterwards there is nothing left to describe. A command that acts on a track reports a `track` in the shape `fr tracks --json` lists.

**`changed` is not "did it succeed".** It is whether the project differs: `fr tag T-1 add cc` on a task already tagged `cc` succeeds and reports `changed: false`. A caller deciding whether to commit needs those told apart.

**`--json` never answers a confirmation prompt.** `fr delete`, `fr track rename --prefix` and `fr check --fix` ask before destroying data. Under `--json` the caller is a program, so blocking on a prompt would hang and confirming for it would let the flag silently escalate a destructive command. They fail instead, naming the flag that grants permission:

```
$ fr --json delete M-001
error: confirmation required for delete: pass --yes with --json
```

**Errors are not documents.** They go to stderr as `error: …` with a non-zero exit, and stdout stays empty — so a failed run never prints a result document describing changes that did not land. stdout carries the answer, stderr the error, the exit code the verdict.

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

Fields print in a fixed order — `conflict`, `added`, `resolved`, `dep`, `spec`, `ref`, `note` — with `--json` using the same sequence. Short fields first and the note last, because a note has no length bound and anything after one is past the fold. `--context`, the TUI Detail view and the markdown itself all use this order; see [format.md](format.md#field-order).

An existing file is not rewritten to match. Frame writes a task in canonical order the first time it edits that task, so a project converges task by task rather than in one sweeping diff, and `fr show` reads correctly either way.

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
fr search PATTERN [--track TRACK] [--no-archive] [--json]
```

| Flag | Description |
|------|-------------|
| `--track TRACK` | Limit to specific track |
| `--no-archive` | Skip archived tasks |

Searches across all fields: ID, title, tags, notes, deps, refs, spec. Includes inbox items (title, tags, body) when no track filter is set. Archived tasks (`frame/archive/*.md` files created by `fr clean`) are included by default — finding something you finished last month is a common reason to search at all — and are prefixed with `[archive:track_id]`. `--no-archive` skips them, for a project whose archives have grown large enough to bury live results.

Only active tracks are searched unless `--track` names one explicitly, matching `fr list`'s default — so a shelved track's tasks are found by `fr search --track shelved-id PATTERN` and not otherwise.

With `--json`, results come back as three arrays — `tasks`, `archived`, `inbox` — alongside the `pattern`. Concatenating them in that order gives the same sequence the human output prints. `archived` is always present, empty under `--no-archive`, so the shape doesn't change with the flag. Every entry carries `matched_fields`, listing *all* the fields that matched (`title`, `tag`, `note`, `dep`, …); the human output names a field only in the rare case where it cannot resolve the hit to a task.

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

Show the dependency tree for a task.

```
fr deps EFF-014 [--json]
```

Each node in the tree is one of four kinds, and the distinction matters:

| Marker | Meaning |
|--------|---------|
| *(none)* | resolved — the task was found and its own dependencies are expanded below |
| `(circular)` | the id is its own ancestor on this branch: a genuine cycle |
| `(already shown)` | the task was expanded elsewhere in the same tree — a shared dependency, not a problem |
| `(not found)` | no task anywhere in the project holds this id |

`(already shown)` is what a *diamond* looks like: two tasks depending on the same third. That is the ordinary shape of a real backlog, and it is not a cycle — each id is expanded once per tree, and the second reference points at the expansion rather than repeating it.

With `--json`, the same tree is emitted nested, each node carrying `id` and `status` (`resolved` / `cycle` / `repeat` / `missing`). A `resolved` node also carries `track`, `title`, `state`, `tags` and its own `deps`; the other three carry `id` and `status` only, since their full record is either elsewhere in the same document or nonexistent.

### `fr check`

Validate project integrity. Read-only unless `--fix` is passed.

**Exit status: 0 when the project has no errors, 1 when it has any** — so `fr check && git commit` and a CI step both work without grepping stdout. Warnings do not affect it: the status answers "is this project sound", and a warning is by definition something frame is willing to live with. `--json` sets the same status, agreeing with the `valid` field. `--fix` follows the rule on the state it leaves behind, including when it had nothing to repair — most errors have no repair by design, so "nothing to repair" is the common way a broken project leaves `--fix`.
 Reports dangling dependencies, broken refs/specs, duplicate IDs, missing metadata, and format warnings. Also flags actor issues: this clone's token drifting from `actors.toml`, and **multiple active tokens sharing one provenance name** (a sign a machine has accumulated tokens — e.g. a git-worktree-per-session workflow — with a suggested `fr actor merge` to collapse them).

It flags **refs that resolve here and nowhere else** — a `ref:`/`spec:` path that is absolute or escapes the project root, and one git is ignoring. These are the same paths `fr ref add` refuses, applied to values already in a file: written by `--force`, by an older `fr`, in the TUI, or by hand. They are **warnings**, not errors, because they resolve — nothing about the project is invalid here, and a passing project should not go red because a rule was added later. There is no `--fix`: which file inside the project was meant is a guess, and un-ignoring one is a decision about the repository rather than the task. The gitignore half is silent outside a git repository, and never fires on a file that is tracked despite a rule.

It flags **ID collisions involving an archive**, which nothing else catches — the duplicate-ID *error* and `fr clean`'s duplicate resolution both compare live tracks only. These are two different problems and are reported separately:

- a **live task holding an archived task's ID**: the number was reissued after the original was archived (possible before the [ID frontier](architecture.md#id-frontier-durable-mint) became durable). Renumber the live task by hand.
- **one ID appearing more than once inside the archives**, with no live task involved: the same task's history was written twice, not a number handed out twice. Delete the extra copies. This came from an archive append that wasn't idempotent (now fixed), so it only appears on pre-existing data or a hand-edited archive.

It also flags **archived IDs left on a prefix their track no longer uses**. `fr track rename --prefix` used to rename only the live tasks — it read the archive as a track file, found no `## Section` headers, and wrote nothing while reporting success — so a project renamed before that still holds archived IDs under the old prefix. It is a warning because nothing is wrong yet: those tasks are readable and their IDs unique. What it is one step away from is not: the abandoned prefix is not reserved, so giving it to another track hands that track a namespace whose numbers are already spent in a file its mint scan never looks at. `--fix` renames them onto the current prefix, and refuses if any would land on an ID that already exists.

The two collision findings above are warnings rather than errors for a different reason: there is no automatic repair, and they fire on data that predates the fixes. It also reports an **unreadable ID frontier store** (the next mint resets it and falls back to scanning, which can't see another worktree's uncommitted tasks) and a leftover `frame-ids.toml.bak`, which means the frontier *was* reset at some point and numbers minted in that window may have been reissued. Deleting the `.bak` clears that one.

It flags **working-copy-local frame files leaking into git** — `frame/.state.json`, `frame/.lock`, `frame/.actor`, `frame/.inflight`, and (for projects outside git, where the store is working-copy-local) `frame/.ids.toml`, `frame/.ids.lock` and `frame/.recovery.log`. Committing these leaks machine-local state into shared history; the append-only recovery log also conflicts on every merge that touches it. The recovery-log names stay on the list even though the log's default home is now inside `.git/`: a project outside git still keeps it in `frame/`, and one left there by an older frame must not be committed on its way out.

`fr init` covers them with a single `.gitignore` pattern, `frame/.*`, rather than an entry each. Enumeration can't cover a file that doesn't exist yet — a project created before an entry was added never got that line, and had to be told about it after the fact — whereas the pattern covers the next one automatically. **This depends on a rule: nothing under `frame/` that needs to be committed may start with a dot.** That is already the convention (`actors.toml` is the one deliberately shared machine-relevant file, and is deliberately not a dotfile); if a committed dotfile ever becomes necessary, a `!frame/.foo` line after the pattern is the escape hatch. The pattern covers dotfiles directly inside `frame/`, not nested ones.

Check reports a file that git already **tracks** (needs `git rm --cached <path>` as well as a `.gitignore` line — ignore rules don't apply to files already in the index) and one that exists but **isn't ignored** (the next `git add -A` commits it). Projects outside git are skipped.

It reports **a track with two sections of one kind** — two `## Done`, say — as an **error**. A line-by-line git merge of a track file produces this, and `Track::section_tasks` returns only the first, so everything in the second becomes invisible to archiving, section reconciliation and the roughly hundred call sites built on it, while remaining findable by ID. The file round-trips byte-identically, so it never heals on its own. **The next write merges the sections**, keeping every task in order — `fr check` itself is read-only and only reports it.

It reports **a `##` heading frame does not recognise** as an error, even when nothing is behind it. In a track file the parser sends an unknown heading to literal text, and every task line after it goes the same way until the next heading frame knows — so the heading is a trapdoor, and the next task written under it stops being a task. In an archive or the inbox, which have no sections, a heading below the title ends the task list. `frame/archive/_tracks/` is exempt from the second rule: those are whole archived track files and their `## Backlog` is correct. No `--fix`: whether the heading is a mistake or the content behind it belongs somewhere else are both decisions about someone's writing.

It flags **a track holding more open work than [`limits.track_warn_bytes`](concepts.md#limits)**, as one line per track:

```
warning: backend — 1.5MB of open work exceeds the 512KB limit (file is 3.0MB) — consider splitting the track or closing work
```

It names no individual task, deliberately: no single task is the problem, the aggregate is, and the remedy is splitting the track or closing work rather than editing any one of them. The measure is `## Backlog` plus `## Parked` — Done is excluded because [`[clean]`](concepts.md#clean) already bounds it automatically, and does so by swinging between `done_bytes_retain` and `done_bytes_threshold`; a warning that counted that swing would fire before a clean and clear after one with the open work untouched. The file size is shown for context and decides nothing. No `--fix`: open work cannot be archived, and how much of it belongs in one track is not frame's judgement to make.

An **oversize note is not reported at all.** `limits.note_max_bytes` is a guardrail on frame's own commands, not an invariant on the file, and a note that predates the limit is a supported state rather than damage.

It reports **unclaimed rescue copies**: files the TUI could not save and dumped into `frame/.rescue/` at exit (see [TUI save failures](tui.md)). The exit message names that directory once, on a terminal that is usually closed shortly afterwards — so without this the copies sit there being the only version of that work with nobody looking. A warning, and with no repair: moving a copy into place would overwrite a live file that may be newer, and deleting it destroys the thing the directory exists to protect. Clearing the directory clears the warning.

It reports an **interrupted operation**: a multi-file operation that started and did not finish, recorded in `frame/.inflight`. Normally the next write command completes it and clears the marker, so seeing this means either nothing has been written since, or recovery declined to act because a precondition no longer held. See [Multi-file writes](architecture.md) and `fr recovery` for the detail.

It reconciles the **track roster** in `project.toml` against the files in `tracks/`, in both directions, and reports either mismatch as an error. This is the one check that cannot work from the loaded project: `load_project` skips a configured track whose file is missing, so such a track — and every task in it — is absent from `fr list`, from the TUI, and from every other check here. Nothing else notices.

- a **missing track file**: `project.toml` names a file that is not there. An archived track is expected to live in `archive/_tracks/` instead, and is only reported when it is missing from there too.
- an **unreferenced track file**: a `.md` in `tracks/` that no `[[tracks]]` entry points at. Its tasks are invisible, and its IDs are invisible to the duplicate-ID check as well, so a collision with a live track goes unreported until the file is wired back in.

- an **unclaimed archived track file**: a `.md` in `archive/_tracks/` that no *archived* `[[tracks]]` entry claims. A **warning** where the two above are errors, because the consequence is milder — this is archived content, absent from views that would not have shown it anyway, so it does not fail a build. Three things reach it, and the message says which: no config entry carries that id at all (residue of a merge, a manual `mv`, or a `fr track rename --new-id` run on an archived track by a version of frame that allowed it); or an entry does but is `active` or `shelved`, meaning a copy stayed behind under `archive/` after the track came back out. That last one pairs with the missing-file error above when an unarchive was interrupted, and the two findings together name the file to move and where it goes. No `--fix`: adopting it invents an id, a name and a prefix, deleting it discards content, and when it is the far half of an old rename only the person who renamed it knows which id it should answer to.

The first two usually appear together, because the usual cause is one rename that only half landed — an interrupted `fr track rename --new-id`, a merge that took one side's `project.toml` and the other's file layout, a manual `mv`, or an editor's "rename file". Neither is repairable by `--fix`: dropping the config entry discards a track a `git checkout` may restore, recreating the file fabricates content, and adopting an unreferenced file invents an id, a name and an ID prefix when what you probably want is the original entry back.

It reports a **stranded line**: content frame could not attribute to any task — a line indented deeper than the level it sits at that is neither metadata, nor a task, nor part of a `- note:` block. Two findings, by where the line sits, because the likely remedy differs:

- `stranded_line` — the line sits *between* two tasks. The warning names the task it sits **above**. Usually mis-indented prose that belongs to whatever came before it.
- `stranded_line_under` — the line sits *inside* a task, past its metadata. The warning names the task it sits **under**. Usually a note that lost its `- note:` key, so adding one back is the usual fix.
 Frame keeps such a line exactly where it found it on every write (see [the conservation rule](architecture.md#selective-rewrite-parser-design)) but does not read it as anything, so `dep:`, `note:` or subtask content stranded this way is inert until the indentation is fixed. There is no automatic repair: where the line was meant to go is a guess.

Finally, it warns about **task notes and inbox item bodies that leave a code fence open**. Frame itself parses these correctly — a note's extent is set by [indentation, not fence state](format.md#metadata) — but an unclosed fence makes every markdown renderer downstream (GitHub, editor previews) swallow the rest of the file into a code block. The warning names the offending opener, e.g. ` ```rust `. Fence balance follows CommonMark, so a fence carrying an info string cannot close a block: ` ```lace ` / ` ```rust ` / ` ``` ` is balanced and does *not* warn.

#### `fr check --fix`

```
fr check --fix [--dry-run] [--yes]
```

Applies the repairs check would otherwise only describe. Bare `fr check` never writes — the repair path is only reached with `--fix`.

The plan is exactly what check reported: one warning in, at most one repair out. Eight findings are repairable:

| Finding | Repair | Deletes? |
|---|---|---|
| unclosed note fence | append a closing fence to the note | no |
| unclosed inbox fence | append a closing fence to the body | no |
| duplicate archived ID | drop the extra copies, keeping one | **yes** |
| leftover `frame-ids.toml.bak` | delete the stale backup | **yes** |
| interrupted operation recovery declined | clear the `.inflight` marker | **yes** |
| subtask ID that doesn't extend its parent's | renumber it under that parent | **yes** |
| archived IDs on a prefix the track dropped | rename them onto the current prefix | **yes** |

**Archived IDs on a dead prefix** are renamed onto the one the track uses now, which is what the prefix rename should have done in the first place. It counts as deleting for the same reason the subtask renumber does: the old IDs stop existing. If any ID would collide with one that already exists — live or archived — the whole file is skipped and the warning stays, naming the ID that blocked it, because a half-rename would leave one archive holding two prefixes with nothing recording which tasks moved.

A **subtask whose ID escaped its parent** — `M-020` nested under `M-003` — gets the next free child number under the parent it actually sits below, in the namespace its own ID already carries rather than yours. Its descendants are rekeyed with it and every `dep:` pointing at the old IDs follows. It counts as deleting because the old ID stops existing anywhere in the project, and frame cannot rewrite a reference you kept somewhere else. Renumbering a reissued *top-level* ID is not repairable for a related but different reason: there, two legitimate holders exist and which one moves is your call.

An **interrupted operation** (`frame/.inflight`) is normally not repaired here at all — the next write command completes it automatically and clears the marker. The repair above exists only for the case where recovery declined to act because a precondition no longer held, so the marker would otherwise stand forever with no way to acknowledge it.

Everything else check reports is left alone, because it has no repair that is safe to apply without a decision — renumbering a reissued ID rewrites something other work may reference, a `ref:` can be legitimately absent on the current branch, `fr actor merge` renumbers a whole namespace, and a `#lost` tag exists precisely to be read by a human.

**`--fix` does not touch git configuration.** Anything about `.gitignore`, `.gitattributes` or the merge driver is [`fr git setup`](#fr-git-setup)'s, whether it is a missing ignore pattern, an unregistered driver, or a local file git already tracks (which needs `git rm --cached` from you either way). `--fix` used to add the ignore pattern and nothing else, which left no way to predict which part of git readiness it would repair. One command owns that surface now, and `--fix` names it.

An **unresolved merge conflict** — a task still carrying the `conflict:` line `fr merge` left on it — is reported as an *error* with no repair, for the same reason a reissued ID is: which side should win is the judgment the merge could not make. See [`fr merge`](#fr-merge).

The message says where the other side actually is, having looked. The marker is committed and travels to every clone; the recovery log holding the discarded version is working-copy-local and does not. When the entry is here you get `fr recovery --for <ID>`; when it is not — a marker pulled from someone else's merge, or a pruned log — the message says so and sends you to version control instead. The `--json` form carries the same answer as `evidence`.

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
| `worktree` | the branch this **linked git worktree** has checked out, and the clone's main working tree. Omitted in the main tree, where there is nothing to distinguish |
| `actor`    | this clone's token — the literal token, `primary` (null), or `unclaimed` |
| `tracks`   | count of active tracks                                             |
| `frontier` | last ID number handed out per prefix in this clone's namespace, and the [frontier store](architecture.md#id-frontier-durable-mint) it came from |

```
fr info [--json]
```

`version` shows the build's short commit in parentheses (`0.1.6 (ad763b0)`), as does `fr --version`. Nothing else does: the `--help` banner and the JSON `version` field stay on the bare crate version, so anything parsing a version string doesn't have to cope with a suffix — the JSON reports the commit separately in `commit` (`null` when the binary wasn't built from a git checkout).

`frontier` is normally invisible; it's what to look at when a minted ID isn't the number you expected. It reads `none recorded` before this clone has minted anything, and `unreadable` when the store is corrupt (which `fr check` explains).

`worktree` is stated outright rather than left to be inferred, because nothing else in the output can say it: `project` is the *committed* name, so every worktree of a clone reports the same one, and `frame_dir` only implies the answer by being a path you have to recognise. It names the clone's main working tree too — that is where this clone's shared state lives (the [actor token](concepts.md#actors), the [ID frontier](architecture.md#id-frontier-durable-mint), the [recovery log](concepts.md#recovery)), which is why the `frontier` line below it points somewhere other than here. A detached worktree has no branch to name and falls back to its directory name.

```
worktree   feature-x  (linked worktree; main tree /Users/you/dev/lace)
```

With `--json`, the `actor` field distinguishes all three states for machine consumers: a literal token string (`"a"`), `"null"` for the primary clone, and JSON `null` when unclaimed. `worktree` and `main_worktree` are always present, and `null` in the main working tree — present-but-null rather than absent, so a consumer can tell "the main tree" from "a frame too old to report it". The JSON object also includes `shelved_tracks` and `archived_tracks` counts, and an `id_frontier` object (`path`, `state`, `namespace`, and `recorded` as a prefix → number map).

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

Add to a task's note. **Appends by default**, separated by a blank line; `--replace` overwrites instead.

```
fr note EFF-014 "Found while working on closures"
fr note EFF-014 "Superseded by the design doc" --replace
```

Refused if the result would exceed [`limits.note_max_bytes`](concepts.md#limits) (16 KB by default) *and* be longer than the note already is. Since appending can only lengthen a note, an append onto a note that is already over the limit is always refused — which is the point, as appending is how notes get that size. Nothing is written when a write is refused; the text is still yours to shorten and retry.

Also refused if the appended text repeats a paragraph the note already holds (`limits.note_repeat_bytes`, 120 bytes by default) — the signature of an append that was meant to be a replacement:

```
error: MAI-b7 note already contains this text (663B):
         "Spec unique-types.md §3.4a: `val y* = x` (x owned unique) moves x…"
       nothing was written — `fr note` appends. If you meant to replace the note,
       use `fr note MAI-b7 "…" --replace`; if you meant to add to it, leave out
       what is already there
```

A note that predates the limit keeps working and can be edited down in as many passes as you like: any write that leaves it shorter than it was is allowed, whether or not the result is under the limit. There is no `--force` — set `note_max_bytes = "off"` if you do not want the limit.

### `fr ref ID ACTION PATH...`
### `fr spec ID ACTION PATH...`

Manage the files a task points at. `ref:` is the files it touches, `spec:` the documents it implements; the two differ in meaning only, and take the same actions — the same `add`/`rm` shape as [`fr tag`](#fr-tag-id-action-tag) and [`fr dep`](#fr-dep-id-action-dep_id).

| Action | Effect |
|---|---|
| `add` | Append paths the task does not already have |
| `rm` | Drop the named paths; removing the last one removes the metadata line |
| `set` | Replace the whole list |

```
fr ref EFF-014 add doc/design/effects.md
fr ref EFF-014 add src/parser.rs:807 src/parser.rs:920-934
fr ref EFF-014 rm src/parser.rs:807
fr spec EFF-014 set doc/spec.md#closure-effects doc/rfc-012.md
```

**Prefer `add`.** It needs no read of the current list, so two agents adding different paths to one task do not clobber each other, and it cannot silently discard a list the caller did not know was there. It is idempotent — adding a path already present changes nothing and says so. `set` is the deliberate destructive form.

#### Paths, and what is checked

Paths are relative to the project root, each optionally carrying a `#anchor`, `:line`, `:line-range` or `:line:col` suffix (see [format.md](format.md#metadata-types)). The suffix is kept and never validated — only the file has to exist.

**`add` and `set` refuse a path with no file behind it**, naming every bad path and writing nothing:

```
$ fr ref EFF-014 add src/parsr.rs
error: no such file: src/parsr.rs
  (pass --force to add it anyway)
```

`fr check` reports a broken ref as an error, so the alternative is frame creating work for itself: the typo is cheapest to fix where it is typed. `--force` covers the case the check cannot tell apart — pointing at a file you are about to write.

**`add` and `set` also refuse a path that leaves the project** — one that escapes upward, or one named from the filesystem root:

```
$ fr ref EFF-014 add ../notes.md
error: ../notes.md leaves the project root — nothing outside it travels with the project

$ fr ref EFF-014 add /Users/me/proj/doc/design.md
error: /Users/me/proj/doc/design.md is absolute — a ref is relative to the
  project root, so this one means nothing on another machine
```

This is the opposite failure from a broken ref, and the reason it is worth refusing: the file *is* there, so nothing looks wrong until someone else clones the project. It is checked before the existence check, so `../typo.md` is reported as the escape it is rather than as a missing file. The test runs on the folded value, so `doc/../../outside.md` is caught too, while `doc/../src/parser.rs` — out of a subdirectory and back in — is fine. `--force` overrides this the same way it overrides the existence check.

**`add` and `set` refuse a path git is ignoring**, for the same reason: a gitignored file is in your working copy and will be in nobody else's.

```
$ fr ref EFF-014 add scratch/notes.md
error: scratch/notes.md is ignored by git — it is in this working copy and
  will not be in anyone else's
```

`git check-ignore` decides, so the rule is git's own rather than a second copy of it, and the **resolved** path is what git is asked about — `doc/draft.tmp:12` is not a filename. Two things are deliberately not refused: a **tracked** file, even when a rule covers it (ignore rules do not apply to what is already in the index, so it does travel), and anything at all when the project is **not in a git repository** or `git` cannot be run — frame cannot tell, so it allows.

**`rm` never checks the filesystem**, since a path is most worth removing precisely when the file behind it is gone. It is never refused for leaving the project or being ignored either: a ref already in a file is exactly what you need to be able to take out.

#### One file, one entry

A path is stored in its **normal form**: `.` and `..` segments are folded away, so `./sub/../real.md` is stored as `real.md`. The suffix is untouched — `./doc/../design.md#why` becomes `design.md#why`, and a file whose *name* contains `..` or `#` is left exactly as it is.

Both `add` and `rm` **match by normal form**, so a spelling reaches a stored value whichever of the two is the awkward one:

```
$ fr ref EFF-014 rm real.md            # stored as ./sub/../real.md
EFF-014 ref removed: ./sub/../real.md
```

The message names what left the file rather than what you typed. This is what keeps values written by older versions of frame reachable — no existing file is rewritten to satisfy the rule. `add` uses the same comparison, so it will not append a second entry for a file the task already carries under another spelling, and `set` drops later entries that duplicate earlier ones the same way.

The suffix is part of a reference's identity: `rm src/parser.rs` does **not** remove `src/parser.rs:807`. A reference to a file and a reference to a line in it are different references.

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

Set track state to `active`. **This is also how a track is un-archived**: on an archived track it moves the file back out of `frame/archive/_tracks/` as well as setting the state, since the state alone would leave the config naming a file that is not there — and a configured track whose file is missing is skipped entirely, taking its tasks out of every view. On a shelved track there is nothing to move; the file never left `tracks/`.

### `fr track archive ID`

Set track state to `archived` and move file to `frame/archive/_tracks/`. Reverse it with `fr track activate`.

### `fr track delete ID`

Delete an empty track (no tasks, no archive files). Non-empty tracks must be archived instead.

### `fr track mv ID POSITION`

Reorder a track to a new position (0-indexed among active tracks).

### `fr track cc-focus [ID] [--clear]`

Set or clear the cc-focus track. The cc-focus track is optional — when set, its tasks sort first in `fr ready --cc` output. Use `--clear` to remove the setting, which empties `cc_focus` rather than deleting the line: empty is what the shipped `project.toml` writes and documents as meaning none, and keeping the key keeps the comment next to it.

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

`--prefix` rewrites the track's **archived** task IDs too, and reports how many. It used to rewrite only the live ones — it read the archive as a track file, found no `## Section` headers, and wrote nothing while printing success — so archives renamed before this carry the old prefix still. `fr check` reports that state and `--fix` repairs it.

**An archived track cannot be renamed** — by any of the three flags. Archived is frozen, which is what every other operation already means by it: shelving, reordering and cc-focus all refuse an archived track, and so does adding a task to one. Rename is now the same, and says how to proceed:

```
$ fr track rename api --new-id api-v1
error: cannot rename archived track 'api' — unarchive it first: `fr track activate api`
```

The round trip is `fr track activate`, rename, `fr track archive`, and it renames everything a live rename does — the track file, the done-task archive, the archived task IDs. Renaming in place would mean writing a file the archive owns, and for `--prefix` it would mean loading a track that is deliberately not loaded, which is why this refuses rather than growing a second way to do it.

Refusing also replaces three misleading messages. `--new-id` used to report success while moving nothing (leaving a project `fr check` then called an error), `--name` rewrote `project.toml` and silently skipped the track file's `# Title`, and `--prefix` said `track not found` — which was false, since the track exists and only its file lives elsewhere. `fr track delete` said the same false thing and now names the real reason too. A track that genuinely is absent still gets `track not found` from every flag.

## Maintenance

### `fr clean`

Run project maintenance — the work frame expects to do for you as tasks come and go.

```
fr clean [--dry-run] [--normalize] [--json]
```

Actions performed:
- Assign missing task IDs
- Add missing `added` dates
- Add missing `resolved` dates to done tasks
- Resolve duplicate IDs — the first occurrence in track order keeps the ID. A duplicated **subtask** is renumbered under its own parent (`M-003.3` → `M-003.4`), not given a top-level number, so its ID keeps saying where it lives. This is the resolution path for the one ID collision the frontier doesn't prevent: two worktrees of a clone adding a subtask to the same parent.
- Archive done tasks exceeding either archival threshold — `done_threshold` (how many) or `done_bytes_threshold` (how much text). Whichever trips, the drain goes down to the corresponding retain level rather than back to the trigger; see [`[clean]`](concepts.md#clean).
- Move top-level tasks into the section matching their state
- Report dangling dependencies and broken refs
- Report tasks whose fields are out of [canonical order](format.md#field-order) — counted, not changed, unless `--normalize` is given
- Suggest actions (e.g., "all subtasks done — consider marking done")

**Clean runs unattended**, not only when you ask: with `auto_clean` on (the default) the TUI runs it after every file reload. So everything above must be correct with nobody watching and no output read — that constraint is what decides whether a repair belongs here or behind [`fr check --fix`](#fr-check---fix), which is invoked deliberately after a diagnosis has been read. Destructiveness is not the line: clean already archives tasks and renumbers IDs.

Missing `resolved:` dates are filled *after* archival, deliberately. Archive retention ranks done tasks by that date and treats a missing one as oldest, so stamping it earlier in the run would make the oldest task look like the newest completion — retained over genuinely recent work, and surfacing at the top of `fr recent`.

IDs assigned or reassigned by a real (non-`--dry-run`) clean are minted in this clone's [actor-token namespace](concepts.md#minting-in-a-token-namespace), auto-claiming a token on first use. Archival and thresholds key on task state and `resolved:` dates, not ID structure, so they are unaffected by the token. A `--dry-run` previews without claiming a token or writing anything.

`--json` emits the whole report as one document — the finding categories flattened in as arrays, the way `fr check --json` reads, plus `field_order` and two flags:

```json
{
  "dry_run": true,
  "normalize": false,
  "ids_assigned": [{ "track_id": "main", "assigned_id": "M-002", "title": "No id at all" }],
  "dangling_deps": [{ "track_id": "main", "task_id": "M-001", "dep_id": "M-404" }],
  "tasks_archived": [{ "track_id": "main", "task_id": "M-007", "title": "Old work" }],
  "archived_by_track": [{
    "track_id": "main",
    "reason": "bytes",
    "tasks": 163,
    "done_bytes_before": 1582399,
    "done_bytes_after": 62410,
    "done_bytes_threshold": 262144,
    "archive_path": "frame/archive/main.md"
  }],
  "field_order": {
    "reordered": [{ "track_id": "main", "task": "M-001", "was": ["added", "note", "dep"], "now": ["added", "dep", "note"] }],
    "skipped": []
  }
}
```

The flags carry what the arrays cannot. `dry_run` is whether anything was written. `normalize` is what `field_order.reordered` *means*: with it those tasks were rewritten, without it they were only found. A consumer that ignores it reads a preview as a result. The document is printed after the write succeeds, so a run that failed to save prints nothing.

`archived_by_track` is one row per track that archived anything, answering what a flat list of task IDs cannot: which limit tripped (`reason` is `"count"`, `"bytes"` or `"both"`) and whether it helped. `done_bytes_after` matters for the one case draining cannot fix — a retained task larger than `done_bytes_retain` leaves the section over budget however much is archived around it, and without the number every later clean would look like it had quietly done nothing. The human output says the same thing in one line per track, and prints the task-by-task list only when ten or fewer moved.

#### `fr clean --normalize`

Rewrite every task whose fields are out of [canonical order](format.md#field-order).

Frame writes a task in canonical order the first time it edits that task, so a project converges on its own — over as long as it takes to touch every task. `--normalize` is that convergence asked for at once, for the tasks nobody is about to touch.

A plain `fr clean` counts them and stops there, so a project written before the canonical order existed says so without being rewritten behind your back:

```
Field order:
  599 tasks have fields out of canonical order — run `fr clean --normalize` to rewrite them
```

With the flag, each task is named with the order it had and the order it got:

```
Field order normalized:
  [main] MAI-137.6: added, note, spec, resolved → added, resolved, spec, note
```

**Off by default, and not something clean does on its own.** Everything else `fr clean` does has to be correct unattended, because the TUI runs it after every reload. Reordering every task in a project is a large, boring diff, and this project has already paid for one: a clean run that rewrote a whole track to fill one `resolved:` date, with a one-line deletion hidden inside it that got committed unread. So this one is asked for explicitly, and `--dry-run` shows you the list first.

Only tasks actually out of order are rewritten; the rest stay byte-identical, so the diff is exactly the tasks reported. Running it twice changes nothing the second time. Marking a task for rewrite re-canonicalizes all of that task's own lines, not just the order — checkbox spacing, the note block form, and the `", "` join on `dep:`/`ref:`/`spec:` — which is the same form the task would reach the next time anyone edited it.

One task is reported but left alone:

```
Field order left alone (stranded lines a note would absorb):
  [main] M-014: added, note, resolved
```

That task carries stranded lines — content the parser could not attribute — indented as deep as a note body, so moving its note last would swallow them ([format.md](format.md#field-order)). It is named rather than skipped silently, because the file has damage worth looking at by hand.

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
fr recovery [--limit N] [--since ISO-8601] [--for ID] [--here] [--json]
```

| Flag | Description |
|------|-------------|
| `--limit N` | Show at most N entries (default: 10, or all when `--for` is given) |
| `--since TIMESTAMP` | Only show entries after this ISO-8601 timestamp |
| `--for ID` | Only show entries naming this task, or carrying this RFC 3339 timestamp |
| `--here` | Only show entries written from this working tree |
| `--json` | Output as JSON array |

The log is [shared by every git worktree of a clone](concepts.md#recovery), so the listing spans all of them by default and each entry records its `Origin:`. When entries come from more than one working tree the listing says so.

When `--limit` holds entries back, the listing ends by saying how many and how to see the rest — a truncated log and an empty one otherwise look identical, which matters because [`fr check`](#fr-check) sends you here to find one specific entry.

`--for` is how you follow a [`conflict:` marker](format.md#metadata-types): pass the task ID, or the timestamp the marker carries. Matching is on ID boundaries, so `M-1` does not match `M-100`, and a parent does not match its own subtasks (`M-4` does not match `M-4.1`) — pass the exact ID you want.

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

Print the absolute path to the recovery log file — wherever it resolved to, which is not always `frame/.recovery.log`.

```
fr recovery path
```

The log's size, retention and location are set by [`[recovery]` in `project.toml`](concepts.md#recovery-1), with `FRAME_RECOVERY_LOG` overriding the configured path for one machine.

## Version Control

### `fr git setup`

Configure this clone to work with frame. Idempotent — run it after every clone, and again whenever you want to check.

```
fr git setup [--dry-run] [--json]
```

Three things, reported individually:

| Step | What it does |
|------|--------------|
| `.gitignore` | ensures the blanket `frame/.*` pattern, and collapses any per-file entries it covers into it |
| `.gitattributes` | routes `frame/tracks/*.md`, `frame/archive/*.md` and `frame/inbox.md` to the merge driver |
| `.git/config` | registers the driver: `merge.frame.driver` |

`fr init` runs this for you inside a repo, so a new project needs nothing extra.

**The first two are committed; the third cannot be.** `.git/config` is per-clone, so a teammate who clones a correctly-configured project gets the attributes but *not* the driver, and git silently goes back to merging track files line by line. `fr check` warns when the driver is missing for exactly this reason — it is what tells a fresh clone to run this. Every worktree of one clone shares the config, so once per clone is enough.

The `.gitignore` migration only ever removes lines it can name: an exact match for a working-copy-local frame file (in any of its usual spellings, with or without leading or trailing slashes) that the blanket pattern genuinely covers. An entry frame does not recognise, a nested one like `frame/archive/.keep`, and a negation are all left alone.

`--dry-run` reports what would change and writes nothing. Outside a git repository the command reports that there is nothing to configure and exits 0 — it will not create a `.gitignore` where there is no repo.

### `fr merge`

Three-way merge two versions of a track or the inbox. **Normally invoked by git, not by you** — `fr git setup` registers it as a merge driver and it runs during `git merge`, `git rebase`, `git cherry-pick` and `git stash pop`.

```
fr merge --base FILE --ours FILE --theirs FILE [--path PATH] [--kind track|inbox]
fr merge --resolve ID...
```

Not to be confused with [`fr actor merge`](#fr-actor-merge), which collapses one actor's ID namespace into another.

**Why this exists.** `fr done` moves a task from `## Backlog` to `## Done`. A line-based merge reads that as a deletion plus an unrelated insertion, conflicts, and — if the conflict is resolved by keeping both sides — leaves two copies of one task, one open and one done. Frame merges by task ID instead, so a relocation is just a task whose section changed, and that case stops being a conflict at all. Additions from both sides land; a change to a task the other side did not touch is taken; subtasks merge independently of their parent.

**Exit status is the interface.**

| Status | Meaning |
|--------|---------|
| `0` | merged cleanly |
| `1` | merged, but something was left undecided — the VCS stops |
| `2` | not a frame file, or the merge could not run — the VCS falls back to its own merge |

Status `2` is why `project.toml` and `actors.toml` are not routed to the driver: they are TOML that merges fine line by line, and frame declines anything it does not recognise rather than guessing.

**On conflict, no conflict markers are written.** A file full of `<<<<<<<` is not valid frame markdown, so it breaks the parser, `fr check` and `fr show` at exactly the moment you need them. Instead:

- your version is kept in the file, which still parses;
- their version goes to the [recovery log](#fr-recovery), whose absolute path the merge prints, along with the `fr recovery --for <ID>` that retrieves it;
- the task gets a `conflict:` line, which `fr check` reports as an error;
- the merge exits 1, so git marks the path unmerged and stops.

The log is located from the file being merged, not from the working directory, and the merge declines to write into a project that does not hold that file — otherwise merging files kept elsewhere would file the discarded side under an unrelated project. When no owning project can be found the merge says so loudly, names the directory it searched, and tells you to recover their side from version control. That warning and the "in the recovery log" line are mutually exclusive by construction: the reader is never told to go and read something that was not written.

Git still shows the path as conflicted (`git status`, `git ls-files -u`) — there is simply nothing in the file to grep for. Apply whatever is missing with `fr note` / `fr state`, then:

```
fr merge --resolve BAC-179
```

which clears the marker and nothing else. Clearing it is you recording the judgment; frame cannot check that the right thing came out.

**Running it by hand.** The three file arguments and `--path` mirror what a VCS passes (`%O %A %B %P` in git). The merged result is written over `--ours`. `--path` decides whether the file is a track or the inbox; `--kind` forces it when there is no meaningful path.

## Project Registry

Frame maintains a global project registry at `~/.config/frame/projects.toml` (or `$XDG_CONFIG_HOME/frame/projects.toml`). Projects register automatically when you run `fr init`, use `fr` in a project directory, or add them explicitly.

### `fr projects`

List registered projects sorted by most recently accessed via CLI.

```
fr projects
```

Output includes project name, path (abbreviated with `~`), and relative time since last access. Missing projects (directory no longer exists) show `(not found)`.

**Git worktrees are listed under the project they are a worktree of**, labelled by the branch they have checked out:

```
  Lace              ~/dev/lang/lace                        2 min ago
    └ alt-work      ~/dev/lang/lace/.claude/worktrees/alt   just now
    └ sibling-work  ~/dev/lang/lace-sibling                 5 min ago
```

Every worktree of a clone carries the same project name — `project.toml` is committed — so the branch is what tells them apart, and it is what a person calls the worktree anyway. A worktree sorts with its project rather than among the projects, so it never floats to the top away from the row that explains it. One whose clone is not itself registered is listed at the top level, since there is nothing to nest it under.

**A worktree's entry retires itself when the worktree is removed.** Listing is when a dead row would be seen, so listing is when it goes — no `fr projects prune`, and a note says how many went. This is safe for a worktree specifically, and only for a worktree: its row is derivative (the project has its own) and everything a removed worktree held that exists nowhere else — the [ID frontier](architecture.md#id-frontier-durable-mint), the [recovery log](concepts.md#recovery) — lives in the git common directory, which the removal does not touch. Two guards keep it to that case: the entry must be recorded as a worktree, and the clone's main working tree must still be present, so an unmounted volume takes nothing with it.

Provenance is recorded when an entry is **created**, and a listing re-asks git for any entry that does not have it — so an entry written by an older frame is stamped the first time you list, which is what lets it group and, later, retire itself. Recording it before the worktree dies is the whole point: `git worktree remove` deletes the directory *and* prunes git's own record of it, so afterwards nothing can say whose worktree the path was. The listing costs one `git worktree list` per clone, since asking from any working tree of a clone returns the whole set.

The picker (`p` in the TUI) groups and retires on exactly the same terms.

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

If the name is ambiguous (multiple projects share the same name — which every worktree of a clone does), the error lists the candidate paths so you can pick one.

### `fr projects prune`

Remove every registry entry whose project directory no longer exists (the same `(not found)` entries shown by `fr projects`). Useful for clearing out stale entries left behind by deleted or temporary projects.

```
fr projects prune            # remove all not-found entries
fr projects prune --dry-run  # list what would be removed, change nothing
```

Add `--json` for machine-readable output (an array of `{name, path}`). Only registry entries are removed — no project files are touched.

For a **worktree** the test is its own directory rather than the `frame/` inside it: a live worktree checked out to a branch that predates the project has no `frame/`, shows `(not found)`, and must not be pruned — it is sitting right there. Removed worktrees rarely reach this command at all, since a listing retires them first.

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
