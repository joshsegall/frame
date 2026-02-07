use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, Mode};

/// Render the status row (bottom of screen)
pub fn render_status_row(frame: &mut Frame, app: &App, area: Rect) {
    let bg = app.theme.background;
    let width = area.width as usize;

    let line = match &app.mode {
        Mode::Navigate if app.status_message.is_some() => {
            render_centered_message(app.status_message.as_deref().unwrap(), width, bg)
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
            let mode_label = Span::styled(
                "-- EDIT --",
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let hint = "Enter confirm  Esc cancel";
            let mut spans = vec![Span::styled(" ", Style::default().bg(bg)), mode_label];
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let hint_width = hint.chars().count();
            if content_width + hint_width < width {
                let padding = width - content_width - hint_width;
                spans.push(Span::styled(
                    " ".repeat(padding),
                    Style::default().bg(bg),
                ));
                spans.push(Span::styled(
                    hint,
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ));
            }
            Line::from(spans)
        }
        Mode::Move => {
            let mode_label = Span::styled(
                "-- MOVE --",
                Style::default()
                    .fg(app.theme.highlight)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            let hint = "\u{2191}\u{2193} move  Enter \u{2713}  Esc \u{2717}";
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
            let mode_label = Span::styled(
                "-- TRIAGE --",
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
        Mode::Confirm => {
            let message = app.confirm_state.as_ref().map(|s| s.message.as_str()).unwrap_or("Confirm?");
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
            spans.push(Span::styled(
                " ".repeat(padding),
                Style::default().bg(bg),
            ));
            spans.push(Span::styled(padded_msg, msg_style));
            spans.push(Span::styled(
                " ".repeat(spacer),
                Style::default().bg(bg),
            ));
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
        spans.push(Span::styled(
            " ".repeat(padding),
            Style::default().bg(bg),
        ));
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
        spans.push(Span::styled(
            " ".repeat(padding),
            Style::default().bg(bg),
        ));
        spans.push(Span::styled(
            hint,
            Style::default().fg(text_bright).bg(bg),
        ));
    }
}

/// Render a centered status message spanning the full width.
fn render_centered_message<'a>(msg: &str, width: usize, bg: Color) -> Line<'a> {
    let msg_len = msg.chars().count();
    let left_pad = width.saturating_sub(msg_len) / 2;
    let right_pad = width.saturating_sub(msg_len + left_pad);
    Line::from(vec![
        Span::styled(" ".repeat(left_pad), Style::default().bg(bg)),
        Span::styled(
            msg.to_string(),
            Style::default()
                .fg(Color::LightMagenta)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(right_pad), Style::default().bg(bg)),
    ])
}
