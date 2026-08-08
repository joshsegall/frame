use crate::model::inbox::Inbox;

/// Serialize an inbox back to its markdown representation.
/// Clean items emit verbatim source; dirty items emit canonical format.
///
/// # Mixed spacing is accepted output — do not "fix" it
///
/// Because a clean item is emitted verbatim and a dirty one canonically, editing
/// one item of a compactly-written inbox leaves the file with both spellings:
///
/// ```text
/// - one
/// - two, edited     <- gains a blank below it, being dirty now
///
/// - three
/// ```
///
/// That is the selective-rewrite design working, not a bug in it. A compact
/// inbox migrates to canonical **one item at a time**, as each item is touched,
/// and every intermediate state is a fixpoint of this pair — re-reading and
/// re-writing reproduces it exactly, so it is stable rather than churning.
///
/// The alternative was rejected on purpose: normalising every item whenever any
/// one is dirty turns a one-word edit into a whole-file diff, which is the exact
/// cost `source_text` exists to avoid. Making the separator depend on what the
/// neighbours look like was rejected too — it makes the output a function of
/// parse state, which is far harder to state as an invariant than "clean is
/// verbatim, dirty is canonical".
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
            lines.extend(item.trailing_lines.iter().cloned());
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

        // Add blank line separator between items (not after the last one).
        //
        // Skipped when this item carries stranded content, because that content
        // already begins with the blank lines that separated it from the item —
        // see `InboxItem::trailing_lines`. Adding another here would insert a
        // blank the file never had, on every write.
        if item.trailing_lines.is_empty() && i < inbox.items.len() - 1 {
            lines.push(String::new());
        }
        lines.extend(item.trailing_lines.iter().cloned());
    }

    // Same as the track serializer: with no items left, the blank under the
    // `# Inbox` header has nothing to separate and would be re-added forever.
    if inbox.items.is_empty() {
        crate::parse::pop_trailing_blanks(&mut lines);
    }

    let mut out = lines.join("\n");
    out.push('\n');
    inbox.eol.apply(out)
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

- Read the Koka paper on named handlers #research
";

        let (inbox, _) = parse_inbox(source);
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

- Simple item #bug
";

        let (inbox, _) = parse_inbox(source);
        let output = serialize_inbox(&inbox);
        assert_eq!(output, source);
    }

    #[test]
    fn test_round_trip_inbox_empty() {
        let source = "# Inbox\n";
        let (inbox, _) = parse_inbox(source);
        let output = serialize_inbox(&inbox);
        assert_eq!(output, source);
    }

    #[test]
    fn test_round_trip_inbox_body_with_blank_lines() {
        let source = "\
# Inbox

- Multi-paragraph item #design
  First paragraph.

  Second paragraph.

- Simple item #bug
";

        let (inbox, _) = parse_inbox(source);
        let output = serialize_inbox(&inbox);
        assert_eq!(output, source);
    }
}
