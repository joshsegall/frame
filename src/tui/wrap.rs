use crate::util::unicode;
use unicode_segmentation::UnicodeSegmentation;

/// A single visual (screen) line produced by wrapping a logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualLine {
    /// Index into the note's logical lines
    pub logical_line: usize,
    /// Byte offset within the logical line where this visual line starts
    pub byte_start: usize,
    /// Byte offset (exclusive) within the logical line where this visual line ends
    pub byte_end: usize,
    /// Byte offset within the logical line where this visual line starts (same as byte_start)
    pub char_start: usize,
    /// Byte offset (exclusive) within the logical line where this visual line ends (same as byte_end)
    pub char_end: usize,
    /// True for the first visual row of a logical line (gets a line number in gutter)
    pub is_first: bool,
}

/// A grapheme with its byte offset and display width.
struct Grapheme<'a> {
    s: &'a str,
    byte_offset: usize,
    display_width: usize,
}

/// Collect graphemes from a string with byte offsets and display widths.
fn graphemes(line: &str) -> Vec<Grapheme<'_>> {
    line.grapheme_indices(true)
        .map(|(i, g)| Grapheme {
            s: g,
            byte_offset: i,
            display_width: grapheme_display_width(g),
        })
        .collect()
}

fn grapheme_display_width(g: &str) -> usize {
    if g == "\t" {
        4
    } else {
        unicode_width::UnicodeWidthStr::width(g)
    }
}

/// Wrap a single logical line into visual lines.
///
/// Word boundary rules (priority order):
/// 1. Whitespace — break after space/tab
/// 2. After hyphens — break after `-`
/// 3. Character wrap — fallback if single token > width
///
/// Fill heuristic: if content before break < 50% of width, char-wrap inline
/// instead of pushing to next row.
pub fn wrap_line(line: &str, width: usize, logical_line: usize) -> Vec<VisualLine> {
    if width == 0 {
        return vec![VisualLine {
            logical_line,
            byte_start: 0,
            byte_end: line.len(),
            char_start: 0,
            char_end: line.len(),
            is_first: true,
        }];
    }

    let dw = unicode::display_width(line);
    if dw <= width {
        return vec![VisualLine {
            logical_line,
            byte_start: 0,
            byte_end: line.len(),
            char_start: 0,
            char_end: line.len(),
            is_first: true,
        }];
    }

    let gs = graphemes(line);
    let total = gs.len();

    let mut result = Vec::new();

    // Current visual line start (grapheme index)
    let mut vl_start: usize = 0;
    let mut col: usize = 0; // display column within current visual line

    let mut i: usize = 0; // grapheme index

    // Helper: byte offset at grapheme index (or line.len() if past end)
    let byte_at = |idx: usize| -> usize {
        if idx < gs.len() {
            gs[idx].byte_offset
        } else {
            line.len()
        }
    };

    while i < total {
        let token_start = i;
        let is_ws = gs[i].s.chars().all(|c| c.is_whitespace());

        if is_ws {
            while i < total && gs[i].s.chars().all(|c| c.is_whitespace()) {
                i += 1;
            }
        } else {
            while i < total && !gs[i].s.chars().all(|c| c.is_whitespace()) {
                let was_hyphen = gs[i].s == "-";
                i += 1;
                if was_hyphen && i < total && !gs[i].s.chars().all(|c| c.is_whitespace()) {
                    break;
                }
            }
        }

        let token_dw: usize = gs[token_start..i].iter().map(|g| g.display_width).sum();

        if col + token_dw <= width {
            col += token_dw;
        } else if col == 0 && !is_ws {
            // First token on line but too wide — grapheme-wrap it
            let mut placed_dw = 0;
            let mut j = token_start;
            while j < i {
                let gdw = gs[j].display_width;
                if placed_dw + gdw > width && placed_dw > 0 {
                    let be = byte_at(j);
                    let bs = byte_at(vl_start);
                    result.push(VisualLine {
                        logical_line,
                        byte_start: bs,
                        byte_end: be,
                        char_start: bs,
                        char_end: be,
                        is_first: result.is_empty(),
                    });
                    vl_start = j;
                    placed_dw = 0;
                }
                placed_dw += gdw;
                j += 1;
            }
            col = placed_dw;
        } else if is_ws {
            // Whitespace at wrap point — emit current visual line, skip whitespace
            let bs = byte_at(vl_start);
            let be = byte_at(token_start);
            result.push(VisualLine {
                logical_line,
                byte_start: bs,
                byte_end: be,
                char_start: bs,
                char_end: be,
                is_first: result.is_empty(),
            });
            vl_start = i;
            col = 0;
        } else {
            // Word doesn't fit — check fill heuristic
            let remaining_space = width.saturating_sub(col);
            let blank_fraction = if width > 0 {
                remaining_space as f64 / width as f64
            } else {
                0.0
            };

            if blank_fraction > 0.5 && remaining_space > 0 {
                // Char-wrap inline: fill remaining space
                let mut placed = 0;
                let mut j = token_start;
                while j < i && placed + gs[j].display_width <= remaining_space {
                    placed += gs[j].display_width;
                    j += 1;
                }

                let bs = byte_at(vl_start);
                let be = byte_at(j);
                result.push(VisualLine {
                    logical_line,
                    byte_start: bs,
                    byte_end: be,
                    char_start: bs,
                    char_end: be,
                    is_first: result.is_empty(),
                });

                vl_start = j;
                col = 0;
                i = j;
            } else {
                // Word-wrap: emit current line, put this word on the next
                let bs = byte_at(vl_start);
                let be = byte_at(token_start);
                if token_start > vl_start {
                    result.push(VisualLine {
                        logical_line,
                        byte_start: bs,
                        byte_end: be,
                        char_start: bs,
                        char_end: be,
                        is_first: result.is_empty(),
                    });
                    vl_start = token_start;
                }
                col = token_dw;

                // If the token itself is wider than width, grapheme-wrap it
                if token_dw > width {
                    let mut placed_dw = 0;
                    let mut j = token_start;
                    while j < i {
                        let gdw = gs[j].display_width;
                        if placed_dw + gdw > width && placed_dw > 0 {
                            let vbs = byte_at(vl_start);
                            let vbe = byte_at(j);
                            result.push(VisualLine {
                                logical_line,
                                byte_start: vbs,
                                byte_end: vbe,
                                char_start: vbs,
                                char_end: vbe,
                                is_first: result.is_empty(),
                            });
                            vl_start = j;
                            placed_dw = 0;
                        }
                        placed_dw += gdw;
                        j += 1;
                    }
                    col = placed_dw;
                }
            }
        }
    }

    // Emit final visual line
    let bs = byte_at(vl_start);
    result.push(VisualLine {
        logical_line,
        byte_start: bs,
        byte_end: line.len(),
        char_start: bs,
        char_end: line.len(),
        is_first: result.is_empty(),
    });

    result
}

