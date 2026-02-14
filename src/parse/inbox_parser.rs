use crate::model::inbox::{Inbox, InboxItem};
use crate::parse::has_continuation_at_indent;
use crate::parse::task_parser::parse_title_and_tags;

/// Parse an inbox file from its source text.
///
/// Inbox format: items separated by blank lines, each starting with `- `.
/// The first line is the title (with optional `#tags`).
/// Subsequent indented lines are the body text.
///
/// Returns the parsed Inbox and a list of lines that were dropped (not recognized
/// as items, headers, or blank lines). Callers can log these to the recovery log.
pub fn parse_inbox(source: &str) -> (Inbox, Vec<String>) {
    let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();

    // Parse header lines (everything before the first item)
    let mut header_lines = Vec::new();
    let mut idx = 0;

    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.starts_with("- ") {
            break;
        }
        header_lines.push(lines[idx].clone());
        idx += 1;
    }

    // Parse items
    let mut items = Vec::new();
    let mut dropped_lines = Vec::new();

    while idx < lines.len() {
        let line = &lines[idx];
        let trimmed = line.trim();

        if let Some(title_content) = trimmed.strip_prefix("- ") {
            let item_start = idx;
            // Skip "- "
            let (title, mut tags) = parse_title_and_tags(title_content);

            idx += 1;

            // Check for tag-only continuation lines before body text.
            // Lines like `  #design` or `  #cc-added #bug` are tags, not body.
            while idx < lines.len() {
                let cont_line = &lines[idx];
                let cont_trimmed = cont_line.trim();
                if cont_trimmed.is_empty()
                    || (!cont_line.starts_with(' ') && cont_trimmed.starts_with("- "))
                {
                    break;
                }
                if is_tag_only_line(cont_trimmed) {
                    // Parse tags from this line
                    for word in cont_trimmed.split_whitespace() {
                        if let Some(tag) = word.strip_prefix('#')
                            && !tag.is_empty()
                        {
                            tags.push(tag.to_string());
                        }
                    }
                    idx += 1;
                } else {
                    break;
                }
            }

            // Collect body lines (indented lines until blank line or next item)
            let mut body_lines = Vec::new();
            let mut in_code_fence = false;
            while idx < lines.len() {
                let body_line = &lines[idx];
                let body_trimmed = body_line.trim();

                // Track fenced code blocks so blank lines inside them don't end the body
                if body_trimmed.starts_with("```") {
                    in_code_fence = !in_code_fence;
                }

                if !in_code_fence {
                    if body_trimmed.is_empty() {
                        // Blank line — check if more body content follows
                        // (indented lines at 1+ spaces). If so, this is a
                        // paragraph break within the body, not the item separator.
                        if has_continuation_at_indent(&lines, idx + 1, 1) {
                            body_lines.push(String::new());
                            idx += 1;
                            continue;
                        }
                        break;
                    }

                    if body_trimmed.starts_with("- ") && !body_line.starts_with(' ') {
                        // Next item at top level
                        break;
                    }
                }

                // Body line — strip 2 spaces of indent if present
                body_lines.push(strip_body_indent(body_line));
                idx += 1;
            }

            // Skip blank lines between items
            while idx < lines.len() && lines[idx].trim().is_empty() {
                idx += 1;
            }

            let body = if body_lines.is_empty() {
                None
            } else {
                Some(body_lines.join("\n"))
            };

            let source_text = Some(lines[item_start..idx].to_vec());

            items.push(InboxItem {
                title,
                tags,
                body,
                source_text,
                dirty: false,
            });
        } else if trimmed.is_empty() {
            // Skip blank lines
            idx += 1;
        } else {
            // Unexpected non-item line — record as dropped
            dropped_lines.push(lines[idx].clone());
            idx += 1;
        }
    }

    (
        Inbox {
            header_lines,
            items,
            source_lines: lines,
        },
        dropped_lines,
    )
}

/// Check if a line consists entirely of `#tag` words
fn is_tag_only_line(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    trimmed
        .split_whitespace()
        .all(|word| word.starts_with('#') && word.len() > 1)
}

