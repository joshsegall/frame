# [>] frame

[![CI](https://github.com/joshsegall/frame/actions/workflows/ci.yml/badge.svg)](https://github.com/joshsegall/frame/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/frame.svg)](https://crates.io/crates/frame)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

**your backlog is plain text**

A markdown task tracker with a terminal UI for humans and a CLI for agents.

Tasks live in markdown files inside your repo. Position is priority.
Git is your history.

## How it works

Your project gets a `frame/` directory with markdown files — one per
work stream (called a **track**). Tasks are checkbox list items with
optional IDs, tags, dependencies, notes, and subtasks:

```markdown
- [>] `EFF-014` Implement effect inference for closures #cc
  - dep: EFF-003
  - spec: doc/spec/effects.md#closure-effects
  - note:
    The desugaring needs to handle three cases:
    1. Simple perform with no resumption
    2. Perform with single-shot resumption
    3. Perform with multi-shot resumption
  - [ ] `EFF-014.1` Add effect variables to closure types
  - [>] `EFF-014.2` Unify effect rows during inference #cc
  - [ ] `EFF-014.3` Test with nested closures
```

The TUI gives you a full visual interface. The CLI gives agents
programmatic access. Both read and write the same markdown files.

## Install

```bash
cargo install frame
```

Or download a pre-built binary from [GitHub Releases](https://github.com/joshsegall/frame/releases).

## Quick start

```bash
cd my-project
fr init --track effects "Effect System"
fr add effects "Parse effect syntax"
fr push effects "Fix parser crash"       # Push to top (highest priority)
fr                                       # Open TUI
```

## Task states

| Char  | State   | Meaning              |
|-------|---------|----------------------|
| `[ ]` | todo    | Not started          |
| `[>]` | active  | In progress          |
| `[-]` | blocked | Waiting on something |
| `[x]` | done    | Complete             |
| `[~]` | parked  | Deferred             |

`Space` cycles todo → active → done. `b` sets blocked. `~` sets parked.

## Project structure

```
my-project/
  frame/
    project.toml          # Config: tracks, tags, colors
    inbox.md              # Quick capture
    tracks/
      effects.md          # One file per work stream
      compiler-infra.md
    archive/
      effects.md          # Done tasks, auto-archived
```

See [`project.toml`](src/templates/project.toml) for the full configuration
reference — the template is self-documenting.

## TUI

`fr` with no arguments launches the TUI.

Vim-style modal interface. Four primary modes: **NAVIGATE**, **EDIT**,
**MOVE**, **SEARCH**, plus **SELECT** for bulk operations.
Press `?` for the full key binding reference.

The short version: `↑↓`/`jk` to move, `←→`/`hl` to collapse/expand,
`1`–`9` for track tabs, `Space` to cycle state, `e` to edit,
`a` to add, `m` to move, `/` to search, `Enter` for detail view.
`QQ` or `Ctrl-Q` to quit.

## CLI

Designed for coding agents. Every read command supports `--json`.
See [`skills/managing-frame-tasks/SKILL.md`](skills/managing-frame-tasks/SKILL.md)
for the full agent reference, and [`doc/agent-setup.md`](doc/agent-setup.md)
for how to configure frame for agent use in your project.

```bash
# What should I work on?
fr ready --cc --json

# Capture something quickly
fr inbox "Parser crashes on empty blocks" --tag bug

# Task lifecycle
fr add effects "Implement handler desugaring" --after EFF-003
fr state EFF-004 active
fr dep EFF-004 add EFF-003
fr sub EFF-004 "Add effect variables"
fr state EFF-004 done

# Move and organize
fr mv EFF-010 --top
fr mv EFF-010 --track infra

# Maintenance
fr clean             # Normalize, assign IDs, archive done tasks
fr check             # Validate deps and file refs
```

## Key concepts

**Tracks** are ordered work streams. Active tracks appear as tabs.
Tracks can be shelved or archived.

**Position is priority.** Top of the backlog = highest priority.
Use `m` (TUI) or `fr mv` (CLI) to reprioritize.

**Tags** are freeform labels. Conventional: `#cc`, `#bug`,
`#research`, `#design`, `#needs-input`.

**Dependencies** are cross-track references. `fr ready` filters to
tasks whose deps are all done.

**Inbox** is a quick-capture queue. Triage items into tracks when
you're ready.

**`fr clean`** normalizes formatting, assigns IDs, archives completed
tasks, and validates references. Runs automatically when external
changes are detected.

**Multiple projects** are supported via a global registry — use
`fr projects` to list, add, or remove projects.

**Personal tool** — designed for one developer (and their agents) 
managing their own work. It uses advisory file locking, not a 
database, so it's not built for concurrent team access.

## Terminal support

Tested on macOS with Ghostty. It should work on any terminal that
supports the Kitty keyboard protocol (Kitty, WezTerm, foot), 
but hasn't been tested.
