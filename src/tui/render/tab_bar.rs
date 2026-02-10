use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, StateFilter, View};
use crate::util::unicode;

/// Result of tab layout computation: labels and layout mode
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TabLayout {
    /// Display label per track (same length as input names)
    pub labels: Vec<String>,
    /// Whether the project name fits
    pub show_project_name: bool,
    /// Whether scrolling is needed (too many tabs even after shrinking)
    pub scroll_mode: bool,
}

/// Width of a single track tab given its label and whether it has cc-focus star
fn track_tab_width(label: &str, is_cc: bool) -> usize {
    // " label " = display_width(label) + 2, plus "|" separator = +1
    // cc-focus adds "★ " = +2
    let base = unicode::display_width(label) + 2 + 1;
    if is_cc { base + 2 } else { base }
}

/// Compute the total width of fixed elements (leading icon + special tabs)
fn fixed_width(inbox_count: usize) -> usize {
    let leading = 3; // " ▶ "
    let tracks_tab = 4; // " ▶ " + "|"
    let inbox_tab = if inbox_count > 0 {
        // " *N " + "|"
        3 + digit_count(inbox_count) + 1
    } else {
        // " * " + "|"
        4
    };
    let recent_tab = 4; // " ✓ " + "|"
    leading + tracks_tab + inbox_tab + recent_tab
}

fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

/// Check if a set of labels fits within the available width
fn fits(labels: &[String], cc_focus_idx: Option<usize>, fixed: usize, available: usize) -> bool {
    let track_total: usize = labels
        .iter()
        .enumerate()
        .map(|(i, l)| track_tab_width(l, Some(i) == cc_focus_idx))
        .sum();
    track_total + fixed <= available
}

/// Truncate a string to at most `n` display-width cells (no ellipsis)
fn truncate_display(s: &str, n: usize) -> String {
    if unicode::display_width(s) <= n {
        return s.to_string();
    }
    let mut width = 0;
    let mut result = String::new();
    for c in s.chars() {
        let cw = unicode::char_display_width(c);
        if width + cw > n {
            break;
        }
        width += cw;
        result.push(c);
    }
    result
}

/// Compute tab layout with progressive shrinking and optional scroll mode.
///
/// This is a pure function for testability.
pub(crate) fn compute_tab_layout(
    names: &[String],
    prefixes: &[Option<String>],
    cc_focus_idx: Option<usize>,
    available_width: usize,
    project_name_len: usize,
    inbox_count: usize,
) -> TabLayout {
    let fixed = fixed_width(inbox_count);
    let project_name_width = if project_name_len > 0 {
        project_name_len + 2 // " name "
    } else {
        0
    };

    if names.is_empty() {
        return TabLayout {
            labels: Vec::new(),
            show_project_name: fixed + project_name_width <= available_width,
            scroll_mode: false,
        };
    }

    let full_names: Vec<String> = names.to_vec();

    // Phase 0: Try with project name
    if fits(
        &full_names,
        cc_focus_idx,
        fixed + project_name_width,
        available_width,
    ) {
        return TabLayout {
            labels: full_names,
            show_project_name: true,
            scroll_mode: false,
        };
    }

    // Phase 0b: Try without project name
    if fits(&full_names, cc_focus_idx, fixed, available_width) {
        return TabLayout {
            labels: full_names,
            show_project_name: false,
            scroll_mode: false,
        };
    }

    // Incremental shrink: each iteration, find the single longest label and
    // shrink it by one char. Rightmost wins ties, so shrinking is balanced.
    // When a label reaches exactly its prefix length, swap to the prefix.
    // Each label has a per-label floor: prefix length if it has one, else 3.
    // Once all labels are at their floor, switch to scroll mode.
    let mut labels = full_names;
    let mut using_prefix = vec![false; labels.len()];
    let default_floor = 3usize;
    let label_floors: Vec<usize> = prefixes
        .iter()
        .map(|p| {
            p.as_ref().map_or(default_floor, |s| {
                unicode::display_width(s).min(default_floor)
            })
        })
        .collect();

    loop {
        // Find the longest label above its floor (rightmost among ties)
        let mut best_idx: Option<usize> = None;
        let mut best_len: usize = 0;
        for (i, label) in labels.iter().enumerate() {
            let len = unicode::display_width(label);
            if len > label_floors[i] && len >= best_len {
                best_len = len;
                best_idx = Some(i);
            }
        }

        let idx = match best_idx {
            Some(i) => i,
            None => break, // all labels at min_len — need scroll mode
        };

        // Truncate by one display-width cell
        let current_width = unicode::display_width(&labels[idx]);
        labels[idx] = truncate_display(&labels[idx], current_width - 1);

        // When the label reaches exactly the prefix length, swap to the
        // prefix (a purpose-built short identifier) instead of a truncated
        // name. This is a zero-width swap so it doesn't skip any sizes.
        let new_len = unicode::display_width(&labels[idx]);
        if !using_prefix[idx]
            && let Some(ref prefix) = prefixes[idx]
            && unicode::display_width(prefix) == new_len
        {
            labels[idx] = prefix.clone();
            using_prefix[idx] = true;
        }

        if fits(&labels, cc_focus_idx, fixed, available_width) {
            return TabLayout {
                labels,
                show_project_name: false,
                scroll_mode: false,
            };
        }
    }

    // Phase 4: Scrolling mode
    TabLayout {
        labels,
        show_project_name: false,
        scroll_mode: true,
    }
}

