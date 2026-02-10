use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ops::track_ops::task_counts;
use crate::tui::app::{App, EditTarget, Mode};

use super::push_highlighted_spans;

/// Width of each stat column (right-aligned numbers)
const COL_W: usize = 5;

/// Column headers (short names) and their markdown checkbox representations
const HEADERS: [&str; 5] = ["todo", "act", "blk", "done", "park"];
const CHECKBOXES: [&str; 5] = ["[ ]", "[>]", "[-]", "[x]", "[~]"];

/// Render the tracks overview as a grid with state columns
pub fn render_tracks_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let cursor = app.tracks_cursor;

    // Group tracks by state
    let mut active_tracks = Vec::new();
    let mut shelved_tracks = Vec::new();
    let mut archived_tracks = Vec::new();

    for tc in &app.project.config.tracks {
        match tc.state.as_str() {
            "active" => active_tracks.push(tc),
            "shelved" => shelved_tracks.push(tc),
            "archived" => archived_tracks.push(tc),
            _ => {}
        }
    }

    let cc_focus = app.project.config.agent.cc_focus.as_deref();
    let search_re = app.active_search_re();

    // Calculate name column width from longest track name
    let total_tracks = active_tracks.len() + shelved_tracks.len() + archived_tracks.len();
    let num_width = if total_tracks >= 10 { 2 } else { 1 };
    let computed_name_len = app
        .project
        .config
        .tracks
        .iter()
        .map(|tc| tc.name.chars().count())
        .max()
        .unwrap_or(10);
    // Columns can grow but not shrink within a single Tracks view session
    let max_name_len = computed_name_len.max(app.tracks_name_col_min);
    app.tracks_name_col_min = max_name_len;
    let max_id_len = app
        .project
        .config
        .tracks
        .iter()
        .map(|tc| {
            app.project
                .config
                .ids
                .prefixes
                .get(&tc.id)
                .map(|p| p.chars().count())
                .unwrap_or_else(|| tc.id.chars().count())
        })
        .max()
        .unwrap_or(2);

    // name_col = total width before stat columns in data rows:
    // border(1) + num + "  " + name + "  " + id
    let name_col = 1 + num_width + 2 + max_name_len + 2 + max_id_len;

    // Project name header
    lines.push(Line::from(Span::styled(
        format!(" Project: {}", app.project.config.project.name),
        Style::default()
            .fg(app.theme.text)
            .bg(app.theme.background)
            .add_modifier(Modifier::BOLD),
    )));

    // Top header: short state names aligned to stat columns
    lines.push(render_col_names(app, name_col, max_id_len));

    let mut flat_idx = 0usize;

    // Detect inline edit state
    let editing_track_id = match (&app.mode, &app.edit_target) {
        (Mode::Edit, Some(EditTarget::ExistingTrackName { track_id, .. })) => {
            Some(track_id.clone())
        }
        _ => None,
    };
    let is_new_track_edit = matches!(
        (&app.mode, &app.edit_target),
        (Mode::Edit, Some(EditTarget::NewTrackName))
    );
    let editing_prefix_track_id = match (&app.mode, &app.edit_target) {
        (Mode::Edit, Some(EditTarget::ExistingPrefix { track_id, .. })) => Some(track_id.clone()),
        _ => None,
    };

    // Active section
    if !active_tracks.is_empty() || is_new_track_edit {
        lines.push(render_section_row(app, "Active", name_col, false));
        for (track_i, tc) in active_tracks.iter().enumerate() {
            // Insert new-track edit row before this track when cursor == track_i
            if is_new_track_edit && flat_idx == cursor && track_i == cursor {
                lines.push(render_new_track_edit_row(
                    app,
                    flat_idx + 1,
                    num_width,
                    area.width,
                ));
                flat_idx += 1;
            }
            let is_cursor = flat_idx == cursor;
            let is_flash = app.is_track_flashing(&tc.id);
            if editing_track_id.as_deref() == Some(&tc.id) && is_cursor {
                lines.push(render_edit_row(
                    app,
                    tc,
                    flat_idx + 1,
                    num_width,
                    max_name_len,
                    max_id_len,
                    cc_focus,
                    area.width,
                ));
            } else {
                lines.push(render_track_row(
                    app,
                    tc,
                    flat_idx + 1,
                    num_width,
                    max_name_len,
                    max_id_len,
                    is_cursor,
                    is_flash,
                    cc_focus,
                    area.width,
                    search_re.as_ref(),
                ));
            }
            // Prefix edit row below the track
            if editing_prefix_track_id.as_deref() == Some(&tc.id) && is_cursor {
                lines.push(render_prefix_edit_row(app, num_width, area.width));
            }
            flat_idx += 1;
        }
        // New track edit row at end of active section (cursor == active_count)
        if is_new_track_edit && flat_idx == cursor {
            lines.push(render_new_track_edit_row(
                app,
                flat_idx + 1,
                num_width,
                area.width,
            ));
            flat_idx += 1;
        }
        lines.push(Line::from(""));
    }

    // Shelved section
    if !shelved_tracks.is_empty() {
        lines.push(render_section_row(app, "Shelved", name_col, false));
        for tc in &shelved_tracks {
            let is_cursor = flat_idx == cursor;
            let is_flash = app.is_track_flashing(&tc.id);
            if editing_track_id.as_deref() == Some(&tc.id) && is_cursor {
                lines.push(render_edit_row(
                    app,
                    tc,
                    flat_idx + 1,
                    num_width,
                    max_name_len,
                    max_id_len,
                    cc_focus,
                    area.width,
                ));
            } else {
                lines.push(render_track_row(
                    app,
                    tc,
                    flat_idx + 1,
                    num_width,
                    max_name_len,
                    max_id_len,
                    is_cursor,
                    is_flash,
                    cc_focus,
                    area.width,
                    search_re.as_ref(),
                ));
            }
            if editing_prefix_track_id.as_deref() == Some(&tc.id) && is_cursor {
                lines.push(render_prefix_edit_row(app, num_width, area.width));
            }
            flat_idx += 1;
        }
        lines.push(Line::from(""));
    }

    // Archived section
    if !archived_tracks.is_empty() {
        lines.push(render_section_row(app, "Archived", name_col, true));
        for tc in &archived_tracks {
            let is_cursor = flat_idx == cursor;
            let is_flash = app.is_track_flashing(&tc.id);
            lines.push(render_track_row(
                app,
                tc,
                flat_idx + 1,
                num_width,
                max_name_len,
                max_id_len,
                is_cursor,
                is_flash,
                cc_focus,
                area.width,
                search_re.as_ref(),
            ));
            flat_idx += 1;
        }
    }

    if flat_idx == 0 {
        lines.clear();
        let bg = app.theme.background;
        lines.push(Line::from(vec![
            Span::styled(" No tracks — press ", Style::default().fg(app.theme.text).bg(bg)),
            Span::styled("a", Style::default().fg(app.theme.highlight).bg(bg)),
            Span::styled(" to create one", Style::default().fg(app.theme.text).bg(bg)),
        ]));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
}

