//! Shared scroll-offset adjustment for the scrolling list views.
//!
//! Every list view (track, inbox, recent, search, board) builds the full set of
//! lines it could render, locates the cursor item within them, and then needs to
//! pick a scroll offset that keeps that item on screen. This module centralizes
//! that decision so the behavior is identical everywhere:
//!
//! * the cursor item's *full* extent is revealed when it fits (so a multi-line
//!   task summary is never clipped to its first line), and
//! * a scrolloff `margin` is kept between the cursor and the top/bottom edges, so
//!   the view starts scrolling before the cursor reaches the very edge.

/// Lines to keep between the cursor item and the top/bottom edge while scrolling.
pub const SCROLL_MARGIN: usize = 4;

/// Compute a new scroll offset (in display-line space) that keeps the cursor item
/// visible with a scrolloff `margin`.
///
/// * `scroll` — current scroll offset (index of the first visible line)
/// * `viewport` — number of visible lines
/// * `item_start` — first line of the cursor item
/// * `item_end` — last line of the cursor item, inclusive (== `item_start` for a
///   single-line item)
/// * `margin` — desired scrolloff; clamped so the top and bottom margins can't
///   overlap within the viewport
///
/// The bottom of the item is revealed first and the top second, so an item that
/// is taller than the viewport is anchored to its first line and truncated at the
/// bottom rather than the reverse.
pub fn adjust_scroll(
    scroll: usize,
    viewport: usize,
    item_start: usize,
    item_end: usize,
    margin: usize,
) -> usize {
    if viewport == 0 {
        return scroll;
    }
    // Don't let the top and bottom margins overlap (or exceed the viewport).
    let margin = margin.min(viewport.saturating_sub(1) / 2);
    let mut scroll = scroll;

    // 1. Reveal the bottom of the item (plus margin) if it sits below the fold.
    let want_bottom = item_end + margin;
    if want_bottom >= scroll + viewport {
        scroll = want_bottom + 1 - viewport;
    }
    // 2. Reveal the top of the item (minus margin) if it sits above the fold.
    //    This runs second so that, for an item taller than the viewport, the
    //    first line always wins and the item is truncated at the bottom.
    let want_top = item_start.saturating_sub(margin);
    if want_top < scroll {
        scroll = want_top;
    }
    scroll
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_when_comfortably_visible() {
        // Cursor item well inside the viewport, away from both margins.
        assert_eq!(adjust_scroll(0, 20, 10, 10, SCROLL_MARGIN), 0);
    }

    #[test]
    fn viewport_zero_is_noop() {
        assert_eq!(adjust_scroll(5, 0, 100, 100, SCROLL_MARGIN), 5);
    }

    #[test]
    fn scrolls_down_with_bottom_margin() {
        // Single-line cursor approaching the bottom edge: scroll early so the
        // cursor keeps `margin` lines below it.
        // viewport 20, margin 4 => cursor at line 18 wants bottom = 22 visible.
        // new scroll = 22 + 1 - 20 = 3.
        assert_eq!(adjust_scroll(0, 20, 18, 18, 4), 3);
    }

    #[test]
    fn scrolls_up_with_top_margin() {
        // Cursor near the top of the current window: keep `margin` lines above.
        // scroll 10, cursor at 11, margin 4 => want_top = 7 < 10 => scroll = 7.
        assert_eq!(adjust_scroll(10, 20, 11, 11, 4), 7);
    }

    #[test]
    fn multiline_item_fully_revealed() {
        // A 5-line item whose bottom is just past the fold should be pulled fully
        // into view (plus margin), not clipped to its first line.
        // viewport 20, item lines 17..=21, margin 4 => want_bottom = 25 =>
        // scroll = 25 + 1 - 20 = 6. item_start 17 >= 6+4, top step no-op.
        assert_eq!(adjust_scroll(0, 20, 17, 21, 4), 6);
    }

    #[test]
    fn tall_item_anchors_to_top() {
        // Item taller than the viewport: show from its first line and truncate at
        // the bottom (top step wins over the bottom step).
        // viewport 10, item lines 30..=50. bottom step => scroll = 50+4+1-10 = 45,
        // then top step => want_top = 30 - margin. margin clamps to (10-1)/2 = 4,
        // so want_top = 26 < 45 => scroll = 26, and line 30 is visible.
        let scroll = adjust_scroll(0, 10, 30, 50, 4);
        assert!(scroll <= 30, "first line of tall item must be visible");
        assert_eq!(scroll, 26);
    }

    #[test]
    fn margin_clamped_in_small_viewport() {
        // Viewport too small for a margin of 4: clamp keeps it sane and the
        // cursor still ends up on screen.
        // viewport 3 => margin clamps to (3-1)/2 = 1.
        // cursor at 10, scroll 0 => want_bottom = 11 => scroll = 11 + 1 - 3 = 9.
        assert_eq!(adjust_scroll(0, 3, 10, 10, 4), 9);
    }

    #[test]
    fn single_line_viewport() {
        // Degenerate 1-line viewport: margin clamps to 0, cursor pinned.
        assert_eq!(adjust_scroll(0, 1, 7, 7, 4), 7);
    }
}
