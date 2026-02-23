use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, BoardColumn, BoardItem, BoardMode};
use crate::util::unicode;

use super::detail_view::state_flash_colors;
use super::push_highlighted_spans;

/// Render the board view: three-column kanban layout
pub fn render_board_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns = app.build_board_columns();
    let done_days = app.project.config.ui.board_done_days;
    let total_width = area.width as usize;

    // Determine layout mode based on width
    if total_width < 50 {
        app.board_state.visible_columns = 1;
        render_single_column(frame, app, area, &columns, done_days);
    } else if total_width < 77 || done_days == 0 {
        app.board_state.visible_columns = 2;
        render_two_columns(frame, app, area, &columns, done_days);
    } else {
        app.board_state.visible_columns = 3;
        render_three_columns(frame, app, area, &columns);
    }
}

/// Three-column layout (Ready | In Progress | Done)
fn render_three_columns(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    columns: &[Vec<BoardItem>; 3],
) {
    let col_areas = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .split(area);

    render_column(
        frame,
        app,
        col_areas[0],
        &columns[0],
        BoardColumn::Ready,
        None,
    );
    render_column(
        frame,
        app,
        col_areas[1],
        &columns[1],
        BoardColumn::InProgress,
        None,
    );
    render_column(
        frame,
        app,
        col_areas[2],
        &columns[2],
        BoardColumn::Done,
        None,
    );
}

/// Two-column layout (Ready | In Progress), Done hidden
fn render_two_columns(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    columns: &[Vec<BoardItem>; 3],
    done_days: u32,
) {
    let col_areas =
        Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).split(area);

    let done_count = columns[2]
        .iter()
        .filter(|item| matches!(item, BoardItem::Task { .. }))
        .count();
    let done_hint = if done_days > 0 && done_count > 0 {
        Some(format!("\u{2192} Done ({})", done_count))
    } else {
        None
    };

    render_column(
        frame,
        app,
        col_areas[0],
        &columns[0],
        BoardColumn::Ready,
        None,
    );
    render_column(
        frame,
        app,
        col_areas[1],
        &columns[1],
        BoardColumn::InProgress,
        done_hint.as_deref(),
    );
}

/// Single-column layout (show only focused column)
fn render_single_column(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    columns: &[Vec<BoardItem>; 3],
    done_days: u32,
) {
    let focus = app.board_state.focus_column;
    // Limit navigation: if done_days == 0, don't allow Done column focus
    if done_days == 0 && focus == BoardColumn::Done {
        app.board_state.focus_column = BoardColumn::InProgress;
    }
    let col_idx = app.board_state.focus_column.index();
    render_column(
        frame,
        app,
        area,
        &columns[col_idx],
        app.board_state.focus_column,
        None,
    );
}

