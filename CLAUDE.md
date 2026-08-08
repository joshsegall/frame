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

### Manual smoke-testing (IMPORTANT)

When running the real `fr` binary by hand to verify a change, **always go through `scripts/fr-dev`**, never `target/debug/fr` or `cargo run` directly:

```bash
scripts/fr-dev init          # runs fr against an isolated config sandbox
scripts/fr-dev info
scripts/fr-dev projects list # inspect the sandbox registry
```

`fr` auto-registers any project it touches into the global registry at `~/.config/frame/projects.toml`. Running the binary directly against a throwaway project therefore pollutes the user's real project list. `scripts/fr-dev` redirects `XDG_CONFIG_HOME`/`HOME` to a sandbox under `target/`, so registration still happens (and stays testable) without touching the real registry. The automated test suites already isolate themselves the same way.

If a stray entry does leak in, `fr projects prune` removes not-found entries, and the `scripts/check-registry.sh` guard (wired as a `.githooks/pre-commit` hook via `git config core.hooksPath .githooks`) warns about them at commit time.

### Toolchain freshness

CI always runs the latest stable Rust, so a stale local toolchain can miss `clippy` lints that fail CI. A `.githooks/pre-push` hook runs `rustup check` and warns (non-blocking) when a newer stable is available — run `rustup update stable` when it does. Both hooks require `git config core.hooksPath .githooks` (one-time, per clone).

## Architecture

Frame is a markdown-based task tracker (TUI + CLI) where `.md` files are the source of truth. The binary is `fr`.

### Module Layout

- **`src/model/`** — Data types: `Task`, `Track`, `Archive`, `Inbox`, `ProjectConfig`, `Project`
- **`src/parse/`** — Markdown parser and serializer pairs for tasks, tracks, archives, and inbox
- **`src/io/`** — Project discovery, file locking, config I/O, UI state persistence, file watcher, project registry, durable ID frontier (`ids.rs`), in-flight operation marker (`inflight.rs`), debug-only write fault injection (`fault.rs`)
- **`src/ops/`** — Business logic: task CRUD, ID minting (`ids.rs` — the one chokepoint), track management, inbox, search, clean, check, automatic repair (`fix.rs`), interrupted-operation recovery (`recover.rs`), import
- **`src/cli/`** — CLI interface (clap commands, handlers, JSON/human output)
- **`src/tui/`** — TUI interface: app state, undo, input handling, rendering
  - `input/` — Key handling, split into submodules: `common`, `navigate`, `select`, `search`, `edit`, `move_mode`, `triage`, `confirm`, `command`, `popups`, `tracks`, `recent`
  - `render/` — UI rendering, with shared utilities in `helpers.rs`

See `doc/architecture.md` for detailed design decisions and invariants.

## Project Structure on Disk

A frame project has a `frame/` directory containing:
- `project.toml` — project config
- `inbox.md` — inbox items
- `tracks/*.md` — track files (one per track)
- `archive/*.md` — done-task archives (per track, created by `fr clean`)
- `archive/_tracks/` — archived whole-track files
- `.lock` — advisory lock file
- `.state.json` — TUI state (cursor, scroll, expanded sets)
- `.actor` — this working copy's actor token (gitignored)
- `.inflight` — records a multi-file operation in progress; present only while one is running, or after one was interrupted. The next write command completes it (`ops::recover`) and removes it
- `.rescue/` — copies of work the TUI could not save, written at exit (best-effort)
- `.ids.toml` / `.ids.lock` — ID frontier, **only for projects outside git**; inside git it lives at `<git-common-dir>/frame-ids.toml` so every worktree of the clone shares one
- `.recovery.log` / `.recovery.lock` — recovery log, **only for projects outside git** (or when `[recovery] path` says so); inside git it lives at `<git-common-dir>/frame-recovery.log`, shared by every worktree. It holds content that reached no other file, and `git worktree remove` deletes gitignored files silently — a per-worktree log is the only copy of something in a directory git will delete on request

**Two files are clone-shared rather than worktree-local, and the test is the same for both**: does it hold something that exists nowhere else, or that every worktree must agree on? The ID frontier and the recovery log qualify; `.lock`, `.state.json` and `.inflight` do not — each is *about* one working tree (its inodes, its view, its interrupted operation), and sharing them would over-serialize or cross wires.

Working-copy-local files (`.lock`, `.state.json`, `.actor`, `.inflight`, `.ids.*`, `.recovery.*`, `.rescue/`) are listed in one constant, `io::project_io::LOCAL_ONLY_FRAME_FILES`, which drives `fr check`'s leak guard — add new ones there, not in the caller. The clone-shared names stay listed: a project outside git still keeps them in `frame/`, and one left there by an older frame must not be committed on its way out. `.gitignore` coverage is a single pattern (`frame/.*`, see `gitignore_pattern`), so a new local file needs no `.gitignore` change at all. **The rule that makes that safe: nothing under `frame/` that needs committing may start with a dot.**

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

## The test suites and what each is for

Each answers a different question; a new test usually belongs in one of them rather than in a new file.

| Suite | Question it answers |
|---|---|
| `round_trip.rs` | does a fixture survive parse → serialize? |
| `parse_properties.rs` | P1–P6 on each **parse/serialize pair** (track, archive, inbox): no panic, content preserved, conserved against ground truth, converges, every line accounted for, line ending kept |
| `conservation.rs` | P7 on **operation sequences**: does a random run of real ops lose a title, an ID, a line frame does not own, or a `dep:` that resolved — and does every file it writes land settled? |
| `damaged_corpus.rs` | does `fr check` report **exactly** the right findings for a known-damaged project, and repair exactly what it claims? |
| `undo_properties.rs` | P9 on **TUI action sequences**: does undoing everything restore the project byte for byte, and redoing everything restore the result? |
| `concurrency.rs` | P8 on **interleavings**: does a TUI session and a CLI writer over one project lose work either one acknowledged, move a track out of the state the CLI left it in, hand an ID out twice, or leave a file unsettled? |
| `merge_simulation.rs` | do independent actors minting IDs concurrently ever collide? |
| `parity.rs` | do human and `--json` output, and CLI and TUI, agree? |
| `cli_integration.rs` | the CLI surface, including crash injection via `FRAME_FAIL_WRITE` |

**When adding a detector to `fr check`, add a case to `damaged_corpus.rs`** — `every_finding_tag_has_a_case` fails the build until you do, on purpose.

Two suites share helpers under `tests/support/`, included with `#[path]` rather than copied. `tui_steps.rs` drives a real `App` through generated semantic key sequences (P9 and P8); `tree_checks.rs` reads a project off disk the way the code does — three file shapes, three parse/serialize pairs — and answers what is present and what is settled (P7 and P8). Neither is a test target of its own.
