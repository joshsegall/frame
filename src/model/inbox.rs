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
    /// Lines that sit *after* this item, separated from it by a blank, that are
    /// neither body nor the next item — a stray note, a heading somebody added,
    /// the residue of a hand edit. Carried verbatim and re-emitted in place,
    /// **including the blank lines that precede them**.
    ///
    /// The inbox's answer to [`crate::model::task::Task::trailing_lines`], and
    /// anchored the same way, to the item *above*. That doc records why: content
    /// anchored to its successor is carried by the wrong neighbour and left
    /// behind when that neighbour moves. Stranded inbox content always has a
    /// predecessor — a line before the first item is header, not stranded — so
    /// the predecessor anchor is total.
    ///
    /// The blanks are part of it because they are what makes the line stranded
    /// rather than body: `- one\nstray` parses as one item whose body is
    /// "stray", while `- one\n\nstray` leaves "stray" with no owner. Emitting
    /// the line without its blank would change what it means on the next read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trailing_lines: Vec<String>,
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
            trailing_lines: Vec::new(),
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
    /// The line ending the file used, re-applied when it is written back.
    /// See [`crate::parse::LineEnding`].
    pub eol: crate::parse::LineEnding,
}

impl Inbox {
    /// Remove an item that is **leaving the inbox** — triaged to a track, or
    /// deleted — and re-anchor anything stranded under it.
    ///
    /// Use this rather than `items.remove` wherever an item goes away for good.
    /// A plain `remove` takes the item's [`InboxItem::trailing_lines`] with it,
    /// which silently loses content frame was carrying precisely so as not to
    /// lose it — P7 catches that on its first generated operation.
    ///
    /// **Not for reordering.** A move removes and re-inserts the same item, so
    /// the anchor is not going anywhere and re-anchoring would duplicate the run
    /// onto its neighbour.
    ///
    /// The run moves to the item above, or to the header when there is no item
    /// above — which is the same position in the file, since the stranded run
    /// sat between this item and the next. It never travels with the item: a
    /// note separated from an item by a blank line was never part of it, and
    /// carrying it into a track on triage would put it somewhere it never was.
    ///
    /// # The run is appended verbatim, blanks and all
    ///
    /// Which can leave one more blank line than strictly needed, when the new
    /// anchor already ended with its own separator. That is deliberate, for two
    /// reasons.
    ///
    /// **An extra blank is the safe direction to err in.** A blank too many is
    /// cosmetic and stable — the file re-reads and re-writes identically. A
    /// blank too *few* changes what the content means: without a separator the
    /// stranded line parses as the anchor's body on the next read, which turns
    /// preserved content into a silent semantic change. Trimming would have to
    /// know whether the anchor emits a separator of its own, and that depends on
    /// whether it is clean or dirty and on whether it is the last item — three
    /// conditions to get right in exchange for one blank line.
    ///
    /// **It makes [`Self::restore_item`] an exact inverse**, which undo needs.
    /// A pure append reverses as a pure truncate; a conditional trim would have
    /// to record what it trimmed in order to put it back. P9 compares the
    /// project byte for byte after undoing everything, so an inverse that is
    /// merely nearly right fails there — as the first version of this did.
    pub fn take_item(&mut self, index: usize) -> InboxItem {
        let item = self.items.remove(index);
        if item.trailing_lines.is_empty() {
            return item;
        }
        let run = item.trailing_lines.clone();
        if index > 0 {
            self.items[index - 1].trailing_lines.extend(run);
        } else {
            // Straight after the header, which is where it already sat.
            self.header_lines.extend(run);
        }
        item
    }

    /// Put back an item [`Self::take_item`] removed, undoing its re-anchoring.
    ///
    /// The exact inverse: the run is a suffix of whatever adopted it, so
    /// dropping that many lines restores the anchor and re-inserting the item
    /// restores the run. Undo of a delete goes through here.
    pub fn restore_item(&mut self, index: usize, item: InboxItem) {
        let n = item.trailing_lines.len();
        if n > 0 {
            let anchor = if index > 0 {
                &mut self.items[index - 1].trailing_lines
            } else {
                &mut self.header_lines
            };
            anchor.truncate(anchor.len().saturating_sub(n));
        }
        self.items.insert(index, item);
    }
}

#[cfg(test)]
mod tests {
    use crate::parse::{parse_inbox, serialize_inbox};

    /// Removing the item that anchors a stranded run must not take the run with
    /// it. P7 catches this on its very first generated operation
    /// (`Triage { item: 2 }`), which is how it was found.
    #[test]
    fn taking_an_item_leaves_what_was_stranded_under_it() {
        let src = "# Inbox\n\n- one\n\nSTRANDED\n\n- two\n";
        let (mut inbox, _) = parse_inbox(src);
        let taken = inbox.take_item(0);

        assert_eq!(taken.title, "one");
        let after = serialize_inbox(&inbox);
        assert!(after.contains("STRANDED"), "{after}");
        assert!(!after.contains("- one"), "the item did leave: {after}");
        // And it is still stranded rather than absorbed as the next item's body.
        let (reread, dropped) = parse_inbox(&after);
        assert!(dropped.is_empty());
        assert_eq!(reread.items.len(), 1);
        assert_eq!(reread.items[0].body, None, "not body: {after}");
    }

    /// The same when the anchor is not the first item, so the run moves onto the
    /// item above rather than into the header.
    #[test]
    fn a_stranded_run_moves_to_the_item_above() {
        let src = "# Inbox\n\n- one\n\n- two\n\nSTRANDED\n\n- three\n";
        let (mut inbox, _) = parse_inbox(src);
        inbox.take_item(1);

        let after = serialize_inbox(&inbox);
        assert!(after.contains("STRANDED"), "{after}");
        assert_eq!(inbox.items[0].title, "one");
        assert!(
            !inbox.items[0].trailing_lines.is_empty(),
            "the item above adopted it"
        );
    }

    /// `restore_item` is `take_item`'s **exact** inverse, byte for byte. Undo
    /// depends on it: P9 compares the whole project after undoing everything,
    /// and a nearly-right inverse leaves the run on the adopting neighbour *and*
    /// on the restored item. Both `InboxDelete` and `InboxTriage` failed that
    /// way before this existed.
    #[test]
    fn restoring_an_item_is_the_exact_inverse_of_taking_it() {
        for src in [
            "# Inbox\n\n- one\n\nSTRANDED\n\n- two\n",
            "# Inbox\n\n- one\n\n- two\n\nSTRANDED\n\n- three\n",
            "# Inbox\n\n- one\n\n- two\n\nSTRANDED\n",
            // No stranded content at all: the pair must still be a round trip.
            "# Inbox\n\n- one\n\n- two\n",
        ] {
            for index in 0..parse_inbox(src).0.items.len() {
                let (mut inbox, _) = parse_inbox(src);
                let taken = inbox.take_item(index);
                inbox.restore_item(index, taken);
                assert_eq!(
                    serialize_inbox(&inbox),
                    src,
                    "take/restore at {index} did not round-trip for {src:?}"
                );
            }
        }
    }
}