/// Render the tab bar: track tabs + special tabs, with separator line below
pub fn render_tab_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    // Split into tab row and separator row
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tabs
            Constraint::Length(1), // separator
        ])
        .split(area);

    let sep_cols = render_tabs(frame, app, chunks[0]);
    render_separator(frame, app, chunks[1], &sep_cols);
}

/// Render tabs and return the column positions of each separator character.
fn render_tabs(frame: &mut Frame, app: &mut App, area: Rect) -> Vec<usize> {
    let total_width = area.width as usize;

    // Gather track info
    let cc_focus = app.project.config.agent.cc_focus.as_deref();
    let names: Vec<String> = app
        .active_track_ids
        .iter()
        .map(|id| app.track_name(id).to_string())
        .collect();
    let prefixes: Vec<Option<String>> = app
        .active_track_ids
        .iter()
        .map(|id| app.project.config.ids.prefixes.get(id.as_str()).cloned())
        .collect();
    let cc_focus_idx = cc_focus.and_then(|cf| app.active_track_ids.iter().position(|id| id == cf));
    let inbox_count = app.inbox_count();
    let project_name = app.project.config.project.name.clone();
    let project_name_len = unicode::display_width(&project_name);

    let layout = compute_tab_layout(
        &names,
        &prefixes,
        cc_focus_idx,
        total_width,
        project_name_len,
        inbox_count,
    );

    let mut spans: Vec<Span> = Vec::new();
    let mut sep_cols: Vec<usize> = Vec::new();
    let sep = Span::styled(
        "\u{2502}",
        Style::default().fg(app.theme.dim).bg(app.theme.background),
    );
    let bg_style = Style::default().bg(app.theme.background);

    // Leading icon
    spans.push(Span::styled(" ", bg_style));
    spans.push(Span::styled(
        "\u{25B6}",
        Style::default()
            .fg(app.theme.purple)
            .bg(app.theme.background),
    ));
    spans.push(Span::styled(" ", bg_style));

    if layout.scroll_mode {
        // --- Scrolling mode ---
        let fixed = fixed_width(inbox_count);
        let budget = total_width.saturating_sub(fixed);

        // Determine active track index
        let active_idx = match &app.view {
            View::Track(i) => Some(*i),
            View::Detail { track_id, .. } => {
                app.active_track_ids.iter().position(|id| id == track_id)
            }
            _ => None,
        };

        // Clamp tab_scroll
        let n = layout.labels.len();
        if app.tab_scroll >= n {
            app.tab_scroll = n.saturating_sub(1);
        }

        // Ensure active tab is visible: adjust tab_scroll
        if let Some(aidx) = active_idx {
            if aidx < app.tab_scroll {
                app.tab_scroll = aidx;
            }
            // Scroll right until active tab fits fully
            loop {
                let (vis_end, _) =
                    visible_range(&layout.labels, cc_focus_idx, app.tab_scroll, budget);
                if aidx < vis_end {
                    break;
                }
                if app.tab_scroll >= n.saturating_sub(1) {
                    break;
                }
                app.tab_scroll += 1;
            }
        }

        // Only deduct left indicator from budget; the right indicator (▸)
        // is rendered flush-right within any remaining space, so it doesn't
        // need to be reserved upfront.
        let has_left_initial = app.tab_scroll > 0;
        let left_cost = usize::from(has_left_initial);
        let track_budget = budget.saturating_sub(left_cost);

        // Calculate how many tabs fit fully from tab_scroll
        let (full_end, full_used) =
            visible_range(&layout.labels, cc_focus_idx, app.tab_scroll, track_budget);
        let has_right = full_end < n;

        // Try to fill remaining space on the left with a partial tab
        // (back down tab_scroll by 1, right-truncate the label).
        let mut first_partial: Option<String> = None;
        let mut first_partial_show_cc = false;

        if app.tab_scroll > 0 {
            let prev = app.tab_scroll - 1;
            // Recalculate available space: backing down tab_scroll may
            // remove the ◂ indicator, freeing 1 extra char.
            let prev_has_left = prev > 0;
            let prev_left_cost = usize::from(prev_has_left);
            let prev_track_budget = budget.saturating_sub(prev_left_cost);
            let partial_space = prev_track_budget.saturating_sub(full_used);

            if partial_space >= 4 {
                let is_cc = Some(prev) == cc_focus_idx;
                // For partial cc-focus tabs, show ★ only if 6+ chars available
                let (overhead, show_cc) = if is_cc {
                    if partial_space >= 6 {
                        (5, true)
                    } else {
                        (3, false)
                    }
                } else {
                    (3, false)
                };
                let max_chars = partial_space.saturating_sub(overhead);
                if max_chars > 0 {
                    app.tab_scroll = prev;
                    first_partial = Some(truncate_display(&layout.labels[prev], max_chars));
                    first_partial_show_cc = show_cc;
                }
            }
        }

        // Recalculate left indicator after potential tab_scroll adjustment
        let has_left = app.tab_scroll > 0;

        // Left scroll indicator
        if has_left {
            spans.push(Span::styled(
                "\u{25C2}",
                Style::default().fg(app.theme.dim).bg(app.theme.background),
            ));
        }

        // Render first tab (partial, right-truncated) if we backed down tab_scroll
        let mut tabs_used = full_used;
        let full_start = if let Some(ref label) = first_partial {
            let show_cc = first_partial_show_cc;
            tabs_used += track_tab_width(label, show_cc);
            render_track_tab(
                &mut spans,
                app,
                app.tab_scroll,
                label,
                show_cc,
                &sep,
                &mut sep_cols,
            );
            app.tab_scroll + 1
        } else {
            app.tab_scroll
        };

        // Render fully visible track tabs
        for i in full_start..full_end {
            let label = &layout.labels[i];
            let is_cc = Some(i) == cc_focus_idx;
            render_track_tab(&mut spans, app, i, label, is_cc, &sep, &mut sep_cols);
        }

        // Compute remaining space on the right (for partial tab + ▸ + padding)
        let actual_left_cost = usize::from(has_left);
        let right_avail = budget
            .saturating_sub(actual_left_cost)
            .saturating_sub(tabs_used);

        // Right partial: if there's enough space for a partial tab (4+) plus ▸ (1)
        if has_right && full_end < n && right_avail >= 5 {
            let is_cc = Some(full_end) == cc_focus_idx;
            let partial_budget = right_avail - 1; // reserve 1 for ▸
            let (overhead, show_cc) = if is_cc {
                if partial_budget >= 6 {
                    (5, true)
                } else {
                    (3, false)
                }
            } else {
                (3, false)
            };
            let max_chars = partial_budget.saturating_sub(overhead);
            if max_chars > 0 {
                let trunc_label = truncate_display(&layout.labels[full_end], max_chars);
                let partial_w = track_tab_width(&trunc_label, show_cc);
                tabs_used += partial_w;
                render_track_tab(
                    &mut spans,
                    app,
                    full_end,
                    &trunc_label,
                    show_cc,
                    &sep,
                    &mut sep_cols,
                );
            }
        }

        // Right scroll indicator, flush-right with padding before it
        if has_right {
            let final_right = budget
                .saturating_sub(actual_left_cost)
                .saturating_sub(tabs_used);
            let pad = final_right.saturating_sub(1);
            if pad > 0 {
                spans.push(Span::styled(
                    " ".repeat(pad),
                    Style::default().bg(app.theme.background),
                ));
            }
            spans.push(Span::styled(
                "\u{25B8}",
                Style::default().fg(app.theme.dim).bg(app.theme.background),
            ));
        }
    } else {
        // --- Non-scrolling mode: render all track tabs ---
        app.tab_scroll = 0;
        for (i, label) in layout.labels.iter().enumerate() {
            let is_cc = Some(i) == cc_focus_idx;
            render_track_tab(&mut spans, app, i, label, is_cc, &sep, &mut sep_cols);
        }
    }

    // Tracks view tab (▶)
    let is_tracks = app.view == View::Tracks;
    spans.push(Span::styled(" \u{25B6} ", tab_style(app, is_tracks)));
    sep_cols.push(
        spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum(),
    );
    spans.push(sep.clone());

    // Inbox tab with count (*N)
    let is_inbox = app.view == View::Inbox;
    let tab_bg = if is_inbox {
        app.theme.selection_bg
    } else {
        app.theme.background
    };
    let style = tab_style(app, is_inbox);
    spans.push(Span::styled(" ", style));
    spans.push(Span::styled(
        "*",
        Style::default().fg(app.theme.purple).bg(tab_bg),
    ));
    if inbox_count > 0 {
        spans.push(Span::styled(format!("{} ", inbox_count), style));
    } else {
        spans.push(Span::styled(" ", style));
    }
    sep_cols.push(
        spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum(),
    );
    spans.push(sep.clone());

    // Recent tab (✓)
    let is_recent = app.view == View::Recent;
    spans.push(Span::styled(" \u{2713} ", tab_style(app, is_recent)));
    sep_cols.push(
        spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum(),
    );
    spans.push(sep.clone());

    // Right-justify project name in remaining space (only in non-scroll mode)
    if layout.show_project_name {
        let tabs_width: usize = spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        let available = total_width.saturating_sub(tabs_width);
        let name_style = Style::default().fg(app.theme.text).bg(app.theme.background);

        if available >= project_name_len + 2 {
            let pad = available - project_name_len - 2;
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), bg_style));
            }
            spans.push(Span::styled(format!(" {} ", project_name), name_style));
        }
    }

    let line = Line::from(spans);
    let tabs = Paragraph::new(line).style(Style::default().bg(app.theme.background));
    frame.render_widget(tabs, area);
    sep_cols
}

