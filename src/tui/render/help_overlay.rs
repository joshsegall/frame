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
pub fn render_help_overlay(frame: &mut Frame, app: &mut App, area: Rect) {
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
            render_entry(
                &left[row],
                &mut spans,
                key_w,
                col_w,
                key_style,
                desc_style,
                header_style,
                blank_style,
            );
        } else {
            spans.push(Span::styled(" ".repeat(col_w), blank_style));
        }

        // Gap
        spans.push(Span::styled(" ".repeat(gap), blank_style));

        // Right column
        if row < right.len() {
            render_entry(
                &right[row],
                &mut spans,
                key_w,
                col_w,
                key_style,
                desc_style,
                header_style,
                blank_style,
            );
        }

        lines.push(Line::from(spans));
    }

    // Fixed width from content: 2 columns + gap + borders
    let popup_w = ((col_w * 2 + gap) as u16 + 2).min(area.width.saturating_sub(2));
    let inner_w = (popup_w.saturating_sub(2)) as usize;

    // Footer: blank line + version/URL row
    let version_left = format!("[>] frame v{}", env!("CARGO_PKG_VERSION"));
    let url_right = "github.com/joshsegall/frame";
    let footer_style = desc_style;

    lines.push(Line::from(Span::styled(" ".repeat(inner_w), blank_style)));
    let usable_w = inner_w.saturating_sub(2); // 1 space padding on each side
    let padding = usable_w.saturating_sub(version_left.len() + url_right.len());
    let footer_text = format!(
        " {}{}{}{}",
        version_left,
        " ".repeat(padding),
        url_right,
        " ",
    );
    lines.push(Line::from(Span::styled(footer_text, footer_style)));

    // Dynamic height from content + borders
    let popup_h = ((lines.len() as u16) + 2).min(area.height.saturating_sub(2));
    let visible_h = popup_h.saturating_sub(2) as usize; // minus top/bottom border
    let max_scroll = lines.len().saturating_sub(visible_h);
    app.help_scroll = app.help_scroll.min(max_scroll);

    let overlay_area = centered_rect_fixed(popup_w, popup_h, area);

    frame.render_widget(Clear, overlay_area);

    // Build border with scroll indicators
    let can_scroll_up = app.help_scroll > 0;
    let can_scroll_down = app.help_scroll < max_scroll;
    let border_style = Style::default().fg(dim).bg(bg);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(bg));

    if can_scroll_up && can_scroll_down {
        block = block
            .title_top(Span::styled(" \u{25B2} ", Style::default().fg(dim).bg(bg)))
            .title_bottom(Span::styled(" \u{25BC} ", Style::default().fg(dim).bg(bg)));
    } else if can_scroll_up {
        block = block.title_top(Span::styled(" \u{25B2} ", Style::default().fg(dim).bg(bg)));
    } else if can_scroll_down {
        block = block.title_bottom(Span::styled(" \u{25BC} ", Style::default().fg(dim).bg(bg)));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((app.help_scroll as u16, 0))
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, overlay_area);
}

#[allow(clippy::too_many_arguments)]
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
        View::Detail { .. } => build_detail_columns(),
        View::Tracks => build_tracks_columns(),
        View::Inbox => build_inbox_columns(),
        View::Recent => build_recent_columns(),
    }
}

/// Standard "Views" section entries, identical across overlays.
/// `include_tab` is false for Detail view (Tab/S-Tab repurposed for region nav).
fn views_entries(include_tab: bool) -> Vec<HelpEntry> {
    let mut entries = vec![
        HelpEntry::Header("Views".into()),
        HelpEntry::Binding("1-9".into(), "Track N".into()),
    ];
    if include_tab {
        entries.push(HelpEntry::Binding(
            "Tab/S-Tab".into(),
            "Prev / next view".into(),
        ));
    }
    entries.push(HelpEntry::Binding("0/`".into(), "Tracks overview".into()));
    entries.push(HelpEntry::Binding("i".into(), "Inbox".into()));
    entries.push(HelpEntry::Binding("r".into(), "Recent".into()));
    entries
}

