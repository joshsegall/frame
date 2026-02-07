use serde::{Deserialize, Serialize};

/// An inbox item (quick-capture, no ID)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    /// The title/first line of the item
    pub title: String,
    /// Tags (without `#` prefix)
    pub tags: Vec<String>,
    /// Body text (subsequent indented lines)
    pub body: Option<String>,
    /// Original source lines for round-trip preservation
    #[serde(skip)]
    pub source_text: Option<Vec<String>>,
    /// Whether this item has been modified
    #[serde(skip)]
    pub dirty: bool,
}

impl InboxItem {
    pub fn new(title: String) -> Self {
        InboxItem {
            title,
            tags: Vec::new(),
            body: None,
            source_text: None,
            dirty: true,
        }
    }
}

/// The parsed inbox file
#[derive(Debug, Clone)]
pub struct Inbox {
    /// The header lines (e.g., `# Inbox\n`)
    pub header_lines: Vec<String>,
    /// Inbox items
    pub items: Vec<InboxItem>,
    /// The original source lines
    pub source_lines: Vec<String>,
}