/// Calculate how many track tabs fit starting from `start` within `budget` chars.
/// Returns (end_exclusive, total_width_used).
fn visible_range(
    labels: &[String],
    cc_focus_idx: Option<usize>,
    start: usize,
    budget: usize,
) -> (usize, usize) {
    let mut used = 0;
    for (i, label) in labels.iter().enumerate().skip(start) {
        let is_cc = Some(i) == cc_focus_idx;
        let w = track_tab_width(label, is_cc);
        if used + w > budget {
            return (i, used);
        }
        used += w;
    }
    (labels.len(), used)
}

/// Render a single track tab, pushing spans and recording separator position
fn render_track_tab(
    spans: &mut Vec<Span<'static>>,
    app: &App,
    track_idx: usize,
    label: &str,
    is_cc: bool,
    sep: &Span<'static>,
    sep_cols: &mut Vec<usize>,
) {
    let track_id = &app.active_track_ids[track_idx];
    let is_current = app.view == View::Track(track_idx)
        || matches!(&app.view, View::Detail { track_id: tid, .. } if tid == track_id.as_str());
    let style = tab_style(app, is_current);

    if is_cc {
        spans.push(Span::styled(format!(" {} ", label), style));
        spans.push(Span::styled(
            "\u{2605}",
            Style::default().fg(app.theme.purple).bg(if is_current {
                app.theme.selection_bg
            } else {
                app.theme.background
            }),
        ));
        spans.push(Span::styled(
            " ",
            Style::default().bg(if is_current {
                app.theme.selection_bg
            } else {
                app.theme.background
            }),
        ));
    } else {
        spans.push(Span::styled(format!(" {} ", label), style));
    }
    sep_cols.push(
        spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum(),
    );
    spans.push(sep.clone());
}

