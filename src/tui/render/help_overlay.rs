use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::{App, View};

/// A single entry in a help column
enum HelpEntry {
    Header(String),
    Binding(String, String),
    Blank,
}

/// Render the help overlay (toggled with ?)
pub fn render_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let highlight = app.theme.highlight;
    let dim = app.theme.dim;

    let key_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(text_color).bg(bg);
    let header_style = Style::default()
        .fg(bright)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let blank_style = Style::default().bg(bg);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(" Key Bindings", header_style)));
    lines.push(Line::from(""));

    // Build left and right column entries based on view
    let (left, right) = build_columns(&app.view);

    // Merge columns into lines
    let max_rows = left.len().max(right.len());
    let key_w = 12usize;
    let desc_w = 22usize;
    let col_w = key_w + desc_w; // total width per column
    let gap = 3usize;

    for row in 0..max_rows {
        let mut spans: Vec<Span> = Vec::new();

        // Left column
        if row < left.len() {
            render_entry(&left[row], &mut spans, key_w, col_w, key_style, desc_style, header_style, blank_style);
        } else {
            spans.push(Span::styled(" ".repeat(col_w), blank_style));
        }

        // Gap
        spans.push(Span::styled(" ".repeat(gap), blank_style));

        // Right column
        if row < right.len() {
            render_entry(&right[row], &mut spans, key_w, col_w, key_style, desc_style, header_style, blank_style);
        }

        lines.push(Line::from(spans));
    }

    // Fixed width from content: 2 columns + gap + borders
    let popup_w = ((col_w * 2 + gap) as u16 + 2).min(area.width.saturating_sub(2));
    // Dynamic height from content + borders
    let popup_h = ((lines.len() as u16) + 2).min(area.height.saturating_sub(2));
    let overlay_area = centered_rect_fixed(popup_w, popup_h, area);

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, overlay_area);
}

fn render_entry(
    entry: &HelpEntry,
    spans: &mut Vec<Span<'_>>,
    key_w: usize,
    col_w: usize,
    key_style: Style,
    desc_style: Style,
    header_style: Style,
    blank_style: Style,
) {
    match entry {
        HelpEntry::Header(text) => {
            let padded = format!(" {:<width$}", text, width = col_w - 1);
            spans.push(Span::styled(padded, header_style));
        }
        HelpEntry::Binding(key, desc) => {
            let padded_key = format!(" {:<width$}", key, width = key_w - 1);
            let padded_desc = format!("{:<width$}", desc, width = col_w - key_w);
            spans.push(Span::styled(padded_key, key_style));
            spans.push(Span::styled(padded_desc, desc_style));
        }
        HelpEntry::Blank => {
            spans.push(Span::styled(" ".repeat(col_w), blank_style));
        }
    }
}

fn build_columns(view: &View) -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    match view {
        View::Track(_) => build_track_columns(),
        View::Tracks => build_tracks_columns(),
        View::Inbox => build_inbox_columns(),
        View::Recent => build_recent_columns(),
    }
}

fn build_track_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{2191}\u{2193}/jk".into(), "Move up/down".into()),
        HelpEntry::Binding("\u{2190}/h".into(), "Collapse / parent".into()),
        HelpEntry::Binding("\u{2192}/l".into(), "Expand / child".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("Esc".into(), "Back / close".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Task State".into()),
        HelpEntry::Binding("Space".into(), "Cycle state".into()),
        HelpEntry::Binding("x".into(), "Mark done".into()),
        HelpEntry::Binding("b".into(), "Toggle blocked".into()),
        HelpEntry::Binding("~".into(), "Toggle parked".into()),
        HelpEntry::Binding("c".into(), "Toggle cc tag".into()),
    ];

    let right = vec![
        HelpEntry::Header("Edit".into()),
        HelpEntry::Binding("e".into(), "Edit title".into()),
        HelpEntry::Binding("a".into(), "Add task (bottom)".into()),
        HelpEntry::Binding("o/-".into(), "Insert after cursor".into()),
        HelpEntry::Binding("p".into(), "Push to top".into()),
        HelpEntry::Binding("A".into(), "Add subtask".into()),
        HelpEntry::Binding("m".into(), "Move mode".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Views & Other".into()),
        HelpEntry::Binding("1-9".into(), "Track N".into()),
        HelpEntry::Binding("Tab".into(), "Next track".into()),
        HelpEntry::Binding("0/`".into(), "Tracks overview".into()),
        HelpEntry::Binding("i".into(), "Inbox".into()),
        HelpEntry::Binding("r".into(), "Recent".into()),
        HelpEntry::Binding("/".into(), "Search".into()),
        HelpEntry::Binding("C".into(), "Set cc-focus".into()),
        HelpEntry::Binding("z/u".into(), "Undo".into()),
        HelpEntry::Binding("Z".into(), "Redo".into()),
        HelpEntry::Binding("?".into(), "Help".into()),
        HelpEntry::Binding("QQ".into(), "Quit".into()),
    ];

    (left, right)
}

fn build_tracks_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{2191}\u{2193}/jk".into(), "Move cursor".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("1-9".into(), "Switch to track N".into()),
        HelpEntry::Binding("m".into(), "Reorder track".into()),
        HelpEntry::Binding("C".into(), "Set cc-focus".into()),
    ];

    let right = vec![
        HelpEntry::Header("Views & Other".into()),
        HelpEntry::Binding("Tab".into(), "Next view".into()),
        HelpEntry::Binding("/".into(), "Search".into()),
        HelpEntry::Binding("?".into(), "Help".into()),
        HelpEntry::Binding("QQ".into(), "Quit".into()),
    ];

    (left, right)
}

fn build_inbox_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{2191}\u{2193}/jk".into(), "Move cursor".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("/".into(), "Search inbox".into()),
    ];

    let right = vec![
        HelpEntry::Header("Views & Other".into()),
        HelpEntry::Binding("Tab".into(), "Next view".into()),
        HelpEntry::Binding("?".into(), "Help".into()),
        HelpEntry::Binding("QQ".into(), "Quit".into()),
    ];

    (left, right)
}

fn build_recent_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{2191}\u{2193}/jk".into(), "Move cursor".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("/".into(), "Search".into()),
    ];

    let right = vec![
        HelpEntry::Header("Views & Other".into()),
        HelpEntry::Binding("Tab".into(), "Next view".into()),
        HelpEntry::Binding("?".into(), "Help".into()),
        HelpEntry::Binding("QQ".into(), "Quit".into()),
    ];

    (left, right)
}

fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}
