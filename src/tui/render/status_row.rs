use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, EditTarget, Mode, MoveState, TriageSource};

/// Render the status row (bottom of screen)
pub fn render_status_row(frame: &mut Frame, app: &App, area: Rect) {
    let bg = app.theme.background;
    let width = area.width as usize;

    let line = match &app.mode {
        Mode::Navigate if app.status_message.is_some() => render_centered_message(
            app.status_message.as_deref().unwrap(),
            width,
            bg,
            app.status_is_error,
            app.theme.text_bright,
        ),
        Mode::Navigate if app.filter_pending => {
            let mut spans = vec![
                Span::styled(
                    " f",
                    Style::default()
                        .fg(app.theme.highlight)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("\u{258C}", Style::default().fg(app.theme.highlight).bg(bg)),
            ];
            let hint = "a=active o=todo b=blocked p=parked r=ready t=tag f=clear";
            build_mode_hint(&mut spans, hint, width, bg, app.theme.text_bright);
            Line::from(spans)
        }
        Mode::Navigate => {
            if let Some(ref pattern) = app.last_search {
                let mut spans = vec![Span::styled(
                    format!("/{}", pattern),
                    Style::default().fg(app.theme.text_bright).bg(bg),
                )];
                let hint = "n/N next/prev  Esc clear";
                build_right_side(app, &mut spans, hint, width, bg, true);
                Line::from(spans)
            } else {
                Line::from(Span::styled(" ".repeat(width), Style::default().bg(bg)))
            }
        }
        Mode::Search => {
            let mut spans = vec![
                Span::styled(
                    format!("/{}", app.search_input),
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ),
                Span::styled("\u{258C}", Style::default().fg(app.theme.highlight).bg(bg)),
            ];
            let hint = "Enter search  Esc cancel";
            build_right_side(app, &mut spans, hint, width, bg, false);
            Line::from(spans)
        }
        Mode::Edit => {
            let is_filter_tag = matches!(
                app.edit_target,
                Some(crate::tui::app::EditTarget::FilterTag)
            );
            let is_jump_to = matches!(app.edit_target, Some(crate::tui::app::EditTarget::JumpTo));
            let label = if is_filter_tag {
                "filter tag:"
            } else if is_jump_to {
                "jump:"
            } else {
                "-- EDIT --"
            };
            let mode_label = Span::styled(
                label,
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let hint = if is_filter_tag {
                "Enter select  Esc cancel"
            } else if is_jump_to {
                "Enter jump  Esc cancel"
            } else {
                "Enter confirm  Esc cancel"
            };
            let mut spans = vec![Span::styled(" ", Style::default().bg(bg)), mode_label];
            if is_filter_tag || is_jump_to {
                spans.push(Span::styled(" ", Style::default().bg(bg)));
                spans.push(Span::styled(
                    app.edit_buffer.clone(),
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
                spans.push(Span::styled(
                    "\u{258C}",
                    Style::default().fg(app.theme.highlight).bg(bg),
                ));
            }
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let hint_width = hint.chars().count();
            if content_width + hint_width < width {
                let padding = width - content_width - hint_width;
                spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
                spans.push(Span::styled(
                    hint,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
            }
            Line::from(spans)
        }
        Mode::Move => {
            let label_text = if let Some(MoveState::BulkTask {
                ref removed_tasks, ..
            }) = app.move_state
            {
                format!("-- MOVE ({}) --", removed_tasks.len())
            } else {
                "-- MOVE --".to_string()
            };
            let mode_label = Span::styled(
                label_text,
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let hint =
                "\u{25B2}\u{25BC} move  \u{25C0}\u{25B6} depth  m/Enter \u{2713}  Esc \u{2717}";
            let mut spans = vec![Span::styled(" ", Style::default().bg(bg)), mode_label];
            build_mode_hint(&mut spans, hint, width, bg, app.theme.text_bright);
            Line::from(spans)
        }
        Mode::Triage => {
            let is_select_track = matches!(
                &app.triage_state,
                Some(ts) if matches!(ts.step, crate::tui::app::TriageStep::SelectTrack)
            );
            let step_text = if is_select_track {
                "Select track:"
            } else {
                "Select position:"
            };
            let is_cross_track = matches!(
                &app.triage_state,
                Some(ts) if matches!(ts.source, TriageSource::CrossTrackMove { .. } | TriageSource::BulkCrossTrackMove { .. })
            );
            let label_text = if is_cross_track {
                "-- MOVE TO TRACK --"
            } else {
                "-- TRIAGE --"
            };
            let mode_label = Span::styled(
                label_text,
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let mut spans = vec![
                Span::styled(" ", Style::default().bg(bg)),
                mode_label,
                Span::styled("  ", Style::default().bg(bg)),
            ];
            // In track selection step, show the edit buffer with cursor
            if is_select_track {
                spans.push(Span::styled(
                    step_text,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
                spans.push(Span::styled(" ", Style::default().bg(bg)));
                spans.push(Span::styled(
                    app.edit_buffer.clone(),
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
                spans.push(Span::styled(
                    "\u{258C}",
                    Style::default().fg(app.theme.highlight).bg(bg),
                ));
            } else {
                spans.push(Span::styled(
                    step_text,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
            }
            // Pad to full width
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            if content_width < width {
                spans.push(Span::styled(
                    " ".repeat(width - content_width),
                    Style::default().bg(bg),
                ));
            }
            Line::from(spans)
        }
        Mode::Select if app.status_message.is_some() => render_centered_message(
            app.status_message.as_deref().unwrap(),
            width,
            bg,
            app.status_is_error,
            app.theme.text_bright,
        ),
        Mode::Select => {
            let count = app.selection.len();
            let is_range = app.range_anchor.is_some();
            let label_text = if is_range {
                "-- RANGE --"
            } else {
                "-- SELECT --"
            };
            let mode_label = Span::styled(
                label_text,
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let count_text = format!("{} selected", count);
            let is_bulk_edit = matches!(
                &app.edit_target,
                Some(EditTarget::BulkTags) | Some(EditTarget::BulkDeps)
            );
            let hint = if is_bulk_edit {
                "Enter confirm  Esc cancel"
            } else if is_range {
                "V end range  Esc cancel"
            } else {
                "x/b/o/~ t d m Esc"
            };
            let mut spans = vec![Span::styled(" ", Style::default().bg(bg)), mode_label];
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let count_width = count_text.chars().count();
            let hint_width = hint.chars().count();
            let right_width = count_width + 4 + hint_width;
            if content_width + right_width < width {
                let padding = width - content_width - right_width;
                spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
                spans.push(Span::styled(
                    count_text,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
                spans.push(Span::styled(" ".repeat(4), Style::default().bg(bg)));
                spans.push(Span::styled(
                    hint,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
            } else if content_width + hint_width < width {
                let padding = width - content_width - hint_width;
                spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
                spans.push(Span::styled(
                    hint,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
            }
            Line::from(spans)
        }
        Mode::Command => {
            let mode_label = Span::styled(
                "-- COMMAND --",
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let hint = "\u{25B2}\u{25BC} navigate  Enter \u{2713}  Esc \u{2717}";
            let mut spans = vec![Span::styled(" ", Style::default().bg(bg)), mode_label];
            build_mode_hint(&mut spans, hint, width, bg, app.theme.text_bright);
            Line::from(spans)
        }
        Mode::Confirm => {
            let message = app
                .confirm_state
                .as_ref()
                .map(|s| s.message.as_str())
                .unwrap_or("Confirm?");
            let mut spans = vec![
                Span::styled(" ", Style::default().bg(bg)),
                Span::styled(
                    message.to_string(),
                    Style::default()
                        .fg(Color::LightMagenta)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            if content_width < width {
                spans.push(Span::styled(
                    " ".repeat(width - content_width),
                    Style::default().bg(bg),
                ));
            }
            Line::from(spans)
        }
    };

    // In key debug mode, show the raw KeyEvent instead of the normal status row
    let line = if app.key_debug {
        let kitty_tag = if app.kitty_enabled {
            "kitty:on"
        } else {
            "kitty:off"
        };
        if let Some(ref event_str) = app.last_key_event {
            let mut spans = vec![
                Span::styled(
                    " KEY ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightYellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", Style::default().bg(bg)),
                Span::styled(
                    event_str.clone(),
                    Style::default().fg(Color::LightYellow).bg(bg),
                ),
            ];
            let right = format!("{}  ^D off", kitty_tag);
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let right_width = right.chars().count();
            if content_width + right_width + 2 < width {
                let padding = width - content_width - right_width;
                spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
                spans.push(Span::styled(
                    right,
                    Style::default().fg(app.theme.dim).bg(bg),
                ));
            }
            Line::from(spans)
        } else {
            let text = format!(" KEY DEBUG ON  {}  press any key...  ^D off", kitty_tag);
            let mut spans = vec![
                Span::styled(
                    " KEY DEBUG ON ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightYellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}  press any key...  ^D off", kitty_tag),
                    Style::default().fg(app.theme.dim).bg(bg),
                ),
            ];
            let content_width = text.chars().count();
            if content_width < width {
                spans.push(Span::styled(
                    " ".repeat(width - content_width),
                    Style::default().bg(bg),
                ));
            }
            Line::from(spans)
        }
    } else {
        line
    };

    let paragraph = Paragraph::new(line).style(Style::default().bg(bg));
    frame.render_widget(paragraph, area);
}

/// Build the right side of the status bar: optional message + spacer + key hints.
/// In Navigate mode, wrap messages take priority over match count.
/// In Search mode, only match count is shown (no wrap messages).
fn build_right_side<'a>(
    app: &App,
    spans: &mut Vec<Span<'a>>,
    hint: &'a str,
    width: usize,
    bg: Color,
    is_navigate: bool,
) {
    let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let hint_width = hint.chars().count();

    // Determine message text and style
    let message: Option<(String, Style)> = if is_navigate {
        if let Some(ref wrap_msg) = app.search_wrap_message {
            Some((
                wrap_msg.clone(),
                Style::default().fg(Color::LightMagenta).bg(bg),
            ))
        } else {
            match_count_message(app, bg)
        }
    } else {
        match_count_message(app, bg)
    };

    let spacer = 8;

    if let Some((ref msg_text, msg_style)) = message {
        let padded_msg = format!(" {} ", msg_text);
        let msg_width = padded_msg.chars().count();
        let right_width = msg_width + spacer + hint_width;
        if content_width + right_width < width {
            let padding = width - content_width - right_width;
            spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
            spans.push(Span::styled(padded_msg, msg_style));
            spans.push(Span::styled(" ".repeat(spacer), Style::default().bg(bg)));
            spans.push(Span::styled(
                hint,
                Style::default().fg(app.theme.text_bright).bg(bg),
            ));
            return;
        }
    }

    // Fallback: no message, just hints
    if content_width + hint_width < width {
        let padding = width - content_width - hint_width;
        spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
        spans.push(Span::styled(
            hint,
            Style::default().fg(app.theme.text_bright).bg(bg),
        ));
    }
}

/// Build the match count message with appropriate styling.
fn match_count_message(app: &App, bg: Color) -> Option<(String, Style)> {
    let count = app.search_match_count?;
    let text = if count == 1 {
        "1 match".to_string()
    } else {
        format!("{} matches", count)
    };
    let style = if count == 0 && app.search_zero_confirmed {
        Style::default()
            .fg(app.theme.text_bright)
            .bg(Color::Rgb(0x8D, 0x0B, 0x0B))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text_bright).bg(bg)
    };
    Some((text, style))
}

/// Append right-aligned hint text to a span list.
fn build_mode_hint<'a>(
    spans: &mut Vec<Span<'a>>,
    hint: &'a str,
    width: usize,
    bg: Color,
    text_bright: Color,
) {
    let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let hint_width = hint.chars().count();
    if content_width + hint_width < width {
        let padding = width - content_width - hint_width;
        spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
        spans.push(Span::styled(hint, Style::default().fg(text_bright).bg(bg)));
    }
}

/// Render a centered status message spanning the full width.
fn render_centered_message<'a>(
    msg: &str,
    width: usize,
    bg: Color,
    is_error: bool,
    text_bright: Color,
) -> Line<'a> {
    let msg_text = if is_error {
        format!(" {} ", msg)
    } else {
        msg.to_string()
    };
    let msg_len = msg_text.chars().count();
    let left_pad = width.saturating_sub(msg_len) / 2;
    let right_pad = width.saturating_sub(msg_len + left_pad);
    let msg_style = if is_error {
        Style::default()
            .fg(text_bright)
            .bg(Color::Rgb(0x8D, 0x0B, 0x0B))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::LightMagenta)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(" ".repeat(left_pad), Style::default().bg(bg)),
        Span::styled(msg_text, msg_style),
        Span::styled(" ".repeat(right_pad), Style::default().bg(bg)),
    ])
}