/// Standard "Other" section entries, identical core across overlays.
/// Per-view extras (D, C, .) are inserted between J and T.
fn other_entries(include_deps: bool, include_cc: bool, include_repeat: bool) -> Vec<HelpEntry> {
    let mut entries = vec![
        HelpEntry::Header("Other".into()),
        HelpEntry::Binding("/".into(), "Search".into()),
        HelpEntry::Binding(">".into(), "Command palette".into()),
        HelpEntry::Binding("J".into(), "Jump to task".into()),
    ];
    if include_deps {
        entries.push(HelpEntry::Binding("D".into(), "Show deps".into()));
    }
    if include_cc {
        entries.push(HelpEntry::Binding("C".into(), "Set cc-focus".into()));
    }
    if include_repeat {
        entries.push(HelpEntry::Binding(".".into(), "Repeat last action".into()));
    }
    entries.push(HelpEntry::Binding("T".into(), "Tag colors".into()));
    entries.push(HelpEntry::Binding("P".into(), "Projects".into()));
    entries.push(HelpEntry::Binding("z/u".into(), "Undo".into()));
    entries.push(HelpEntry::Binding("Z".into(), "Redo".into()));
    entries.push(HelpEntry::Binding("?".into(), "Help".into()));
    entries.push(HelpEntry::Binding("QQ".into(), "Quit".into()));
    entries
}

