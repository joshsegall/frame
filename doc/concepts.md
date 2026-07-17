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
number (e.g. `EFF-a14`), reserved for future namespacing; frame currently mints
only tokenless IDs. An ID's position is a stable handle, not its priority —
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

When the Done section exceeds the configured threshold (default: 100 tasks), `fr clean` archives the oldest tasks to a per-track archive file in `frame/archive/`, retaining the most recently resolved tasks (default: 10) so they remain visible in the Recent view.

## Recovery

Frame includes a recovery log (`frame/.recovery.log`) that prevents silent data loss. If the parser drops unrecognized lines, a write operation fails, or an edit conflict is dismissed in the TUI, the affected data is captured in the log.

View the log with `fr recovery`, prune old entries with `fr recovery prune`, or open it from the TUI command palette ("View recovery log").

Tasks tagged `#lost` were created by the recovery system after a failed cross-track move or other mutation error. The `fr check` command warns about any `#lost` tasks.

## Actors

An **actor** is a *working copy* — a single git clone of the project. The working copy, not a person or a session, is the unit of identity. Two agent sessions running in the same clone share that clone's identity (and are serialized by the file lock); two separate clones are two distinct actors.

Each actor holds one **token**. Tokens let separate clones mint task IDs concurrently without colliding: every newly minted ID is created in the minting clone's **token-namespace**.

- **`null`** is a real token, spelled `null`. It means the empty-token (default) namespace — the IDs you already see, like `EFF-14`. Exactly one working copy holds `null`; it's the **primary** (the clone that ran `fr init`).
- **Safe alphabet**: auto-assigned tokens are single letters from `a–z` minus `i`, `l`, and `o` (which read as digits) — 23 in all. Teams that outgrow 23 can manually claim multi-character tokens (`aa`, `foo`); those may use any lowercase letters.

Three files track this:

- **`frame/actors.toml`** (committed): the registry of every known token — its state (`active` or `retired`) and provenance (`name`, defaulting to the machine hostname, plus claim/retire dates). It's committed so a fresh clone can see what's already taken and so claims are recorded in git history.
- **`<git-common-dir>/frame-actor`** (the *shared* token): a single line holding the clone-wide token that every git **worktree** inherits by default. It lives under the git common directory (`git rev-parse --git-common-dir`, e.g. `<root>/.git/frame-actor`), which resolves to the same path from the main working tree and every linked worktree — so a worktree-per-session workflow keeps *one* actor identity instead of each worktree auto-claiming its own. It's outside every working tree, so it's never committed. Projects not in a git repo have no shared token and use only the local file.
- **`frame/.actor`** (gitignored, the *local* token): a single line that overrides the shared token for this one working copy. Like `.state.json` and `.lock`, it's local and never committed. The primary records its `null` here (see below); a worktree writes one only via an explicit `fr actor claim --local` / `fr actor set --local` to deliberately diverge onto its own token.

**Resolution precedence** is local, then shared: a `frame/.actor` wins if present, otherwise the shared token applies, otherwise the clone is unclaimed.

**Retirement** tombstones a token (`state = retired`): it leaves the pool of auto-assignable tokens but stays in the registry and can be reclaimed later with `fr actor set <token>`. A project created before actor tokens existed simply has no `actors.toml`; it operates as the untokened primary until someone runs `fr actor set null` (or any claim), which creates the registry.

Manage tokens with the `fr actor` commands (see `doc/cli.md`).

### Minting in a token-namespace

An ID is a `prefix-segment(.segment)*` chain, and **each segment carries its own token** — the token of whoever minted that segment. The primary (`null`) clone mints bare numbers (`EFF-14`, `EFF-15`); a clone with token `a` mints `EFF-a1`, `EFF-a2`, …; token `b` mints `EFF-b1`, and so on. A subtask's *last* segment carries the adding clone's token while the parent's segments are preserved verbatim, so actor `b` adding a child under `EFF-a14` produces `EFF-a14.b1`.

Numbers auto-increment by scanning **only the minter's own namespace** for the highest existing number and adding one (an empty namespace starts at 1). Because the scan ignores every other namespace — including `null` versus tokened — two unsynced clones can each mint freely and never produce the same ID. Reclaiming a retired token continues its sequence automatically, since the next number derives from the max already present.

A clone resolves its token (local, then shared) the first time it mints. A fresh clone or worktree with neither file **auto-claims** a token from the frontier on its first mint and writes it to the *shared* file — so sibling worktrees of the same clone inherit it rather than each auto-claiming their own. It announces the claim once. (The primary already recorded `null` locally at `fr init`, so it keeps minting bare numbers.) If every token is taken and the clone is still unclaimed, the mint fails and routes you to `fr actor set <token>` rather than guessing.

Because worktrees share one token but **not** one lock (`frame/.lock` is per-worktree), two worktrees minting at the exact same moment can still produce a colliding id. That's a deliberate trade: an occasional collision — caught by `fr check`'s duplicate-id report and repaired by `fr actor merge` — in exchange for never silently proliferating a new actor per worktree. To run genuinely concurrent worktrees as distinct actors, give one an explicit `fr actor claim --local`.

**Strict null policy.** The null namespace belongs *only* to a clone that deliberately took it — the one that ran `fr init`, or one that ran an explicit `fr actor set null`. A clone with no `.actor` is **not** the null actor, so it must never mint null-namespace IDs. Explicit mints handle this by auto-claiming a letter (above; the frontier never offers `null`). Background and passive paths — TUI startup auto-assign, post-external-change auto-clean, `fr clean --dry-run` previews — go further: on an unclaimed clone they **skip ID assignment entirely**, leaving tasks ID-less rather than falling back to null. The blank IDs are filled later, when an explicit action resolves (and if needed claims) a token. This is what keeps a machine that doesn't own `null` from silently re-introducing cross-clone ID collisions.

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
auto_clean = true          # run clean after file reload in TUI (default: true)
done_threshold = 100       # max done tasks per track before archiving (default: 100)
done_retain = 10           # number of recent done tasks to keep in track after archiving (default: 10)
archive_per_track = true   # separate archive file per track (default: true)
```

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
