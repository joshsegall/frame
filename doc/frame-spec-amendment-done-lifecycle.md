# Spec Amendment: Done Task Lifecycle

Amends: `frame-design-v3_3.md`

---

## 1. Done Transition in Track View (TUI)

When a task is marked done in the track view, it does **not** immediately
disappear. The task remains in its current position in the Backlog,
rendered in done style (dim, `[x]`), for a grace period.

The task moves from the Backlog to the Done section of the track file
when **either** of these conditions is met:

- **5 seconds of inactivity** (no key presses)
- **Any view change** (tab switch, drill into detail view, open
  tracks/inbox/recent — anything that leaves the current view)

If a dialog or popup is open when the grace period expires, the move
may be deferred until the view containing the task is active again.
This is an implementation choice — the important contract is that the
task eventually moves and that user input is never interrupted.

During the grace period, `u` (undo) reverses the state change with the
task still visible in place — no hunting for it in the Done section.
`Space` also works during the grace period: it cycles the state back
to todo (standard cycle behavior), effectively undoing the completion.

**CLI behavior**: `fr state <id> done` moves the task to the Done
section immediately on write. The grace period is a TUI-only concern.

---

## 2. Parent Tasks with Incomplete Children

Marking a parent task done moves the **entire subtree** to Done as a
unit — the parent and all its children, regardless of child state.
Children that are already done also move with the parent (the subtree
is always kept together).

This is intentional. Marking a parent done is a deliberate "this scope
is finished" action. It's the fast path for closing out a feature where
some subtasks became irrelevant or were handled differently than planned.

**No warning is shown.** Incomplete children are not flagged by
`fr check` or `fr clean`. The children retain their original state
markers in the Done section and in the Recent view, so it's always
visible which subtasks were completed and which weren't.

**The grace period applies to the whole subtree.** The parent and its
children all remain visible in the Backlog during the grace period,
then move together.

---

## 3. Recent View Structure

The Recent view (`r` tab) displays done tasks with their **tree structure
preserved**, not as a flat list.

**Layout:**

```
 Effects │ Unique │ Infra │ ▸ │ 📥5 │ ✓ │
────────────────────────────────────────────────

 Today
   [x] EFF-014 Implement effect inference    effects
       [ ] .1 Add effect variables
       [x] .2 Unify effect rows
       [ ] .3 Test with nested closures
   [x] INFRA-021 Add span tracking           infra

 Yesterday
   [x] EFF-003 Implement handler desugaring  effects

 May 12
   [x] EFF-002 Parse effect declarations     effects
```

**Behavior:**

- Top-level done tasks appear as entries, grouped by resolved date
  (reverse-chronological)
- Subtasks are nested under their parent, **collapsed by default**
- Expand/collapse with `Enter` (or →/l and ←/h)
- Children show their actual state in bracket notation (`[ ]` for
  todo, `[>]` for active, `[-]` for blocked, `[~]` for parked,
  `[x]` for done), making it clear what was and wasn't completed
- Track origin shown on the right (dimmed)
- Each entry is a single top-level task — a parent with 3 children
  is one entry, not four

**Reopening:**

- `Space` on a done task reopens it: sets state back to todo, and
  after the standard grace period (5 seconds of inactivity or any
  view change), moves the entire subtree back to the originating
  track's Backlog (appended to top)
- During the grace period the task shows as `[ ]` in place. `Space`
  can re-close it (cycling back to done), mirroring the track view
  grace period symmetrically
- Children's states are preserved as-is on reopen — incomplete
  children remain in their original state, done children stay done
- Reopening a child independently is not supported from the Recent
  view — reopen the parent, then manage children in the track view
- `Enter` expands/collapses the subtree (does **not** reopen)

**Data source:** The Recent view reads from both the Done section of
each track file and the per-track archive files.

---

## 4. Design Spec Changes

### Section: Track File Structure (after "Backlog / Parked / Done" paragraph)

Add:

> **Done transition (TUI):** When a task is marked done in the track
> view, it remains visible in its Backlog position for a grace period
> (5 seconds of inactivity or any view change), rendered in done style.
> After the grace period, it moves to the Done section. This applies
> to the entire subtree when a parent is marked done — all children
> move with the parent regardless of their own state.

### Section: Recent view description

Replace:

> **Recent view** (`r`):
> Completed tasks, reverse-chronological, grouped by date. Can reopen.

With:

> **Recent view** (`r`):
> Completed tasks, reverse-chronological, grouped by resolved date.
> Tree structure is preserved — a parent with subtasks appears as a
> single expandable entry (`Enter` to expand/collapse) showing
> children's actual states in bracket notation. `Space` reopens a
> task (with the same grace period as marking done), moving the
> entire subtree back to the track's Backlog.

### Design Decisions Log — new rows

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Done grace period | 5s inactivity or any view change | Prevents jarring disappearance; easy undo window |
| Grace period deferral | May defer if dialog/popup open | Never interrupt user input; move when view is active |
| Parent done with incomplete children | Allowed, no warning, subtree moves as unit | Fast path for closing scope; states visible in Recent |
| Recent view structure | Tree with collapsed children, not flat list | Preserves context; shows what was/wasn't completed |
| Reopen grace period | Same as done (5s / view change), Space re-closes | Symmetric behavior; prevents accidental reopen |
| Reopen key | Space only (Enter expands/collapses) | Matches state-change convention; reduces accidents |