/// Render a single board column into the given area
fn render_column(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    items: &[BoardItem],
    column: BoardColumn,
    header_suffix: Option<&str>,
) {
    let bg = app.theme.background;
    let is_focused = app.board_state.focus_column == column;
    let col_idx = column.index();
    let col_width = area.width as usize;

    // Count selectable tasks
    let task_count = items
        .iter()
        .filter(|item| matches!(item, BoardItem::Task { .. }))
        .count();

    // Mode label
    let mode_label = match app.board_state.mode {
        BoardMode::Cc => "cc",
        BoardMode::All => "all",
    };

    // Column header
    let (col_name, col_color) = match column {
        BoardColumn::Ready => (
            "Ready",
            app.theme.state_color(crate::model::TaskState::Todo),
        ),
        BoardColumn::InProgress => (
            "In Progress",
            app.theme.state_color(crate::model::TaskState::Active),
        ),
        BoardColumn::Done => ("Done", app.theme.state_color(crate::model::TaskState::Done)),
    };

    let header_bg = if is_focused {
        app.theme.selection_bg
    } else {
        bg
    };
    let header_style = if is_focused {
        Style::default()
            .fg(col_color)
            .bg(header_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(col_color).bg(header_bg)
    };

    let mut header_spans = vec![Span::styled(
        format!(" {} ({}) ", col_name, task_count),
        header_style,
    )];

    // Show mode on Ready column only
    if column == BoardColumn::Ready {
        header_spans.push(Span::styled(
            format!(" {} ", mode_label),
            Style::default().fg(app.theme.dim).bg(header_bg),
        ));
    }

    // Done hint for two-column mode
    if let Some(hint) = header_suffix {
        header_spans.push(Span::styled(
            format!(" {}", hint),
            Style::default().fg(app.theme.dim).bg(header_bg),
        ));
    }

    // Pad header to full width
    let header_used: usize = header_spans
        .iter()
        .map(|s| unicode::display_width(&s.content))
        .sum();
    if header_used < col_width {
        header_spans.push(Span::styled(
            " ".repeat(col_width - header_used),
            Style::default().bg(header_bg),
        ));
    }

    // Build all lines
    let mut lines: Vec<Line> = Vec::new();

    // Header line
    lines.push(Line::from(header_spans));

    // Separator
    let sep = "\u{2500}".repeat(col_width);
    lines.push(Line::from(Span::styled(
        sep,
        Style::default().fg(app.theme.dim).bg(bg),
    )));

    // Clamp cursor
    if !items.is_empty() {
        let cursor = app.board_state.cursor[col_idx].min(items.len().saturating_sub(1));
        app.board_state.cursor[col_idx] = cursor;
        // Ensure cursor is on a selectable item
        if matches!(items.get(cursor), Some(BoardItem::TrackHeader { .. })) {
            // Try to move to next task
            if let Some(next) = items[cursor..]
                .iter()
                .position(|item| matches!(item, BoardItem::Task { .. }))
            {
                app.board_state.cursor[col_idx] = cursor + next;
            }
        }
    } else {
        app.board_state.cursor[col_idx] = 0;
    }

    let cursor = app.board_state.cursor[col_idx];
    let search_re = app.active_search_re();

    // Body height (area minus header and separator)
    let body_height = area.height.saturating_sub(2) as usize;

    // Empty state
    if items.is_empty() {
        let cc_mode = app.board_state.mode == BoardMode::Cc;
        let msg = match column {
            BoardColumn::Ready => {
                if cc_mode {
                    "No #cc tasks ready \u{2014} press c for all"
                } else {
                    "No ready tasks"
                }
            }
            BoardColumn::InProgress => {
                if cc_mode {
                    "No #cc tasks active"
                } else {
                    "Nothing active"
                }
            }
            BoardColumn::Done => {
                if cc_mode {
                    "No #cc tasks completed recently"
                } else {
                    "No tasks completed recently"
                }
            }
        };
        lines.push(Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(app.theme.dim).bg(bg),
        )));
    } else {
        // Scroll adjustment
        let scroll = &mut app.board_state.scroll[col_idx];
        if cursor < *scroll {
            *scroll = cursor;
        } else if cursor >= *scroll + body_height {
            *scroll = cursor + 1 - body_height;
        }
        let scroll_val = *scroll;

        // Build card lines
        let mut card_lines: Vec<Line> = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            match item {
                BoardItem::TrackHeader { track_name } => {
                    // Dimmed track separator
                    let label = format!(" {}", track_name);
                    let truncated = super::truncate_with_ellipsis(&label, col_width);
                    card_lines.push(Line::from(Span::styled(
                        truncated,
                        Style::default().fg(app.theme.dim).bg(bg),
                    )));
                }
                BoardItem::Task {
                    id_display,
                    title,
                    tags,
                    task_id,
                    state,
                    ..
                } => {
                    let is_cursor = is_focused && idx == cursor;
                    let is_flash = app.is_flashing(task_id);
                    let (flash_bg, flash_border) = state_flash_colors(*state, &app.theme);
                    let row_bg = if is_flash {
                        flash_bg
                    } else if is_cursor {
                        app.theme.selection_bg
                    } else {
                        bg
                    };

                    // Determine ID color from track prefix
                    let prefix = id_display.split('-').next().unwrap_or("").to_lowercase();
                    let id_color = if !prefix.is_empty() {
                        app.theme.tag_color(&prefix)
                    } else {
                        app.theme.text
                    };

                    let card_text = format!("{} {}", id_display, title);

                    // Soft-wrap to column width, cap at 4 visual lines
                    let wrapped = soft_wrap(&card_text, col_width.saturating_sub(2));
                    let max_lines = 4;
                    let line_count = wrapped.len().min(max_lines);

                    for (li, wrap_line) in wrapped.iter().take(max_lines).enumerate() {
                        let mut spans: Vec<Span> = Vec::new();

                        // Left accent for flash/cursor row
                        if is_flash {
                            spans.push(Span::styled(
                                "\u{258E}",
                                Style::default().fg(flash_border).bg(row_bg),
                            ));
                        } else if is_cursor {
                            spans.push(Span::styled(
                                "\u{2502}",
                                Style::default().fg(app.theme.selection_border).bg(row_bg),
                            ));
                        } else {
                            spans.push(Span::styled(" ", Style::default().bg(row_bg)));
                        }

                        let base_style = if li == 0 && wrap_line.starts_with(id_display.as_str()) {
                            // First line: color the ID part differently
                            let id_len = id_display.len();
                            let id_part = &wrap_line[..id_len.min(wrap_line.len())];
                            let rest = if id_len < wrap_line.len() {
                                &wrap_line[id_len..]
                            } else {
                                ""
                            };

                            let id_style = Style::default().fg(id_color).bg(row_bg);
                            let text_style = Style::default().fg(app.theme.text).bg(row_bg);
                            let hl_style = Style::default()
                                .fg(app.theme.highlight)
                                .bg(row_bg)
                                .add_modifier(Modifier::BOLD);

                            push_highlighted_spans(
                                &mut spans,
                                id_part,
                                id_style,
                                hl_style,
                                search_re.as_ref(),
                            );
                            push_highlighted_spans(
                                &mut spans,
                                rest,
                                text_style,
                                hl_style,
                                search_re.as_ref(),
                            );

                            // Pad to width
                            let used: usize = spans
                                .iter()
                                .map(|s| unicode::display_width(&s.content))
                                .sum();
                            if used < col_width {
                                spans.push(Span::styled(
                                    " ".repeat(col_width - used),
                                    Style::default().bg(row_bg),
                                ));
                            }

                            card_lines.push(Line::from(spans));
                            continue;
                        } else {
                            Style::default().fg(app.theme.text).bg(row_bg)
                        };

                        let hl_style = Style::default()
                            .fg(app.theme.highlight)
                            .bg(row_bg)
                            .add_modifier(Modifier::BOLD);

                        let display_line = if li == max_lines - 1 && wrapped.len() > max_lines {
                            // Truncate last visible line with ellipsis
                            super::truncate_with_ellipsis(wrap_line, col_width.saturating_sub(2))
                        } else {
                            wrap_line.clone()
                        };

                        push_highlighted_spans(
                            &mut spans,
                            &display_line,
                            base_style,
                            hl_style,
                            search_re.as_ref(),
                        );

                        // Pad to width
                        let used: usize = spans
                            .iter()
                            .map(|s| unicode::display_width(&s.content))
                            .sum();
                        if used < col_width {
                            spans.push(Span::styled(
                                " ".repeat(col_width - used),
                                Style::default().bg(row_bg),
                            ));
                        }

                        card_lines.push(Line::from(spans));
                    }

                    // Show tags on cursor item (compact, after the card text)
                    if is_cursor && !tags.is_empty() {
                        let mut tag_spans: Vec<Span> = Vec::new();
                        tag_spans.push(Span::styled(" ", Style::default().bg(row_bg)));
                        for (ti, tag) in tags.iter().enumerate() {
                            if ti > 0 {
                                tag_spans.push(Span::styled(" ", Style::default().bg(row_bg)));
                            }
                            let tag_color = app.theme.tag_color(tag);
                            tag_spans.push(Span::styled(
                                format!("#{}", tag),
                                Style::default().fg(tag_color).bg(row_bg),
                            ));
                        }
                        let used: usize = tag_spans
                            .iter()
                            .map(|s| unicode::display_width(&s.content))
                            .sum();
                        if used < col_width {
                            tag_spans.push(Span::styled(
                                " ".repeat(col_width - used),
                                Style::default().bg(row_bg),
                            ));
                        }
                        card_lines.push(Line::from(tag_spans));
                    }

                    let _ = line_count; // used for max_lines cap above
                }
            }
        }

        // Apply scroll: skip `scroll_val` lines, take `body_height`
        for line in card_lines.into_iter().skip(scroll_val).take(body_height) {
            lines.push(line);
        }
    }

    // Pad remaining area with empty lines
    while lines.len() < area.height as usize {
        lines.push(Line::from(Span::styled(
            " ".repeat(col_width),
            Style::default().bg(bg),
        )));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(bg));
    frame.render_widget(paragraph, area);
}

/// Soft-wrap text into lines of at most `width` display cells.
fn soft_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = unicode::display_width(word);
        if current_width == 0 {
            // First word on line
            current = word.to_string();
            current_width = word_width;
        } else if current_width + 1 + word_width <= width {
            // Fits on current line
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            // Start new line
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