/// Render the top header line with short state names
fn render_col_names<'a>(app: &'a App, name_col: usize, max_id_len: usize) -> Line<'a> {
    let bg = app.theme.background;
    let header_style = Style::default().fg(app.theme.text).bg(bg);
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);

    let mut spans: Vec<Span> = Vec::new();

    // Pad to align "id" header with ID column in data rows
    let pre_id_col = name_col - max_id_len;
    spans.push(Span::styled(
        " ".repeat(pre_id_col),
        Style::default().bg(bg),
    ));

    // "pfx" header aligned to prefix column
    spans.push(Span::styled(
        format!("{:<width$}", "pfx", width = max_id_len),
        dim_style,
    ));

    for header in &HEADERS {
        spans.push(Span::styled(
            format!("{:>width$}", header, width = COL_W),
            header_style,
        ));
    }

    Line::from(spans)
}

/// Render a section header with name + markdown checkbox sub-headers
fn render_section_row<'a>(
    app: &'a App,
    label: &'static str,
    name_col: usize,
    is_dim: bool,
) -> Line<'a> {
    let bg = app.theme.background;
    let label_color = if is_dim {
        app.theme.dim
    } else {
        app.theme.text
    };
    let label_style = Style::default()
        .fg(label_color)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let cb_style = Style::default().fg(app.theme.text).bg(bg);

    let mut spans: Vec<Span> = Vec::new();

    // " Label" left-aligned, then pad to name_col
    let label_text = format!(" {}", label);
    let label_len = label_text.chars().count();
    spans.push(Span::styled(label_text, label_style));

    if label_len < name_col {
        spans.push(Span::styled(
            " ".repeat(name_col - label_len),
            Style::default().bg(bg),
        ));
    }

    // Checkbox representations aligned to stat columns
    for cb in &CHECKBOXES {
        spans.push(Span::styled(
            format!("{:>width$}", cb, width = COL_W),
            cb_style,
        ));
    }

    Line::from(spans)
}

