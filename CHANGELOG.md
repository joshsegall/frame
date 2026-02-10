# Changelog

All notable changes to frame will be documented in this file.

## [Unreleased]

### Added
- Soft word wrap for notes in Detail view and Inbox (view mode always wraps; edit mode wraps by default, togglable with `w` / `Alt+w`)
- `fr ready --cc` now scans all active tracks for `#cc`-tagged tasks (focus track tasks sort first); `cc_focus` is no longer required
- `fr track cc-focus --clear` to remove the cc-focus setting

### Fixed
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