fn render_separator(frame: &mut Frame, app: &App, area: Rect, sep_cols: &[usize]) {
    let width = area.width as usize;
    let bg = app.theme.background;
    let dim = app.theme.dim;

    // Build filter indicator text if filter is active and in track view
    let is_track_view = matches!(app.view, View::Track(_));
    let filter = &app.filter_state;

    if is_track_view && filter.is_active() {
        // Build indicator spans: "filter: " + state + " " + #tag
        let mut indicator_spans: Vec<Span> = Vec::new();
        indicator_spans.push(Span::styled(
            "filter: ",
            Style::default().fg(app.theme.purple).bg(bg),
        ));

        if let Some(sf) = &filter.state_filter {
            let state_color = match sf {
                StateFilter::Active => app.theme.state_color(crate::model::TaskState::Active),
                StateFilter::Todo => app.theme.state_color(crate::model::TaskState::Todo),
                StateFilter::Blocked => app.theme.state_color(crate::model::TaskState::Blocked),
                StateFilter::Parked => app.theme.state_color(crate::model::TaskState::Parked),
                StateFilter::Ready => app.theme.state_color(crate::model::TaskState::Active),
            };
            indicator_spans.push(Span::styled(
                sf.label(),
                Style::default().fg(state_color).bg(bg),
            ));
        }

        if let Some(ref tag) = filter.tag_filter {
            if filter.state_filter.is_some() {
                indicator_spans.push(Span::styled(" ", Style::default().bg(bg)));
            }
            let tag_color = app.theme.tag_color(tag);
            indicator_spans.push(Span::styled(
                format!("#{}", tag),
                Style::default().fg(tag_color).bg(bg),
            ));
        }

        // Calculate indicator width
        let indicator_width: usize = indicator_spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        // +2: one space before indicator, one space after (right edge buffer)
        let separator_end = width.saturating_sub(indicator_width + 2);

        let mut spans: Vec<Span> = Vec::new();
        // Build separator chars up to where indicator starts
        let mut sep_text = String::with_capacity(separator_end * 3);
        for col in 0..separator_end {
            if sep_cols.contains(&col) {
                sep_text.push('\u{2534}');
            } else {
                sep_text.push('\u{2500}');
            }
        }
        spans.push(Span::styled(sep_text, Style::default().fg(dim).bg(bg)));
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        spans.extend(indicator_spans);
        // Trailing space
        let current_width: usize = spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        if current_width < width {
            spans.push(Span::styled(
                " ".repeat(width - current_width),
                Style::default().bg(bg),
            ));
        }

        let line = Line::from(spans);
        let sep_widget = Paragraph::new(line).style(Style::default().bg(bg));
        frame.render_widget(sep_widget, area);
    } else {
        // No filter — plain separator
        let mut line: String = String::with_capacity(width * 3);
        for col in 0..width {
            if sep_cols.contains(&col) {
                line.push('\u{2534}');
            } else {
                line.push('\u{2500}');
            }
        }
        let sep_widget = Paragraph::new(line).style(Style::default().fg(dim).bg(bg));
        frame.render_widget(sep_widget, area);
    }
}

