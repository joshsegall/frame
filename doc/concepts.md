# Frame Concepts

Frame is a markdown-based task tracker. `.md` files are the source of truth — you can edit them by hand or through the CLI/TUI.

## Projects

A frame project is any directory tree containing a `frame/` subdirectory. Running `fr init` creates one:

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
    .inflight          # only present while a multi-file operation is running
```

Everything in `frame/` is committed except the dotfiles, which belong to a single
working copy — UI state, the lock, this clone's actor token, the recovery log.
`fr init` covers them with one `.gitignore` line, `frame/.*`, so a local file
added in a later version of frame needs no change to your `.gitignore`. The rule
that keeps this working: **nothing under `frame/` that needs to be committed
starts with a dot.**

Project discovery walks up from the current directory until it finds a `frame/` folder.

**If you find a `.inflight` file sitting there**, an operation that writes more
than one file — a cross-track move, a track archival, an actor merge, a triage —
started and didn't finish, because the process was killed or the disk gave out
partway through. Nothing is lost: those operations always write the new copy
before removing the old one, so an interruption duplicates rather than deletes.

You don't need to do anything about it. The next command that writes anything
finishes the interrupted operation, says what it did, and removes the file.
`fr check` reports it in the meantime, and `fr recovery` keeps a record
afterwards. The only case needing a decision is when the project changed
underneath the interrupted operation — a hand edit, or a `git checkout` in
between — where frame won't guess: it leaves everything alone, says why, and
keeps warning until you look and run `fr check --fix --yes`.

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
| `shelved` | Hidden from default views, preserved for later. Rejects new tasks (`fr add`/`push`/`sub`/`triage`/`mv --track`) and task activation (`fr state active`/`fr start`) until re-activated with `fr track activate`. Existing tasks can still be closed out or re-opened (done/parked/todo). |
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

Each dotted segment may optionally carry a leading lowercase **token** before its
number (e.g. `EFF-a14`): the [actor-token namespace](#actors) of the working copy
that minted that segment. The primary clone mints tokenless numbers like
`EFF-001`. An ID's position is a stable handle, not its priority —
ordering within a section is positional, and `added:` is the authority for
relative age. IDs that don't match the grammar are kept verbatim and ignored by
ID minting (see `doc/format.md`).

### Tags

Tags are `#word` tokens at the end of a task line:

```
- [>] `EFF-014` Implement effect inference #cc
```

Tags are stored without the `#` prefix internally.

## Metadata

Tasks can have metadata lines indented below the task line:

| Field | Format | Description |
|-------|--------|-------------|
| `added` | `added: 2025-05-14` | Date the task was created |
| `resolved` | `resolved: 2025-05-14` | Date the task was completed |
| `dep` | `dep: EFF-003, INFRA-007` | Task dependencies (comma-separated IDs) |
| `ref` | `ref: doc/design.md, src/lib.rs:807` | Files the task touches (comma-separated paths) |
| `spec` | `spec: doc/spec.md#section, doc/rfc.md` | Files the task implements (comma-separated paths) |
| `note` | `note: Free text` | Note (single-line or multi-line block) |
| `conflict` | `conflict: both-edited 2026-08-03T04:08:38Z` | An unresolved merge conflict, written by `fr merge` and cleared by `fr merge --resolve` |

## Track Files Are Generated, Not Hand-Merged

The `.md` files are the source of truth, and they are meant to be read and hand-edited. But they are *written* by frame, and their structure carries meaning that plain text does not: a task's ID is its identity, and the section it sits in is derived from its state.

That has one practical consequence, and it is the difference between a merge that works and a corrupted file. `fr done` does not edit a line in place — it **moves** the task from `## Backlog` to `## Done`. Merge two branches line by line and that reads as a deletion in one place plus an insertion in another, with nothing to say they are the same task. Resolve the resulting conflict by keeping both sides — the obvious move, and correct when both sides genuinely appended — and you get two tasks with one ID, one open and one done. The file still looks reasonable. `fr show` will disagree with it.

