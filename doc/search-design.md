# Search: Design & Implementation Plan

## Summary

Rationalize Frame's search into two clear modes in the TUI, and make CLI search inclusive by default. The two TUI modes serve different mental models: **view search** (`/`) filters and navigates within the current view; **project search** (`S`) finds matches across the entire project and presents unified results.

---

## 1. What Changes

### 1.1 CLI: `fr search` includes archives by default

**Current:** Archives excluded unless `--archive` is passed.
**New:** Archives included by default. The `--archive` flag becomes a no-op (accept silently, don't error) for backward compatibility, and is removed from documentation. No `--no-archive` flag — YAGNI.

**Rationale:** `fr search` is a grep-like "find it anywhere" tool. Excluding archives by default forces the user to guess whether something has been archived and re-run. The performance cost of scanning archive files is negligible for a personal tool.

### 1.2 TUI: New Project Search (`S`) with Search Results view

A new keybinding `S` (uppercase, available in Navigate mode from any view) opens a search prompt. On submit, results appear in a new **Search** view that shows matches across all tracks, inbox, and archives.

### 1.3 TUI: View search (`/`) unchanged

The existing `/` search continues to work exactly as it does today: filters and highlights within the current view, persists across tab switches, `n`/`N` navigation. No changes.

---

## 2. Project Search — Detailed Design

### 2.1 Entry

- **Keybinding:** `S` in Navigate mode, any view.
- **Prompt:** Same style as the existing `/` search prompt. Label: `Search all:` (vs the existing `Search:` for view search). Regex input, same editing keys.
- **Submit:** `Enter` executes the search and switches to the Search view.
- **Cancel:** `Esc` returns to the previous view without searching.
- **Not live-updating.** Unlike view search (`/`), project search does not update on every keystroke. It runs once on `Enter`. Scanning all tracks + inbox + archives on every character would be slow, and the "refine by searching again" pattern is natural here — this is a one-shot find, not an interactive filter.
- **History:** Project search gets its own search history, separate from view search. `Up`/`Down` in the prompt cycles it.

### 2.2 Search Results View

A new view, peer to Track/Tracks/Inbox/Recent. It does not get a number key (`1`–`9` are tracks, `0` is Tracks, `i` is Inbox, `r` is Recent). It's only reachable via `S` or by returning from a result jump.

**No tab in the tab bar.** The Search view replaces the tab bar with a search header showing the query and result count:

```
Search: effect inference  ─  14 matches  ─  tracks + inbox + archive
```

If the user switches to another view via `1`–`9`, `0`, `i`, `r`, or `Tab`, the Search view is dismissed (but the last search results are cached so `S` `Enter` re-runs quickly, and the search history remembers the query).

**Result grouping and ordering:**

Results are grouped by source in this order:
1. Active tracks (in tab order, each track as a group header)
2. Inbox
3. Archive (each archived track as a group header, labeled `archive:track_id`)

Within each group, results appear in backlog order (position order), preserving the "position is priority" principle.

**Result row format:**

```
  [>] EFF-014    Implement effect inference for closures  #cc
                 note: ...handle three cases: simple perform with no...
  [ ] EFF-014.2  Unify effect rows during inference       #cc
```

Each result row shows:
- State indicator (checkbox character) to the left of the ID
- Task ID (or inbox index for inbox items)
- Task title (with match highlighting if the match is in the title)
- Tags
- If the match is *not* in the title (or is in the title *and* another field), a context line below showing the field name and a snippet of the matching content with the match highlighted. The snippet is truncated to fit one line with `...` on either side as needed. This shows the user *what* matched and *where*, not just that something matched.
- Archive results are dimmed slightly and show `[archive]` prefix on the group header

**Group headers:**

```
── Effects (EFF) ──────────────────────────  3 matches
  [>] EFF-014    Implement effect inference for closures  #cc
                 note: ...handle three cases: simple perform with no...
  [ ] EFF-014.2  Unify effect rows ...
── Inbox ──────────────────────────────────  1 match
  1  Parser crashes on empty effect block  #bug
── archive:effects ────────────────────────  2 matches
  [x] EFF-003    Implement handler desugaring
```

### 2.3 Navigation in Search Results View

| Key | Action |
|-----|--------|
| `j`, `Down` | Next result |
| `k`, `Up` | Previous result |
| `g`, `Home` | First result |
| `G`, `End` | Last result |
| `Alt+Up` | Jump to previous section (track/inbox/archive group) |
| `Alt+Down` | Jump to next section |
| `Enter` | Jump to task in its native view (Track view for active tasks, Inbox view for inbox items, Detail view for archive hits) |
| `Esc` | Return to search results (from jump target), or close search results and return to previous view (from results) |
| `S`, `/` | Start a new project search (replaces current results) |
| `n` | Next result (alias for `j`, maintains muscle memory from view search) |
| `N` | Previous result (alias for `k`) |

**`Esc` has two behaviors depending on context:**
- When viewing a task you jumped to from search results: `Esc` returns to the Search Results view at the previous cursor/scroll position.
- When in the Search Results view itself: `Esc` closes the results and returns to whatever view was active before `S` was pressed.

**Jump behavior (`Enter`):**

- For active track tasks: switches to the track's Tab view with the cursor on the matching task, expanding parents if needed. The view search is *not* activated (the user jumped to a specific task, not filtering a view).
- For inbox items: switches to Inbox view with the cursor on the item.
- For archive tasks: opens Detail view for the task (read-only, since archived tasks aren't editable in the TUI). This is new — currently there's no way to view archived task details in the TUI. If this is too much scope for the initial implementation, archive results can just show the info in the results list and `Enter` can be a no-op on archive items.
- After jumping, `Backspace` returns to the Search Results view at the same scroll position. This mirrors the Detail view's `Esc`/`Backspace` return-to-origin pattern.

### 2.4 What Gets Searched

Same fields as `fr search` on the CLI and the current TUI view search:
- Task ID
- Title
- Tags
- Note content
- Dep IDs
- Ref paths
- Spec path

For inbox items:
- Title
- Tags
- Body text

The regex engine and matching logic already exist. Project search reuses them, just running against all tracks + inbox + archive rather than the current view's flat items.

### 2.5 Interaction with View Search

The two search modes are fully independent:
- Project search (`S`) does not set or clear the view search (`/`).
- A view search can be active when the user presses `S`; the view search remains active on the underlying view.
- Jumping from Search Results to a track does not activate view search on that track.

This avoids any confusing cross-talk between the two features.

### 2.6 Interaction with Filters

Project search ignores TUI filters. If a state filter or tag filter is active on a track, project search still shows all matches from that track regardless. The search is project-wide and unfiltered — the user explicitly asked to "find everything."

---

## 3. Implementation Plan

### Phase 1: CLI changes

1. **`src/cli/` (search handler):** Remove the `--archive` flag requirement — always load archive files when performing search. Keep the flag accepted but ignored (no breaking change for scripts using `--archive`).

2. **`src/ops/search.rs` (or wherever search aggregation lives):** Ensure the search function loads archive tracks unconditionally. The archive-loading code already exists (behind the `--archive` flag); this just changes the default.

### Phase 2: TUI — Search prompt and data model

3. **`src/tui/app.rs`:**
   - Add `View::Search` variant.
   - Add `SearchResults` struct to `App`:
     ```rust
     struct SearchResults {
         query: String,
         regex: Regex,
         items: Vec<SearchResultItem>,
         cursor: usize,
         scroll_offset: usize,
         return_view: View,  // view to return to on Esc
     }
     ```
   - `SearchResultItem`:
     ```rust
     struct SearchResultItem {
         kind: SearchResultKind,  // Track { track_idx }, Inbox { item_idx }, Archive { track_id }
         task_id: Option<String>,
         title: String,
         state: Option<TaskState>,
         tags: Vec<String>,
         match_field: MatchField,  // Title, Note, Tags, Deps, Refs, Spec, Body
         group_header: bool,       // true for the first item in each group
     }
     ```
   - Add `search_results: Option<SearchResults>` to `App`.
   - Add separate `project_search_history: Vec<String>` (parallel to existing search history).

4. **`src/tui/input/` — new `project_search.rs` submodule:**
   - Handle `S` keypress in Navigate mode (any view): store `return_view`, switch to a search prompt mode.
   - On `Enter`: run the search across all tracks + inbox + archives, populate `SearchResults`, switch to `View::Search`.
   - On `Esc`: restore `return_view`.

5. **`src/tui/input/navigate.rs` (or `common.rs`):**
   - Add `S` to the Navigate mode handler as a global keybinding (like `i`, `r`, `0`).

### Phase 3: TUI — Search execution

6. **Search execution function** (new, probably in `src/ops/search.rs` or a new `src/tui/search.rs`):
   - Takes the compiled regex + `Project` + archive data.
   - Iterates active tracks (in tab order), inbox, then archive tracks.
   - For each task/item, checks all searchable fields.
   - Returns `Vec<SearchResultItem>` grouped by source.
   - Reuses the existing field-matching logic from the current search implementation.

7. **Archive loading in TUI context:**
   - The TUI currently doesn't load archive files. The search execution will need to read and parse `frame/archive/*.md` on demand.
   - This can be done lazily — only when `S` triggers a search. No need to keep archives in memory at all times.
   - Parse with the existing track parser, same as CLI `--archive` does.

### Phase 4: TUI — Results view rendering

8. **`src/tui/render/search_view.rs` (new):**
   - Render the search header (query, match count, scope indicator).
   - Render grouped results with group headers as separator rows.
   - Highlight matching text in titles (reuse existing search highlight spans).
   - Show the `── match in: field ──` annotation for non-title matches.
   - Dimmed rendering for archive results.
   - Standard scrollable list behavior (same patterns as Track view / Recent view).

### Phase 5: TUI — Navigation from results

9. **Jump-to-task from Search view:**
   - Track tasks: identify the track index and task ID, switch to `View::Track(idx)`, set cursor to the task (expand parents if collapsed). Store `View::Search` as the return target.
   - Inbox items: switch to `View::Inbox`, set cursor to the item index.
   - Archive items: for v1, show a read-only detail popup or just don't support `Enter` on archive results. Add full archive detail view in a follow-up if there's demand.

10. **Return navigation:**
    - `Esc` from the jumped-to view returns to Search Results at the previous cursor/scroll position. This follows the standard `Esc`-returns-to-origin pattern used throughout the TUI.
    - `Esc` from the Search Results view itself returns to the view that was active before `S` was pressed.
    - This requires storing the search results persistently (not clearing them on view switch to a jump target). The results are cleared when: the user presses `Esc` in Search view, switches to a non-jump view (`0`, `i`, `r`, `1`-`9`, `Tab`), or runs a new search.

### Phase 6: Help and documentation

11. **Update `src/tui/render/help_overlay.rs`:** Add `S` to the Global keybindings section.

12. **Update `src/tui/input/command.rs`:** Add "Project search" to the command palette (triggers the same flow as `S`).

---

## 4. Doc Changes

### `doc/tui.md`

**Views section** — add Search view:

> ### Search View
>
> Project-wide search results grouped by source. Open with `S` from any view. Shows matches across all active tracks, inbox, and archived tasks.

**Keybindings — Global** — add:

| Key | Action |
|-----|--------|
| `S` | Project search (all tracks, inbox, archive) |

**New section — Search View keybindings:**

| Key | Action |
|-----|--------|
| `j`, `Down`, `n` | Next result |
| `k`, `Up`, `N` | Previous result |
| `g`, `Home` | First result |
| `G`, `End` | Last result |
| `Alt+Up` | Jump to previous section |
| `Alt+Down` | Jump to next section |
| `Enter` | Jump to task in native view |
| `Esc` | Return to search results (from jump), or close results (from results view) |
| `S` | New search |

**Search Mode section** — add clarifying note:

> `/` searches within the current view. `S` searches the entire project. The two are independent — a view search can be active while browsing project search results.

**Features > Filtering section** — add note that project search ignores view filters.

### `doc/cli.md`

**`fr search` section** — remove `--archive` flag from the documentation. Update description:

> Search tasks, inbox, and archived tasks by regex pattern.
>
> ```
> fr search PATTERN [--track TRACK]
> ```

Remove the `-a`, `--archive` row from the flags table. Add a note:

> Searches include archived tasks by default. Use `--track` to limit scope.

### `skills/managing-frame-tasks/SKILL.md`

**Command Reference > Reading** — update the `fr search` rows:

| Command | Description |
|---------|-------------|
| `fr search <pattern>` | Regex search across tasks, inbox, and archives |
| `fr search <pattern> --track <id>` | Search within one track |

Remove the `fr search <pattern> --archive` row.

### `CHANGELOG.md` — Unreleased section

Replace:
```
### Added
- `fr search --archive` flag to include archived tasks in search results
```

With:
```
### Added
- Project search (`S`) in TUI: search across all tracks, inbox, and archives with unified results view
- Search Results view with grouped results, match field annotations, and jump-to-task navigation

### Changed
- `fr search` now includes archived tasks by default (previously required `--archive` flag)
- `--archive` flag is still accepted but no longer necessary
```

### `README.md`

No changes needed — the README doesn't document search in detail.

### `doc/architecture.md`

Add a brief section after "Filtering & Ancestor Context":

> ## Project Search
>
> `S` triggers a project-wide search that scans all active tracks, inbox, and archive files. Archive `.md` files are parsed on demand (not kept in memory). Results are stored in `SearchResults` on the `App` and rendered as a dedicated `View::Search`.
>
> Project search is independent of view search (`/`) and view filters. The two search modes maintain separate histories and state.

---

## 5. Scope and Non-Goals

**In scope:**
- `S` keybinding and search prompt
- Search Results view with grouping, annotations, navigation
- Jump-to-task for active tracks and inbox
- CLI default change for archives
- Doc updates

**Deferred (build only if needed):**
- Archive task detail view (archive results are visible in the list but `Enter` may be a no-op initially)
- Search results filtering (e.g., filter results by track or state within the results view)
- Saved/pinned searches
- `--no-archive` CLI flag

**Non-goals:**
- Changing view search (`/`) behavior in any way
- Full-text indexing or search optimization (file scanning is fast enough for personal projects)
- Fuzzy matching (regex is the search language, consistent with `/`)