/// Wrap multiple logical lines, returning all visual lines in order.
pub fn wrap_lines(lines: &[&str], width: usize) -> Vec<VisualLine> {
    let mut result = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        result.extend(wrap_line(line, width, idx));
    }
    result
}

/// Compute the gutter width for line numbers: max(1, digits(line_count)) + 1.
/// Minimum gutter is 3 (2 digits + 1 space).
pub fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        line_count.to_string().len()
    };
    (digits + 1).max(3)
}

/// Map a logical cursor position (line, col_byte_offset) to a visual row index
/// within the given visual lines. `col` is a byte offset within the logical line.
pub fn logical_to_visual_row(visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
    for (i, vl) in visual_lines.iter().enumerate() {
        if vl.logical_line == line {
            if col < vl.byte_end || (col == vl.byte_end && i + 1 >= visual_lines.len()) {
                return i;
            }
            // Check if this is the last visual row for this logical line
            let next_is_same = visual_lines
                .get(i + 1)
                .is_some_and(|next| next.logical_line == line);
            if col >= vl.byte_start && !next_is_same {
                return i;
            }
        }
    }
    visual_lines.len().saturating_sub(1)
}

/// Map a visual row index back to a logical cursor position (line, byte_offset).
/// `target_visual_col` is the desired display column (terminal cells) within the visual row.
pub fn visual_row_to_logical(
    visual_lines: &[VisualLine],
    row: usize,
    target_visual_col: usize,
    lines: &[&str],
) -> (usize, usize) {
    if let Some(vl) = visual_lines.get(row) {
        let logical_line = vl.logical_line;
        let line_str = lines.get(logical_line).copied().unwrap_or("");
        let vl_text = &line_str[vl.byte_start..vl.byte_end];
        let byte_within_vl = unicode::display_col_to_byte_offset(vl_text, target_visual_col);
        let col = vl.byte_start + byte_within_vl;
        (logical_line, col.min(vl.byte_end))
    } else {
        (0, 0)
    }
}

