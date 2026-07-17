# Changelog

All notable changes to frame will be documented in this file.

## Unreleased

### Removed
- `fr clean` no longer generates `ACTIVE.md`. The summary was write-only — nothing read it back — and went stale after any change that wasn't a clean, making the committed copy misleading. Use `fr ready` / `fr list` for a live view of active work.

### Added
- Git worktrees now share one actor identity by default. The clone-wide token lives under the git common directory (`<root>/.git/frame-actor`), which every worktree resolves to the same path, so a worktree-per-session workflow no longer auto-claims a fresh token per worktree. A local `frame/.actor` still overrides it for a single working copy; `fr actor claim --local` / `fr actor set --local` write that override deliberately (to run a worktree as a distinct concurrent actor), while the primary's `null` stays local. `fr actor` now shows whether the token is local or shared. (Worktrees share the token but not the lock, so simultaneous mints can still collide — caught by `fr check` and repaired by `fr actor merge`; this trades a rare, fixable collision for no token proliferation.)
- `fr actor merge <from>... --into <token>` collapses several actor namespaces into one, renumbering every id minted by a source token into the target's sequence and retiring the sources. Useful when a machine accumulates tokens (e.g. a git-worktree-per-session workflow, where each fresh working copy auto-claims its own token). The remap is per-segment — a subtask minted by a third actor is preserved (`SEC-d1.a3` → `SEC-b2.a3`, not `SEC-b2.b1`) — and covers all tracks, archives, and `dep:` references. Prints the full `OLD → NEW` map (`--json` supported). `--dry-run` previews without writing; `--rewrite-notes` also rewrites id mentions in note/spec/ref prose, skipping git citations like `fix(SEC-d1)`.
- `fr check` now reports actor-registry drift: when this clone's gitignored `frame/.actor` token has no row in the committed `frame/actors.toml` (or its row is retired while the clone still holds it), check emits a warning pointing to the fix. Surfaced in both the CLI and the TUI check overlay.
- `fr check` also flags actor proliferation: when several *active* tokens share one provenance name (typically a hostname, e.g. a machine that auto-claimed a token per git worktree), it warns and suggests the `fr actor merge` to collapse them. Surfaced in the CLI and TUI check overlay.

### Changed
- Shelved tracks now reject new tasks and task activation. `fr add`, `fr push`, `fr sub`, `fr import`, `fr triage`, and `fr mv --track` into a shelved track fail with a message pointing to `fr track activate`, instead of silently writing to a track meant to be paused (usually the result of a stale `--track` argument). Likewise `fr state <id> active` / `fr start <id>` on a task in a shelved track is rejected. Closing out or re-opening existing work in a shelved track (done/parked/todo) is still allowed.

### Fixed
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
