### Search Implementation

**Data source:** Search operates on the in-memory `Project` model. No file re-reading.

**Fields searched per task:** ID, title, tags, note content, dep IDs, ref paths, spec paths. All fields are searched as plain text against the regex pattern.

**Subtasks:** Searched independently — a matching subtask appears in results even if its parent doesn't match. Each match carries its full ID path (e.g., `EFF-014.2`).

**Return type:**
```rust
struct SearchHit {
    track_id: String,
    task_id: String,
    field: MatchField,       // Title, Note, Tag, Dep, Ref, Spec, Id
    spans: Vec<Range<usize>>, // byte ranges within the matched field text
}
```

**Scoping:**
- CLI: `--track` limits to one track; default is all active tracks
- TUI: scope follows current view (track → that track, tracks view → all, inbox → inbox items, recent → done tasks)

**TUI behavior:**
- SEARCH mode highlights matches in real time as the user types
- After execute (`Enter`), `n`/`N` cycles through `SearchHit` results
- Collapsed subtrees containing matches auto-expand to reveal the match
- Cursor jumps to the matched task, not just the matched line

**Inbox search:** Searches title, body text, and tags of inbox items. Same regex, same highlighting.

** Matching text not visible on the screen:**

If a task matches the content not shown on the screen, we append a message to the end of the text on that line showing what matched. For example, if TUI-004 has a match in its note, it might look like:

```
◐ TUI-004 Render track content view  #phase4 [1 match: note]
```

The "[1 match: note]" shows there is one match in the content of the task, and it's in the note field. It should be highlighted just like the matching term would be.

Other details:
- If multiple fields match, show up to three of them with ellipses for more, e.g. [2 matches: note, ref] or [4 matches: note, tag, dep, ...]
- There may be multiple matches in just one field: [6 matches: note]
- If the appended text and the exiting text won't fit in the space available, truncate the line so there's enough space for the match indicator.


When you hit Enter on a search, the search bar dims, including the search term and the hotkey hint. The color needs to be
  brighter, more like the task IDs, or even keep it the same brightness as regular text until you hit Esc. We should also show the
  hint for hitting Esc to clear the text.

  Also: let's add search history to the search bar. You should be able to us the up and down arrows to replace the content of the
  search with the historical searches. It should allow up to 200 previous searches and persist between sessions. When you hit "up"
  it shows you the most recent search. Hit up again it shows the one before that, etc. And hitting down reverses this, continuing
  until you reach the "new" search. If the user had typed something into the new search it should be retained and show up when you
  reach the newest search, but the newest search should not be added to history unless the user hits Enter. When moving between
  search history entries, the cursor should always be places at the end of the search text.


- When search wraps around, the status bar should have a message saying "Search wrapped to top" or "Search wrapped to bottom" depending on the direction
    - The message should appear justified right, just to the left of the shortcut key text, separated by 8 spaces
    - The text color should be bright magenta
    - the message should disappear on the next search (n/N), or if you hit escape to end the search
- The status bar should show you the number of tasks matching: "# matches" where "#" is the number of matching tasks.
    - it should be shown right justified like the wrapping message, but with the normal text color (same as the search term)
    - It shows all the time while still in search mode, except if the wrapping message is shown
    - if there are 0 matches when the user hits Enter, highlight the number of matches with a red background color. Only when they hit enter. If the number of matches is 0 while they are typing in the search bar the text remains the same background as the rest of the text.


