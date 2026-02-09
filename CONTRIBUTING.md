# Contributing to frame

## Getting started

```bash
git clone https://github.com/joshsegall/frame.git
cd frame
cargo build
cargo test
```

The binary is `fr`. Run it with no arguments to launch the TUI.

## Before submitting a PR

```bash
cargo fmt --check        # fix with: cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must pass — CI enforces this.

## Project structure

- `doc/architecture.md` — internal design decisions and invariants
- `doc/format.md` — markdown file format specification
- `doc/concepts.md` — domain concepts (tracks, tasks, states)
- `doc/tui.md` — TUI modes, keybindings, behavior
- `doc/cli.md` — CLI command reference

The architecture doc explains the parser's selective rewrite invariant,
the TUI state model, and other design choices that are easy to
accidentally break. Worth reading before touching the parser or TUI.

## AI agents

If you're using a coding agent, point it at
[`skills/managing-frame-tasks/SKILL.md`](skills/managing-frame-tasks/SKILL.md)
for the CLI reference and
[`CLAUDE.md`](CLAUDE.md) for build/test commands.

## Filing issues

Bug reports and feature requests are welcome. For bugs, include the
output of `fr --version` and steps to reproduce. For features, describe
the problem you're solving — frame is opinionated about scope, so
understanding the "why" helps.
