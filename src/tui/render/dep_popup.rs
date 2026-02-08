use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::model::TaskState;
use crate::tui::app::{App, DepPopupEntry};

use super::truncate_with_ellipsis;

/// Render the dep popup overlay
pub fn render_dep_popup(frame: &mut Frame, app: &App, area: Rect) {
    let dp = match &app.dep_popup {
        Some(dp) => dp,
        None => return,
    };

    let bg = app.theme.background;
    let bright = app.theme.text_bright;
    let highlight = app.theme.highlight;
    let dim = app.theme.dim;
    let red = app.theme.red;
    let sel_bg = app.theme.selection_bg;

    // Sizing: 80% width, min 40, max 100
    let target_w = (area.width as f32 * 0.8) as u16;
    let inner_w = target_w.clamp(40, 100).min(area.width.saturating_sub(2)) as usize;
    let popup_w = (inner_w as u16) + 2; // +2 for borders

    // Reserve 1 char right margin so text never touches the border
    let usable_w = inner_w.saturating_sub(1);

    // Build lines
    let mut lines: Vec<Line> = Vec::new();
    let mut in_blocking_section = false;

    // Blank line at top for spacing
    lines.push(Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(bg),
    )));

    for (entry_idx, entry) in dp.entries.iter().enumerate() {
        match entry {
            DepPopupEntry::SectionHeader { label } => {
                // Add blank line between sections (before "Blocking")
                if in_blocking_section {
                    // This is the second header — was already set on first
                } else if *label == "Blocking" {
                    in_blocking_section = true;
                    lines.push(Line::from(Span::styled(
                        " ".repeat(inner_w),
                        Style::default().bg(bg),
                    )));
                }

                let header_style = Style::default()
                    .fg(bright)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD);
                let mut spans = vec![Span::styled(format!("  {}", label), header_style)];
                let used = 2 + label.len();
                if used < inner_w {
                    spans.push(Span::styled(
                        " ".repeat(inner_w - used),
                        Style::default().bg(bg),
                    ));
                }
                lines.push(Line::from(spans));
            }
            DepPopupEntry::Nothing => {
                let nothing_style = Style::default().fg(dim).bg(bg);
                let text = "    (nothing)";
                let mut spans = vec![Span::styled(text.to_string(), nothing_style)];
                let used = text.len();
                if used < inner_w {
                    spans.push(Span::styled(
                        " ".repeat(inner_w - used),
                        Style::default().bg(bg),
                    ));
                }
                lines.push(Line::from(spans));
            }
            DepPopupEntry::Task {
                task_id,
                title,
                state,
                track_id,
                depth,
                has_children,
                is_expanded,
                is_circular,
                is_dangling,
                is_upstream: _,
            } => {
                let is_selected = entry_idx == dp.cursor;
                let row_bg = if is_selected { sel_bg } else { bg };
                let row_pad = Style::default().bg(row_bg);

                let mut spans: Vec<Span> = Vec::new();

                // Indentation: base 4 + 2 per depth level
                let indent = 4 + depth * 2;
                let indent_str = " ".repeat(indent);
                spans.push(Span::styled(indent_str.clone(), row_pad));

                if *is_circular {
                    // Circular: "↻ EFF-012  (circular)"
                    let circ_style = Style::default().fg(dim).bg(row_bg);
                    spans.push(Span::styled("\u{21BB} ", circ_style));
                    spans.push(Span::styled(task_id.to_string(), circ_style));
                    spans.push(Span::styled("  (circular)", circ_style));
                    pad_to_width(&mut spans, inner_w, row_pad);
                    lines.push(Line::from(spans));
                    continue;
                }

                if *is_dangling {
                    // Dangling: "[?] MOD-099  (not found)"
                    let dang_style = Style::default().fg(red).bg(row_bg);
                    spans.push(Span::styled("[?] ", dang_style));
                    let id_style = if is_selected {
                        Style::default()
                            .fg(bright)
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(bright).bg(row_bg)
                    };
                    spans.push(Span::styled(task_id.to_string(), id_style));
                    let not_found_style = Style::default().fg(dim).bg(row_bg);
                    spans.push(Span::styled("  (not found)", not_found_style));
                    pad_to_width(&mut spans, inner_w, row_pad);
                    lines.push(Line::from(spans));
                    continue;
                }

                let task_state = state.unwrap_or(TaskState::Todo);
                let is_done = task_state == TaskState::Done;

                // Expand/collapse indicator
                if *has_children {
                    let arrow = if *is_expanded {
                        "\u{25BC} "
                    } else {
                        "\u{25B6} "
                    };
                    let arrow_style = Style::default().fg(dim).bg(row_bg);
                    spans.push(Span::styled(arrow, arrow_style));
                } else {
                    spans.push(Span::styled("  ", row_pad));
                }

                // Checkbox state: [x], [>], etc.
                let checkbox_char = task_state.checkbox_char();
                let state_color = app.theme.state_color(task_state);
                let cb_style = if is_done {
                    Style::default().fg(dim).bg(row_bg)
                } else {
                    Style::default().fg(state_color).bg(row_bg)
                };
                spans.push(Span::styled(format!("[{}] ", checkbox_char), cb_style));

                // Task ID
                let id_style = if is_done {
                    Style::default().fg(dim).bg(row_bg)
                } else if is_selected {
                    Style::default()
                        .fg(bright)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(bright).bg(row_bg)
                };
                spans.push(Span::styled(format!("{}  ", task_id), id_style));

                // Calculate space for title and track name
                // Layout: indent + arrow(2) + checkbox(4) + id + 2 + title + gap + track
                let fixed_left = indent + 2 + 4 + task_id.len() + 2;
                let track_name = track_id
                    .as_ref()
                    .map(|tid| app.track_name(tid))
                    .unwrap_or("");
                let right_part_len = track_name.len() + 2; // 2 for spacing before track name
                let title_max = usable_w
                    .saturating_sub(fixed_left)
                    .saturating_sub(right_part_len);

                let display_title = truncate_with_ellipsis(title, title_max);
                // Follow track view conventions: text_bright for normal, dim for done
                let title_style = if is_done {
                    Style::default().fg(dim).bg(row_bg)
                } else if is_selected {
                    Style::default()
                        .fg(bright)
                        .bg(row_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(bright).bg(row_bg)
                };
                spans.push(Span::styled(display_title.clone(), title_style));

                // Pad between title and track name
                let title_display_len = display_title.chars().count();
                let used_so_far = fixed_left + title_display_len;
                let target_end = usable_w.saturating_sub(right_part_len);
                if used_so_far < target_end {
                    spans.push(Span::styled(" ".repeat(target_end - used_so_far), row_pad));
                }

                // Track name (dimmed if same track as root)
                let is_same_track = track_id.as_deref() == Some(&dp.root_track_id);
                let track_style = if is_same_track || is_done {
                    Style::default().fg(dim).bg(row_bg)
                } else {
                    Style::default().fg(bright).bg(row_bg)
                };
                spans.push(Span::styled(format!("  {}", track_name), track_style));

                // Pad to fill width
                pad_to_width(&mut spans, inner_w, row_pad);

                lines.push(Line::from(spans));
            }
        }
    }

    // Blank line before hint bar
    lines.push(Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(bg),
    )));

    // Hint bar
    let hint_style = Style::default().fg(dim).bg(bg);
    let hint = "\u{2190}\u{2192} expand   Enter jump   Esc close";
    let hint_len = hint.chars().count();
    let hint_pad = inner_w.saturating_sub(hint_len);
    let left_pad = hint_pad / 2;
    let right_pad = hint_pad - left_pad;
    lines.push(Line::from(vec![
        Span::styled(" ".repeat(left_pad), Style::default().bg(bg)),
        Span::styled(hint, hint_style),
        Span::styled(" ".repeat(right_pad), Style::default().bg(bg)),
    ]));

    // Height: content-sized up to 70% of terminal
    let max_h = ((area.height as f32) * 0.7) as u16;
    let content_h = lines.len() as u16;
    let popup_h = (content_h + 2)
        .min(max_h)
        .min(area.height.saturating_sub(2)); // +2 for borders

    // Position: centered
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    // Title
    let title = format!(" Dependencies: {} ", dp.root_task_id);
    let title_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    // Scroll: inner height = popup_h - 2 (borders)
    let inner_h = popup_h.saturating_sub(2) as usize;
    let scroll = dp.scroll_offset;

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0))
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, popup_area);

    // Scroll indicators in border
    if scroll > 0 {
        let up_area = Rect::new(popup_area.x + popup_w - 2, popup_area.y, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled("\u{25B2}", Style::default().fg(dim).bg(bg))),
            up_area,
        );
    }
    if content_h > inner_h as u16 && scroll + inner_h < content_h as usize {
        let down_area = Rect::new(popup_area.x + popup_w - 2, popup_area.y + popup_h - 1, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled("\u{25BC}", Style::default().fg(dim).bg(bg))),
            down_area,
        );
    }
}

/// Pad spans to fill `target_width` with background.
fn pad_to_width<'a>(spans: &mut Vec<Span<'a>>, target_width: usize, pad_style: Style) {
    let total_used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if total_used < target_width {
        spans.push(Span::styled(
            " ".repeat(target_width - total_used),
            pad_style,
        ));
    }
}
