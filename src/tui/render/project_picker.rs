use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::io::registry::abbreviate_path;
use crate::tui::app::{App, ProjectPickerState};
use crate::tui::theme::Theme;
use crate::util::unicode;

/// Render the project picker popup overlay using the App's theme and picker state.
pub fn render_project_picker(frame: &mut Frame, app: &App, area: Rect) {
    let picker = match &app.project_picker {
        Some(p) => p,
        None => return,
    };
    render_project_picker_inner(frame, picker, &app.theme, area);
}

/// Render the project picker with an explicit theme (used by standalone picker mode).
pub fn render_project_picker_standalone(
    frame: &mut Frame,
    picker: &ProjectPickerState,
    theme: &Theme,
    area: Rect,
) {
    render_project_picker_inner(frame, picker, theme, area);
}

fn render_project_picker_inner(
    frame: &mut Frame,
    picker: &ProjectPickerState,
    theme: &Theme,
    area: Rect,
) {
    let bg = theme.background;
    let text_color = theme.text;
    let bright = theme.text_bright;
    let dim = theme.dim;
    let highlight = theme.highlight;
    let sel_bg = theme.selection_bg;

    let bg_style = Style::default().bg(bg);

    // Sizing: 60% width, min 40, max 62 (inner 60 + 2 borders)
    let target_w = (area.width as f32 * 0.6) as u16;
    let popup_w = target_w.clamp(40, 62).min(area.width.saturating_sub(2));
    let inner_w = (popup_w - 2) as usize; // subtract borders
    let content_w = inner_w.saturating_sub(2); // 1-char padding each side

    let mut lines: Vec<Line> = Vec::new();

    // Blank line at top
    lines.push(Line::from(Span::styled(" ".repeat(inner_w), bg_style)));

    // Why the picker is open, when it was not opened on purpose. Wrapped, because
    // it names a path and a truncated path is not one.
    if let Some(notice) = &picker.notice {
        let notice_style = Style::default().fg(highlight).bg(bg);
        for visual in crate::tui::wrap::wrap_line(notice, content_w.max(1), 0) {
            let text = format!(" {}", &notice[visual.byte_start..visual.byte_end]);
            let mut spans = vec![Span::styled(text.clone(), notice_style)];
            pad_to_width(&mut spans, inner_w, bg_style);
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(Span::styled(" ".repeat(inner_w), bg_style)));
    }

    if picker.rows.is_empty() {
        let empty_lines = [
            " No projects registered.",
            "",
            " Run `fr init` in a project directory",
            " or `fr projects add <path>` to register.",
        ];
        for text in &empty_lines {
            let padded = format!(" {}", text);
            let mut spans = vec![Span::styled(
                padded.clone(),
                Style::default().fg(dim).bg(bg),
            )];
            let used = unicode::display_width(&padded);
            if used < inner_w {
                spans.push(Span::styled(" ".repeat(inner_w - used), bg_style));
            }
            lines.push(Line::from(spans));
        }
    } else {
        // A worktree row is indented under its project, so the indent counts
        // toward the column the paths line up after.
        let labels: Vec<String> = picker
            .rows
            .iter()
            .map(|row| {
                if row.nested {
                    format!("  {}", row.label())
                } else {
                    row.label()
                }
            })
            .collect();
        let max_name = labels
            .iter()
            .map(|l| unicode::display_width(l))
            .max()
            .unwrap_or(0)
            .min(content_w / 3);
        let name_col = max_name + 2; // +2 for padding after name

        for (i, (row, label)) in picker.rows.iter().zip(&labels).enumerate() {
            let entry = &row.entry;
            let is_selected = i == picker.cursor;
            let is_current = picker
                .current_project_path
                .as_ref()
                .is_some_and(|p| *p == entry.path);
            let exists = std::path::Path::new(&entry.path).join("frame").exists();

            let row_bg = if is_selected { sel_bg } else { bg };
            let row_pad = Style::default().bg(row_bg);
            let row_style = if is_selected {
                Style::default()
                    .fg(bright)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                row_pad
            };

            let mut spans: Vec<Span> = Vec::new();

            // Cursor indicator: " ▶ " on selected, "   " otherwise (matches autocomplete)
            let indicator = if is_selected { " \u{25B6} " } else { "   " };
            spans.push(Span::styled(indicator, row_style));

            // Project name, or a worktree's branch under it
            let name_display: String = if unicode::display_width(label) > max_name {
                let truncated: String = label.chars().take(max_name.saturating_sub(1)).collect();
                format!("{}\u{2026}", truncated)
            } else {
                label.clone()
            };

            let name_color = if !exists {
                dim
            } else if is_current {
                highlight
            } else if row.nested {
                // Subordinate to the project row above it, which carries the name.
                text_color
            } else {
                bright
            };
            let name_style = if is_selected {
                Style::default()
                    .fg(name_color)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(name_color).bg(row_bg)
            };
            spans.push(Span::styled(name_display.clone(), name_style));

            // Pad name column
            let name_chars = unicode::display_width(&name_display);
            let pad = name_col.saturating_sub(name_chars);
            spans.push(Span::styled(" ".repeat(pad), row_pad));

            // Path column — truncate to fit remaining space with 1-char right margin
            let used_so_far = 3 + name_chars + pad; // indicator + name + pad
            let path_budget = inner_w.saturating_sub(used_so_far + 1); // 1 for right margin
            let path_full = if !exists {
                "(not found)".to_string()
            } else {
                abbreviate_path(&entry.path)
            };
            let path_display = if unicode::display_width(&path_full) > path_budget {
                let truncated: String = path_full
                    .chars()
                    .take(path_budget.saturating_sub(1))
                    .collect();
                format!("{}\u{2026}", truncated)
            } else {
                path_full
            };
            let path_color = if !exists { dim } else { text_color };
            spans.push(Span::styled(
                path_display,
                Style::default().fg(path_color).bg(row_bg),
            ));

            // Confirm remove indicator
            if picker.confirm_remove == Some(i) {
                spans.push(Span::styled(
                    "  remove? X",
                    Style::default()
                        .fg(highlight)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            pad_to_width(&mut spans, inner_w, row_pad);
            lines.push(Line::from(spans));
        }
    }

    // Blank line before footer
    lines.push(Line::from(Span::styled(" ".repeat(inner_w), bg_style)));

    // Sort indicator
    let sort_label = if picker.sort_alpha {
        "sorted by: name"
    } else {
        "sorted by: recent"
    };
    let sort_style = Style::default().fg(text_color).bg(bg);
    let mut sort_spans = vec![
        Span::styled(" ", bg_style),
        Span::styled(sort_label, sort_style),
    ];
    let sort_used = 1 + unicode::display_width(sort_label);
    if sort_used < inner_w {
        sort_spans.push(Span::styled(" ".repeat(inner_w - sort_used), bg_style));
    }
    lines.push(Line::from(sort_spans));

    // Key hints — two lines to avoid truncation
    let hint_style = Style::default().fg(dim).bg(bg);
    let hint1 = " \u{2191}\u{2193}/jk navigate  Enter open  s sort";
    let hint2 = " X remove  Esc close";
    let hint1_len = unicode::display_width(hint1);
    let hint2_len = unicode::display_width(hint2);
    let mut hint1_spans = vec![Span::styled(hint1, hint_style)];
    if hint1_len < inner_w {
        hint1_spans.push(Span::styled(" ".repeat(inner_w - hint1_len), bg_style));
    }
    lines.push(Line::from(hint1_spans));
    let mut hint2_spans = vec![Span::styled(hint2, hint_style)];
    if hint2_len < inner_w {
        hint2_spans.push(Span::styled(" ".repeat(inner_w - hint2_len), bg_style));
    }
    lines.push(Line::from(hint2_spans));

    // Height: content-sized up to 70%
    let max_h = ((area.height as f32) * 0.7) as u16;
    let content_h = lines.len() as u16;
    let popup_h = (content_h + 2)
        .min(max_h)
        .min(area.height.saturating_sub(2));

    // Position: centered
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    // Title
    let title = " Projects ";
    let title_style = Style::default()
        .fg(text_color)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(text_color).bg(bg))
        .style(Style::default().bg(bg));

    // Scroll
    let scroll = picker.scroll_offset;

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0))
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, popup_area);
}

