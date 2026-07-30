# Changelog

All notable changes to frame will be documented in this file.

## Unreleased

### Removed
- `fr clean` no longer generates `ACTIVE.md`. The summary was write-only — nothing read it back — and went stale after any change that wasn't a clean, making the committed copy misleading. Use `fr ready` / `fr list` for a live view of active work.

### Added
- TUI detail view: press `y` to copy the current task's markdown to the clipboard. Copies the full task block (task line, metadata, and all subtasks) exactly as it appears in the track file, using the same clipboard mechanism as the note editor.
- Git worktrees now share one actor identity by default. The clone-wide token lives under the git common directory (`<root>/.git/frame-actor`), which every worktree resolves to the same path, so a worktree-per-session workflow no longer auto-claims a fresh token per worktree. A local `frame/.actor` still overrides it for a single working copy; `fr actor claim --local` / `fr actor set --local` write that override deliberately (to run a worktree as a distinct concurrent actor), while the primary's `null` stays local. `fr actor` now shows whether the token is local or shared. (Worktrees share the token but not the lock; the ID frontier below is what keeps their mints from colliding.)
- `fr actor merge <from>... --into <token>` collapses several actor namespaces into one, renumbering every id minted by a source token into the target's sequence and retiring the sources. Useful when a machine accumulates tokens (e.g. a git-worktree-per-session workflow, where each fresh working copy auto-claims its own token). The remap is per-segment — a subtask minted by a third actor is preserved (`SEC-d1.a3` → `SEC-b2.a3`, not `SEC-b2.b1`) — and covers all tracks, archives, and `dep:` references. Prints the full `OLD → NEW` map (`--json` supported). `--dry-run` previews without writing; `--rewrite-notes` also rewrites id mentions in note/spec/ref prose, skipping git citations like `fix(SEC-d1)`.
- `fr --version` and `fr info` now name the commit the binary was built from (`0.1.6 (ad763b0)`), so a report from a dev build identifies the exact source. Only those two: the `--help` banner and the JSON `version` field keep the bare crate version, and `fr info --json` reports the commit separately in `commit` (`null` when the build didn't come from a git checkout, e.g. a published crate). No dirty marker — Cargo only re-runs the build script when `HEAD` or the branch ref moves, so a "clean" claim computed there would go stale on the next edit.
- `fr info` now shows the **ID frontier**: the last number handed out per track prefix in this clone's namespace, and the path of the [frontier store](doc/architecture.md) it came from. Normally invisible state — this is what to look at when a minted ID isn't the number you expected. Reads `none recorded` before this clone has minted, `unreadable` when the store is corrupt. `--json` gains an `id_frontier` object (`path`, `state`, `namespace`, `recorded`).
- `fr check` now flags **ID collisions involving an archive**, which nothing caught before: the duplicate-ID *error* and `fr clean`'s duplicate resolution both compare live tracks only, and so did minting (see Fixed). These are two different problems with different repairs, so they are reported separately:
  - **a live task holding an archived task's ID** (`id_reissued_after_archive`): the number was reissued after the original was archived. Renumber the live task by hand.
  - **one ID appearing more than once inside the archives**, with no live task involved (`duplicate_archived_id`): the same task's history was written twice — not a number handed out twice. Delete the extra copies. Reports a count and the distinct files, so several copies in one archive read as "appears 2 times in archive/main.md" rather than naming that file twice.

  Both are warnings rather than errors: there is no automatic repair, and they fire on data that predates these fixes. Surfaced in the CLI, the TUI check overlay, and `--json`.
- `fr check` now flags a **damaged ID frontier store**: one that exists but doesn't parse (the next mint resets it and falls back to scanning, which can't see another worktree's uncommitted tasks), and a leftover `frame-ids.toml.bak`, which means the frontier *was* reset at some point and numbers minted in that window may have been reissued. Check is read-only here — it deliberately leaves an unreadable store in place, so the warning names a file you can still inspect. `--json`: `id_frontier_unreadable`, `id_frontier_was_reset`.
- `fr check` now reports actor-registry drift: when this clone's gitignored `frame/.actor` token has no row in the committed `frame/actors.toml` (or its row is retired while the clone still holds it), check emits a warning pointing to the fix. Surfaced in both the CLI and the TUI check overlay.
- `fr check` now flags working-copy-local frame files leaking into git: `frame/.state.json`, `frame/.lock`, `frame/.recovery.log`, `frame/.actor`, and (for projects outside git) `frame/.ids.toml` / `frame/.ids.lock`. It distinguishes a file git already **tracks** (ignore rules no longer apply — needs `git rm --cached` plus a `.gitignore` line) from one that exists but **isn't ignored** (the next `git add -A` commits it), and prints the fix for each. `fr init` writes them all to `.gitignore`, but only at init, so a project created before an entry existed never got it and nothing noticed until the file was committed — or, for the append-only recovery log, conflicted on every merge. The file list is now a single constant shared by `fr init` and `fr check`, so the two can't drift. Surfaced in the CLI, the TUI check overlay, and `--json` (`local_file_committed`). Projects outside git are skipped.
- `fr check` also flags actor proliferation: when several *active* tokens share one provenance name (typically a hostname, e.g. a machine that auto-claimed a token per git worktree), it warns and suggests the `fr actor merge` to collapse them. Surfaced in the CLI and TUI check overlay.
- `fr check` now warns when a task note or inbox item body **leaves a code fence open**. Frame parses these correctly (see Fixed below — note extent is bound by indentation, not fence state), but an unclosed fence makes markdown renderers downstream — GitHub, editor previews — swallow the rest of the file into a code block, so a track file can look mangled everywhere except in `fr`. The warning names the offending opener (e.g. `` ```rust ``) and the task or 1-based inbox index. Fence balance follows CommonMark, where a fence carrying an info string cannot close a block, so `` ```lace `` / `` ```rust `` / `` ``` `` is balanced and does not warn. Surfaced in the CLI, the TUI check overlay, and `--json` (`unclosed_note_fence`, `unclosed_inbox_fence`).

### Changed
- `fr init`'s summary line now names the `.gitignore` entries it actually added, rather than a hardcoded three-of-four list that omitted `frame/.recovery.log` and named entries that were already present.
- Shelved tracks now reject new tasks and task activation. `fr add`, `fr push`, `fr sub`, `fr import`, `fr triage`, and `fr mv --track` into a shelved track fail with a message pointing to `fr track activate`, instead of silently writing to a track meant to be paused (usually the result of a stale `--track` argument). Likewise `fr state <id> active` / `fr start <id>` on a task in a shelved track is rejected. Closing out or re-opening existing work in a shelved track (done/parked/todo) is still allowed.

### Fixed
- **`fr clean` no longer writes a task into the archive twice.** Archival appends the batch to `archive/<track>.md` and *only then* removes those tasks from the track — deliberately, so a failure between the two writes can't lose a task. But the append wasn't idempotent, and the losing state (archived, yet still in Done) is reachable: a crash, or a `git checkout`/`reset`/`stash` reverting the track file after the archive write landed. The next `fr clean` then appended the same batch again. Found in a real project whose `archive/main.md` held 20 tasks twice — 41 task lines for 21 distinct tasks — by the new duplicate-archive check above. The append now skips any task whose ID the archive already holds, and drains it from Done regardless (leaving it there would make every future clean retry the same batch). The live copy being dropped should be identical to the archived one, but if it was edited after the first write it goes to the recovery log rather than vanishing.
- **Task ID numbers are no longer reissued.** Minting scanned the live track for the highest number in its namespace and added one, which is not a durable frontier — it moves *backwards* whenever the live maximum drops. Three ways it did:
  - **Two git worktrees of one clone.** Worktrees share an actor token, so they mint in the same namespace, but each scanned only its own working copy — so a task added in one and not yet committed was invisible to the other, and both handed out the same ID. This was the reported bug (a worktree-per-session agent workflow hit it repeatedly).
  - **`fr clean` archiving.** Archived done tasks leave the live track, so their numbers became available again — `frame/archive/main.md` holding `M-050` did not stop the next `fr add` from minting `M-050`.
  - **`fr delete`.** Deleting the highest-numbered task handed its number to the next mint.

  A mint now takes `max(scan floor, recorded frontier) + 1`. The floor includes the track's archives (`archive/<track>.md` and `archive/_tracks/<track>.md`), and the frontier is recorded durably in a file every worktree of the clone shares: `<root>/.git/frame-ids.toml` inside git — the same path from every linked worktree, and impossible to commit — or `frame/.ids.toml` outside it (added to `fr init`'s `.gitignore` list and `fr check`'s leak guard). The record is keyed by `(project, prefix, namespace)`, so it stays a handful of lines regardless of how many tasks exist, and it's written *before* the task is, so a number is spoken for from the instant it's handed out. Numbers are never reused and gaps are expected, so an abandoned mint costs nothing — no leases, no expiry, nothing to clean up.

  The store is regenerable cache and every failure degrades to the previous scan-only behavior rather than to a wrong answer: delete it, corrupt it (moved aside as `.bak`), or clone onto another machine and minting still works, just without cross-worktree protection. Writes are atomic under a dedicated lock file, so a crash mid-write can't leave a torn store. Two allocators are deliberately unchanged: subtask numbering (`PARENT.N`), where two worktrees adding a subtask to the *same* parent can still collide, and `fr actor merge`'s bulk renumber, whose target namespace belongs to another clone entirely.
- **A code fence in a task note no longer silently restructures the track file.** A note body whose fences didn't pair up — a pasted code sample, or prose naming two fence kinds — caused the note to absorb every following line to end of file: sibling tasks, the `## Parked` and `## Done` headers, and every completed task. The file stayed valid markdown and `fr check` reported it clean, so nothing warned; the damage surfaced as `fr show` returning `task not found` for a task `grep` could still see. The next write then committed it — appending to the note demoted the swallowed tail into note text, while any other rewrite of that task dropped it from the file entirely. A note's extent is now bound by **indentation alone** and code fences are not tracked at all, which is what makes the rule symmetric with the serializer (it re-indents every note line to the block indent) and the round-trip safe. Unbalanced fences are preserved verbatim and stay inside their note. The one shape this cannot represent is a fenced block containing flush-left lines — the serializer would have re-indented and corrupted that code anyway, so the boundary is now explicit rather than silently lossy.
- The same hazard in the **inbox**: an unclosed fence in one item's body absorbed every item after it, so three items parsed as one. Triaging the swallowing item then carried the absorbed items off into the new task's note and emptied the inbox. The next-item boundary (a `- ` at column 0) is now absolute and never suspended by fence state.
- `fr list` no longer panics on a track file with an unclosed fence in a note. Stripping a note line's block indent guarded on byte *length* rather than indentation, so it sliced characters off less-indented lines (`## Done` → `one`) and could split a multi-byte character — `line[4..]` inside a `§` aborted the process. The guard is now on indentation, which also makes the slice provably char-boundary-safe.
- Git worktrees of an `fr init`-created project no longer auto-claim a fresh actor token. The primary's `null` is recorded only in the main working tree's gitignored `frame/.actor` — never in the clone-wide shared file — so a linked worktree saw no token at all, concluded it was an unclaimed clone, and on its first mint claimed a new token *and wrote that claim into the committed `frame/actors.toml`*. Every worktree of every such project hit this, splitting one clone into two actors and leaving a spurious registry entry in whatever commit came next. Token resolution now falls back to the main working tree's local token (local → shared → main worktree), so a worktree inherits its clone's identity — including `null` — silently and without writing anything. `fr actor` reports when a token was inherited and from where.
- A linked git worktree now **never** auto-claims a token. When nothing resolves for it (a clone with no token anywhere, e.g. a project predating actor tokens), a mint fails with a routing message — `fr actor claim` in the main working tree to claim for the whole clone, or `fr actor claim --local` here to run this worktree as its own actor — instead of writing committed registry state as a side effect of `fr add`. A clone's own main working tree still auto-claims on first mint as before. `fr actor` in an unclaimed worktree explains this up front rather than leaving the next mint to be the messenger.
- A clone-wide `fr actor claim` / `fr actor set <token>` now clears this working copy's local `frame/.actor` (reporting what it removed) instead of leaving it to shadow the new token. Previously `fr actor set b` in the primary clone wrote the shared token but kept resolving — and minting — as `null`, so the command looked like a no-op. Claiming from a worktree also warns when the main working tree's local override still shadows the shared token there.
- `fr list --state done` now shows Done tasks in human output, matching what `--json` already returned. The human listing only read the Backlog and Parked sections, so filtering for `done` silently printed an empty track. Completed tasks are still omitted from an unfiltered `fr list` (they appear under a `-- Done --` header only when explicitly filtered for).
- Moving a task that lives in the Parked or Done section now works, both in the CLI (`fr mv`) and the TUI (`M` cross-track move) — previously only Backlog tasks could be moved. A cross-track move or reorder of a completed task used to fail with `task not found` even though `fr show` resolved it, because the move only scanned the Backlog. The task now moves and keeps its state — a moved Done task lands in the target track's Done section with its `resolved:` date intact, rather than being silently reopened — and the TUI move is fully undo/redo-safe in its original section. (The actor-token id form like `TOO-b8` was a red herring; the failure was the section, not the token.)
- Mint operations now self-heal a drifted actor registry: if this clone holds a token (`frame/.actor`) that is missing from `frame/actors.toml`, the next `fr add`/`push`/`sub`/triage re-registers it (announced once) instead of silently minting against an absent registry row. This recovers the case where a concurrent clone overwrote the committed registry — or a `git reset`/`restore` reverted an uncommitted claim — leaving the gitignored `.actor` orphaned. A deliberately-retired token is left alone (reported by `fr check` rather than resurrected).

## v0.1.6 - 2026-06-28

### Added
- Board view now shows subtasks: subtasks with Active, Todo (ready), or Done states appear in the appropriate board columns alongside top-level tasks
- Concurrent task IDs via actor tokens. Each working copy mints task IDs in its own actor-token namespace, so independent unsynced clones can create tasks in parallel without ever producing colliding IDs that clash on merge.
  - **Actor tokens:** `fr actor` (status), `fr actor claim`, `fr actor set <token|null>`, `fr actor retire <token>`, and `fr actor list` manage per-working-copy tokens, recorded in a committed `frame/actors.toml` registry and a gitignored `frame/.actor` file. `fr init` claims the `null` (primary) token; the first mint in an unclaimed clone auto-claims a token (announced once).
  - **Namespaced minting:** the primary (`null`) clone mints bare numbers (`EFF-14`); a clone with token `a` mints `EFF-a1`, and a subtask added by clone `b` under `EFF-a14` becomes `EFF-a14.b1`. Numbers auto-increment per namespace. Applies to `fr add`, `fr push`, `fr sub`, `fr import`, inbox triage, and the IDs assigned/reassigned by `fr clean`, plus the equivalent TUI actions.
  - **Namespace-correct re-keying on move:** cross-track moves (`fr mv --track`, TUI `M`) and reparent/promote (`fr mv --parent`/`--promote`, TUI move `h`/`l`) re-mint the new ID segments in the mover's namespace (e.g. clone `c` moving `EFF-a14` into track INF produces `INF-c1`, and a moved subtree re-keys to `INF-c1.c1`, `INF-c1.c2`). A move with no claimable token aborts with the `fr actor set …` routing message, changing nothing. A cross-track move changes the ID prefix, so creator provenance is not preserved across it.
  - **Token-aware integrity:** `fr check`, `dep:` resolution, ID comparison, lookup (`--after`/`--parent`/`--track`/jump-to-task), prefix rename, and abbreviated display all distinguish namespaces — `EFF-a14`, `EFF-14`, and `EFF-b14` are three distinct tasks, so only a genuine same-namespace collision is reported as a duplicate (the post-merge safety net) and a `dep:` on a tokened ID resolves to that exact task.
  - **At-a-glance surfacing:** the TUI Tracks overview header shows this clone's token compactly (`Project: NAME · actor: a` / `· primary` / `· unclaimed`), and a new read-only `fr info` command prints version, project name, frame directory, actor token, and active-track count (human or `--json`). Both are display-only and never claim a token; in `--json`, `actor` is the literal token, `"null"` for primary, or JSON `null` when unclaimed.
- `fr projects prune` removes registry entries whose project directory no longer exists (the `(not found)` entries shown by `fr projects`). Supports `--dry-run` and `--json`. Useful for clearing stale entries left by deleted or temporary projects.

### Changed
- TUI list scrolling now keeps a 4-line scrolloff margin between the cursor and the top/bottom edge, and reveals the cursor item's full (multi-line) summary instead of clipping it to its first line. An item taller than the viewport anchors to its first line and truncates at the bottom. Applies uniformly to the track, inbox, recent, search, and board views.

### Fixed
- CLI cross-track move (`fr mv <id> --track <t>`) now updates `dep:` references to the moved task across all other tracks, matching the TUI; previously it re-keyed the task but left dependents dangling
- Board view displayed task IDs with the track prefix doubled (e.g. `ST-ST-001`) when `[ids.prefixes]` was set; now shows the correct id (`ST-001`)
- Reparenting a task under a parent (TUI move mode) could reuse a deleted sibling's subtask number, producing a duplicate ID; the new child number is now gap-safe

## v0.1.5 - 2026-02-24

### Added
- Board view (`K` key in TUI): kanban-style cross-track view with Ready, In Progress, and Done columns. Features CC/All mode toggle (`c`), independent column navigation (`h`/`l`), tag filtering (`ft`), and all standard task actions (state changes, edit, deps, cross-track move). Layout adapts to terminal width (3-column, 2-column, single-column).
- `board_done_days` config option: number of days of completed tasks to show in the Board Done column (default: 7, 0 = hide Done column)
- "Open Board" command palette action
- Project-wide search (`S` key in TUI): search across all active tracks, inbox, and archives with grouped results, section jumping, and jump-to-task navigation
- "Project search" command palette action
- `Cmd+J` / `Ctrl+J` in multi-line note editing: vim-style join lines (appends next line to current with space, trims leading whitespace)
- `K` keybinding now shown in all help overlay "Views" sections

### Changed
- `fr search` now includes archived tasks by default (previously required `--archive` flag; flag still accepted for backward compatibility)

## v0.1.4 - 2026-02-15

### Added
- Soft word wrap for inbox item titles in both view and edit modes
- Soft word wrap for track view task titles in both view and edit modes
- `done_retain` config option: number of recent done tasks to keep in track after archiving (default: 10)

### Fixed
- Archived tasks not appearing in Recent view — archive file header caused parser to return zero tasks
- File watcher incorrectly matching archive files (e.g., `archive/main.md`) as track files — caused track to display "No tasks yet" after auto-clean archived done tasks; also fixed same bug for `archive/inbox.md`
- Auto-clean not saving track file after archiving done tasks or reconciling sections
- Right arrow on expanded task with done subtasks caused cursor to disappear (landed on non-selectable DoneSummary item)
- Jump-to (`J`) on done subtasks showed "not found" instead of opening detail view
- `G` key not jumping to bottom in Recent view

### Changed
- `done_threshold` now counts top-level done tasks instead of serialized lines (default changed from 250 to 100)
- Refactored TUI input handling: split 12,679-line `input/mod.rs` into 13 focused submodules (common, navigate, select, search, edit, move_mode, triage, confirm, command, popups, tracks, recent)
- Extracted shared render utilities (`state_symbol`, `abbreviated_id`, `collect_metadata_list`, `spans_width`) into `render/helpers.rs`
- Deduplicated parse utilities: shared `parse_title_and_tags` and `count_indent` across task and inbox parsers

## v0.1.3 - 2026-02-12

### Added
- `fr start <ID>` CLI command as a shortcut for `fr state <ID> active`
- `fr done <ID>` CLI command as a shortcut for `fr state <ID> done`
- `Alt+Up`/`Alt+Down` in recovery log overlay to jump between log entries
- `fr delete <id>...` CLI command for permanently removing tasks (with `--yes` flag to skip confirmation)
- Task deletion via command palette in Track, Detail, and Recent views (supports bulk deletion with multi-select)
- Results overlay for displaying structured output from project checks and clean previews
- "Check project" command palette action — runs `fr check` inline and displays results in the TUI
- "Preview clean" command palette action — shows what `fr clean` would do without writing changes
- "Prune recovery" command palette action — prune old recovery log entries with confirmation
- "Unarchive track" command palette action — restore archived tracks to active state
- "Import tasks" command palette action — import tasks from a markdown file into the current track
- `c` key binding in Detail view to toggle the `#cc` tag (also works on subtasks when cursor is in the Subtasks region)

### Fixed
- Subtask ID collision: adding a new subtask after deleting one could reuse an existing sibling's ID, causing edits/deletions to target the wrong task

### Changed
- `X` (archive/delete track) and `R` (rename prefix) keybindings removed from Tracks view; these actions are now palette-only ("Archive track", "Delete track", "Rename track prefix" via `>`)
- Archive and delete are now separate palette actions: "Archive track" appears for non-empty tracks, "Delete track" for empty tracks

## v0.1.2 - 2026-02-10

### Added
- `N` key binding to edit note with cursor at the start (both Detail and Inbox views); `n` now consistently places cursor at the end in both views
- Recovery log (`frame/.recovery.log`) prevents silent data loss: captures parser-dropped lines, write failures, and dismissed TUI edit conflicts
- `fr recovery` command to view, prune, and manage the recovery log; `fr check` integration reports `#lost` tasks and log summary
- Recovery log overlay in TUI command palette ("View recovery log")
- Atomic file writes using temp file + rename for all track, inbox, config, and state saves
- Soft word wrap for notes in Detail view and Inbox (view mode always wraps; edit mode wraps by default, togglable with `w` / `Alt+w`)
- `fr ready --cc` now scans all active tracks for `#cc`-tagged tasks (focus track tasks sort first); `cc_focus` is no longer required
- `fr track cc-focus --clear` to remove the cc-focus setting
- Undo stack is now capped at 500 entries to prevent unbounded memory growth in long TUI sessions

### Fixed
- Cursor in wrapped edit mode no longer goes off the right side of the window when positioned at the end of a line that fills the available width; it now wraps to the next visual row
- Spaces typed at the wrap boundary in edit mode are now visible on the next visual line instead of being silently consumed
- Triage validates destination (backlog section and after-target) before removing inbox item, preventing data loss if validation fails
- Triage and cross-track move saves now write new data before deleting old data (track before inbox, target before source), preventing loss if the second write fails
- `fr clean` archive writes the archive file before extracting done tasks from the track; if the archive write fails, tasks are left in place
- TUI pending move flushes and critical multi-save sites now log to the recovery log on failure instead of silently discarding errors
- `[ids.prefixes]` and `[ui.tag_colors]` key order in `project.toml` no longer randomizes on each save; order now matches the original file
- Parking a task with `~` now moves it to the Parked section after the grace period (previously only updated state without moving)
- Parked tasks no longer disappear when the track has no `## Parked` section; the section is now created automatically on first use
- Tasks in the wrong section for their state (e.g., parked task in Backlog) are automatically moved to the correct section on TUI load, file reload, and `fr clean`
- CLI `fr state ID parked` now moves tasks to the Parked section (and un-parking/reopening moves them back to Backlog)
- New tracks created with `fr track add` now include a `## Parked` section
- Unicode correctness throughout TUI: CJK, emoji, combining marks, and fullwidth characters now display and edit correctly
- Cursor movement in edit mode uses grapheme clusters instead of raw bytes, preventing panics on non-ASCII text
- Display width calculations use terminal cell width instead of character count, fixing column alignment for wide characters
- Word wrap in note editor respects grapheme boundaries and character display widths

## v0.1.1 - 2026-02-10

### Added
- Subtask reparenting in TUI move mode: `h` outdents (promotes), `l` indents (makes child of sibling above), `j`/`k` cross parent boundaries; IDs re-keyed on confirm
- CLI `fr mv --promote` and `fr mv --parent <id>` flags for subtask reparenting
- Search highlighting in detail view (title, ID, tags, deps, spec, refs, note, subtasks)
- `n`/`N` navigation in detail view to cycle between search matches
- Startup hints in status bar (`? help  > commands  QQ quit`) until first keypress
- Actionable empty-state messages ("No tracks — press **a** to create one", "No tasks yet — press **a** to add one")
- `fr show --context` flag to display ancestor chain (parent tasks root-first) for subtasks; JSON output always includes `ancestors` array
- `cc_only` setting in `[agent]` config (default: `true`); included in `fr ready --cc --json` output so agents know whether to broaden search when no `#cc` tasks are available
- Agent setup guide (`doc/agent-setup.md`) documenting how to configure frame for AI coding agents

### Changed
- **Breaking:** `fr note ID "text"` now appends to existing notes instead of replacing; use `--replace` for the old overwrite behavior
- Search match count now only counts visible tasks (excludes Done section, respects filters, skips context rows)
- Search match count refreshes on tab/view switch

### Fixed
- Subtask move undo operating on wrong sibling list (added `parent_id` to `Operation::TaskMove`)

## v0.1.0 - 2026-02-09

Initial release.
