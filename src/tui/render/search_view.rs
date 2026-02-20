use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ops::search::MatchField;
use crate::tui::app::{App, SearchResultKind};
use crate::util::unicode;

use super::push_highlighted_spans;

/// Render the project search results view
pub fn render_search_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let sr = match &app.project_search_results {
        Some(sr) => sr,
        None => {
            let empty = Paragraph::new(" No search results")
                .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
            frame.render_widget(empty, area);
            return;
        }
    };

    if sr.items.is_empty() {
        let msg = format!(" No matches for \"{}\"", sr.query);
        let empty =
            Paragraph::new(msg).style(Style::default().fg(app.theme.dim).bg(app.theme.background));
        frame.render_widget(empty, area);
        return;
    }

    let cursor = sr.cursor;
    let visible_height = area.height as usize;
    let search_re = Some(&sr.regex);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line: Option<usize> = None;
    let mut current_group_idx = 0;

    let highlight_style = Style::default()
        .fg(app.theme.highlight)
        .bg(app.theme.background)
        .add_modifier(Modifier::BOLD);

    for (item_idx, item) in sr.items.iter().enumerate() {
        // Insert group header if this item starts a new group
        while current_group_idx < sr.groups.len() && sr.groups[current_group_idx].0 == item_idx {
            let (_, ref label, count) = sr.groups[current_group_idx];
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            let header_text = format!(
                " \u{2500}\u{2500} {} \u{2500}\u{2500}  {} match{}",
                label,
                count,
                if count == 1 { "" } else { "es" }
            );
            lines.push(Line::from(Span::styled(
                header_text,
                Style::default()
                    .fg(app.theme.text)
                    .bg(app.theme.background)
                    .add_modifier(Modifier::BOLD),
            )));
            current_group_idx += 1;
        }

        let is_cursor = item_idx == cursor;
        if is_cursor {
            cursor_line = Some(lines.len());
        }
        let bg = if is_cursor {
            app.theme.selection_bg
        } else {
            app.theme.background
        };
        let is_archive = matches!(item.kind, SearchResultKind::Archive { .. });

        let mut spans: Vec<Span> = Vec::new();

        // Cursor indicator
        if is_cursor {
            spans.push(Span::styled(
                "\u{258E}",
                Style::default()
                    .fg(app.theme.selection_border)
                    .bg(app.theme.selection_bg),
            ));
        } else {
            spans.push(Span::styled(" ", Style::default().bg(bg)));
        }

        // State checkbox (for tasks) or bullet (for inbox)
        let is_inbox = matches!(item.kind, SearchResultKind::Inbox { .. });
        if is_inbox {
            spans.push(Span::styled(
                " \u{2022} ",
                Style::default().fg(app.theme.dim).bg(bg),
            ));
        } else if let Some(state) = &item.state {
            let state_char = match state {
                crate::model::TaskState::Todo => "[ ]",
                crate::model::TaskState::Active => "[>]",
                crate::model::TaskState::Done => "[x]",
                crate::model::TaskState::Blocked => "[b]",
                crate::model::TaskState::Parked => "[~]",
            };
            let color = app.theme.state_color(*state);
            let style = if is_archive {
                Style::default().fg(app.theme.dim).bg(bg)
            } else {
                Style::default().fg(color).bg(bg)
            };
            spans.push(Span::styled(format!(" {} ", state_char), style));
        } else {
            spans.push(Span::styled("     ", Style::default().bg(bg)));
        }

        // Task ID — with search highlighting (issue #6)
        if !item.task_id.is_empty() {
            let id_style = if is_archive {
                Style::default().fg(app.theme.dim).bg(bg)
            } else {
                Style::default().fg(app.theme.purple).bg(bg)
            };
            let id_highlight = Style::default()
                .fg(app.theme.highlight)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            // Pad ID to 12 chars for alignment
            let id_display = format!("{:<12}", item.task_id);
            push_highlighted_spans(&mut spans, &id_display, id_style, id_highlight, search_re);
        } else {
            spans.push(Span::styled("            ", Style::default().bg(bg)));
        }

        // Title with search highlighting
        let title_style = if is_archive {
            Style::default().fg(app.theme.dim).bg(bg)
        } else {
            Style::default().fg(app.theme.text_bright).bg(bg)
        };
        let title_highlight = Style::default()
            .fg(app.theme.highlight)
            .bg(bg)
            .add_modifier(Modifier::BOLD);
        push_highlighted_spans(
            &mut spans,
            &item.title,
            title_style,
            title_highlight,
            search_re,
        );

        // Tags
        if !item.tags.is_empty() {
            spans.push(Span::styled(" ", Style::default().bg(bg)));
            for tag in &item.tags {
                let tag_color = if is_archive {
                    app.theme.dim
                } else {
                    app.theme.tag_color(tag)
                };
                spans.push(Span::styled(
                    format!("#{} ", tag),
                    Style::default().fg(tag_color).bg(bg),
                ));
            }
        }

        // Pad line to full width
        let line_width: usize = spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        let total_width = area.width as usize;
        if line_width < total_width {
            spans.push(Span::styled(
                " ".repeat(total_width - line_width),
                Style::default().bg(bg),
            ));
        }

        lines.push(Line::from(spans));

        // Annotation lines for non-title/non-ID matching fields
        let max_annotations = 3;
        let total_annotations = item.annotations.len();
        let show_count = total_annotations.min(max_annotations);

        for annotation in item.annotations.iter().take(show_count) {
            let indent = "                 ";
            let label = field_label_str(&annotation.field);
            let mut ctx_spans: Vec<Span> = Vec::new();
            ctx_spans.push(Span::styled(
                format!("{}{}: ", indent, label),
                Style::default().fg(app.theme.dim).bg(app.theme.background),
            ));
            let snippet_style = Style::default().fg(app.theme.text).bg(app.theme.background);
            push_highlighted_spans(
                &mut ctx_spans,
                &annotation.snippet,
                snippet_style,
                highlight_style,
                search_re,
            );
            lines.push(Line::from(ctx_spans));
        }

        if total_annotations > max_annotations {
            let extra = total_annotations - max_annotations;
            let indent = "                 ";
            lines.push(Line::from(Span::styled(
                format!(
                    "{}+{} more field{}",
                    indent,
                    extra,
                    if extra == 1 { "" } else { "s" }
                ),
                Style::default().fg(app.theme.dim).bg(app.theme.background),
            )));
        }
    }

    // Auto-scroll to keep cursor visible
    let scroll_offset = if let Some(sr) = &mut app.project_search_results {
        if let Some(cl) = cursor_line {
            if cl < sr.scroll_offset {
                sr.scroll_offset = cl;
            } else if cl >= sr.scroll_offset + visible_height {
                sr.scroll_offset = cl + 1 - visible_height;
            }
        }
        sr.scroll_offset
    } else {
        0
    };

    // Render visible lines
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll_offset)
        .take(visible_height)
        .collect();

    let remaining = visible_height.saturating_sub(visible_lines.len());
    let mut all_lines = visible_lines;
    for _ in 0..remaining {
        all_lines.push(Line::from(Span::styled(
            " ".repeat(area.width as usize),
            Style::default().bg(app.theme.background),
        )));
    }

    let paragraph = Paragraph::new(all_lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
}

fn field_label_str(field: &MatchField) -> &'static str {
    match field {
        MatchField::Id => "id",
        MatchField::Title => "title",
        MatchField::Tag => "tag",
        MatchField::Note => "note",
        MatchField::Dep => "dep",
        MatchField::Ref => "ref",
        MatchField::Spec => "spec",
        MatchField::Body => "body",
    }
}
