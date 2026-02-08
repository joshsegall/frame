# Track Management — Spec Additions

These additions cover TUI track management in the Tracks view and the
CLI `fr track rename` command.

---

## 1. Tracks View — TUI Interactions

**Replace** the existing Tracks view description (starting at "Tracks
view (`0` or `` ` ``):" through the ASCII mockup) with:

---

**Tracks view** (`0` or `` ` ``):
Full-screen listing of all tracks grouped by state: Active, Shelved,
Archived. Archived tracks are always visible, rendered in dim text.
Manage track lifecycle, reorder, set cc-focus.

```
 Effects │ Unique │ Infra │ ▸ │ *5 │ ✓ │
──────────────────────────────────────────────────────────
                     id   todo  act  blk done park
 Active                    [ ]  [>]  [-]  [x]  [~]
▎1  Effect System    EFF     8    3    1   12    2  ★ cc
 2  Unique Types     UNQ     5    1    0    3    0
 3  Compiler Infra   INF     3    2    0    8    1

 Shelved                   [ ]  [>]  [-]  [x]  [~]
 4  Module System    MOD     3    0    1    6    0

 Archived                  [ ]  [>]  [-]  [x]  [~]
 5  Bootstrap        BST     0    0    0   15    0
```

**Tracks view keys (NAVIGATE mode):**

| Key     | Action                                          |
|---------|-------------------------------------------------|
| `a`     | Add new track (inline EDIT for name)            |
| `e`     | Edit track name (inline EDIT)                   |
| `s`     | Toggle shelved↔active                           |
| `D`     | Archive or delete track (with confirmation)     |
| `C`     | Set cc-focus on selected track                  |
| `m`     | Enter MOVE mode (reorder tracks)                |
| `Enter` | Switch to selected track's tab view             |
| `/`     | Search across all tracks                        |

Standard navigation (`↑↓`, `g`/`G`, `Cmd+↑/↓`) works as in other views.

**Adding a track (`a`):**

Opens an inline single-line editor at the bottom of the Active section.
Type the track name (e.g. "Effect System"). `Enter` confirms. Frame
auto-derives the track id by slugifying the name (lowercase, spaces to
hyphens) and generates a 3-letter prefix using the standard prefix
rules. The track is created as active, with an empty track file at
`tracks/{id}.md` and a config entry in `project.toml`.

If the auto-derived id or prefix collides with an existing track, Frame
resolves collisions using the same rules as `fr init` (extend the prefix
from earlier segments). The user can customize the id and prefix in
`project.toml` afterward, or use the CLI `fr track rename` for a full
rename.

`Esc` cancels without creating anything.

**Undo**: Adding a track is one undo step. Undo removes the track file,
removes the config entry, and removes the tab. This is safe because a
freshly added track has no tasks.

**Editing a track name (`e`):**

Enters EDIT mode on the selected track's name. Single-line editor,
`Enter` confirms, `Esc` cancels. Updates the display name in both
`project.toml` and the `# Title` header line in the track's markdown
file, keeping them in sync.

The track id, prefix, and filename are not affected.

**Undo**: One undo step, restores the previous name in both locations.

**Toggling shelved/active (`s`):**

On an active track: sets state to shelved. The track disappears from
the tab bar and moves to the Shelved section in the Tracks view. The
cursor stays in place (advances to next track if at end of section).

On a shelved track: sets state to active. The track appears in the tab
bar and moves to the Active section.

On an archived track: no effect. Unarchiving is a heavier operation —
use `fr track activate <id>` from the CLI.

**Undo**: One undo step, restores the previous state. If a track was
shelved, undo reactivates it (and vice versa).

**Archiving / deleting (`D`):**

`D` shows a confirmation prompt in the status row. The prompt message
depends on whether the track has task history:

- **Track has tasks** (any tasks in backlog, parked, done, or an archive
  file exists):
  ```
  Archive "Effect System"? (14 tasks will be preserved in archive)  y/n
  ```
  Confirming moves the track file to `archive/_tracks/{id}.md`, sets
  the config state to `archived`, and removes the tab. The per-task
  archive file (if any) remains in `archive/{id}.md`.

- **Track is empty** (no task lines in the file, no archive file):
  ```
  Delete "New Track"? (empty track, will be removed)  y/n
  ```
  Confirming deletes the track file, removes the config entry entirely.
  The track is gone — this is the "oops, typo" escape hatch.

`n` or `Esc` cancels.

**Undo for archive**: One undo step. Undo restores the track file from
`archive/_tracks/` back to `tracks/`, sets config state back to its
previous value, and restores the tab. This works because the file was
moved, not destroyed.

**Undo for delete (empty track)**: One undo step. Undo recreates the
empty track file and config entry. No data to lose, so this is
straightforward.

**Setting cc-focus (`C`):**

Sets the selected track as the cc-focus track (the track where agents
look for work via `fr ready --cc`). The ★cc indicator moves to the
selected track. Only one track can be cc-focus at a time — setting it
on one track implicitly unsets it on the previous.

Works on active tracks only. On shelved or archived tracks: no effect.

**Undo**: One undo step. Undo restores cc-focus to the previously
focused track (or clears it if there was none before).

---

## 2. CLI — `fr track rename`

**Add** to the CLI Writing section under Track management:

```
# Track management
fr track new <id> "name"
fr track shelve <id>
fr track activate <id>
fr track archive <id>
fr track delete <id>
fr track mv <id> <position>
fr track cc-focus <id>
fr track rename <id> [--name "Name"] [--id new-id] [--prefix NEW]
```

### `fr track rename`

Rename aspects of a track. Flags can be combined in a single command.

```bash
# Change display name only
fr track rename effects --name "Effect Handlers"

# Change track id (moves file, updates config)
fr track rename effects --id effect-handlers

# Change prefix (bulk renames all task IDs + dep references)
fr track rename effects --prefix FX

# All at once
fr track rename effects --name "Effect Handlers" --id effect-handlers --prefix FX

# Preview prefix change without writing
fr track rename effects --prefix FX --dry-run
```

**`--name "Name"`**: Changes the display name in `project.toml` and
the `# Title` header in the track's markdown file, keeping them in
sync.

**`--id new-id`**: Changes the track id in `project.toml`, moves the
track file from `tracks/{old}.md` to `tracks/{new}.md`, and moves the
archive file if one exists (`archive/{old}.md` → `archive/{new}.md`).
The task prefix is not changed — task IDs remain the same. Validates
that the new id doesn't collide with an existing track.

**`--prefix NEW`**: The heavy operation. Rewrites every task ID in the
track from the old prefix to the new one (`EFF-014` → `FX-014`), and
updates every `dep:` reference across all tracks that pointed to the
old prefix. Subtask IDs are updated recursively (`EFF-014.2` →
`FX-014.2`). Updates the `[ids.prefixes]` entry in `project.toml`.

Before writing, prints a summary:

```
Renaming prefix EFF → FX:
  18 tasks in effects
  6 dep references across 3 other tracks
Proceed? [y/n]
```

With `--dry-run`, prints the summary and exits without writing.

Validates that the new prefix doesn't collide with an existing track's
prefix.

### `fr track delete`

Deletes a track entirely. Only works if the track is empty (no task
lines in the file, no archive file). If the track has tasks, prints
an error directing the user to `fr track archive` instead.

```bash
fr track delete new-track
# Error: track "new-track" has 3 tasks. Use `fr track archive` instead.

fr track delete empty-track
# Deleted track "empty-track"
```

---

## 3. Implementation Plan Additions

### Phase 2 additions

```
2.5a Track lifecycle operations (ops/track_ops.rs)
     - archive_track: move file to archive/_tracks/, update config state
     - delete_track: remove file + config entry (only if empty)
     - is_track_empty(track): check for zero task lines + no archive file
     - rename_track_name: update display name in config + # Title header
       in track file (both locations must stay in sync)
     - rename_track_id: update config, move file(s)
     - rename_track_prefix: bulk rewrite task IDs, update deps across
       all tracks, update config prefix entry
     - generate_track_id(name): slugify name for auto-derived id
```

### Phase 3 additions

```
3.4a Implement track rename CLI command
     - fr track rename <id> [--name] [--id] [--prefix] [--dry-run]
     - Summary output before prefix changes
     - Confirmation prompt (skippable with --yes)
     - fr track delete <id> (empty-only guard)
```

### Phase 4 additions

```
4.7a Tracks view interactions (tui/input/tracks_view.rs)
     - a: inline EDIT for new track name, auto-derive id/prefix
     - e: inline EDIT on selected track name
     - s: toggle shelved↔active
     - D: archive/delete with confirmation prompt
     - C: set cc-focus
     - Confirmation prompt rendering in status row
     - Undo operations for all track management actions
```

### Undo stack additions (Phase 5.5)

New `Operation` variants:

```
TrackAdd { track_id }
  → undo: remove track file + config entry

TrackNameEdit { track_id, old_name }
  → undo: restore old name in config + track file header

TrackStateChange { track_id, old_state }
  → undo: restore old state in config, move file back if archived

TrackArchive { track_id, old_state, file_contents }
  → undo: recreate track file from stored contents, restore config

TrackDelete { track_id, config_entry, file_contents }
  → undo: recreate track file + config entry from stored data

TrackCcFocus { old_focus_track_id }
  → undo: restore cc-focus to old track (or clear if None)
```

Note: `TrackArchive` stores the file contents in the undo operation
so the file can be restored even though it was moved. This is an
in-memory snapshot — if the TUI restarts, the undo stack is gone and
git is the recovery path (consistent with the session-only undo model).

---

## 4. Design Decisions Log Additions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Track deletion | Archive by default; delete only if empty | Preserves history; git-friendly |
| Archived tracks display | Always visible in Tracks view, dimmed | Simple; no toggle needed until scale demands it |
| Unarchive | CLI only (`fr track activate`) | Rare operation; not worth a TUI key |
| Track creation input | Name only; id/prefix auto-derived | Minimal friction; customize in config or via rename |
| Prefix rename | Editable any time; triggers bulk rewrite | Flexibility outweighs complexity; machinery exists in dep-update logic |
| Track rename CLI | Single command, composable flags | One command for all rename aspects; `--dry-run` for safety |
| Track name sync | `project.toml` name and `# Title` header updated together | Single source of truth would be ideal but both are user-visible; keeping them in sync avoids confusion |
