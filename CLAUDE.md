# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build              # Build the project (binary: fr)
cargo test               # Run all tests (unit + integration)
cargo test <test_name>   # Run a single test by name
cargo test --lib         # Run only unit tests
cargo test --test round_trip  # Run only integration tests
cargo clippy --all-targets -- -D warnings  # Lint (matches CI)
cargo fmt --check        # Check formatting
```

## Architecture

Frame is a markdown-based task tracker (TUI + CLI) where `.md` files are the source of truth. The binary is `fr`.

### Module Layout

- **`src/model/`** — Data types: `Task`, `Track`, `Inbox`, `ProjectConfig`, `Project`
- **`src/parse/`** — Markdown parser and serializer pairs for tasks, tracks, and inbox
- **`src/io/`** — Project discovery, file locking, config I/O, UI state persistence, file watcher, project registry
- **`src/ops/`** — Business logic: task CRUD, track management, inbox, search, clean, check, import
- **`src/cli/`** — CLI interface (clap commands, handlers, JSON/human output)
- **`src/tui/`** — TUI interface: app state, undo, input handling, rendering

See `doc/architecture.md` for detailed design decisions and invariants.

## Project Structure on Disk

A Frame project has a `frame/` directory containing:
- `project.toml` — project config
- `inbox.md` — inbox items
- `tracks/*.md` — track files (one per track)
- `archive/*.md` — done-task archives (per track, created by `fr clean`)
- `archive/_tracks/` — archived whole-track files
- `.lock` — advisory lock file
- `.state.json` — TUI state (cursor, scroll, expanded sets)

## Documentation

- `doc/architecture.md` — Internal design decisions and invariants
- `doc/format.md` — Markdown format specification
- `doc/concepts.md` — Domain concepts (tracks, tasks, inbox, states)
- `doc/tui.md` — TUI modes, keybindings, and behavior
- `doc/cli.md` — CLI command reference

## Pre-completion Checks

After any plan or task that modifies Rust code, always run these checks before considering the work done:

```bash
cargo fmt --check        # Fix any issues with: cargo fmt
cargo clippy --all-targets -- -D warnings  # Lint all targets (including tests), deny warnings
cargo test               # Ensure all tests pass
```

Do not skip these steps. Fix all formatting and clippy issues before finishing.

## Test Fixtures

Integration tests live in `tests/round_trip.rs`. Fixture files in `tests/fixtures/` cover: simple/complex tracks, metadata variants, 3-level nesting, empty sections, code blocks in notes, inbox items, and project config.