/// Strip 2 spaces of indent from a body line
fn strip_body_indent(line: &str) -> String {
    if let Some(stripped) = line.strip_prefix("  ") {
        stripped.to_string()
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inbox_basic() {
        let source = "\
# Inbox

- Parser crashes on empty effect block #bug
  Saw this when testing with empty `handle {}` blocks.
  Stack trace points to parser/effect.rs line 142.

- Think about whether `perform` should be an expression or statement
  #design

- Read the Koka paper on named handlers #research
";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.header_lines.len(), 2); // "# Inbox" + ""
        assert_eq!(inbox.items.len(), 3);

        assert_eq!(inbox.items[0].title, "Parser crashes on empty effect block");
        assert_eq!(inbox.items[0].tags, vec!["bug"]);
        assert!(
            inbox.items[0]
                .body
                .as_ref()
                .unwrap()
                .contains("Saw this when testing")
        );

        assert_eq!(
            inbox.items[1].title,
            "Think about whether `perform` should be an expression or statement"
        );
        assert_eq!(inbox.items[1].tags, vec!["design"]);

        assert_eq!(
            inbox.items[2].title,
            "Read the Koka paper on named handlers"
        );
        assert_eq!(inbox.items[2].tags, vec!["research"]);
        assert!(inbox.items[2].body.is_none());
    }

    #[test]
    fn test_parse_inbox_with_code_block_in_body() {
        let source = "\
# Inbox

- Think about whether `perform` should be an expression or statement
  #design
  If it's an expression, we get composability:
  ```lace
  let x = perform Ask() + 1
  ```
  But it makes the effect type more complex.

- Simple item #bug
";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert!(body.contains("```lace"));
        assert!(body.contains("perform Ask()"));
    }

    #[test]
    fn test_parse_inbox_code_block_with_blank_line() {
        let source = "\
# Inbox

- Item with code block containing blank line #bug
  Here's the code:
  ```
  fn main() {

      println!(\"hello\");
  }
  ```
  Text after code block.

- Next item
";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert!(body.contains("fn main()"));
        assert!(body.contains("println!"));
        assert!(body.contains("Text after code block."));
        // The blank line inside the code block should be preserved
        assert!(body.contains("\n\n"));

        assert_eq!(inbox.items[1].title, "Next item");
    }

    #[test]
    fn test_parse_inbox_empty() {
        let source = "# Inbox\n";
        let (inbox, _) = parse_inbox(source);
        assert!(inbox.items.is_empty());
    }

    #[test]
    fn test_parse_inbox_body_with_blank_lines() {
        let source = "\
# Inbox

- Multi-paragraph item #design
  First paragraph of body.

  Second paragraph of body.

- Next item #bug";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert!(body.contains("First paragraph"));
        assert!(body.contains("Second paragraph"));
        assert!(
            body.contains("\n\n"),
            "blank line within body should be preserved"
        );

        assert_eq!(inbox.items[1].title, "Next item");
        assert_eq!(inbox.items[1].tags, vec!["bug"]);
    }

    #[test]
    fn test_parse_inbox_body_multiple_blank_lines() {
        let source = "\
# Inbox

- Item with double blank #tag
  Para one.


  Para two.

- Next";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert!(body.contains("Para one."));
        assert!(body.contains("Para two."));
        // Two consecutive blank lines should both be preserved
        assert!(body.contains("\n\n\n"));
    }

    #[test]
    fn test_parse_inbox_body_blank_line_before_code_block() {
        let source = "\
# Inbox

- Item with code #dev
  Some text.

  ```
  fn main() {}
  ```

- Next";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert!(body.contains("Some text."));
        assert!(body.contains("fn main()"));
    }

    #[test]
    fn test_parse_inbox_dropped_lines() {
        // Lines between items that don't start with "- " are dropped.
        // Note: lines before the first item are collected as header, not dropped.
        let source = "\
# Inbox

- First item #bug

Stray line between items
Another stray line
- Second item
";
        let (inbox, dropped) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);
        assert_eq!(inbox.items[0].title, "First item");
        assert_eq!(inbox.items[1].title, "Second item");

        assert_eq!(dropped.len(), 2);
        assert_eq!(dropped[0], "Stray line between items");
        assert_eq!(dropped[1], "Another stray line");
    }

    #[test]
    fn test_parse_inbox_no_dropped_lines() {
        let source = "\
# Inbox

- First item #bug

- Second item #design
";
        let (inbox, dropped) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);
        assert!(dropped.is_empty());
    }

    #[test]
    fn test_parse_inbox_trailing_blank_not_in_body() {
        // A blank line followed by a new item should NOT be included in the body
        let source = "\
# Inbox

- First item
  Body text.

- Second item";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert_eq!(body, "Body text.");
        assert!(!body.contains('\n'), "no trailing blank should be in body");
    }
}
