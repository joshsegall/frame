use crate::tui::app::{App, View};

/// Which views an action is available in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewContext {
    TrackView,
    DetailView,
    InboxView,
    RecentView,
    TracksView,
    /// Available in all views
    Global,
}

/// Categories for default ordering when no filter text is entered
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionCategory {
    State,
    Create,
    Edit,
    Move,
    Filter,
    Select,
    Navigate,
    Search,
    Manage,
    System,
}

/// A single action that can appear in the command palette
#[derive(Debug, Clone)]
pub struct PaletteAction {
    pub id: &'static str,
    pub label: String,
    pub shortcut: Option<&'static str>,
    pub contexts: &'static [ViewContext],
    pub category: ActionCategory,
}

/// Fuzzy match result for a palette action
#[derive(Debug, Clone)]
pub struct ScoredAction {
    pub action: PaletteAction,
    pub score: i32,
    /// Matched character indices within the label
    pub label_matched: Vec<usize>,
    /// Matched character indices within the shortcut
    pub shortcut_matched: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Fuzzy matching
// ---------------------------------------------------------------------------

/// Fuzzy score a query against a target string.
/// Returns None if no match, or Some((score, matched_indices)).
pub fn fuzzy_score(query: &str, target: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, vec![]));
    }

    let query_lower: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let target_chars: Vec<char> = target.chars().collect();
    let target_lower: Vec<char> = target.chars().flat_map(|c| c.to_lowercase()).collect();

    let mut matched_indices = Vec::with_capacity(query_lower.len());
    let mut search_from = 0;

    for &qc in &query_lower {
        match target_lower[search_from..]
            .iter()
            .position(|&tc| tc == qc)
        {
            Some(pos) => {
                let idx = search_from + pos;
                matched_indices.push(idx);
                search_from = idx + 1;
            }
            None => return None,
        }
    }

    // Score calculation
    let mut score: i32 = 0;
    let half = target_chars.len() / 2;

    for (mi, &idx) in matched_indices.iter().enumerate() {
        // Word boundary bonus: start of string or after space/hyphen/paren
        let is_word_start = idx == 0
            || matches!(target_chars.get(idx.wrapping_sub(1)), Some(' ' | '-' | '(' | ':'));
        if is_word_start {
            score += 10;
        }

        // Consecutive bonus
        if mi > 0 && idx == matched_indices[mi - 1] + 1 {
            score += 5;
        }

        // First-half bonus
        if idx < half {
            score += 3;
        }

        // Gap penalty
        if mi > 0 {
            let gap = idx.saturating_sub(matched_indices[mi - 1] + 1);
            score -= gap as i32;
        }
    }

    Some((score, matched_indices))
}

/// Filter and score actions against a query. Matches against the combined
/// string "label shortcut" so typing "x" finds "Mark done" via its shortcut.
/// Returns scored results sorted by score descending, then label alphabetically.
pub fn filter_actions(query: &str, actions: &[PaletteAction]) -> Vec<ScoredAction> {
    let mut results: Vec<ScoredAction> = actions
        .iter()
        .filter_map(|a| {
            let shortcut = a.shortcut.unwrap_or("");
            let combined = if shortcut.is_empty() {
                a.label.clone()
            } else {
                format!("{} {}", a.label, shortcut)
            };
            let (score, indices) = fuzzy_score(query, &combined)?;

            // Split indices into label vs shortcut portions
            let label_char_count = a.label.chars().count();
            let separator = 1; // the space between label and shortcut
            let mut label_matched = Vec::new();
            let mut shortcut_matched = Vec::new();
            for idx in indices {
                if idx < label_char_count {
                    label_matched.push(idx);
                } else if idx >= label_char_count + separator {
                    shortcut_matched.push(idx - label_char_count - separator);
                }
            }

            Some(ScoredAction {
                action: a.clone(),
                score,
                label_matched,
                shortcut_matched,
            })
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.action.label.cmp(&b.action.label))
    });

    results
}

// ---------------------------------------------------------------------------
// Action registry
// ---------------------------------------------------------------------------

