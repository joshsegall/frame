use crate::model::inbox::{Inbox, InboxItem};
use crate::parse::has_continuation_at_indent;

/// Parse an inbox file from its source text.
///
/// Inbox format: items separated by blank lines, each starting with `- `.
/// The first line is the title (with optional `#tags`).
/// Subsequent indented lines are the body text.
pub fn parse_inbox(source: &str) -> Inbox {
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

    while idx < lines.len() {
        let line = &lines[idx];
        let trimmed = line.trim();

        if let Some(title_content) = trimmed.strip_prefix("- ") {
            let item_start = idx;
            // Skip "- "
            let (title, mut tags) = parse_inbox_title_and_tags(title_content);

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
            // Unexpected non-item line — skip
            idx += 1;
        }
    }

    Inbox {
        header_lines,
        items,
        source_lines: lines,
    }
}

/// Parse an inbox item's title line into title and tags.
/// Tags are `#word` tokens at the end of the line.
fn parse_inbox_title_and_tags(s: &str) -> (String, Vec<String>) {
    let s = s.trim_end();
    if s.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut tags = Vec::new();
    let mut remaining = s;

    loop {
        let trimmed = remaining.trim_end();
        if trimmed.is_empty() {
            break;
        }

        if let Some(last_space) = trimmed.rfind(' ') {
            let last_word = &trimmed[last_space + 1..];
            if let Some(tag) = last_word.strip_prefix('#')
                && !tag.is_empty()
                && !tag.contains('#')
            {
                tags.push(tag.to_string());
                remaining = &trimmed[..last_space];
                continue;
            }
        } else if let Some(tag) = trimmed.strip_prefix('#')
            && !tag.is_empty()
            && !tag.contains('#')
        {
            tags.push(tag.to_string());
            remaining = "";
            continue;
        }
        break;
    }

    tags.reverse();
    (remaining.trim_end().to_string(), tags)
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
        let inbox = parse_inbox(source);
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
        let inbox = parse_inbox(source);
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
        let inbox = parse_inbox(source);
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
        let inbox = parse_inbox(source);
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
        let inbox = parse_inbox(source);
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
        let inbox = parse_inbox(source);
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
        let inbox = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert!(body.contains("Some text."));
        assert!(body.contains("fn main()"));
    }

    #[test]
    fn test_parse_inbox_trailing_blank_not_in_body() {
        // A blank line followed by a new item should NOT be included in the body
        let source = "\
# Inbox

- First item
  Body text.

- Second item";
        let inbox = parse_inbox(source);
        assert_eq!(inbox.items.len(), 2);

        let body = inbox.items[0].body.as_ref().unwrap();
        assert_eq!(body, "Body text.");
        assert!(!body.contains('\n'), "no trailing blank should be in body");
    }
}
