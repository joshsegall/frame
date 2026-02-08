# Markdown File Format

Frame uses plain markdown files as its data store. This document specifies the exact syntax recognized by the parser.

## Track Files

Each track is a single `.md` file with this structure:

```markdown
# Track Title

> Optional description line.

## Backlog

- [ ] `ID-001` Task title #tag
  - added: 2025-05-10
- [>] `ID-002` Active task

## Parked

- [~] `ID-010` Parked task

## Done

- [x] `ID-003` Completed task
  - resolved: 2025-05-14
```

### Sections

Three section headers are recognized (case-insensitive):

- `## Backlog` — todo, active, and blocked tasks
- `## Parked` — intentionally paused tasks
- `## Done` — completed tasks

Any content that isn't a section header or task is treated as literal passthrough text and preserved verbatim.

### Title and Description

The first `# Title` line sets the track's display title. An optional `> Description` line can follow.

## Task Syntax

A task line has this format:

```
INDENT- [STATE] `ID` Title text #tag1 #tag2
```

**Indentation**: 0 spaces for top-level, 2 for subtasks, 4 for sub-subtasks (3 nesting levels max).

**Checkbox states**:

| Char | State |
|------|-------|
| ` ` (space) | Todo |
| `>` | Active |
| `-` | Blocked |
| `x` | Done |
| `~` | Parked |

**ID** (optional): Enclosed in backticks after the checkbox. Format: `PREFIX-NNN` for top-level, `PREFIX-NNN.N` for subtasks, `PREFIX-NNN.N.N` for sub-subtasks.

**Tags** (optional): `#word` tokens at the end of the line. Parsed right-to-left from the end; only trailing `#word` sequences are recognized as tags.

Examples:

```markdown
- [ ] Task with no ID
- [>] `EFF-014` Implement inference #cc
  - [ ] `EFF-014.1` First subtask
    - [ ] `EFF-014.1.1` Deep subtask
```

## Metadata Lines

Metadata lines are indented under their task (task indent + 2 spaces) and start with `- key: value`:

```markdown
- [>] `EFF-014` Task title
  - added: 2025-05-10
  - dep: EFF-003, INFRA-007
  - ref: doc/design.md, src/parser.rs
  - spec: doc/spec.md#section
  - note: Short note text
```

### Metadata Types

**`added: YYYY-MM-DD`** — Creation date.

**`resolved: YYYY-MM-DD`** — Completion date.

**`dep: ID1, ID2`** — Comma-separated dependency task IDs.

**`ref: path1, path2`** — Comma-separated file paths (relative to project root).

**`spec: path#section`** — Single spec file path with optional `#anchor`.

**`note: text`** — Single-line note, or multi-line block:

```markdown
- [>] `EFF-014` Task title
  - note:
    First line of note.

    Second paragraph.

    ```rust
    fn example() {}
    ```

    Text after code block.
```

Multi-line notes: continuation lines are indented under the `note:` key. Blank lines within the note are preserved. Fenced code blocks (`` ``` ``) are tracked to avoid parsing their contents as tasks or metadata.

## Nesting

Tasks nest up to 3 levels deep. Each level adds 2 spaces of indentation:

```markdown
- [>] `N-001` Top level (depth 0)
  - added: 2025-05-10
  - [ ] `N-001.1` Subtask (depth 1)
    - note: Details here
    - [ ] `N-001.1.1` Sub-subtask (depth 2)
    - [ ] `N-001.1.2` Another sub-subtask
  - [>] `N-001.2` Second subtask
```

Metadata always comes before subtasks for a given task. Subtask IDs follow the pattern `PARENT.N`.

## Blank Lines

- Non-task lines (including blank lines) **terminate** task parsing — the track parser handles inter-section blank lines.
- Blank lines between the section header and first task are preserved.
- Blank lines after the last task in a section are preserved.
- Blank lines within multi-line notes are preserved.

## Inbox File

`inbox.md` uses a simpler format — list items with no checkboxes or IDs:

```markdown
# Inbox

- Parser crashes on empty blocks #bug
  Stack trace points to parser.rs line 142.

- Think about expression vs statement design #design
  #research
  If it's an expression, we get composability:
  ```
  let x = perform Ask() + 1
  ```
  But it makes the type system more complex.

- Quick idea for later #todo
```

### Inbox Item Structure

**Title line**: `- Title text #tag1 #tag2`

**Tag-only continuation lines**: Indented lines where every word starts with `#` are parsed as additional tags, not body text:

```markdown
- Some idea
  #design #research
```

**Body text**: Any other indented continuation lines (1+ spaces). Body text is stripped of 2 leading spaces when present.

**Item separation**: Blank lines between items.

## Selective Rewrite

Frame uses a selective rewrite strategy for round-trip preservation:

- Each parsed task stores its original source lines (`source_text`) and a `dirty` flag.
- `source_text` contains only the task's own lines (task line + metadata), **not** subtask lines.
- On serialization:
  - **Clean tasks** (not modified): emit `source_text` verbatim, preserving exact formatting.
  - **Dirty tasks** (modified): regenerate in canonical format.
  - Subtasks are always serialized independently regardless of parent's dirty state.

This means editing one task never reformats its parent, siblings, or unrelated tasks. If no mutations occur, parse-then-serialize produces byte-identical output.
