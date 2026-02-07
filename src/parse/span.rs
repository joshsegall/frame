use std::ops::Range;

/// Source span information for a parsed node
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    /// Line range in the original file (0-indexed, exclusive end)
    pub line_range: Range<usize>,
}

impl SourceSpan {
    pub fn new(start: usize, end: usize) -> Self {
        SourceSpan {
            line_range: start..end,
        }
    }

    pub fn start(&self) -> usize {
        self.line_range.start
    }

    pub fn end(&self) -> usize {
        self.line_range.end
    }
}
