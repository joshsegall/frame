# Agent Setup

How to configure a frame project so AI coding agents can pick up tasks, report progress, and file findings.

## Overview

Frame's agent integration works through the `fr` CLI. Agents don't read or write markdown files directly — they use CLI commands like `fr ready`, `fr state`, and `fr add`. The human controls what work is available through task ordering, `#cc` tags, and the `cc_only` config setting.

The setup has three parts:

1. **Project config** — tell frame how agents should behave (`project.toml`)
2. **Skill file** — give the agent a reference for `fr` commands
3. **Agent config** — tell the agent that frame exists and how to use it

## Project Configuration

### The `[agent]` section

In your project's `frame/project.toml`, add or edit the `[agent]` section:

```toml
[agent]
cc_focus = "api"        # track for `fr ready --cc`
cc_only = true          # default: true
```

**`cc_focus`** — which track `fr ready --cc` looks at. Set this to the track where you'll put agent-ready work. You can change it anytime with `fr track cc-focus <id>`.

**`cc_only`** — controls how far the agent can look for work:

| Value | Behavior |
|-------|----------|
| `true` (default) | Agent only works on `#cc`-tagged tasks. When none are available, it stops and asks for direction. |
| `false` | Agent may fall back to any unblocked task across active tracks when no `#cc` work is available. |

Start with `true` (the default). This gives you explicit control over what the agent touches. Switch to `false` once you're comfortable with the agent working more autonomously.

### Tagging tasks

Add `#cc` to any task you want the agent to pick up:

```
- [ ] `API-003` Add rate limiting to auth endpoints #cc
```

You can do this in the TUI (`c` toggles the `#cc` tag on the selected task), or from the CLI:

```bash
fr tag API-003 add cc
```

The agent checks `fr ready --cc` to find these tasks. Tasks appear in `fr ready` output only when they are `todo`, not `blocked`, and have no unresolved dependencies — so you can tag a task `#cc` early and it won't surface until it's actually ready.

## Installing the Skill File

Frame ships a skill file at `skills/managing-frame-tasks/SKILL.md` that gives agents a complete reference: concepts, workflows, command tables, and conventions. The agent needs access to this file to know how to use `fr`.

How you install it depends on your agent tooling.

### Claude Code

Copy or symlink to the global skills directory:

```bash
# Symlink (stays in sync with frame repo updates)
mkdir -p ~/.claude/skills/managing-frame-tasks
ln -sf /path/to/frame/skills/managing-frame-tasks/SKILL.md \
       ~/.claude/skills/managing-frame-tasks/SKILL.md

# Or copy
cp /path/to/frame/skills/managing-frame-tasks/SKILL.md \
   ~/.claude/skills/managing-frame-tasks/SKILL.md
```

The skill auto-activates in any project with a `frame/` directory.

### Other agents

If your agent reads a rules file, project instructions, or system prompt, include the contents of `SKILL.md` (or a link to it) in that configuration. The file is self-contained — it covers concepts, workflows, command reference, and conventions.

## Agent Config

Add a short section to your agent's project-level instructions (e.g., `CLAUDE.md`, `.cursorrules`, or equivalent). This doesn't need to repeat the full command reference — the skill file handles that. It just needs to establish the habits:

```markdown
## Task Tracking

This project uses frame (`fr`) for task management. Tasks live in `frame/`.

- Check `fr ready --cc` for available work at session start
- `fr show ID --context` before starting a task
- Mark `active` when starting, `done` when finishing
- File findings: `fr inbox "desc" --tag bug` or `fr add <track> "title" --found-from ID`
- Tag agent-created tasks with `#cc-added`
- Run `fr clean` after completing work
```

## Workflow

A typical agent session:

```
1. Check for work       →  fr ready --cc
2. Read task details    →  fr show ID --context
3. Claim task           →  fr state ID active
4. (optional) Break     →  fr sub ID "subtask title"
   into subtasks
5. Do the work          →  (write code, run tests, etc.)
6. Report progress      →  fr state ID.1 done
                           fr ref ID src/changed_file.rs
                           fr note ID "implementation details"
7. File discoveries     →  fr inbox "bug found" --tag bug
                           fr add track "new task" --found-from ID
8. Complete             →  fr state ID done
9. Maintenance          →  fr clean
10. Next task           →  fr ready --cc
```

### When the agent has no work

The agent checks `fr ready --cc --json` and reads the `cc_only` field:

- **`cc_only: true`** — No broadening. The agent checks `fr blocked` to see if it can unblock a `#cc` task. Otherwise it stops and asks for direction.
- **`cc_only: false`** — The agent runs `fr ready` to find any unblocked task across active tracks.

## Conventions

**`#cc-added` tag** — Agents should tag every task they create with `#cc-added` so you can tell which tasks were filed by an agent vs. a human. Include it inline in the title:

```bash
fr add api "Handle empty response body #cc-added" --found-from API-003
```

**`--found-from`** — When the agent discovers a bug or new task while working on something else, use `--found-from ID` to record the context. If the agent isn't sure which track it belongs on, use `fr inbox` instead.

**`--context`** — Agents should always use `fr show ID --context` (not plain `fr show ID`). Parent tasks often contain specs, notes, and dependencies that explain why a subtask exists.

**`--json`** — Agents should use `--json` on read commands when parsing output programmatically. Human-formatted output may change between versions.
