use crate::model::inbox::{Inbox, InboxItem};
use crate::parse::has_continuation_at_indent;
use crate::parse::task_parser::parse_title_and_tags;

/// Parse an inbox file from its source text.
///
/// Inbox format: items separated by blank lines, each starting with `- `.
/// The first line is the title (with optional `#tags`).
/// Subsequent indented lines are the body text.
///
/// Content this parser does not model — a stray note between items, a heading
/// somebody added — is **carried on the item above it** ([`InboxItem::trailing_lines`])
/// rather than dropped. Nothing routine reaches the recovery log.
///
/// Returns the parsed Inbox and a list of dropped lines, which callers log to the
/// recovery log. That list is empty for every input today and
/// `nothing_is_ever_dropped` pins it that way; it stays as the reporting path for
/// a format addition this parser does not yet understand.
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
    // Kept as a backstop and pinned empty (`nothing_is_ever_dropped`). Every
    // shape that used to land here is now carried by the item above it, but the
    // channel stays: a future addition to the format that this parser does not
    // model should reach the recovery log rather than vanish, and a caller that
    // has already wired up the reporting is what makes that automatic.
    let dropped_lines = Vec::new();

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

            // Collect body lines (indented lines until blank line or next item).
            //
            // Both boundaries below are absolute — never conditioned on
            // code-fence state. `serialize_inbox` indents every body line by 2,
            // so a `- ` at column 0 is always the next item, never body content.
            // Suspending that check inside a fence once let an unbalanced fence
            // in one body absorb every item after it; triaging the swallowing
            // item then took the absorbed items with it. Blank lines inside a
            // fenced block are already covered by `has_continuation_at_indent`.
            let mut body_lines = Vec::new();
            while idx < lines.len() {
                let body_line = &lines[idx];
                let body_trimmed = body_line.trim();

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

                // Body line — strip 2 spaces of indent if present
                body_lines.push(strip_body_indent(body_line));
                idx += 1;
            }

            // The blank lines that follow the item. Where they end up depends on
            // what comes after them, so remember where they started.
            let blanks_start = idx;
            while idx < lines.len() && lines[idx].trim().is_empty() {
                idx += 1;
            }

            // Anything here that is not another item is content frame does not
            // model — a stray note, a heading somebody added, the residue of a
            // hand edit. It used to be dropped and reported to the recovery log.
            // It is now carried on the item above it, verbatim.
            //
            // **The blank lines come with it, and that is the whole trick.** A
            // stranded line is only stranded *because* a blank separates it from
            // the item: without one the parser reads it as body
            // (`- one\nstray` gives item "one" with body "stray"). Re-emitting
            // the line without its blank would turn a preserved line into body
            // content on the next read — trading a visible drop for a silent
            // change of meaning, which is worse. So the blanks leave
            // `source_text` and travel with the run.
            let mut trailing_lines = Vec::new();
            if idx < lines.len() && !is_item_line(&lines[idx]) {
                while idx < lines.len() && !is_item_line(&lines[idx]) {
                    idx += 1;
                }
                trailing_lines = lines[blanks_start..idx].to_vec();
            }

            let body = if body_lines.is_empty() {
                None
            } else {
                Some(body_lines.join("\n"))
            };

            // Stops before the blanks when they went to `trailing_lines`, so
            // nothing is emitted twice.
            let own_end = if trailing_lines.is_empty() {
                idx
            } else {
                blanks_start
            };
            let source_text = Some(lines[item_start..own_end].to_vec());

            items.push(InboxItem {
                title,
                tags,
                body,
                source_text,
                trailing_lines,
                dirty: false,
            });
        } else {
            // Blank lines before the first item, or anything else that reached
            // here with no item to carry it. The header loop above takes
            // everything up to the first item, so this is only ever blanks.
            idx += 1;
        }
    }

    (
        Inbox {
            header_lines,
            items,
            source_lines: lines,
            eol: crate::parse::LineEnding::detect(source),
        },
        dropped_lines,
    )
}

/// Whether a line starts a new top-level item.
///
/// The same test the body loop uses to stop: `- ` at column zero. An indented
/// `- ` is a list inside somebody's body text, not the next item.
fn is_item_line(line: &str) -> bool {
    !line.starts_with(' ') && line.trim().starts_with("- ")
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

    /// The next-item boundary is absolute. Regression: suspending it inside a
    /// code fence let an unbalanced fence in one body absorb every later item,
    /// so triaging the swallowing item carried them off with it.
    #[test]
    fn test_parse_inbox_unbalanced_fence_does_not_swallow_later_items() {
        let source = "\
# Inbox

- First item with an unbalanced fence
  Example:
  ```
- Second item
- Third item
";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 3);
        assert_eq!(inbox.items[0].title, "First item with an unbalanced fence");
        assert_eq!(inbox.items[1].title, "Second item");
        assert_eq!(inbox.items[2].title, "Third item");

        // The first item keeps its own body — and only its own.
        let body = inbox.items[0].body.as_ref().unwrap();
        assert_eq!(body, "Example:\n```");
    }

    /// An unbalanced fence in the *last* item has no later items to swallow, but
    /// must still leave the body intact.
    #[test]
    fn test_parse_inbox_unbalanced_fence_in_last_item() {
        let source = "\
# Inbox

- Only item
  ```rust
  fn main() {}
";
        let (inbox, _) = parse_inbox(source);
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(
            inbox.items[0].body.as_deref(),
            Some("```rust\nfn main() {}")
        );
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

    /// Lines between items that are neither body nor a new item are carried on
    /// the item above, not dropped.
    ///
    /// This test used to assert the opposite — that they landed in `dropped` and
    /// went to the recovery log. That is the wrong home for them: the log is a
    /// last resort for content that could not be kept, and this content can be
    /// kept. Lines before the first item are header, not stranded, which is what
    /// makes the item-above anchor total.
    #[test]
    fn test_parse_inbox_keeps_stray_lines_on_the_item_above() {
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

        assert!(
            dropped.is_empty(),
            "nothing routine reaches the log: {dropped:?}"
        );
        assert_eq!(
            inbox.items[0].trailing_lines,
            vec![
                "".to_string(),
                "Stray line between items".to_string(),
                "Another stray line".to_string(),
            ],
            "the separating blank travels with the run — without it the next \
             read takes these lines as the item's body"
        );
        assert!(inbox.items[1].trailing_lines.is_empty());

        // And the file comes back byte for byte.
        assert_eq!(crate::parse::serialize_inbox(&inbox), source);
    }

    /// The same content, after the carrying item is edited. The stranded lines
    /// survive a canonical rewrite of their anchor, and the blank that keeps
    /// them stranded survives with them.
    #[test]
    fn a_dirty_item_still_carries_what_was_stranded_under_it() {
        let source = "# Inbox\n\n- First\n\nstray\n\n- Second\n";
        let (mut inbox, _) = parse_inbox(source);
        inbox.items[0].title = "First, edited".to_string();
        inbox.items[0].dirty = true;

        let written = crate::parse::serialize_inbox(&inbox);
        assert_eq!(written, "# Inbox\n\n- First, edited\n\nstray\n\n- Second\n");

        // Re-reading finds it stranded again rather than as body, which is the
        // property the blank is there to hold.
        let (again, dropped) = parse_inbox(&written);
        assert!(dropped.is_empty());
        assert_eq!(again.items[0].body, None, "still not body");
        // The run reaches to the next item line, so the blank on either side of
        // `stray` belongs to it. That is what makes the carry lossless.
        assert_eq!(
            again.items[0].trailing_lines,
            vec!["".to_string(), "stray".to_string(), "".to_string()]
        );
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
