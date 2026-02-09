use crate::model::inbox::Inbox;

/// Serialize an inbox back to its markdown representation.
/// Clean items emit verbatim source; dirty items emit canonical format.
pub fn serialize_inbox(inbox: &Inbox) -> String {
    let mut lines = Vec::new();

    // Emit header lines
    lines.extend(inbox.header_lines.iter().cloned());

    // Emit items
    for (i, item) in inbox.items.iter().enumerate() {
        if !item.dirty
            && let Some(ref source) = item.source_text
        {
            lines.extend(source.iter().cloned());
            continue;
        }

        // Canonical format
        let mut title_line = format!("- {}", item.title);
        for tag in &item.tags {
            title_line.push_str(&format!(" #{}", tag));
        }
        lines.push(title_line);

        if let Some(ref body) = item.body {
            for body_line in body.lines() {
                if body_line.is_empty() {
                    lines.push(String::new());
                } else {
                    lines.push(format!("  {}", body_line));
                }
            }
        }

        // Add blank line separator between items (not after the last one)
        if i < inbox.items.len() - 1 {
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::inbox_parser::parse_inbox;

    #[test]
    fn test_round_trip_inbox() {
        let source = "\
# Inbox

- Parser crashes on empty effect block #bug
  Saw this when testing with empty `handle {}` blocks.
  Stack trace points to parser/effect.rs line 142.

- Think about whether `perform` should be an expression or statement
  #design

- Read the Koka paper on named handlers #research";

        let inbox = parse_inbox(source);
        let output = serialize_inbox(&inbox);
        assert_eq!(output, source);
    }

    #[test]
    fn test_round_trip_inbox_with_code() {
        let source = "\
# Inbox

- Think about whether `perform` should be an expression or statement
  #design
  If it's an expression, we get composability:
  ```lace
  let x = perform Ask() + 1
  ```
  But it makes the effect type more complex.

- Simple item #bug";

        let inbox = parse_inbox(source);
        let output = serialize_inbox(&inbox);
        assert_eq!(output, source);
    }

    #[test]
    fn test_round_trip_inbox_empty() {
        let source = "# Inbox";
        let inbox = parse_inbox(source);
        let output = serialize_inbox(&inbox);
        assert_eq!(output, source);
    }
}