/// Render a single track data row with stat counts in columns
#[allow(clippy::too_many_arguments)]
fn render_track_row<'a>(
    app: &'a App,
    tc: &crate::model::TrackConfig,
    number: usize,
    num_width: usize,
    max_name_len: usize,
    max_id_len: usize,
    is_cursor: bool,
    is_flash: bool,
    cc_focus: Option<&str>,
    width: u16,
    search_re: Option<&regex::Regex>,
) -> Line<'a> {
    let bg = if is_flash {
        app.theme.flash_bg
    } else if is_cursor {
        app.theme.selection_bg
    } else {
        app.theme.background
    };

    // Get stats for this track
    let stats = app
        .project
        .tracks
        .iter()
        .find(|(id, _)| id == &tc.id)
        .map(|(_, track)| task_counts(track))
        .unwrap_or_default();

    let mut spans: Vec<Span> = Vec::new();

    // Column 0: cursor/flash border
    if is_cursor || is_flash {
        let border_color = if is_flash {
            app.theme.yellow
        } else {
            app.theme.selection_border
        };
        spans.push(Span::styled(
            "\u{258E}",
            Style::default().fg(border_color).bg(bg),
        ));
    } else {
        spans.push(Span::styled(" ", Style::default().bg(app.theme.background)));
    }

    // Number (right-aligned in num_width)
    let num_str = format!("{:>width$}", number, width = num_width);
    spans.push(Span::styled(
        num_str,
        Style::default().fg(app.theme.dim).bg(bg),
    ));
    spans.push(Span::styled("  ", Style::default().bg(bg)));

    // Track name (with search highlighting, padded to max_name_len)
    let name_style = Style::default().fg(app.theme.text_bright).bg(bg);
    let hl_style = Style::default()
        .fg(app.theme.search_match_fg)
        .bg(app.theme.search_match_bg)
        .add_modifier(Modifier::BOLD);
    push_highlighted_spans(&mut spans, &tc.name, name_style, hl_style, search_re);

    // Pad name to max_name_len
    let name_len = tc.name.chars().count();
    if name_len < max_name_len {
        spans.push(Span::styled(
            " ".repeat(max_name_len - name_len),
            Style::default().bg(bg),
        ));
    }

    // ID prefix column (gap + uppercase prefix padded to max_id_len)
    spans.push(Span::styled("  ", Style::default().bg(bg)));
    let id_style = Style::default().fg(app.theme.text).bg(bg);
    let prefix = app
        .project
        .config
        .ids
        .prefixes
        .get(&tc.id)
        .map(|p| p.to_uppercase())
        .unwrap_or_else(|| tc.id.to_uppercase());
    let id_text = format!("{:<width$}", prefix, width = max_id_len);
    push_highlighted_spans(&mut spans, &id_text, id_style, hl_style, search_re);

    // Stat columns: todo, active, blocked, done, parked
    let counts = [
        stats.todo,
        stats.active,
        stats.blocked,
        stats.done,
        stats.parked,
    ];
    let colors = [
        app.theme.text,      // todo
        app.theme.highlight, // active
        app.theme.red,       // blocked
        app.theme.text,      // done
        app.theme.yellow,    // parked
    ];

    for (count, color) in counts.iter().zip(colors.iter()) {
        let style = Style::default().fg(*color).bg(bg);
        spans.push(Span::styled(
            format!("{:>width$}", count, width = COL_W),
            style,
        ));
    }

    // cc-focus indicator
    if cc_focus == Some(tc.id.as_str()) {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(
            "\u{2605} cc",
            Style::default().fg(app.theme.purple).bg(bg),
        ));
    }

    // Pad to full width for cursor/flash highlight
    if is_cursor || is_flash {
        let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let w = width as usize;
        if content_width < w {
            spans.push(Span::styled(
                " ".repeat(w - content_width),
                Style::default().bg(bg),
            ));
        }
    }

    Line::from(spans)
}