/// Style for a tab: highlighted if current, normal otherwise
fn tab_style(app: &App, is_current: bool) -> Style {
    if is_current {
        Style::default()
            .fg(app.theme.text_bright)
            .bg(app.theme.selection_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text).bg(app.theme.background)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_fit_with_project_name() {
        let names = vec!["Alpha".to_string(), "Beta".to_string()];
        let prefixes = vec![None, None];
        // "Alpha" tab = 5+2+1 = 8, "Beta" tab = 4+2+1 = 7
        // fixed(0 inbox) = 3+4+4+4 = 15
        // project name "My Project" = 10+2 = 12
        // total = 8+7+15+12 = 42
        let layout = compute_tab_layout(&names, &prefixes, None, 50, 10, 0);
        assert!(layout.show_project_name);
        assert!(!layout.scroll_mode);
        assert_eq!(layout.labels, vec!["Alpha", "Beta"]);
    }

    #[test]
    fn test_project_name_removed() {
        let names = vec!["Alpha".to_string(), "Beta".to_string()];
        let prefixes = vec![None, None];
        // tabs + fixed = 8+7+15 = 30, + project 12 = 42
        // Width 35 < 42 but >= 30
        let layout = compute_tab_layout(&names, &prefixes, None, 35, 10, 0);
        assert!(!layout.show_project_name);
        assert!(!layout.scroll_mode);
        assert_eq!(layout.labels, vec!["Alpha", "Beta"]);
    }

    #[test]
    fn test_shrink_longest_first() {
        let names = vec![
            "Infrastructure".to_string(), // 14
            "Backend".to_string(),        // 7
            "Frontend".to_string(),       // 8
        ];
        let prefixes = vec![None, None, None];
        // Full: 17+10+11 = 38, + fixed 15 = 53
        // Width 45: need to shrink 8 chars of tab width
        let layout = compute_tab_layout(&names, &prefixes, None, 45, 0, 0);
        assert!(!layout.scroll_mode);
        // Longest (Infrastructure) should absorb most shrinking; shorter labels preserved
        let lens: Vec<usize> = layout
            .labels
            .iter()
            .map(|l| unicode::display_width(l))
            .collect();
        assert!(
            lens[0] >= lens[1],
            "longest name should still be >= shorter: {:?}",
            lens
        );
        assert!(
            lens[0] >= lens[2],
            "longest name should still be >= shorter: {:?}",
            lens
        );
        // Backend (7) and Frontend (8) should be mostly intact
        assert_eq!(layout.labels[1], "Backend"); // 7 chars, not touched since Infra absorbs
        let total: usize = layout
            .labels
            .iter()
            .map(|l| unicode::display_width(l) + 2 + 1)
            .sum::<usize>()
            + 15;
        assert!(total <= 45);
    }

    #[test]
    fn test_prefix_swap_at_exact_length() {
        // Prefix swaps in when label is truncated to exactly prefix length
        let names = vec![
            "Infrastructure".to_string(),
            "Backend".to_string(),
            "Frontend".to_string(),
        ];
        let prefixes = vec![
            Some("INF".to_string()), // 3 chars
            Some("BE".to_string()),  // 2 chars
            Some("FE".to_string()),  // 2 chars
        ];
        // Width 32: shrinks all to 3, then FE swaps at 2 → fits at 32.
        // INF swaps at 3 (same-size swap), but Bac(3) stays since BE is 2 chars.
        let layout = compute_tab_layout(&names, &prefixes, None, 32, 0, 0);
        assert!(!layout.scroll_mode);
        // INF swapped at 3, FE swapped at 2, Backend truncated to "Bac"
        assert_eq!(layout.labels[0], "INF");
        assert_eq!(layout.labels[2], "FE");

        // At width 31: Backend also reaches prefix (INF+BE+FE = 6+5+5+15=31)
        let layout2 = compute_tab_layout(&names, &prefixes, None, 31, 0, 0);
        assert!(!layout2.scroll_mode);
        assert_eq!(layout2.labels, vec!["INF", "BE", "FE"]);
    }

    #[test]
    fn test_prefix_swap_is_zero_width() {
        // Prefix swap doesn't cause extra shrinking — rightmost shrinks first
        let names = vec!["Alpha".to_string(), "Bravo".to_string()];
        let prefixes = vec![Some("ALP".to_string()), Some("BRV".to_string())];
        // Full: 8+8+15=31. Width 28: need 3 chars removed.
        // Bravo(rightmost) 5→4, Alpha 5→4, Bravo 4→3 (swap BRV) → 7+6+15=28 fits.
        // Alpha stays at "Alph"(4), not over-shrunk.
        let layout = compute_tab_layout(&names, &prefixes, None, 28, 0, 0);
        assert!(!layout.scroll_mode);
        assert_eq!(layout.labels[0], "Alph"); // not shrunk past 4
        assert_eq!(layout.labels[1], "BRV"); // prefix at 3
    }

    #[test]
    fn test_shrink_past_prefix() {
        // 4-char prefix names that need shrinking below 4
        let names = vec![
            "AAAA".to_string(),
            "BBBB".to_string(),
            "CCCC".to_string(),
            "DDDD".to_string(),
        ];
        let prefixes = vec![
            Some("AAAA".to_string()),
            Some("BBBB".to_string()),
            Some("CCCC".to_string()),
            Some("DDDD".to_string()),
        ];
        // At 4 chars: 7*4=28, +15=43. Width 39: need all at 3 (6*4=24, +15=39).
        let layout = compute_tab_layout(&names, &prefixes, None, 39, 0, 0);
        assert!(!layout.scroll_mode);
        for label in &layout.labels {
            assert_eq!(unicode::display_width(label), 3);
        }
    }

    #[test]
    fn test_scrolling_when_nothing_fits() {
        let names: Vec<String> = (0..20).map(|i| format!("Track{}", i)).collect();
        let prefixes: Vec<Option<String>> = (0..20).map(|i| Some(format!("T{}", i))).collect();
        let layout = compute_tab_layout(&names, &prefixes, None, 60, 0, 0);
        assert!(layout.scroll_mode);
    }

    #[test]
    fn test_no_over_shrink() {
        // Verify that tabs aren't shrunk more than necessary
        let names = vec![
            "TUI Dev".to_string(),  // 7
            "CLI".to_string(),      // 3
            "Another".to_string(),  // 7
            "One More".to_string(), // 8
            "Booyah".to_string(),   // 6
            "Delete".to_string(),   // 6
            "Echo".to_string(),     // 4
            "Further".to_string(),  // 7
        ];
        let prefixes: Vec<Option<String>> = vec![None; 8];
        // Full tab widths: 10+6+10+11+9+9+7+10 = 72, + fixed 15 = 87
        let layout = compute_tab_layout(&names, &prefixes, None, 75, 0, 0);
        assert!(!layout.scroll_mode);
        let total: usize = layout
            .labels
            .iter()
            .map(|l| unicode::display_width(l) + 2 + 1)
            .sum::<usize>()
            + 15;
        assert!(total <= 75, "total {} should be <= 75", total);
        // Should be tight: at most 1 char of slack
        assert!(total >= 74, "total {} shouldn't leave much slack", total);
        // Short labels like "CLI"(3) shouldn't be shrunk when longer ones can absorb
        assert_eq!(layout.labels[1], "CLI");
    }

    #[test]
    fn test_balanced_shrinking_equal_names() {
        // All same-length names: should shrink evenly (rightmost first for ties)
        let names = vec![
            "Alpha".to_string(),
            "Bravo".to_string(),
            "Delta".to_string(),
        ];
        let prefixes = vec![None, None, None];
        // Full: 8*3 = 24, +15 = 39. Width 37: need 2 chars removed.
        let layout = compute_tab_layout(&names, &prefixes, None, 37, 0, 0);
        assert!(!layout.scroll_mode);
        let lens: Vec<usize> = layout
            .labels
            .iter()
            .map(|l| unicode::display_width(l))
            .collect();
        let max = *lens.iter().max().unwrap();
        let min = *lens.iter().min().unwrap();
        assert!(max - min <= 1, "labels should be balanced: {:?}", lens);
    }

    #[test]
    fn test_cc_focus_width_accounting() {
        let names = vec!["Alpha".to_string(), "Beta".to_string()];
        let prefixes = vec![None, None];
        // Without cc: 8+7+15 = 30
        // With cc on Alpha: 8+2+7+15 = 32
        // Width 31: fits without cc, doesn't fit with cc
        let layout_no_cc = compute_tab_layout(&names, &prefixes, None, 31, 0, 0);
        assert!(!layout_no_cc.scroll_mode);

        let layout_cc = compute_tab_layout(&names, &prefixes, Some(0), 31, 0, 0);
        // Should trigger shrinking (project name already hidden)
        assert!(!layout_cc.show_project_name);
    }

    #[test]
    fn test_zero_tracks() {
        let layout = compute_tab_layout(&[], &[], None, 80, 10, 0);
        assert!(layout.labels.is_empty());
        assert!(!layout.scroll_mode);
        assert!(layout.show_project_name); // fixed 15 + project 12 = 27 < 80
    }

    #[test]
    fn test_one_track() {
        let names = vec!["Solo".to_string()];
        let prefixes = vec![None];
        let layout = compute_tab_layout(&names, &prefixes, None, 30, 0, 0);
        assert!(!layout.scroll_mode);
        assert_eq!(layout.labels, vec!["Solo"]);
    }

    #[test]
    fn test_inbox_count_affects_fixed_width() {
        assert!(fixed_width(99) > fixed_width(0));
        assert_eq!(fixed_width(0), 15); // 3+4+4+4
        assert_eq!(fixed_width(99), 17); // 3+4+6+4

        let names = vec!["A".to_string()];
        let prefixes = vec![None];
        // Track "A" = 1+2+1 = 4
        // fixed(0)=15 + 4 = 19 fits in 20
        let layout_0 = compute_tab_layout(&names, &prefixes, None, 20, 0, 0);
        assert!(!layout_0.scroll_mode);
        // fixed(99)=17 + 4 = 21, doesn't fit in 20 without shrinking
        // But "A" is already 1 char, can't shrink further → scroll
        let layout_99 = compute_tab_layout(&names, &prefixes, None, 21, 0, 99);
        assert!(!layout_99.scroll_mode);
    }

    #[test]
    fn test_digit_count() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(1), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(99), 2);
        assert_eq!(digit_count(100), 3);
    }
}
