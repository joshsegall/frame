# Subtask Reparenting

## Summary

Allow subtasks to change depth and parent during move mode (TUI) and via CLI flags. Today, move mode repositions tasks among siblings. This adds `h`/`l` in move mode to outdent/indent, and `--parent`/`--promote` flags to `fr mv`.

## Behavior

### TUI — Move Mode Changes

When move mode is entered on any task (top-level or subtask), the keys work as follows:

| Key | Action |
|-----|--------|
| `j`, `Down` | Move to next valid position at same depth |
| `k`, `Up` | Move to previous valid position at same depth |
| `h`, `Left` | Outdent: reparent under grandparent, or promote to top-level |
| `l`, `Right` | Indent: reparent under the sibling above (becomes its last child) |
| `g`, `Home` | First position at same depth |
| `G`, `End` | Last position at same depth |
| `Enter` | Confirm |
| `Esc` | Cancel and restore |

`j`/`k` are constrained to same-depth positions but can cross parent boundaries. A depth-1 task can move under any top-level task. A depth-2 task can move under any depth-1 task. The valid positions are: between existing siblings of a parent, as the first child of a parent that has no children yet, or as the last child. This means you can move a subtask between parents fluidly without manually outdenting and re-indenting.

#### Outdent (`h`)

- Task becomes a sibling of its current parent, inserted directly after the parent.
- The entire subtree moves with it.
- If the task is already top-level, no-op.
- Blocked if any descendant would exceed max depth (3 levels) after the move. (In practice, outdenting always reduces depth, so this only matters for indent.)

#### Indent (`l`)

- Task becomes the last child of the sibling directly above it in the current list.
- If no sibling above exists, no-op.
- If the task (or any descendant) would exceed max depth (depth 2 = sub-subtask), no-op.
- The entire subtree moves with it.

#### Visual feedback

During move mode, the task's rendered position updates live as the user presses `h`/`l`/`j`/`k`, showing the new indentation level and position. The tree lines redraw to reflect the provisional placement.

The status bar shows:

```
-- MOVE --                          ▲▼ move  ◀▶ depth  m/Enter ✓  Esc ✗
```

The `◀▶ depth` hint is always shown, even for top-level tasks (where `◀` is a no-op but `▶` may indent under the sibling above).

### CLI — `fr mv` Extensions

```bash
fr mv EFF-014.2 --promote              # promote to top-level task
fr mv EFF-014.2 --parent EFF-020       # reparent under EFF-020
```

| Flag | Description |
|------|-------------|
| `--promote` | Promote subtask to top-level. Placed after its former parent by default. Combinable with `--top`, `--after`, or positional arg to control placement. |
| `--parent ID` | Reparent under the given task. Becomes last child. Target must be in the same track (use `--track` for cross-track reparent). |

Validation:
- `--promote` on a top-level task: error.
- `--parent` targeting a task that would exceed max nesting depth: error.
- `--parent` targeting the task itself or one of its descendants: error (cycle).
- `--parent` combined with `--promote`: error (conflicting).

### ID Re-keying

When a task changes parent (or becomes top-level), its ID and all descendant IDs are rewritten:

- **Promote to top-level**: `EFF-014.2` → `EFF-025` (next available top-level ID in the track). Children `EFF-014.2.1` → `EFF-025.1`, etc.
- **Reparent under EFF-020**: `EFF-014.2` → `EFF-020.3` (next available child slot). Children `EFF-014.2.1` → `EFF-020.3.1`, etc.
- **Indent under sibling**: Same as reparent.
- **Outdent**: `EFF-014.2` → `EFF-025` (next top-level) or `EFF-014.2.1` → `EFF-014.3` (next sibling of former parent), depending on target depth.

All dep references across all tracks are updated to point to the new IDs (same scan used by cross-track move).

### Undo

Reparent is a single undo operation. It restores:
- Original parent (or top-level status)
- Original position among siblings
- Original IDs for the task and all descendants
- Original dep references

New `Operation` variant: `Reparent { track_id, task_id, old_parent_id, new_parent_id, old_ids, new_ids, old_position }` (or similar — exact fields TBD during implementation).