/// Compute the visual column (display cells) of a logical cursor within its visual row.
/// `col` is a byte offset within the logical line.
pub fn logical_to_visual_col(
    visual_lines: &[VisualLine],
    line: usize,
    col: usize,
    lines: &[&str],
) -> usize {
    let row = logical_to_visual_row(visual_lines, line, col);
    if let Some(vl) = visual_lines.get(row) {
        let logical_line_str = lines.get(vl.logical_line).copied().unwrap_or("");
        let byte_start = vl.byte_start;
        let byte_cursor = col.min(vl.byte_end);
        let within_vl = &logical_line_str[byte_start..byte_cursor];
        unicode::display_width(within_vl)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_wrap_needed() {
        let vls = wrap_line("hello world", 80, 0);
        assert_eq!(vls.len(), 1);
        assert_eq!(vls[0].byte_start, 0);
        assert_eq!(vls[0].byte_end, 11);
        assert!(vls[0].is_first);
    }

    #[test]
    fn wrap_at_space() {
        // "hello world" with width 7: "hello " fits (6 cells), then "world" wraps
        let vls = wrap_line("hello world", 7, 0);
        assert_eq!(vls.len(), 2);
        assert_eq!(vls[0].byte_start, 0);
        assert!(vls[0].is_first);
        assert_eq!(vls[1].logical_line, 0);
        assert!(!vls[1].is_first);
        let second = &"hello world"[vls[1].byte_start..vls[1].byte_end];
        assert_eq!(second, "world");
    }

    #[test]
    fn wrap_at_hyphen() {
        let vls = wrap_line("long-word here", 6, 0);
        assert!(vls.len() >= 2);
        assert_eq!(vls[0].byte_end, 5); // "long-"
    }

    #[test]
    fn char_wrap_long_word() {
        let vls = wrap_line("abcdefghij", 4, 0);
        assert!(vls.len() >= 2);
        for vl in &vls {
            let text = &"abcdefghij"[vl.byte_start..vl.byte_end];
            assert!(unicode::display_width(text) <= 4);
        }
    }

    #[test]
    fn empty_line() {
        let vls = wrap_line("", 80, 0);
        assert_eq!(vls.len(), 1);
        assert_eq!(vls[0].byte_start, 0);
        assert_eq!(vls[0].byte_end, 0);
        assert!(vls[0].is_first);
    }

    #[test]
    fn zero_width() {
        let vls = wrap_line("hello", 0, 0);
        assert_eq!(vls.len(), 1);
    }

    #[test]
    fn wrap_lines_multiple() {
        let lines = vec!["hello world", "foo"];
        let vls = wrap_lines(&lines, 6);
        assert!(vls.len() >= 3);
        assert_eq!(vls[0].logical_line, 0);
        assert_eq!(vls.last().unwrap().logical_line, 1);
    }

    #[test]
    fn gutter_width_small() {
        assert_eq!(gutter_width(1), 3);
        assert_eq!(gutter_width(9), 3);
        assert_eq!(gutter_width(10), 3);
        assert_eq!(gutter_width(99), 3);
        assert_eq!(gutter_width(100), 4);
    }

    #[test]
    fn logical_to_visual_roundtrip() {
        let text = "hello world foo bar";
        let lines = vec![text];
        let vls = wrap_line(text, 6, 0);
        let row = logical_to_visual_row(&vls, 0, 0);
        assert_eq!(row, 0);
        let (line, col) = visual_row_to_logical(&vls, row, 0, &lines);
        assert_eq!(line, 0);
        assert_eq!(col, 0);
    }

    #[test]
    fn fill_heuristic_50_percent() {
        let vls = wrap_line("abcd xxxxxxxxxx", 10, 0);
        assert!(vls.len() >= 2);
    }

    #[test]
    fn tab_counts_as_four() {
        let vls = wrap_line("\thello", 10, 0);
        assert_eq!(vls.len(), 1);

        let vls = wrap_line("\thello", 8, 0);
        assert!(vls.len() >= 2);
    }

    #[test]
    fn visual_col_computation() {
        let text = "hello world";
        let lines = vec![text];
        let vls = wrap_line(text, 6, 0);
        // "hello" is visual row 0, "world" is visual row 1
        // cursor at byte 6 (w of "world") should be visual col 0 on row 1
        let vcol = logical_to_visual_col(&vls, 0, 6, &lines);
        assert_eq!(vcol, 0);
        // cursor at byte 8 should be visual col 2 on row 1
        let vcol = logical_to_visual_col(&vls, 0, 8, &lines);
        assert_eq!(vcol, 2);
    }

    #[test]
    fn wrap_cjk() {
        // "你好世界" = 8 display cells
        let vls = wrap_line("你好世界", 5, 0);
        assert_eq!(vls.len(), 2);
        // First visual line: "你好" (4 cells)
        let first = &"你好世界"[vls[0].byte_start..vls[0].byte_end];
        assert_eq!(first, "你好");
    }

    #[test]
    fn wrap_emoji() {
        let s = "🎉🚀💫✨";
        // Each emoji is 2 cells, total 8 cells
        let vls = wrap_line(s, 5, 0);
        assert_eq!(vls.len(), 2);
        let first = &s[vls[0].byte_start..vls[0].byte_end];
        assert_eq!(unicode::display_width(first), 4); // 🎉🚀
    }

    #[test]
    fn wrap_never_breaks_grapheme() {
        // Combining character must never be separated from its base
        let s = "cafe\u{0301} is good"; // "café is good"
        let vls = wrap_line(s, 6, 0);
        for vl in &vls {
            let text = &s[vl.byte_start..vl.byte_end];
            // No visual line should start with a combining character
            if let Some(first_char) = text.chars().next() {
                assert!(
                    unicode_width::UnicodeWidthChar::width(first_char) != Some(0)
                        || first_char == '\u{0301}' && text.starts_with("e\u{0301}"),
                    "Line starts with zero-width character: {:?}",
                    text
                );
            }
        }
    }
}