/// Pad spans to fill `target_width` with background.
fn pad_to_width<'a>(spans: &mut Vec<Span<'a>>, target_width: usize, pad_style: Style) {
    let total_used: usize = spans
        .iter()
        .map(|s| unicode::display_width(&s.content))
        .sum();
    if total_used < target_width {
        spans.push(Span::styled(
            " ".repeat(target_width - total_used),
            pad_style,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::registry::{ProjectEntry, ProjectRow};
    use crate::tui::app::ProjectPickerState;
    use crate::tui::render::test_helpers::*;
    use insta::assert_snapshot;

    fn row(name: &str, path: &str, branch: Option<&str>, nested: bool) -> ProjectRow {
        ProjectRow {
            entry: ProjectEntry {
                name: name.into(),
                path: path.into(),
                last_accessed_tui: None,
                last_accessed_cli: None,
                worktree_of: nested.then(|| "/home/user/alpha".to_string()),
            },
            branch: branch.map(|b| b.to_string()),
            nested,
        }
    }

    fn picker(rows: Vec<ProjectRow>, notice: Option<&str>) -> ProjectPickerState {
        ProjectPickerState {
            rows,
            cursor: 0,
            scroll_offset: 0,
            sort_alpha: false,
            current_project_path: Some("/home/user/alpha".into()),
            confirm_remove: None,
            notice: notice.map(|n| n.to_string()),
        }
    }

    #[test]
    fn picker_with_entries() {
        let mut app = app_with_track(SIMPLE_TRACK_MD);
        app.project_picker = Some(picker(
            vec![
                row("Project Alpha", "/home/user/alpha", None, false),
                row("Project Beta", "/home/user/beta", None, false),
            ],
            None,
        ));
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_project_picker(frame, &app, area);
        });
        assert_snapshot!(output);
    }

    /// A worktree sits under its project, labelled by the branch — the project
    /// name is committed, so every row of a clone would otherwise read the same.
    #[test]
    fn picker_nests_worktrees_under_their_project() {
        let mut app = app_with_track(SIMPLE_TRACK_MD);
        app.project_picker = Some(picker(
            vec![
                row("Project Alpha", "/home/user/alpha", None, false),
                row(
                    "Project Alpha",
                    "/home/user/alpha/.claude/worktrees/wt",
                    Some("feature-x"),
                    true,
                ),
                row("Project Beta", "/home/user/beta", None, false),
            ],
            None,
        ));
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_project_picker(frame, &app, area);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn picker_shows_a_notice_when_the_project_vanished() {
        let mut app = app_with_track(SIMPLE_TRACK_MD);
        app.project_picker = Some(picker(
            vec![row("Project Beta", "/home/user/beta", None, false)],
            Some(
                "Project Alpha is gone — ~/alpha no longer exists. 2 unsaved files discarded with it.",
            ),
        ));
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_project_picker(frame, &app, area);
        });
        assert_snapshot!(output);
    }
}