### Edge Cases

- **Max depth exceeded**: `l` (indent) is a no-op if the task or any descendant would land at depth 3+. CLI returns an error.
- **No sibling above for indent**: If no sibling exists directly above at the current depth, `l` is a no-op. CLI returns an error.
- **`j`/`k` across parents**: Any parent at depth N-1 is a valid target for a depth-N task. For depth-1 tasks, every top-level task is a potential parent. For depth-2 tasks, every depth-1 task is a potential parent. `j`/`k` enumerate all valid slots in document order.
- **Cross-track reparent**: `fr mv EFF-014.2 --track infra --parent INF-005`. The task is moved to the new track, reparented, and re-keyed with the new track's prefix. This combines existing cross-track logic with the new reparent logic.
- **Subtree integrity**: The entire subtree always moves as a unit. No partial reparenting.
- **Bulk reparent**: Not supported in v1. Multi-select move uses the existing flat repositioning. Reparenting is single-task only.

## Implementation Plan

### Phase 1: Core ops (`src/ops/task_ops.rs`)

1. Add `reparent_task(project, track_id, task_id, new_parent_id: Option<String>)` function.
   - `None` = promote to top-level.
   - Validates depth constraints, cycle detection.
   - Removes task subtree from old parent's children (or from section top-level list).
   - Re-keys all IDs in the subtree.
   - Inserts into new parent's children (or section top-level list).
   - Scans all tracks to update dep references to old IDs.
   - Returns old/new ID mappings for undo.

2. Add `promote_task(project, track_id, task_id, position)` convenience wrapper — calls `reparent_task` with `None` parent, then positions.

### Phase 2: CLI (`src/cli/`)

1. Add `--promote` and `--parent <ID>` flags to the `mv` command in clap.
2. Add handler logic that calls into the new ops functions.
3. Handle flag conflicts (`--promote` + `--parent`, `--promote` on top-level, etc.).

### Phase 3: TUI move mode (`src/tui/input/`)

1. Extend move mode state to track current depth and parent context.
2. Add `h` handler: compute outdent target, validate, update provisional position.
3. Add `l` handler: compute indent target (sibling above), validate depth, update provisional position.
4. On confirm (`Enter`): if depth/parent changed, call `reparent_task` from ops, then apply position. If only position changed, use existing move logic.
5. Live visual preview: update `FlatItem` list during move to show provisional tree structure.

### Phase 4: Undo (`src/tui/undo.rs`)

1. Add `Operation::Reparent` variant with fields for old/new parent, old/new IDs, old position.
2. Implement undo: restore original parent, re-key back to old IDs, update deps, restore position.
3. Implement redo: re-apply the reparent.
4. `UndoNavTarget`: navigate to the task at its new (or restored) location.

### Phase 5: Docs

1. `doc/tui.md` — Update Move Mode keybinding table to include `h`/`l` for outdent/indent. Add note about depth constraints.
2. `doc/cli.md` — Add `--promote` and `--parent` flags to `fr mv` section.
3. `skills/managing-frame-tasks/SKILL.md` — Add `--promote` and `--parent` to the `fr mv` row in the command reference table.
4. `doc/architecture.md` — Brief note in the Task ID System section about re-keying on reparent.
5. `README.md` — No change needed (move is already mentioned, details are in the reference docs).

### Phase 6: Tests

1. Unit tests in `src/ops/task_ops.rs`:
   - Promote subtask to top-level, verify ID rewrite and dep updates.
   - Reparent under new parent, verify ID and position.
   - Depth limit enforcement (indent at max depth).
   - Cycle detection (reparent under own descendant).
   - Cross-track reparent with `--parent`.
2. Integration tests in `tests/round_trip.rs`:
   - Reparent round-trip: parse → reparent → serialize → parse, verify structure.
   - Verify unmodified tasks preserve source text (selective rewrite invariant).
3. CLI integration tests:
   - `fr mv ID --promote` end-to-end.
   - `fr mv ID --parent ID` end-to-end.
   - Error cases (bad flags, depth exceeded).
