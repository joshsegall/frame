/// A single visual (screen) line produced by wrapping a logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualLine {
    /// Index into the note's logical lines
    pub logical_line: usize,
    /// Byte offset within the logical line where this visual line starts
    pub byte_start: usize,
    /// Byte offset (exclusive) within the logical line where this visual line ends
    pub byte_end: usize,
    /// Char offset within the logical line where this visual line starts
    pub char_start: usize,
    /// Char offset (exclusive) within the logical line where this visual line ends
    pub char_end: usize,
    /// True for the first visual row of a logical line (gets a line number in gutter)
    pub is_first: bool,
}

/// Compute the display width of a string, counting tabs as 4 characters.
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// Compute the display width of a single character.
fn char_display_width(c: char) -> usize {
    if c == '\t' { 4 } else { 1 }
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
            char_end: line.chars().count(),
            is_first: true,
        }];
    }

    let dw = display_width(line);
    if dw <= width {
        return vec![VisualLine {
            logical_line,
            byte_start: 0,
            byte_end: line.len(),
            char_start: 0,
            char_end: line.chars().count(),
            is_first: true,
        }];
    }

    let mut result = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let total_chars = chars.len();

    // Current visual line start positions
    let mut vl_char_start: usize = 0;
    let mut vl_byte_start: usize = 0;
    let mut col: usize = 0; // display column within current visual line

    let mut i: usize = 0; // char index

    while i < total_chars {
        // Find the next token: either a whitespace run or a word
        let token_start = i;
        let is_ws = chars[i].is_whitespace();

        if is_ws {
            // Consume whitespace run
            while i < total_chars && chars[i].is_whitespace() {
                i += 1;
            }
        } else {
            // Consume word (non-whitespace), splitting at hyphens
            while i < total_chars && !chars[i].is_whitespace() {
                let was_hyphen = chars[i] == '-';
                i += 1;
                if was_hyphen && i < total_chars && !chars[i].is_whitespace() {
                    break; // break after hyphen
                }
            }
        }

        let token_chars = &chars[token_start..i];
        let token_dw: usize = token_chars.iter().map(|&c| char_display_width(c)).sum();
        let token_byte_len: usize = token_chars.iter().map(|c| c.len_utf8()).sum();

        if col + token_dw <= width {
            // Fits on current line
            col += token_dw;
        } else if col == 0 && !is_ws {
            // First token on line but too wide — char-wrap it
            let mut placed_dw = 0;
            let mut j = token_start;
            while j < i {
                let cdw = char_display_width(chars[j]);
                if placed_dw + cdw > width && placed_dw > 0 {
                    // Emit visual line
                    let be = vl_byte_start
                        + chars[vl_char_start..j]
                            .iter()
                            .map(|c| c.len_utf8())
                            .sum::<usize>();
                    result.push(VisualLine {
                        logical_line,
                        byte_start: vl_byte_start,
                        byte_end: be,
                        char_start: vl_char_start,
                        char_end: j,
                        is_first: result.is_empty(),
                    });
                    vl_char_start = j;
                    vl_byte_start = be;
                    placed_dw = 0;
                }
                placed_dw += cdw;
                j += 1;
            }
            col = placed_dw;
        } else if is_ws {
            // Whitespace at wrap point — emit current visual line, skip whitespace
            let byte_end = vl_byte_start
                + chars[vl_char_start..token_start]
                    .iter()
                    .map(|c| c.len_utf8())
                    .sum::<usize>();
            result.push(VisualLine {
                logical_line,
                byte_start: vl_byte_start,
                byte_end,
                char_start: vl_char_start,
                char_end: token_start,
                is_first: result.is_empty(),
            });
            // New visual line starts after the whitespace
            vl_char_start = i; // skip whitespace chars
            vl_byte_start = byte_end + token_byte_len;
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
                // Char-wrap inline: fill remaining space, then continue
                let mut placed = 0;
                let mut j = token_start;
                while j < i && placed + char_display_width(chars[j]) <= remaining_space {
                    placed += char_display_width(chars[j]);
                    j += 1;
                }

                // Emit visual line up to j
                let byte_end = vl_byte_start
                    + chars[vl_char_start..j]
                        .iter()
                        .map(|c| c.len_utf8())
                        .sum::<usize>();
                result.push(VisualLine {
                    logical_line,
                    byte_start: vl_byte_start,
                    byte_end,
                    char_start: vl_char_start,
                    char_end: j,
                    is_first: result.is_empty(),
                });

                // Remaining part of token continues on next visual line
                vl_char_start = j;
                vl_byte_start = byte_end;
                col = 0;
                // Rewind i to j so we re-process the remainder
                i = j;
            } else {
                // Word-wrap: emit current line, put this word on the next
                let byte_end = vl_byte_start
                    + chars[vl_char_start..token_start]
                        .iter()
                        .map(|c| c.len_utf8())
                        .sum::<usize>();
                // Only emit if we have content (avoid empty visual lines)
                if token_start > vl_char_start {
                    result.push(VisualLine {
                        logical_line,
                        byte_start: vl_byte_start,
                        byte_end,
                        char_start: vl_char_start,
                        char_end: token_start,
                        is_first: result.is_empty(),
                    });
                    vl_char_start = token_start;
                    vl_byte_start = byte_end;
                }
                col = token_dw;

                // If the token itself is wider than width, char-wrap it
                if token_dw > width {
                    let mut placed_dw = 0;
                    let mut j = token_start;
                    while j < i {
                        let cdw = char_display_width(chars[j]);
                        if placed_dw + cdw > width && placed_dw > 0 {
                            // Emit visual line
                            let be = vl_byte_start
                                + chars[vl_char_start..j]
                                    .iter()
                                    .map(|c| c.len_utf8())
                                    .sum::<usize>();
                            result.push(VisualLine {
                                logical_line,
                                byte_start: vl_byte_start,
                                byte_end: be,
                                char_start: vl_char_start,
                                char_end: j,
                                is_first: result.is_empty(),
                            });
                            vl_char_start = j;
                            vl_byte_start = be;
                            placed_dw = 0;
                        }
                        placed_dw += cdw;
                        j += 1;
                    }
                    col = placed_dw;
                }
            }
        }
    }

    // Emit final visual line
    let final_byte_end = vl_byte_start
        + chars[vl_char_start..]
            .iter()
            .map(|c| c.len_utf8())
            .sum::<usize>();
    result.push(VisualLine {
        logical_line,
        byte_start: vl_byte_start,
        byte_end: final_byte_end,
        char_start: vl_char_start,
        char_end: total_chars,
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

/// Map a logical cursor position (line, col) to a visual row index
/// within the given visual lines.
pub fn logical_to_visual_row(visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
    for (i, vl) in visual_lines.iter().enumerate() {
        if vl.logical_line == line {
            if col < vl.char_end || (col == vl.char_end && i + 1 >= visual_lines.len()) {
                return i;
            }
            // Check if this is the last visual row for this logical line
            let next_is_same = visual_lines
                .get(i + 1)
                .is_some_and(|next| next.logical_line == line);
            if col >= vl.char_start && !next_is_same {
                return i;
            }
        }
    }
    visual_lines.len().saturating_sub(1)
}

/// Map a visual row index back to a logical cursor position (line, col).
/// `target_visual_col` is the desired column within the visual row (sticky column).
pub fn visual_row_to_logical(
    visual_lines: &[VisualLine],
    row: usize,
    target_visual_col: usize,
) -> (usize, usize) {
    if let Some(vl) = visual_lines.get(row) {
        let logical_line = vl.logical_line;
        let row_len = vl.char_end - vl.char_start;
        let col = vl.char_start + target_visual_col.min(row_len);
        (logical_line, col.min(vl.char_end))
    } else {
        (0, 0)
    }
}

/// Compute the visual column of a logical cursor within its visual row.
pub fn logical_to_visual_col(visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
    let row = logical_to_visual_row(visual_lines, line, col);
    if let Some(vl) = visual_lines.get(row) {
        col.saturating_sub(vl.char_start)
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
        assert_eq!(vls[0].char_start, 0);
        assert_eq!(vls[0].char_end, 11);
        assert!(vls[0].is_first);
    }

    #[test]
    fn wrap_at_space() {
        // "hello world" with width 7: "hello " fits (6 chars), then "world" wraps
        let vls = wrap_line("hello world", 7, 0);
        assert_eq!(vls.len(), 2);
        assert_eq!(vls[0].char_start, 0);
        assert!(vls[0].is_first);
        assert_eq!(vls[1].logical_line, 0);
        assert!(!vls[1].is_first);
        // Second line should contain "world"
        let second = &"hello world"[vls[1].byte_start..vls[1].byte_end];
        assert_eq!(second, "world");
    }

    #[test]
    fn wrap_at_hyphen() {
        let vls = wrap_line("long-word here", 6, 0);
        // "long-" fits in 5 chars, then "word" starts next line
        assert!(vls.len() >= 2);
        assert_eq!(vls[0].char_end, 5); // "long-"
    }

    #[test]
    fn char_wrap_long_word() {
        let vls = wrap_line("abcdefghij", 4, 0);
        // 10 chars, width 4 → 3 visual lines
        assert!(vls.len() >= 2);
        // Each visual line should be <= 4 chars wide
        for vl in &vls {
            assert!(vl.char_end - vl.char_start <= 4);
        }
    }

    #[test]
    fn empty_line() {
        let vls = wrap_line("", 80, 0);
        assert_eq!(vls.len(), 1);
        assert_eq!(vls[0].char_start, 0);
        assert_eq!(vls[0].char_end, 0);
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
        assert!(vls.len() >= 3); // "hello", "world", "foo"
        assert_eq!(vls[0].logical_line, 0);
        assert_eq!(vls.last().unwrap().logical_line, 1);
    }

    #[test]
    fn gutter_width_small() {
        assert_eq!(gutter_width(1), 3); // min 3
        assert_eq!(gutter_width(9), 3);
        assert_eq!(gutter_width(10), 3);
        assert_eq!(gutter_width(99), 3);
        assert_eq!(gutter_width(100), 4);
    }

    #[test]
    fn logical_to_visual_roundtrip() {
        let vls = wrap_line("hello world foo bar", 6, 0);
        // Cursor at start
        let row = logical_to_visual_row(&vls, 0, 0);
        assert_eq!(row, 0);
        let (line, col) = visual_row_to_logical(&vls, row, 0);
        assert_eq!(line, 0);
        assert_eq!(col, 0);
    }

    #[test]
    fn fill_heuristic_50_percent() {
        // With width 10 and "abcd" (4 chars, 60% remaining) followed by a 10-char word,
        // the 60% > 50% threshold should trigger char-wrap inline
        let vls = wrap_line("abcd xxxxxxxxxx", 10, 0);
        // First visual line should have "abcd xxxxx" (filling remaining space)
        assert!(vls.len() >= 2);
    }

    #[test]
    fn tab_counts_as_four() {
        // "\thello" = 4 + 5 = 9 display chars
        let vls = wrap_line("\thello", 10, 0);
        assert_eq!(vls.len(), 1); // fits in 10

        let vls = wrap_line("\thello", 8, 0);
        // 9 display chars > 8, should wrap
        assert!(vls.len() >= 2);
    }

    #[test]
    fn visual_col_computation() {
        let vls = wrap_line("hello world", 6, 0);
        // "hello" is visual row 0, "world" is visual row 1
        // cursor at char 6 (w of "world") should be visual col 0 on row 1
        let vcol = logical_to_visual_col(&vls, 0, 6);
        assert_eq!(vcol, 0);
        // cursor at char 8 should be visual col 2 on row 1
        let vcol = logical_to_visual_col(&vls, 0, 8);
        assert_eq!(vcol, 2);
    }
}