/// Get the current ViewContext from the app's view
pub fn current_context(view: &View) -> ViewContext {
    match view {
        View::Track(_) => ViewContext::TrackView,
        View::Detail { .. } => ViewContext::DetailView,
        View::Inbox => ViewContext::InboxView,
        View::Recent => ViewContext::RecentView,
        View::Tracks => ViewContext::TracksView,
    }
}

/// Build the full list of available actions for the current view context.
/// Dynamic actions (track switching) are generated from the app state.
pub fn available_actions(app: &App) -> Vec<PaletteAction> {
    let ctx = current_context(&app.view);
    let mut actions: Vec<PaletteAction> = Vec::new();

    for action in static_actions() {
        if action_matches_context(&action, ctx) {
            actions.push(action);
        }
    }

    // Dynamic: "Switch to track: {name}" for each active track
    for (i, track_id) in app.active_track_ids.iter().enumerate() {
        let name = app.track_name(track_id);
        let shortcut_str: &'static str = match i {
            0 => "1",
            1 => "2",
            2 => "3",
            3 => "4",
            4 => "5",
            5 => "6",
            6 => "7",
            7 => "8",
            8 => "9",
            _ => "",
        };
        actions.push(PaletteAction {
            id: "switch_track",
            label: format!("Switch to track: {}", name),
            shortcut: if shortcut_str.is_empty() {
                None
            } else {
                Some(shortcut_str)
            },
            contexts: &[ViewContext::Global],
            category: ActionCategory::Navigate,
        });
    }

    actions
}

fn action_matches_context(action: &PaletteAction, ctx: ViewContext) -> bool {
    action
        .contexts
        .iter()
        .any(|c| *c == ctx || *c == ViewContext::Global)
}