fn build_track_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let mut left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{25B2}\u{25BC}/jk".into(), "Move up/down".into()),
        HelpEntry::Binding("\u{25C0}/h".into(), "Collapse / parent".into()),
        HelpEntry::Binding("\u{25B6}/l".into(), "Expand / child".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("Enter".into(), "Open detail".into()),
        HelpEntry::Binding("Esc".into(), "Back / close".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Task State".into()),
        HelpEntry::Binding("Space".into(), "Cycle state".into()),
        HelpEntry::Binding("o".into(), "Set todo".into()),
        HelpEntry::Binding("x".into(), "Mark done".into()),
        HelpEntry::Binding("b".into(), "Set blocked".into()),
        HelpEntry::Binding("~".into(), "Set parked".into()),
        HelpEntry::Binding("c".into(), "Toggle cc tag".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Filter (f+key)".into()),
        HelpEntry::Binding("fa".into(), "Active only".into()),
        HelpEntry::Binding("fo".into(), "Todo only".into()),
        HelpEntry::Binding("fb".into(), "Blocked only".into()),
        HelpEntry::Binding("fp".into(), "Parked only".into()),
        HelpEntry::Binding("fr".into(), "Ready (deps met)".into()),
        HelpEntry::Binding("ft".into(), "Filter by tag".into()),
        HelpEntry::Binding("f Space".into(), "Clear state filter".into()),
        HelpEntry::Binding("ff".into(), "Clear all filters".into()),
        HelpEntry::Blank,
    ];
    left.extend(views_entries(true));

    let mut right = vec![
        HelpEntry::Header("Edit".into()),
        HelpEntry::Binding("e".into(), "Edit title".into()),
        HelpEntry::Binding("t".into(), "Edit tags".into()),
        HelpEntry::Binding("a".into(), "Add task (bottom)".into()),
        HelpEntry::Binding("=".into(), "Append to group".into()),
        HelpEntry::Binding("-".into(), "Insert after cursor".into()),
        HelpEntry::Binding("p".into(), "Push to top".into()),
        HelpEntry::Binding("A".into(), "Add subtask".into()),
        HelpEntry::Binding("m".into(), "Move mode".into()),
        HelpEntry::Binding("M".into(), "Move to track".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Select (v)".into()),
        HelpEntry::Binding("v".into(), "Toggle select".into()),
        HelpEntry::Binding("V".into(), "Range select".into()),
        HelpEntry::Binding("Ctrl+A".into(), "Select all".into()),
        HelpEntry::Binding("N".into(), "Select none".into()),
        HelpEntry::Binding("x/b/o/~".into(), "Bulk state".into()),
        HelpEntry::Binding("t/d/m/M".into(), "Bulk tag/dep/move".into()),
        HelpEntry::Blank,
    ];
    right.extend(other_entries(true, true, true));

    (left, right)
}

fn build_detail_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let mut left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{25B2}\u{25BC}/jk".into(), "Move between regions".into()),
        HelpEntry::Binding("Tab/S-Tab".into(), "Next / prev region".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("Esc".into(), "Back / close".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Task State".into()),
        HelpEntry::Binding("Space".into(), "Cycle state".into()),
        HelpEntry::Binding("x".into(), "Mark done".into()),
        HelpEntry::Binding("o".into(), "Set todo".into()),
        HelpEntry::Binding("b".into(), "Set blocked".into()),
        HelpEntry::Binding("~".into(), "Set parked".into()),
        HelpEntry::Binding("M".into(), "Move to track".into()),
    ];
    left.push(HelpEntry::Blank);
    left.extend(views_entries(false));

    let mut right = vec![
        HelpEntry::Header("Edit".into()),
        HelpEntry::Binding("e/Enter".into(), "Edit region / open sub".into()),
        HelpEntry::Binding("t".into(), "Edit tags".into()),
        HelpEntry::Binding("@".into(), "Edit refs".into()),
        HelpEntry::Binding("d".into(), "Edit deps".into()),
        HelpEntry::Binding("n/N".into(), "Edit note".into()),
        HelpEntry::Binding("w/Alt+w".into(), "Toggle note wrap".into()),
        HelpEntry::Blank,
    ];
    right.extend(other_entries(true, false, true));

    (left, right)
}

fn build_tracks_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{25B2}\u{25BC}/jk".into(), "Move cursor".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("Enter".into(), "Open track".into()),
        HelpEntry::Binding("Esc".into(), "Back / close".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Track Actions".into()),
        HelpEntry::Binding("a/=".into(), "Add track (bottom)".into()),
        HelpEntry::Binding("-".into(), "Insert after cursor".into()),
        HelpEntry::Binding("p".into(), "Push to top".into()),
        HelpEntry::Binding("e".into(), "Edit track name".into()),
        HelpEntry::Binding("s".into(), "Shelve / activate".into()),
        HelpEntry::Binding("m".into(), "Move mode".into()),
        HelpEntry::Binding("C".into(), "Set cc-focus".into()),
        HelpEntry::Blank,
        HelpEntry::Binding(">".into(), "More actions...".into()),
    ];

    let mut right = views_entries(true);
    right.push(HelpEntry::Blank);
    right.extend(other_entries(false, false, false));

    (left, right)
}

fn build_inbox_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let mut left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{25B2}\u{25BC}/jk".into(), "Move cursor".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("Esc".into(), "Back / close".into()),
        HelpEntry::Blank,
        HelpEntry::Header("Triage".into()),
        HelpEntry::Binding("Enter".into(), "Triage to track".into()),
        HelpEntry::Blank,
    ];
    left.extend(views_entries(true));

    let mut right = vec![
        HelpEntry::Header("Edit".into()),
        HelpEntry::Binding("a/=".into(), "Add item (bottom)".into()),
        HelpEntry::Binding("-".into(), "Insert after cursor".into()),
        HelpEntry::Binding("p".into(), "Push to top".into()),
        HelpEntry::Binding("e".into(), "Edit title".into()),
        HelpEntry::Binding("t".into(), "Edit tags".into()),
        HelpEntry::Binding("n/N".into(), "Edit note".into()),
        HelpEntry::Binding("w/Alt+w".into(), "Toggle note wrap".into()),
        HelpEntry::Binding("x".into(), "Delete item".into()),
        HelpEntry::Binding("m".into(), "Move mode".into()),
        HelpEntry::Blank,
    ];
    right.extend(other_entries(false, false, false));

    (left, right)
}

fn build_recent_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    let mut left = vec![
        HelpEntry::Header("Navigation".into()),
        HelpEntry::Binding("\u{25B2}\u{25BC}/jk".into(), "Move cursor".into()),
        HelpEntry::Binding("\u{25B6}/l".into(), "Expand subtasks".into()),
        HelpEntry::Binding("\u{25C0}/h".into(), "Collapse subtasks".into()),
        HelpEntry::Binding("Enter".into(), "Open detail".into()),
        HelpEntry::Binding("g/G".into(), "Top / bottom".into()),
        HelpEntry::Binding("Esc".into(), "Back / close".into()),
        HelpEntry::Blank,
    ];
    left.extend(views_entries(true));

    let mut right = vec![
        HelpEntry::Header("Actions".into()),
        HelpEntry::Binding("Space".into(), "Reopen as todo".into()),
        HelpEntry::Blank,
    ];
    right.extend(other_entries(false, false, false));

    (left, right)
}

fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}
