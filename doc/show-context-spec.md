# `fr show --context` — Ancestor Context for Subtasks

## Problem

When an agent (or human) runs `fr show` on a subtask, they only see that task's fields. Parent tasks often contain notes, specs, deps, and titles that give essential context for understanding the subtask. Today agents must manually detect dotted IDs and issue separate `fr show` calls for each ancestor.

## Changes

### CLI: `fr show ID [--context]`

Add a `--context` flag to `fr show`. When present, the output includes the full ancestor chain above the target task, displayed root-first before the task itself.

**Human output** (with `--context`):

```
── Parent ── FOO-001 Implement Vector module
  spec: doc/spec/vector.md
  note:
    Need add, dot, cross, normalize. Use SIMD where possible.

── Parent ── FOO-001.1 Create primitives
  dep: FOO-003

── Task ── FOO-001.1.2 Add Vec3 struct
  state: todo
  added: 2025-06-01
```

Each ancestor shows all its fields (state, tags, deps, spec, refs, note, added, resolved) using the same formatting as the target task. The `── Parent ──` / `── Task ──` separator lines distinguish ancestors from the target.

Without `--context`, output is unchanged from today.

When `--context` is used on a top-level task (no parents), it behaves identically to `fr show` without the flag.

### JSON output

`fr show ID --json` **always** includes an `ancestors` array, regardless of whether `--context` is passed. The array is ordered root-first (outermost parent at index 0). Each entry contains the full task fields. The array is empty for top-level tasks.

```json
{
  "id": "FOO-001.1.2",
  "title": "Add Vec3 struct",
  "state": "todo",
  "tags": [],
  "added": "2025-06-01",
  "ancestors": [
    {
      "id": "FOO-001",
      "title": "Implement Vector module",
      "state": "active",
      "tags": ["cc"],
      "spec": "doc/spec/vector.md",
      "note": "Need add, dot, cross, normalize. Use SIMD where possible."
    },
    {
      "id": "FOO-001.1",
      "title": "Create primitives",
      "state": "todo",
      "tags": [],
      "dep": ["FOO-003"]
    }
  ]
}
```

### SKILL.md update

In the "Pick up work" workflow, change:

```bash
fr show EFF-014
```

to:

```bash
fr show EFF-014 --context
```

Add a note in the conventions section:

> **Subtask context**: Always use `--context` when showing a task. Parent tasks often contain specs, notes, and dependencies that explain why the subtask exists. In `--json` mode, ancestor context is included automatically.