/// Render an inline edit row for existing track name editing (preserves column layout)
#[allow(clippy::too_many_arguments)]
fn render_edit_row<'a>(
    app: &'a App,
    tc: &crate::model::TrackConfig,
    number: usize,
    num_width: usize,
    max_name_len: usize,
    max_id_len: usize,
    cc_focus: Option<&str>,
    width: u16,
) -> Line<'a> {
    let bg = app.theme.selection_bg;
    let mut spans: Vec<Span> = Vec::new();

    // Selection border
    spans.push(Span::styled(
        "\u{258E}",
        Style::default().fg(app.theme.selection_border).bg(bg),
    ));

    // Number
    let num_str = format!("{:>width$}", number, width = num_width);
    spans.push(Span::styled(
        num_str,
        Style::default().fg(app.theme.dim).bg(bg),
    ));
    spans.push(Span::styled("  ", Style::default().bg(bg)));

    // Edit buffer with cursor (occupies the name column)
    let cursor_pos = app.edit_cursor;
    let buffer = &app.edit_buffer;
    let text_style = Style::default().fg(app.theme.text_bright).bg(bg);
    let cursor_style = Style::default().fg(app.theme.highlight).bg(bg);
    let buf_char_len;

    if cursor_pos < buffer.len() {
        let (before, rest) = buffer.split_at(cursor_pos);
        let mut chars = rest.chars();
        let cursor_char = chars.next().unwrap_or(' ');
        let after: String = chars.collect();
        buf_char_len = buffer.chars().count();

        spans.push(Span::styled(before.to_string(), text_style));
        spans.push(Span::styled(
            cursor_char.to_string(),
            Style::default()
                .fg(app.theme.background)
                .bg(app.theme.highlight)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(after, text_style));
    } else {
        buf_char_len = buffer.chars().count() + 1; // +1 for cursor block
        spans.push(Span::styled(buffer.to_string(), text_style));
        spans.push(Span::styled("\u{258C}", cursor_style));
    }

    // Pad name area to max_name_len so ID + stat columns stay aligned
    if buf_char_len < max_name_len {
        spans.push(Span::styled(
            " ".repeat(max_name_len - buf_char_len),
            Style::default().bg(bg),
        ));
    }

    // ID prefix column
    spans.push(Span::styled("  ", Style::default().bg(bg)));
    let id_style = Style::default().fg(app.theme.text).bg(bg);
    let prefix = app
        .project
        .config
        .ids
        .prefixes
        .get(&tc.id)
        .map(|p| p.to_uppercase())
        .unwrap_or_else(|| tc.id.to_uppercase());
    spans.push(Span::styled(
        format!("{:<width$}", prefix, width = max_id_len),
        id_style,
    ));

    // Stat columns
    let stats = app
        .project
        .tracks
        .iter()
        .find(|(id, _)| id == &tc.id)
        .map(|(_, track)| task_counts(track))
        .unwrap_or_default();

    let counts = [
        stats.todo,
        stats.active,
        stats.blocked,
        stats.done,
        stats.parked,
    ];
    let colors = [
        app.theme.text,
        app.theme.highlight,
        app.theme.red,
        app.theme.text,
        app.theme.yellow,
    ];

    for (count, color) in counts.iter().zip(colors.iter()) {
        let style = Style::default().fg(*color).bg(bg);
        spans.push(Span::styled(
            format!("{:>width$}", count, width = COL_W),
            style,
        ));
    }

    // cc-focus indicator
    if cc_focus == Some(tc.id.as_str()) {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(
            "\u{2605} cc",
            Style::default().fg(app.theme.purple).bg(bg),
        ));
    }

    // Pad to full width
    let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let w = width as usize;
    if content_width < w {
        spans.push(Span::styled(
            " ".repeat(w - content_width),
            Style::default().bg(bg),
        ));
    }

    Line::from(spans)
}