fn static_actions() -> Vec<PaletteAction> {
    vec![
        // -- Global actions --
        PaletteAction {
            id: "next_track",
            label: "Next track".into(),
            shortcut: Some("Tab"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "open_inbox",
            label: "Open Inbox".into(),
            shortcut: Some("i"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "open_recent",
            label: "Open Recent".into(),
            shortcut: Some("r"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "open_tracks",
            label: "Open Tracks".into(),
            shortcut: Some("0"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "search",
            label: "Search".into(),
            shortcut: Some("/"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Search,
        },
        PaletteAction {
            id: "jump_to_task",
            label: "Jump to task by ID".into(),
            shortcut: Some("J"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Search,
        },
        PaletteAction {
            id: "toggle_help",
            label: "Toggle help".into(),
            shortcut: Some("?"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "undo",
            label: "Undo".into(),
            shortcut: Some("z/u"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::System,
        },
        PaletteAction {
            id: "redo",
            label: "Redo".into(),
            shortcut: Some("Z"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::System,
        },
        PaletteAction {
            id: "quit",
            label: "Quit".into(),
            shortcut: Some("QQ"),
            contexts: &[ViewContext::Global],
            category: ActionCategory::System,
        },
        // -- Track View --
        PaletteAction {
            id: "cycle_state",
            label: "Cycle state".into(),
            shortcut: Some("Space"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "set_todo",
            label: "Set todo".into(),
            shortcut: Some("o"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "mark_done",
            label: "Mark done".into(),
            shortcut: Some("x"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "set_blocked",
            label: "Set blocked".into(),
            shortcut: Some("b"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "set_parked",
            label: "Set parked".into(),
            shortcut: Some("~"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "toggle_cc",
            label: "Toggle cc tag".into(),
            shortcut: Some("c"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "mark_done_wontdo",
            label: "Mark done (#wontdo)".into(),
            shortcut: None,
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "mark_done_duplicate",
            label: "Mark done (#duplicate)".into(),
            shortcut: None,
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "add_task_bottom",
            label: "Add task (bottom)".into(),
            shortcut: Some("a"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Create,
        },
        PaletteAction {
            id: "insert_after",
            label: "Insert after cursor".into(),
            shortcut: Some("-"),
            contexts: &[ViewContext::TrackView, ViewContext::InboxView],
            category: ActionCategory::Create,
        },
        PaletteAction {
            id: "push_to_top",
            label: "Push to top".into(),
            shortcut: Some("p"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Create,
        },
        PaletteAction {
            id: "add_subtask",
            label: "Add subtask".into(),
            shortcut: Some("A"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Create,
        },
        PaletteAction {
            id: "edit_title",
            label: "Edit title".into(),
            shortcut: Some("e"),
            contexts: &[ViewContext::TrackView, ViewContext::InboxView],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "edit_tags",
            label: "Edit tags".into(),
            shortcut: Some("t"),
            contexts: &[
                ViewContext::TrackView,
                ViewContext::DetailView,
                ViewContext::InboxView,
            ],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "move_task",
            label: "Move task".into(),
            shortcut: Some("m"),
            contexts: &[ViewContext::TrackView, ViewContext::InboxView],
            category: ActionCategory::Move,
        },
        PaletteAction {
            id: "move_to_track",
            label: "Move to track".into(),
            shortcut: Some("M"),
            contexts: &[ViewContext::TrackView, ViewContext::DetailView],
            category: ActionCategory::Move,
        },
        PaletteAction {
            id: "move_to_top",
            label: "Move to top".into(),
            shortcut: None,
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Move,
        },
        PaletteAction {
            id: "move_to_bottom",
            label: "Move to bottom".into(),
            shortcut: None,
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Move,
        },
        PaletteAction {
            id: "filter_active",
            label: "Filter: active only".into(),
            shortcut: Some("fa"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "filter_todo",
            label: "Filter: todo only".into(),
            shortcut: Some("fo"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "filter_blocked",
            label: "Filter: blocked only".into(),
            shortcut: Some("fb"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "filter_ready",
            label: "Filter: ready (deps met)".into(),
            shortcut: Some("fr"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "filter_tag",
            label: "Filter: by tag".into(),
            shortcut: Some("ft"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "clear_state_filter",
            label: "Clear state filter".into(),
            shortcut: Some("f Space"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "clear_all_filters",
            label: "Clear all filters".into(),
            shortcut: Some("ff"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Filter,
        },
        PaletteAction {
            id: "toggle_select",
            label: "Toggle select".into(),
            shortcut: Some("v"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Select,
        },
        PaletteAction {
            id: "range_select",
            label: "Range select".into(),
            shortcut: Some("V"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Select,
        },
        PaletteAction {
            id: "select_all",
            label: "Select all".into(),
            shortcut: Some("Ctrl+A"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Select,
        },
        PaletteAction {
            id: "select_none",
            label: "Select none".into(),
            shortcut: Some("N"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Select,
        },
        PaletteAction {
            id: "open_detail",
            label: "Open detail".into(),
            shortcut: Some("Enter"),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "collapse_all",
            label: "Collapse all".into(),
            shortcut: None,
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "expand_all",
            label: "Expand all".into(),
            shortcut: None,
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "set_cc_focus",
            label: "Set cc-focus".into(),
            shortcut: Some("C"),
            contexts: &[ViewContext::TrackView, ViewContext::TracksView],
            category: ActionCategory::Manage,
        },
        PaletteAction {
            id: "repeat_action",
            label: "Repeat last action".into(),
            shortcut: Some("."),
            contexts: &[ViewContext::TrackView],
            category: ActionCategory::System,
        },
        // -- Detail View --
        PaletteAction {
            id: "edit_region",
            label: "Edit region".into(),
            shortcut: Some("e/Enter"),
            contexts: &[ViewContext::DetailView],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "edit_refs",
            label: "Edit refs".into(),
            shortcut: Some("@"),
            contexts: &[ViewContext::DetailView],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "edit_deps",
            label: "Edit dependencies".into(),
            shortcut: Some("d"),
            contexts: &[ViewContext::DetailView],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "edit_note",
            label: "Edit note".into(),
            shortcut: Some("n"),
            contexts: &[ViewContext::DetailView],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "back_to_track",
            label: "Back to track".into(),
            shortcut: Some("Esc"),
            contexts: &[ViewContext::DetailView],
            category: ActionCategory::Navigate,
        },
        // -- Inbox View --
        PaletteAction {
            id: "add_inbox_item",
            label: "Add item (bottom)".into(),
            shortcut: Some("a"),
            contexts: &[ViewContext::InboxView],
            category: ActionCategory::Create,
        },
        PaletteAction {
            id: "delete_inbox_item",
            label: "Delete item".into(),
            shortcut: Some("x"),
            contexts: &[ViewContext::InboxView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "begin_triage",
            label: "Begin triage".into(),
            shortcut: Some("Enter"),
            contexts: &[ViewContext::InboxView],
            category: ActionCategory::Move,
        },
        // -- Recent View --
        PaletteAction {
            id: "reopen_todo",
            label: "Reopen as todo".into(),
            shortcut: Some("Space"),
            contexts: &[ViewContext::RecentView],
            category: ActionCategory::State,
        },
        PaletteAction {
            id: "toggle_expand",
            label: "Toggle expand".into(),
            shortcut: Some("Enter"),
            contexts: &[ViewContext::RecentView],
            category: ActionCategory::Navigate,
        },
        // -- Tracks View --
        PaletteAction {
            id: "open_track",
            label: "Open track".into(),
            shortcut: Some("Enter"),
            contexts: &[ViewContext::TracksView],
            category: ActionCategory::Navigate,
        },
        PaletteAction {
            id: "add_track",
            label: "Add new track".into(),
            shortcut: Some("a"),
            contexts: &[ViewContext::TracksView],
            category: ActionCategory::Create,
        },
        PaletteAction {
            id: "edit_track_name",
            label: "Edit track name".into(),
            shortcut: Some("e"),
            contexts: &[ViewContext::TracksView],
            category: ActionCategory::Edit,
        },
        PaletteAction {
            id: "shelve_activate",
            label: "Shelve / activate".into(),
            shortcut: Some("s"),
            contexts: &[ViewContext::TracksView],
            category: ActionCategory::Manage,
        },
        PaletteAction {
            id: "archive_delete",
            label: "Archive / delete".into(),
            shortcut: Some("D"),
            contexts: &[ViewContext::TracksView],
            category: ActionCategory::Manage,
        },
        PaletteAction {
            id: "reorder_track",
            label: "Reorder track".into(),
            shortcut: Some("m"),
            contexts: &[ViewContext::TracksView],
            category: ActionCategory::Move,
        },
    ]
}

// ---------------------------------------------------------------------------
// Command palette state
// ---------------------------------------------------------------------------

/// State for the command palette overlay
#[derive(Debug, Clone)]
pub struct CommandPaletteState {
    /// Filter text typed by the user
    pub input: String,
    /// Cursor position in the input
    pub cursor: usize,
    /// Currently selected index in the filtered results
    pub selected: usize,
    /// Filtered and scored results
    pub results: Vec<ScoredAction>,
    /// Total number of actions available (before filtering)
    pub total_count: usize,
}

impl CommandPaletteState {
    pub fn new(app: &App) -> Self {
        let all_actions = available_actions(app);
        let total_count = all_actions.len();
        let results = filter_actions("", &all_actions);
        CommandPaletteState {
            input: String::new(),
            cursor: 0,
            selected: 0,
            results,
            total_count,
        }
    }

    /// Update results based on current input
    pub fn update_filter(&mut self, app: &App) {
        let all_actions = available_actions(app);
        self.total_count = all_actions.len();
        self.results = filter_actions(&self.input, &all_actions);
        // Clamp selection
        if !self.results.is_empty() {
            self.selected = self.selected.min(self.results.len() - 1);
        } else {
            self.selected = 0;
        }
    }

    /// Get the currently selected action's ID, if any
    pub fn selected_action_id(&self) -> Option<&str> {
        self.results.get(self.selected).map(|r| r.action.id)
    }

    /// For "switch_track" actions, extract the track index from the label
    pub fn selected_track_index(&self) -> Option<usize> {
        let scored = self.results.get(self.selected)?;
        if scored.action.id != "switch_track" {
            return None;
        }
        // The label format is "Switch to track: {name}"
        // Find which track index by matching the shortcut (1-9)
        scored.action.shortcut.and_then(|s| s.parse::<usize>().ok().map(|n| n - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_exact_match() {
        let (score, indices) = fuzzy_score("done", "Mark done").unwrap();
        assert!(score > 0);
        assert_eq!(indices.len(), 4);
    }

    #[test]
    fn fuzzy_score_case_insensitive() {
        let result = fuzzy_score("DONE", "Mark done");
        assert!(result.is_some());
    }

    #[test]
    fn fuzzy_score_no_match() {
        let result = fuzzy_score("xyz", "Mark done");
        assert!(result.is_none());
    }

    #[test]
    fn fuzzy_score_empty_query() {
        let (score, indices) = fuzzy_score("", "anything").unwrap();
        assert_eq!(score, 0);
        assert!(indices.is_empty());
    }

    #[test]
    fn fuzzy_score_prefix_bonus() {
        // "Cy" should score higher on "Cycle state" than on "Fancy thing"
        let (score_prefix, _) = fuzzy_score("cy", "Cycle state").unwrap();
        let (score_mid, _) = fuzzy_score("cy", "Fancy cycling").unwrap();
        assert!(score_prefix > score_mid);
    }

    #[test]
    fn fuzzy_score_word_boundary() {
        // "mt" should match "Move task" with word boundary bonus on both chars
        let (score, indices) = fuzzy_score("mt", "Move task").unwrap();
        assert!(score > 0);
        // M at 0 (word start), t at 5 (word start after space)
        assert_eq!(indices, vec![0, 5]);
    }

    #[test]
    fn fuzzy_score_consecutive_bonus() {
        // "mark" should get consecutive bonuses
        let (score, _) = fuzzy_score("mark", "Mark done").unwrap();
        // 10 (word start M) + 5 (consecutive a) + 5 (consecutive r) + 5 (consecutive k) + first-half bonuses
        assert!(score > 20);
    }

    #[test]
    fn filter_actions_sorts_by_score() {
        let actions = vec![
            PaletteAction {
                id: "a",
                label: "Fancy cycling trip".into(),
                shortcut: None,
                contexts: &[ViewContext::Global],
                category: ActionCategory::State,
            },
            PaletteAction {
                id: "b",
                label: "Cycle state".into(),
                shortcut: None,
                contexts: &[ViewContext::Global],
                category: ActionCategory::State,
            },
        ];
        let results = filter_actions("cy", &actions);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].action.id, "b"); // "Cycle state" should rank first
    }

    #[test]
    fn filter_actions_empty_query_returns_all() {
        let actions = vec![
            PaletteAction {
                id: "a",
                label: "Alpha".into(),
                shortcut: None,
                contexts: &[ViewContext::Global],
                category: ActionCategory::State,
            },
            PaletteAction {
                id: "b",
                label: "Beta".into(),
                shortcut: None,
                contexts: &[ViewContext::Global],
                category: ActionCategory::State,
            },
        ];
        let results = filter_actions("", &actions);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn filter_matches_shortcut() {
        let actions = vec![
            PaletteAction {
                id: "done",
                label: "Mark done".into(),
                shortcut: Some("x"),
                contexts: &[ViewContext::Global],
                category: ActionCategory::State,
            },
            PaletteAction {
                id: "todo",
                label: "Set todo".into(),
                shortcut: Some("o"),
                contexts: &[ViewContext::Global],
                category: ActionCategory::State,
            },
        ];
        // Typing "x" should match "Mark done" via its shortcut
        let results = filter_actions("x", &actions);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.id, "done");
        assert!(results[0].label_matched.is_empty());
        assert_eq!(results[0].shortcut_matched, vec![0]);
    }

    #[test]
    fn filter_matches_multi_char_shortcut() {
        let actions = vec![
            PaletteAction {
                id: "fa",
                label: "Filter: active only".into(),
                shortcut: Some("fa"),
                contexts: &[ViewContext::Global],
                category: ActionCategory::Filter,
            },
            PaletteAction {
                id: "fb",
                label: "Filter: blocked only".into(),
                shortcut: Some("fb"),
                contexts: &[ViewContext::Global],
                category: ActionCategory::Filter,
            },
        ];
        // "fa" should match both via label, but "Filter: active only" also via shortcut
        let results = filter_actions("fa", &actions);
        assert!(results.len() >= 1);
        assert_eq!(results[0].action.id, "fa");
    }
}