So frame merges by **task identity** rather than by line, and [`fr git setup`](cli.md#fr-git-setup) registers that as a git merge driver. With it, the relocation is not a conflict at all: it is one task whose section changed. Without it, git merges frame files the wrong way and mostly gets away with it, which is the worst of the available failure modes.

The rule generalises: **don't merge generated state, regenerate it — or merge it with the tool that generated it.** If you ever find yourself hand-splicing conflict regions in a track file, stop; the file has already stopped being valid frame markdown, and every tool that could diagnose it is broken too.

## Inbox

The inbox (`frame/inbox.md`) is a quick-capture bucket for ideas that haven't been assigned to a track yet. Inbox items have a title, optional tags, and optional body text — but no ID, state, or metadata.

**Triage** moves an inbox item into a track, converting it to a proper task with an auto-assigned ID.

## Done Lifecycle

When a task is marked done:

- **TUI**: The task stays in Backlog for a 5-second grace period (undo-able), then moves to the Done section automatically.
- **CLI**: `fr state ID done` moves top-level Backlog tasks to the Done section immediately.

When a top-level task moves between sections (Backlog <-> Done), its entire subtask tree moves with it. Subtasks cannot be moved between sections independently — only top-level tasks trigger section moves.

When the Done section exceeds the configured threshold (default: 100 tasks), `fr clean` archives the oldest tasks to a per-track archive file in `frame/archive/`, retaining the most recently resolved tasks (default: 10) so they remain visible in the Recent view.

## Recovery

Frame includes a recovery log that prevents silent data loss. If the parser drops unrecognized lines, a write operation fails, or an edit conflict is dismissed in the TUI, the affected data is captured in the log.

**The log is shared by every git worktree of a clone**, and lives at `<git-common-dir>/frame-recovery.log` — inside `.git/`, which every worktree resolves to the same path and which can never be committed. For a project outside git it stays at `frame/.recovery.log`, where there are no worktrees to coordinate with. This is the same mechanism the ID frontier and the shared [actor token](#actors) use.

It is shared for two reasons. The log holds content that reached no other file by definition, and a per-worktree copy is invisible from the worktree next door — so an entry written by one working tree reads as never written from another. It is also *ephemeral*: `git worktree remove` deletes ignored files silently, so a log inside a worktree is the only copy of something sitting in a directory git will delete on request.

Every entry records the working tree it came from as `Origin:`, because a field like `Target: tracks/main.md` means nothing once one log serves several working copies. `fr recovery --here` narrows the listing to this one.

A per-worktree log left over from an older frame is read alongside the shared one and moved into it on the next write, oldest entries first.

View the log with `fr recovery`, prune old entries with `fr recovery prune`, print its location with `fr recovery path`, or open it from the TUI command palette ("View recovery log"). Its size, retention and location are configurable — see [`[recovery]`](#recovery).

Tasks tagged `#lost` were created by the recovery system after a failed cross-track move or other mutation error. The `fr check` command warns about any `#lost` tasks.

## Actors

An **actor** is a *working copy* — a single git clone of the project. The working copy, not a person or a session, is the unit of identity. Two agent sessions running in the same clone share that clone's identity (and are serialized by the file lock); two separate clones are two distinct actors.

Each actor holds one **token**. Tokens let separate clones mint task IDs concurrently without colliding: every newly minted ID is created in the minting clone's **token-namespace**.

- **`null`** is a real token, spelled `null`. It means the empty-token (default) namespace — the IDs you already see, like `EFF-14`. Exactly one working copy holds `null`; it's the **primary** (the clone that ran `fr init`).
- **Safe alphabet**: auto-assigned tokens are single letters from `a–z` minus `i`, `l`, and `o` (which read as digits) — 23 in all. Teams that outgrow 23 can manually claim multi-character tokens (`aa`, `foo`); those may use any lowercase letters.

Three files track this:

- **`frame/actors.toml`** (committed): the registry of every known token — its state (`active` or `retired`) and provenance (`name`, defaulting to the machine hostname, plus claim/retire dates). It's committed so a fresh clone can see what's already taken and so claims are recorded in git history — which also means git merges it, so it is written one line per actor and sorted by token to keep those merges rare and safe to resolve ([format.md](format.md#actor-registry-frameactorstoml)).
- **`<git-common-dir>/frame-actor`** (the *shared* token): a single line holding the clone-wide token that every git **worktree** inherits by default. It lives under the git common directory (`git rev-parse --git-common-dir`, e.g. `<root>/.git/frame-actor`), which resolves to the same path from the main working tree and every linked worktree — so a worktree-per-session workflow keeps *one* actor identity instead of each worktree auto-claiming its own. It's outside every working tree, so it's never committed. Projects not in a git repo have no shared token and use only the local file.
- **`frame/.actor`** (gitignored, the *local* token): a single line that overrides the shared token for this one working copy. Like `.state.json` and `.lock`, it's local and never committed. The primary records its `null` here (see below); a worktree writes one only via an explicit `fr actor claim --local` / `fr actor set --local` to deliberately diverge onto its own token.

A fourth file, alongside the shared token and equally machine-local, records the **ID frontier** — the highest number handed out per prefix and namespace: **`<git-common-dir>/frame-ids.toml`** (or `frame/.ids.toml` outside git). It's what stops two worktrees of one clone from minting the same ID; `fr info` shows it. Unlike the three above it holds no identity, only bookkeeping, and is safe to delete — see [ID Frontier](architecture.md#id-frontier-durable-mint).

**Resolution precedence** is local, then shared, then the main working tree:

1. this working copy's `frame/.actor`, if present;
2. otherwise the clone-wide shared token;
3. otherwise — when this is a *linked git worktree* — the **main working tree's** `frame/.actor`;
4. otherwise the working copy is unclaimed.

Tier 3 exists because the primary's `null` is only ever written locally, so a clone created by `fr init` has no shared token at all. Without it, every worktree of that clone would see nothing and auto-claim a token of its own. A linked worktree is the *same clone* as its main working tree, so reading that token is inheritance, not a claim: nothing is written, and no new actor appears in the registry. (`fr actor` reports which tier a token came from.)

**Retirement** tombstones a token (`state = retired`): it leaves the pool of auto-assignable tokens but stays in the registry and can be reclaimed later with `fr actor set <token>`. A project created before actor tokens existed simply has no `actors.toml`; it operates as the untokened primary until someone runs `fr actor set null` (or any claim), which creates the registry.

Manage tokens with the `fr actor` commands (see `doc/cli.md`).

### Minting in a token-namespace

An ID is a `prefix-segment(.segment)*` chain, and **each segment carries its own token** — the token of whoever minted that segment. The primary (`null`) clone mints bare numbers (`EFF-14`, `EFF-15`); a clone with token `a` mints `EFF-a1`, `EFF-a2`, …; token `b` mints `EFF-b1`, and so on. A subtask's *last* segment carries the adding clone's token while the parent's segments are preserved verbatim, so actor `b` adding a child under `EFF-a14` produces `EFF-a14.b1`.

Numbers auto-increment within **the minter's own namespace** (an empty namespace starts at 1). Because numbering ignores every other namespace — including `null` versus tokened — two unsynced clones can each mint freely and never produce the same ID. Reclaiming a retired token continues its sequence automatically.

The next number is the highest **already spoken for** in that namespace, plus one — taking the higher of two sources:

- a **scan** of what this working copy can see: the track, plus its `archive/` files, so a done task archived by `fr clean` keeps its number;
- the **recorded frontier**, a durable note of every number handed out, shared by all git worktrees of the clone.

The second is what makes the frontier only ever move forward. A scan alone slips backwards whenever the live maximum drops — a task archived, a task deleted, or a sibling worktree whose checkout hasn't merged your new tasks yet — and the next mint would reissue a number. Numbers are never reused, and gaps are entirely normal: an ID that gets abandoned before it lands simply stays spent. See [ID Frontier](architecture.md#id-frontier-durable-mint) for where the record lives and how it recovers from being deleted or corrupted.

A working copy resolves its token (local, then shared, then the main working tree) the first time it mints. A **fresh clone** with no token anywhere **auto-claims** one from the frontier on its first mint and writes it to the *shared* file — so sibling worktrees of that clone inherit it rather than each auto-claiming their own. It announces the claim once. (The primary already recorded `null` locally at `fr init`, so it keeps minting bare numbers.) If every token is taken and the clone is still unclaimed, the mint fails and routes you to `fr actor set <token>` rather than guessing.

**A linked worktree never auto-claims.** If nothing resolves for it — a clone with no token anywhere, e.g. a project predating actor tokens — the mint fails with a routing message instead of claiming. A claim is not local bookkeeping: it writes a row into the committed `actors.toml`, so auto-claiming from a worktree would silently split one clone into two actors and land shared, tracked state in whatever commit that worktree makes next. The fix is one explicit choice: `fr actor claim` in the main working tree to claim for the whole clone, or `fr actor claim --local` in the worktree to run it as its own actor.

**Worktrees of one clone don't collide.** They share a token, so they mint in the same namespace, and they have separate checkouts and separate locks (`frame/.lock` is per-worktree) — so neither can see the other's uncommitted tasks. What keeps them apart is the recorded frontier, which lives outside every working tree (under `.git/`) and is therefore shared by all of them: a number handed out in one worktree is unavailable in the others from that instant, committed or not. Sharing one token per clone costs nothing in collisions. To run worktrees as genuinely *distinct* actors — separate namespaces, separate provenance — give one an explicit `fr actor claim --local`.

One case the frontier does not cover: two worktrees adding a **subtask to the same parent task**. Subtask numbers are counted per parent rather than from the namespace frontier, so both can produce `EFF-a14.b1`. `fr check`'s duplicate-ID report catches it.

**Strict null policy.** The null namespace belongs *only* to a clone that deliberately took it — the one that ran `fr init`, or one that ran an explicit `fr actor set null` — and to that clone's linked worktrees, which are the same clone. A clone with no token anywhere is **not** the null actor, so it must never mint null-namespace IDs. Explicit mints handle this by auto-claiming a letter (above; the frontier never offers `null`). Background and passive paths — TUI startup auto-assign, post-external-change auto-clean, `fr clean --dry-run` previews — go further: on an unclaimed clone they **skip ID assignment entirely**, leaving tasks ID-less rather than falling back to null. The blank IDs are filled later, when an explicit action resolves (and if needed claims) a token. This is what keeps a machine that doesn't own `null` from silently re-introducing cross-clone ID collisions.

Tokens disambiguate *who minted* an ID, not *when*: cross-actor ordering comes from each task's `added:` date, not from its number. Numbers are unique per namespace, not globally sortable across actors.

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
cc_focus = "effects"       # optional — prioritizes this track in `fr ready --cc`
cc_only = true             # true: agent only works on #cc tasks (default)
                           # false: agent can pick up any unblocked task
```

The `cc_focus` setting is optional. When set, tasks from the focus track appear first in `fr ready --cc` output. When unset, `fr ready --cc` still scans all active tracks for `#cc`-tagged tasks.

When `cc_only` is `true` (default), agents should only work on `#cc`-tagged tasks and stop to ask for direction when none are available. When `false`, agents may fall back to untagged tasks across active tracks. The setting is included in `fr ready --cc --json` output.

### `[clean]`

Auto-clean and archival settings:

```toml
[clean]
auto_clean = true          # run clean after file reload in TUI (default: true;
                           # always stands down while git is rewriting files)
done_threshold = 100       # max done tasks per track before archiving (default: 100)
done_retain = 10           # number of recent done tasks to keep in track after archiving (default: 10)
done_bytes_threshold = "256KB"  # max bytes of done tasks before archiving (default: 256KB; 0/"off" disables)
done_bytes_retain = "64KB" # what a byte-triggered archive drains down to (default: 64KB)
archive_per_track = true   # separate archive file per track (default: true)
```

**Two triggers, because either alone is blind to a real case.** A count cannot tell 100 done one-liners from 100 done essays; bytes cannot see a track quietly accumulating hundreds of trivial tasks. Archiving runs when *either* is exceeded. The defaults are close to the same statement for a project whose notes are ordinary — at a typical note length, 100 done tasks is roughly 256 KB — and only diverge once notes run long, which is the case the count was missing.

**The drain goes to the retain level, not back to the trigger.** `done_threshold = 100` drains to `done_retain = 10`; `done_bytes_threshold = "256KB"` drains to `done_bytes_retain = "64KB"`. That gap is hysteresis and it is load-bearing: stopping at the threshold would archive one task on every clean for ever. `done_retain` also floors the byte drain, so a handful of recent done tasks always stay in the track — and if one of them is on its own larger than `done_bytes_retain`, the section stays over budget and `fr clean` says so rather than looking like it did nothing.

### `[limits]`

What frame's own commands will not do:

```toml
[limits]
note_max_bytes = "16KB"    # largest note frame will grow a note to (default: 16KB; 0/"off" disables)
note_repeat_bytes = 120    # shortest run of lines an append may not repeat into a note, and
                           # the same threshold `fr check` reports one for holding twice (default: 120)
track_warn_bytes = "512KB" # `fr check` warns past this much open work in one track (default: 512KB)
```

**`note_max_bytes` is enforced non-increasing, not absolutely.** A write is refused only if it would leave the note both over the limit *and* larger than it already was. So a note that predates the limit is never touched, never truncated, and never blocks the operations that do not lengthen it — and it can still be edited down, in as many passes as it takes, because a shrinking write is always legal. Under an absolute check the only legal edit to a 148 KB note would be one landing under the limit in a single shot, which would trap exactly the notes the limit exists to discourage.

There is no `--force`. Setting `note_max_bytes = 0` (or `"off"`) is the escape hatch.

In the TUI the same rule is enforced at the keystroke: the note field caps at `max(note_max_bytes, the note's length when opened)`, so an oversize note opens intact and can only shrink, and a paste that will not fit is rejected whole rather than clipped to fit — keeping its first N bytes would silently discard the tail.

**A guardrail on authoring, not an invariant on the file.** Markdown is the source of truth and stays hand-editable, and `fr import` is exempt, because importing is how content that predates a limit gets in. `fr check` does not report an oversize note as damage.

**`note_repeat_bytes` refuses an append that repeats text the note already holds.** `fr note` appends, and an agent that believes it replaces writes the whole note out again each time — so the note ends up holding N copies of everything that did not change that round. Measured on a real project: one note reached eight copies of itself, 110 KB of its 139 KB, and 5.4% of all note text across the project was duplication of this kind.

The comparison is **runs of consecutive lines**, exact text, at or above the configured length. Lines rather than paragraph blocks because blocks made the guard answer to the author's punctuation instead of to the duplication: a note whose sections are separated by blank lines is several blocks and a re-sent section is caught, while the same sections written as consecutive lines are one block and re-sending three of four verbatim matched nothing. Both notes were duplicating the same text. Exact rather than fuzzy because the repetition is literally re-pasted, so exact matching finds it — and because the failure mode of a similarity threshold is refusing a write that was fine. The refusal asks first for only the new text, since a repeat is usually an append written as a whole-note rewrite; `--replace` is offered second and named as discarding the note, because it is.

**The same threshold drives a `fr check` report**, so what check names is exactly what `fr note` would now refuse. The guard is forward-looking — it stops a note growing another copy of itself and can do nothing about copies already there — and unlike an oversize note, duplication is reported as something worth fixing: a long note is a supported state, but nobody means to store their note twice. No `--fix`, because which copy to keep stops being decidable as soon as the copies diverge.

**`track_warn_bytes` measures open work — `## Backlog` plus `## Parked` — not file size.** Done is excluded because `[clean]` already bounds it, and bounds it by oscillating between `done_bytes_retain` and `done_bytes_threshold`. Folding that swing into the measurement would mean the same track warns just before a clean and goes quiet just after one with its open work untouched: a warning that answers to the archiver's schedule rather than to anything its reader did. The warning is one line per track and names no individual task, because no individual task is the problem — the aggregate is, and the remedy is splitting the track or closing work.

Both accept a plain number of bytes or a string with a unit (`"16KB"`, `"512KB"`), 1024-based, as `[recovery]` does.

### `[recovery]`

Size, retention and location of the [recovery log](#recovery-log):

```toml
[recovery]
max_size = "5MB"           # size past which a write also considers trimming (default: 5MB)
prune_age_days = 30        # how old an entry must be before a trim may remove it (default: 30)
path = "logs/frame.log"    # optional — where the log lives; see below
```

**Size is a trigger, age is the rule.** Outgrowing `max_size` is what makes frame *consider* trimming; nothing younger than `prune_age_days` is ever removed. A log full of recent entries grows past its limit and loses nothing, which is the right way round — the newest entries are the ones still worth having. If retention is what you care about, `prune_age_days` is the setting to change, not `max_size`.

`max_size` accepts a plain number of bytes or a string with a unit: `"5MB"`, `"512KB"`, `"2GB"`. `KB`/`MB`/`GB` are 1024-based, and `KiB`/`MiB`/`GiB` are accepted as synonyms for them.

`prune_age_days` is also the default cutoff for a bare `fr recovery prune`.

**`path`** overrides where the log lives. A *relative* path resolves against the project root and is a choice that is correct on every machine — `path = "frame/.recovery.log"` pins the log to each working copy. An *absolute* path is accepted, but `project.toml` is committed and an absolute path is machine-specific; prefer the `FRAME_RECOVERY_LOG` environment variable, which overrides this setting and belongs to one machine by nature. A log placed outside the default location is yours to gitignore.

### `[ui]`

TUI display settings:

```toml
[ui]
kitty_keyboard = true      # Kitty keyboard protocol for reliable key detection (default: true)
                           # Supported by Kitty, Ghostty, WezTerm, foot, and most modern terminals.
                           # If you experience missed or double keypresses, set to false to fall back
                           # to standard terminal input. The main thing you lose is reliable disambiguation
                           # of some modified keys (e.g., Ctrl+Shift+Z vs Ctrl+Z).
ref_extensions = ["md"]    # file extensions for ref/spec autocomplete (empty = all)
ref_paths = ["doc", "spec", "docs", "design", "papers"]  # directories for ref/spec autocomplete (empty = whole project)
default_tags = ["cc"]      # tags always shown in autocomplete (even if no tasks use them yet)

[ui.tag_colors]
bug = "#FF4444"
design = "#44DDFF"

[ui.colors]
# custom state/UI color overrides (hex values)
```
