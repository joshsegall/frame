# Changelog

All notable changes to frame will be documented in this file.

## [Unreleased]

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