/// Render an inline edit row for new track name entry
fn render_new_track_edit_row<'a>(
    app: &'a App,
    number: usize,
    num_width: usize,
    width: u16,
) -> Line<'a> {
    let bg = app.theme.selection_bg;
    let mut spans: Vec<Span> = Vec::new();

    // Selection border
    spans.push(Span::styled(
        "\u{258E}",
        Style::default().fg(app.theme.selection_border).bg(bg),
    ));

    // Number
    let num_str = format!("{:>width$}", number, width = num_width);
    spans.push(Span::styled(
        num_str,
        Style::default().fg(app.theme.dim).bg(bg),
    ));
    spans.push(Span::styled("  ", Style::default().bg(bg)));

    // Edit buffer with cursor
    let cursor_pos = app.edit_cursor;
    let buffer = &app.edit_buffer;
    let text_style = Style::default().fg(app.theme.text_bright).bg(bg);
    let cursor_style = Style::default().fg(app.theme.highlight).bg(bg);

    if cursor_pos < buffer.len() {
        let (before, rest) = buffer.split_at(cursor_pos);
        let mut chars = rest.chars();
        let cursor_char = chars.next().unwrap_or(' ');
        let after: String = chars.collect();

        spans.push(Span::styled(before.to_string(), text_style));
        spans.push(Span::styled(
            cursor_char.to_string(),
            Style::default()
                .fg(app.theme.background)
                .bg(app.theme.highlight)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(after, text_style));
    } else {
        spans.push(Span::styled(buffer.to_string(), text_style));
        spans.push(Span::styled("\u{258C}", cursor_style));
    }

    // Pad to full width
    let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let w = width as usize;
    if content_width < w {
        spans.push(Span::styled(
            " ".repeat(w - content_width),
            Style::default().bg(bg),
        ));
    }

    Line::from(spans)
}

/// Render the inline prefix edit row below a track (shown when P is pressed)
fn render_prefix_edit_row<'a>(app: &'a App, num_width: usize, width: u16) -> Line<'a> {
    let bg = app.theme.selection_bg;
    let mut spans: Vec<Span> = Vec::new();

    // Indent to align with name column: border(1) + num + "  " + "Prefix: "
    let indent = 1 + num_width + 2;
    spans.push(Span::styled(" ".repeat(indent), Style::default().bg(bg)));
    spans.push(Span::styled(
        "Prefix: ",
        Style::default().fg(app.theme.dim).bg(bg),
    ));

    // Edit buffer with cursor and selection
    let cursor_pos = app.edit_cursor;
    let buffer = &app.edit_buffer;
    let text_style = Style::default()
        .fg(app.theme.text_bright)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let cursor_style = Style::default()
        .fg(app.theme.background)
        .bg(app.theme.highlight)
        .add_modifier(Modifier::BOLD);

    let sel_range = app.edit_selection_range();

    if let Some((sel_start, sel_end)) = sel_range {
        // Render with selection highlight
        let selection_style = Style::default()
            .fg(app.theme.text_bright)
            .bg(app.theme.highlight);
        let before_sel = &buffer[..sel_start];
        let selected = &buffer[sel_start..sel_end];
        let after_sel = &buffer[sel_end..];

        if !before_sel.is_empty() {
            spans.push(Span::styled(before_sel.to_string(), text_style));
        }
        spans.push(Span::styled(selected.to_string(), selection_style));
        if !after_sel.is_empty() {
            spans.push(Span::styled(after_sel.to_string(), text_style));
        }
        // Cursor at end
        if cursor_pos >= buffer.len() {
            spans.push(Span::styled(
                "\u{258C}",
                Style::default().fg(app.theme.highlight).bg(bg),
            ));
        }
    } else if cursor_pos < buffer.len() {
        let (before, rest) = buffer.split_at(cursor_pos);
        let mut chars = rest.chars();
        let cursor_char = chars.next().unwrap_or(' ');
        let after: String = chars.collect();

        spans.push(Span::styled(before.to_string(), text_style));
        spans.push(Span::styled(cursor_char.to_string(), cursor_style));
        spans.push(Span::styled(after, text_style));
    } else {
        spans.push(Span::styled(buffer.to_string(), text_style));
        spans.push(Span::styled(
            "\u{258C}",
            Style::default().fg(app.theme.highlight).bg(bg),
        ));
    }

    // Validation error (dim red text after the editor)
    if let Some(ref pr) = app.prefix_rename
        && !pr.validation_error.is_empty()
    {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(
            pr.validation_error.clone(),
            Style::default().fg(app.theme.red).bg(bg),
        ));
    }

    // Pad to full width
    let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let w = width as usize;
    if content_width < w {
        spans.push(Span::styled(
            " ".repeat(w - content_width),
            Style::default().bg(bg),
        ));
    }

    Line::from(spans)
}
