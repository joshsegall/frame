# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build              # Build the project (binary: fr)
cargo test               # Run all tests (unit + integration)
cargo test <test_name>   # Run a single test by name
cargo test --lib         # Run only unit tests
cargo test --test round_trip  # Run only integration tests
cargo clippy             # Lint
cargo fmt --check        # Check formatting
```

## Architecture

Frame is a markdown-based task tracker (TUI + CLI) where `.md` files are the source of truth. The binary is `fr`.

### Module Layout

- **`src/model/`** — Data types: `Task`, `Track`, `Inbox`, `ProjectConfig`, `Project`. Tasks have a `TaskState` enum (`Todo`/`Active`/`Blocked`/`Done`/`Parked` → checkbox chars ` `/`>`/`-`/`x`/`~`). Tracks contain `TrackNode` variants: `Literal` (passthrough text) and `Section` (Backlog/Parked/Done with tasks).
- **`src/parse/`** — Markdown parser and serializer pairs for tasks, tracks, and inbox. The core design is **selective rewrite with line-span preservation** (see below).
- **`src/io/`** — Project discovery (`discover_project` walks up looking for `frame/`), file locking (Unix `flock`), config I/O with round-trip-safe TOML editing via `toml_edit::DocumentMut`.
- **`src/ops/`** — Business logic (not yet implemented).
- **`src/cli/`** — CLI handlers (not yet implemented).
- **`src/tui/`** — TUI with input/render split (not yet implemented).

### Selective Rewrite Strategy

This is the most important architectural concept. Each parsed task stores:
- `source_lines: Range<usize>` — original line span in the file
- `source_text: Vec<String>` — the task's **own** lines only (task line + metadata), **excluding** subtask lines
- `dirty: bool` — whether the task was modified

On serialization: clean tasks emit `source_text` verbatim; dirty tasks regenerate in canonical format. Subtasks are **always** recursed independently. This means editing one subtask never reformats its parent or siblings.

### Parser Boundaries

- Task parser stops at blank lines — the track parser handles inter-section blank lines as trailing content or section headers.
- Inbox continuation lines that are tag-only (e.g. `  #design`) are parsed as tags, not body text.
- Code blocks in notes are tracked to avoid parsing fenced content as tasks.
- Maximum 3-level nesting (top → sub → sub-sub).

## Project Structure on Disk

A Frame project has a `frame/` directory containing:
- `project.toml` — project config
- `*.md` track files (one per track)
- `inbox.md` — inbox items
- `.lock` — advisory lock file

## Key Design References

- `frame-design-v3_3.md` — Full specification (markdown format, TUI, CLI, file structure)
- `frame-implementation-plan.md` — 8-phase plan; Phase 1 complete, Phases 2-8 not started

## Test Fixtures

Integration tests live in `tests/round_trip.rs`. Fixture files in `tests/fixtures/` cover: simple/complex tracks, metadata variants, 3-level nesting, empty sections, code blocks in notes, inbox items, and project config.
