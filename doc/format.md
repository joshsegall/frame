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

The ID grammar is:

```
task_id  = prefix "-" segment ("." segment)*
segment  = token? number
token    = one or more lowercase letters   (omitted = "null" / default namespace)
number   = digits
prefix   = the track's configured prefix (e.g. EFF)
```

A segment is a maximal run of lowercase letters (the optional token) followed by
a maximal run of digits (the number); because letters and digits are disjoint no
delimiter is needed between them. The token names the
[actor-token namespace](concepts.md#actors) the segment was minted in: the
primary working copy mints tokenless (null-namespace) IDs like `EFF-014` and
`EFF-014.2`, while a clone holding token `a` mints `EFF-a14`, and a subtask added
there under someone else's task reads `EFF-014.a1`.

**A subtask's ID extends its parent's**: every segment but the last matches the
parent exactly, whatever namespace the last one carries. `EFF-014.2` and
`EFF-014.a1` are both children of `EFF-014`; `EFF-020` nested under it is not, and
neither is `EFF-014.2.1`. `fr check` reports a subtask that breaks the rule and
`fr check --fix` renumbers it back under its parent.

**Literal passthrough**: any backtick-wrapped ID that does not match this grammar
(including legacy or hand-written IDs) is preserved verbatim on round-trip and is
ignored when frame computes the next ID to mint, so it never perturbs numbering.
Zero-padding in the number is preserved (`EFF-014` stays `EFF-014`). Such an ID
carries no parent/child relationship either, so a hand-written hierarchy is never
reported as broken.

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
  - spec: doc/spec.md#section, doc/rfc.md
  - note: Short note text
```

### Field order

Metadata is written in a fixed order:

```
conflict, added, resolved, dep, spec, ref, note
```

Short scalar fields first, `note` last. A note is the only field with no length bound, so anything written after one is written past it — a `resolved:` date below a fifty-line note reads as though the task had none. `fr show`, `--json` and the TUI Detail view all present fields in this order too.

The parser accepts any order; a hand-edited file is never wrong for using one. Frame rewrites a task's fields into this order the first time it edits *that task*, so an existing project converges task by task instead of in one large diff, and untouched tasks stay byte-identical. To converge a whole project at once, run [`fr clean --normalize`](cli.md#fr-clean---normalize).

One exception, and it only arises in a damaged file: a note block runs until the first line indented less than its body, so a note written last is closed by the next task line. If a task carries stranded lines — content the parser could not attribute, kept verbatim — indented as deep as a note body, a note moved last would absorb them. Such a task keeps the order it already had, which is the order that reads back correctly.

### Metadata Types

**`added: YYYY-MM-DD`** — Creation date.

**`resolved: YYYY-MM-DD`** — Completion date.

**`dep: ID1, ID2`** — Comma-separated dependency task IDs.

**`ref: path1, path2`** — Comma-separated file paths (relative to project root).

**`spec: path#section, path2`** — Comma-separated spec file paths.

`ref:` and `spec:` differ in meaning, not in form: a spec is the document a task implements, a ref a file it touches. Both hold **file paths relative to the project root**, and both are read by the same rule.

**A path may carry a location suffix**, which says where in the file to look:

| Suffix | Example |
|---|---|
| `#anchor` | `doc/design.md#rationale` |
| `:line` | `src/parser.rs:807` |
| `:line-range` | `src/parser.rs:807-820` |
| `:line:col` | `src/parser.rs:807:12` |

**Only the file is validated.** `fr check` reports a path with no file behind it as an error; it does not open the file, so an anchor naming a heading that moved and a line number gone stale are not errors — they are stale in a way frame cannot distinguish from correct. A filename that genuinely contains `#` or `:` still resolves, because the literal path is tried before any suffix is stripped.

**The separator is the comma and only the comma**, so a path may contain spaces (`doc/design notes.md`). None may contain a comma, since nothing can quote one.

**`conflict: reason timestamp`** — An unresolved merge conflict, written by `fr merge`. The value is a reason slug (`both-edited`, `edited-and-deleted`, `deleted-and-edited`, `ambiguous-title`) and the RFC 3339 timestamp of the merge run that set the other side's version aside:

```markdown
- [ ] `EFF-014` Task title
  - conflict: both-edited 2026-08-03T04:08:38Z
```

The timestamp identifies the run, not one entry: a run takes it once and stamps every task it marks and every recovery entry it logs with the same instant, so several markers and several entries routinely share it. It is there to match a marker to its merge by eye — to find *this task's* version in the log, search for the task rather than the stamp (`fr recovery --for EFF-014`), which is what `fr check` does.

It exists because `fr merge` deliberately writes no `<<<<<<<` markers — those would make the file unparseable — so this line is the only record in the file that a decision is outstanding. `fr check` reports it as an error; `fr merge --resolve EFF-014` removes it. Nothing else writes or reads it.

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

Multi-line notes: continuation lines are indented under the `note:` key. Blank lines within the note are preserved.

**A note's extent is set by indentation alone.** Every line indented to the note's block indent (or deeper) is note content — including lines that look like tasks or metadata, so a `- [ ]` or `- dep:` inside a note stays note text. The first line indented *less* than that ends the note, with no exceptions: code fences (`` ``` ``) are not tracked, and an unbalanced fence therefore cannot extend a note past its own indentation. The serializer re-indents every note line to the block indent, which is what makes the rule symmetric and the round-trip safe.

Two consequences worth knowing:

- A fenced block inside a note cannot contain flush-left lines — the note ends at the first one. Indent the whole block with the rest of the note.
- An unclosed fence is preserved verbatim and parses fine, but breaks downstream markdown rendering. [`fr check`](cli.md#fr-check) warns about it.

**Content frame cannot place is kept, not read.** An indented line that is neither metadata, nor a task, nor inside a `- note:` block — most often prose that lost its indentation, or a metadata key that lost its colon — is preserved byte for byte and written back where it was found. Frame does not interpret it: a `dep:` stranded this way creates no dependency, and stranded subtask text is not a task. [`fr check`](cli.md#fr-check) reports it so the indentation can be fixed — as a *stranded line* when it sits between two tasks, and as a *stranded line under* when it sits inside one, past that task's metadata. Which one decides where the line is carried, and so whether it travels when the task it belongs to is moved.

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

## Line Endings

Frame writes back the line ending the file already used. A CRLF file stays CRLF; an LF file stays LF. A file that mixes both settles on whichever it uses more, since one ending is carried per file rather than per line — mixed files are malformed anyway, and putting a `\r` inside the model would put it inside titles and tags where nothing wants it.

Every file frame creates uses LF.

The ending is a property of the writer, not of the model's line content — the same decision as the terminal newline, and for the same reason. Both used to have nowhere to live: the parsers read with `str::lines()`, which strips `\r` along with `\n`, so a CRLF file came back LF with every line in it rewritten. That matters mostly because of what else writes these files. With `core.autocrlf` or a `text=auto` attribute, git re-applies CRLF on checkout and frame would strip it on the next write, so the two churn against each other forever with neither able to win.

Every file frame writes ends with exactly one terminal newline, whatever the ending. This is added if the file lacked one.

## Archive Files

Two different files live under `frame/archive/`, and they are **not** the same shape. Reading one as the other loses data, which has happened in both directions.

### `archive/<track>.md` — a done-task archive

Written by `fr clean` when a track's `## Done` section passes its threshold. A heading, then a flat task list — **no `## Section` headers at all**:

```markdown
# Archive — main

- [x] `MAI-002` Second task
  - added: 2026-08-01
  - resolved: 2026-08-05
- [x] `MAI-001` First task
  - resolved: 2026-08-04
```

Task syntax, metadata and nesting are exactly as in a track file. The file has three parts:

- **Header** — everything above the first task line. The heading, the blank under it, and anything a person added. Carried verbatim through any rewrite.
- **Tasks** — the flat list. New tasks are appended after the last one.
- **Tail** — anything below the last task the parser will claim: a note, a rule, an HTML comment. Also carried verbatim.

Because there are no sections, walking `## Section` headers finds nothing in one. Parse it with `parse_archive`, never `parse_track`. That applies to *merging* too: `fr merge` reads this shape as `FileKind::Archive` and merges the flat list by task identity. Reading it as a track found zero tasks and silently discarded one side of every archive merge — see `doc/architecture.md` § Merging Under Version Control.

The first task line is found with the parser's own definition of a task line, at any indent. A bullet that merely *looks* like one — an ordinary markdown link, `- [notes](x.md)` — is header text, not the start of the list.

### `archive/_tracks/<track>.md` — an archived whole track

Written by `fr track archive`, which moves the track file there intact. It **is** a track file, sections and all, and parses as one. `fr track activate` moves it back.

Archived task IDs keep the prefix of the track they belong to; `fr track rename --prefix` renames them alongside the live ones, and `fr check` reports any left on a prefix the track no longer uses.

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

**Anything else is kept, not dropped.** A line between two items that is neither body nor a new item — a stray note, a heading somebody added, the residue of a hand edit — is carried on the item **above** it and re-emitted in place, along with the blank lines that separated it. Frame does not understand such a line, but it does not lose it either, and it does not need the recovery log to hold it.

The blank line matters and is part of what gets carried. It is what makes a line stranded rather than body text:

```markdown
- one
stray          <- body of "one" (no blank before it)

- two

stray          <- stranded: belongs to no item, carried on "two"
```

**Spacing may be mixed after an edit**, and that is expected. A clean item is written back verbatim while an edited one is written canonically, so editing one item of a compactly-written inbox leaves a blank line below that item and not below its neighbours. A compact inbox converts to the canonical spacing one item at a time, as each is edited. Every intermediate state is stable: reading and writing it back reproduces it exactly.

## Actor Registry (`frame/actors.toml`)

The one committed non-markdown file frame generates. It maps every known [actor token](concepts.md#actors) to its state and provenance:

```toml
[actors]
null = { name = "origin", state = "active" }
a = { name = "laptop", state = "active", claimed = "2026-06-03" }
c = { name = "desktop", state = "retired", claimed = "2026-06-02", retired = "2026-06-09" }
```

Fields: `name` (provenance, defaulting to the claiming machine's hostname) and `state` (`active` or `retired`) are always present; `claimed` and `retired` are `YYYY-MM-DD` and appear when they apply (`null` has no `claimed` date). Frame reads any valid TOML of this shape, including the one-section-per-actor form (`[actors.a]` with the fields below it) that older versions wrote.

**The file is generated.** It is rewritten whole on every claim or retirement, so comments and hand formatting do not survive. Two properties of how it is written are deliberate, and both are about `git merge`:

**One line per actor.** `actors.toml` is committed and is [not routed to frame's merge driver](cli.md#fr-merge), so git merges it as plain text — and two clones claiming tokens concurrently each add a row. Written as one section per actor, two rows claimed on the same machine on the same day differ only in their `[actors.<token>]` header; git aligns the identical `name`/`state`/`claimed` lines below and conflicts the header alone. Resolving that the obvious way — keep both sides — yields `[actors.e]` immediately followed by `[actors.d]`, which is an empty table for `e`: the registry stops parsing and an actor has silently lost its provenance. One line per actor makes the whole row the conflict region, so keeping both sides is correct and loses nothing.

**Sorted by token, `null` first.** Appending each new actor put every claim at the end of the file, so two concurrent claims always landed at the same anchor and always conflicted. Sorted, they merge cleanly whenever an existing token sorts between them, which is increasingly likely as a project accumulates actors — that is, for exactly the people who merge this file. It does not help when nothing separates the two new tokens (the first two clones of a fresh project, whose registry holds only `null`); that case still conflicts, and is the one the row shape above makes safe. `null` is not a token and would otherwise sort into the middle of the letters, so it is pinned first and stays put.

Do not restore arrival order. It is not a cosmetic choice: it is the difference between conflicting on every merge and conflicting on some of them.

## Selective Rewrite

Frame uses a selective rewrite strategy for round-trip preservation:

- Each parsed task stores its original source lines (`source_text`) and a `dirty` flag.
- `source_text` contains only the task's own lines (task line + metadata), **not** subtask lines.
- On serialization:
  - **Clean tasks** (not modified): emit `source_text` verbatim, preserving exact formatting.
  - **Dirty tasks** (modified): regenerate in canonical format.
  - Subtasks are always serialized independently regardless of parent's dirty state.

This means editing one task never reformats its parent, siblings, or unrelated tasks. If no mutations occur, parse-then-serialize produces byte-identical output.
