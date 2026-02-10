use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Display width in terminal cells. Tabs count as 4 cells.
pub fn display_width(s: &str) -> usize {
    s.split('\t')
        .enumerate()
        .map(|(i, part)| {
            let w = UnicodeWidthStr::width(part);
            if i > 0 { w + 4 } else { w }
        })
        .sum()
}

/// Display width of a single character in terminal cells. Tabs count as 4.
pub fn char_display_width(c: char) -> usize {
    if c == '\t' {
        4
    } else {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

/// Truncate a string to fit within `max_cells` terminal cells, appending `…` if truncated.
pub fn truncate_to_width(s: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    let sw = display_width(s);
    if sw <= max_cells {
        return s.to_string();
    }
    if max_cells <= 1 {
        return "\u{2026}".to_string();
    }
    let budget = max_cells - 1; // reserve 1 cell for '…'
    let mut width = 0;
    let mut result = String::new();
    for grapheme in s.graphemes(true) {
        let gw = grapheme_display_width(grapheme);
        if width + gw > budget {
            break;
        }
        width += gw;
        result.push_str(grapheme);
    }
    result.push('\u{2026}');
    result
}

/// Next grapheme boundary after `byte_offset`. Returns None if at end.
pub fn next_grapheme_boundary(s: &str, byte_offset: usize) -> Option<usize> {
    if byte_offset >= s.len() {
        return None;
    }
    if let Some((i, _)) = s[byte_offset..].grapheme_indices(true).nth(1) {
        return Some(byte_offset + i);
    }
    Some(s.len())
}

/// Previous grapheme boundary before `byte_offset`. Returns None if at start.
pub fn prev_grapheme_boundary(s: &str, byte_offset: usize) -> Option<usize> {
    if byte_offset == 0 {
        return None;
    }
    // Iterate graphemes in s[..byte_offset], find the last boundary
    let prefix = &s[..byte_offset];
    let mut last_start = 0;
    for (i, _) in prefix.grapheme_indices(true) {
        last_start = i;
    }
    Some(last_start)
}

/// The grapheme cluster starting at `byte_offset`.
pub fn grapheme_at(s: &str, byte_offset: usize) -> &str {
    if byte_offset >= s.len() {
        return "";
    }
    s[byte_offset..].graphemes(true).next().unwrap_or("")
}

/// Convert byte offset to display column (terminal cells).
pub fn byte_offset_to_display_col(s: &str, byte_offset: usize) -> usize {
    let clamped = byte_offset.min(s.len());
    display_width(&s[..clamped])
}

/// Convert display column to byte offset, snapping to grapheme boundary.
/// If `target_col` falls within a wide character, returns the byte offset
/// of that character's start. If beyond the string, returns `s.len()`.
pub fn display_col_to_byte_offset(s: &str, target_col: usize) -> usize {
    let mut col = 0;
    for (i, g) in s.grapheme_indices(true) {
        let gw = grapheme_display_width(g);
        if col + gw > target_col {
            return i;
        }
        col += gw;
    }
    s.len()
}

/// Word boundary to the left (grapheme-aware, whitespace-delimited).
pub fn word_boundary_left(s: &str, byte_offset: usize) -> usize {
    if byte_offset == 0 {
        return 0;
    }
    let prefix = &s[..byte_offset];
    let graphemes: Vec<(usize, &str)> = prefix.grapheme_indices(true).collect();
    if graphemes.is_empty() {
        return 0;
    }

    let mut idx = graphemes.len() - 1;

    // Skip trailing whitespace
    while idx > 0 && graphemes[idx].1.chars().all(|c| c.is_whitespace()) {
        idx -= 1;
    }

    // Skip word characters
    while idx > 0 && !graphemes[idx - 1].1.chars().all(|c| c.is_whitespace()) {
        idx -= 1;
    }

    graphemes[idx].0
}

/// Word boundary to the right (grapheme-aware, whitespace-delimited).
pub fn word_boundary_right(s: &str, byte_offset: usize) -> usize {
    if byte_offset >= s.len() {
        return s.len();
    }
    let suffix = &s[byte_offset..];
    let graphemes: Vec<(usize, &str)> = suffix.grapheme_indices(true).collect();
    if graphemes.is_empty() {
        return s.len();
    }

    let mut idx = 0;

    // Skip current word
    while idx < graphemes.len() && !graphemes[idx].1.chars().all(|c| c.is_whitespace()) {
        idx += 1;
    }

    // Skip whitespace
    while idx < graphemes.len() && graphemes[idx].1.chars().all(|c| c.is_whitespace()) {
        idx += 1;
    }

    if idx < graphemes.len() {
        byte_offset + graphemes[idx].0
    } else {
        s.len()
    }
}

/// Display width of a grapheme cluster.
fn grapheme_display_width(g: &str) -> usize {
    // Tab handling
    if g == "\t" {
        return 4;
    }
    UnicodeWidthStr::width(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── display_width ──────────────────────────────────────────────

    #[test]
    fn display_width_ascii() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn display_width_cjk() {
        assert_eq!(display_width("你好"), 4);
    }

    #[test]
    fn display_width_emoji() {
        assert_eq!(display_width("🎉"), 2);
    }

    #[test]
    fn display_width_mixed() {
        assert_eq!(display_width("hello你好"), 9);
    }

    #[test]
    fn display_width_combining() {
        // café with combining accent: c a f e ́
        assert_eq!(display_width("cafe\u{0301}"), 4);
    }

    #[test]
    fn display_width_zero_width_space() {
        assert_eq!(display_width("a\u{200B}b"), 2);
    }

    #[test]
    fn display_width_fullwidth() {
        assert_eq!(display_width("Ｈｉ"), 4);
    }

    #[test]
    fn display_width_tab() {
        assert_eq!(display_width("\thello"), 9);
        assert_eq!(display_width("a\tb"), 6); // 1 + 4 + 1
    }

    #[test]
    fn display_width_empty() {
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn display_width_box_drawing() {
        assert_eq!(display_width("─│┌┐└┘"), 6);
    }

    // ── truncate_to_width ──────────────────────────────────────────

    #[test]
    fn truncate_no_truncation_needed() {
        assert_eq!(truncate_to_width("hi", 10), "hi");
    }

    #[test]
    fn truncate_exact_fit() {
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_to_width("hello world", 8), "hello w\u{2026}");
    }

    #[test]
    fn truncate_cjk_boundary() {
        // "你好世界" is 8 cells. Truncating to 5: "你好" = 4 + "…" = 1 = 5
        assert_eq!(truncate_to_width("你好世界", 5), "你好\u{2026}");
    }

    #[test]
    fn truncate_cjk_off_by_one() {
        // Truncating to 4 cells: budget=3, "你" = 2, next "好" = 2 > 3, so "你…" = 3
        let result = truncate_to_width("你好世界", 4);
        assert!(display_width(&result) <= 4);
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_emoji() {
        assert_eq!(truncate_to_width("🎉🚀💫", 4), "🎉\u{2026}");
    }

    #[test]
    fn truncate_zero() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn truncate_one() {
        assert_eq!(truncate_to_width("hello", 1), "\u{2026}");
    }

    // ── grapheme boundaries ────────────────────────────────────────

    #[test]
    fn next_grapheme_ascii() {
        assert_eq!(next_grapheme_boundary("hello", 0), Some(1));
        assert_eq!(next_grapheme_boundary("hello", 4), Some(5));
        assert_eq!(next_grapheme_boundary("hello", 5), None);
    }

    #[test]
    fn prev_grapheme_ascii() {
        assert_eq!(prev_grapheme_boundary("hello", 5), Some(4));
        assert_eq!(prev_grapheme_boundary("hello", 1), Some(0));
        assert_eq!(prev_grapheme_boundary("hello", 0), None);
    }

    #[test]
    fn next_grapheme_emoji() {
        let s = "a🎉b";
        assert_eq!(next_grapheme_boundary(s, 0), Some(1)); // a -> 🎉
        assert_eq!(next_grapheme_boundary(s, 1), Some(5)); // 🎉 -> b
        assert_eq!(next_grapheme_boundary(s, 5), Some(6)); // b -> end
    }

    #[test]
    fn grapheme_combining() {
        let s = "cafe\u{0301}!"; // café!
        // Bytes: c(0) a(1) f(2) e(3) combining(4,5) !(6) — total 7
        // Graphemes: c(0), a(1), f(2), é(3..6), !(6)
        assert_eq!(next_grapheme_boundary(s, 3), Some(6)); // é -> !
        assert_eq!(prev_grapheme_boundary(s, 6), Some(3)); // ! -> é start
    }

    #[test]
    fn grapheme_zwj() {
        let family = "👨\u{200D}👩\u{200D}👧";
        // One grapheme cluster
        let next = next_grapheme_boundary(family, 0);
        assert_eq!(next, Some(family.len()));
    }

    #[test]
    fn grapheme_at_tests() {
        assert_eq!(grapheme_at("hello", 0), "h");
        assert_eq!(grapheme_at("a🎉b", 1), "🎉");
        assert_eq!(grapheme_at("cafe\u{0301}!", 3), "e\u{0301}");
        assert_eq!(grapheme_at("hello", 5), "");
    }

    // ── byte offset <-> display col ────────────────────────────────

    #[test]
    fn byte_to_display_col_ascii() {
        assert_eq!(byte_offset_to_display_col("hello", 3), 3);
    }

    #[test]
    fn byte_to_display_col_cjk() {
        // "你好" — "你" is 3 bytes, 2 cells
        assert_eq!(byte_offset_to_display_col("你好", 3), 2);
        assert_eq!(byte_offset_to_display_col("你好", 6), 4);
    }

    #[test]
    fn display_col_to_byte_ascii() {
        assert_eq!(display_col_to_byte_offset("hello", 3), 3);
    }

    #[test]
    fn display_col_to_byte_cjk() {
        // "你好" — col 2 should be byte 3 (start of 好)
        assert_eq!(display_col_to_byte_offset("你好", 2), 3);
        assert_eq!(display_col_to_byte_offset("你好", 4), 6);
    }

    #[test]
    fn display_col_to_byte_snaps() {
        // "你好" — col 1 is in the middle of 你 (2 cells), should snap to byte 0
        assert_eq!(display_col_to_byte_offset("你好", 1), 0);
    }

    #[test]
    fn display_col_to_byte_beyond() {
        assert_eq!(display_col_to_byte_offset("hi", 10), 2);
    }

    // ── word boundaries ────────────────────────────────────────────

    #[test]
    fn word_boundary_left_ascii() {
        let s = "hello world";
        assert_eq!(word_boundary_left(s, 11), 6); // end -> "world"
        assert_eq!(word_boundary_left(s, 6), 0); // "world" start -> "hello" start
        assert_eq!(word_boundary_left(s, 0), 0);
    }

    #[test]
    fn word_boundary_right_ascii() {
        let s = "hello world";
        assert_eq!(word_boundary_right(s, 0), 6); // start -> "world" start
        assert_eq!(word_boundary_right(s, 6), 11); // "world" start -> end
        assert_eq!(word_boundary_right(s, 11), 11);
    }

    #[test]
    fn word_boundary_left_cjk() {
        let s = "hello 你好";
        // "hello" ends at byte 5, space at 5, "你" at 6, "好" at 9
        let end = s.len(); // 12
        assert_eq!(word_boundary_left(s, end), 6); // end -> "你好" start
    }

    #[test]
    fn word_boundary_right_cjk() {
        let s = "hello 你好";
        assert_eq!(word_boundary_right(s, 0), 6); // -> "你好"
    }

    // ── char_display_width ─────────────────────────────────────────

    #[test]
    fn char_display_width_tests() {
        assert_eq!(char_display_width('a'), 1);
        assert_eq!(char_display_width('你'), 2);
        assert_eq!(char_display_width('\t'), 4);
        assert_eq!(char_display_width('🎉'), 2);
    }
}
