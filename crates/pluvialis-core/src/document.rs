//! An editable document that steno writes into at the caret.
//!
//! Up to M4b the document was a pure function of the translator's history:
//! every stroke reformatted the whole history and replaced the text wholesale.
//! That is what makes retroactive correction fall out for free, and it is kept,
//! but it cannot support a caret. Anything typed by hand would be discarded by
//! the next stroke, and steno could only ever land at the end.
//!
//! So the formatter is unchanged and its output is treated as a *shadow* of
//! what steno has produced. Each stroke diffs the previous shadow against the
//! new one, which yields "delete this many bytes, insert this text", and that
//! edit is applied at the caret. The document itself is an ordinary buffer that
//! the user may edit however they like.
//!
//! Red raw-steno ranges are byte ranges into that buffer, and every edit
//! shifts, trims or splits them. They are never recomputed from the text, so
//! colour stays attached to the characters it belongs to even after an
//! insertion in the middle of the document.

use crate::format::Formatted;

/// What one stroke changes: delete `backspaces` bytes before the caret, then
/// insert `text`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StenoEdit {
    /// Bytes to delete, for the in-app document, which indexes by byte.
    pub backspaces: usize,
    /// Backspace *keypresses* to send, for another application, since one
    /// keypress deletes one character rather than one byte.
    ///
    /// The two differ for any non-ASCII text and the user's Dutch dictionary is
    /// full of it. Using the byte count to drive keystrokes would eat extra
    /// characters, silently and only sometimes.
    pub backspace_keys: usize,
    pub text: String,
    /// Ranges within `text` that are untranslated steno.
    pub raw_ranges: Vec<(usize, usize)>,
}

impl StenoEdit {
    pub fn is_empty(&self) -> bool {
        self.backspaces == 0 && self.text.is_empty()
    }
}

/// Bytes shared at the start of both strings, backed off to a character
/// boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    let limit = a.len().min(b.len());
    let mut at = 0;
    while at < limit && a.as_bytes()[at] == b.as_bytes()[at] {
        at += 1;
    }
    while at > 0 && !(a.is_char_boundary(at) && b.is_char_boundary(at)) {
        at -= 1;
    }
    at
}

/// Bytes shared at the end of both strings, never reaching back past `floor`
/// in either, and backed off to a character boundary.
fn common_suffix(a: &str, b: &str, floor: usize) -> usize {
    let limit = (a.len() - floor).min(b.len() - floor);
    let mut at = 0;
    while at < limit && a.as_bytes()[a.len() - 1 - at] == b.as_bytes()[b.len() - 1 - at] {
        at += 1;
    }
    while at > 0 && !(a.is_char_boundary(a.len() - at) && b.is_char_boundary(b.len() - at)) {
        at -= 1;
    }
    at
}

/// Move `ranges` across a splice that replaced `start..end` with `inserted`
/// bytes.
///
/// A range wholly before the splice is untouched, one wholly after shifts, and
/// one that straddles it is trimmed. A range spanning the whole replaced
/// region splits in two, which is why this rebuilds the vector rather than
/// editing in place.
fn splice_ranges(
    ranges: &[(usize, usize)],
    start: usize,
    end: usize,
    inserted: usize,
) -> Vec<(usize, usize)> {
    let removed = end - start;
    // Only ever called with `at >= end`, so this cannot underflow.
    let shift = |at: usize| at - removed + inserted;

    let mut out = Vec::with_capacity(ranges.len());
    for &(range_start, range_end) in ranges {
        if range_end <= start {
            out.push((range_start, range_end));
            continue;
        }
        if range_start >= end {
            out.push((shift(range_start), shift(range_end)));
            continue;
        }
        // Straddles the splice: keep whatever lies outside it.
        if range_start < start {
            out.push((range_start, start));
        }
        if range_end > end {
            out.push((shift(end), shift(range_end)));
        }
    }
    out
}

/// The text the user sees, with the caret steno writes at.
#[derive(Debug, Clone, Default)]
pub struct Document {
    text: String,
    /// Sorted, non-overlapping byte ranges of untranslated steno.
    raw_ranges: Vec<(usize, usize)>,
    caret: usize,
    revision: u64,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    /// Increments whenever the text changes, and never otherwise.
    ///
    /// For callers that derive something expensive from the text and are called
    /// every frame. Counting words on a 45,000 word document takes 204us, which
    /// is 1.2% of a core at 60 fps and grows with the document; comparing a
    /// `u64` instead is free. Moving the caret does **not** change this, because
    /// nothing derived from the text needs recomputing when it does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn raw_ranges(&self) -> &[(usize, usize)] {
        &self.raw_ranges
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Move the caret, clamped into the text and onto a character boundary.
    ///
    /// The caret arrives from the text widget, which measures a galley that can
    /// be a frame out of date, so it is not trusted.
    pub fn set_caret(&mut self, byte: usize) {
        let mut at = byte.min(self.text.len());
        while at > 0 && !self.text.is_char_boundary(at) {
            at -= 1;
        }
        self.caret = at;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.raw_ranges.clear();
        self.caret = 0;
        self.revision += 1;
    }

    /// Byte offset for a character offset.
    ///
    /// Text widgets count characters; this document counts bytes. Conflating
    /// the two works perfectly until the first accented character, which the
    /// user's Dutch dictionary has in quantity, and then misplaces the caret.
    pub fn byte_of_char(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(byte, _)| byte)
            .unwrap_or(self.text.len())
    }

    /// Character offset for a byte offset, for handing the caret back.
    pub fn char_of_byte(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        self.text[..byte].chars().count()
    }

    /// Where the caret sits, in characters.
    pub fn caret_char(&self) -> usize {
        self.char_of_byte(self.caret)
    }

    /// Move the caret, given a character offset.
    pub fn set_caret_char(&mut self, char_index: usize) {
        let byte = self.byte_of_char(char_index);
        self.set_caret(byte);
    }

    /// Apply one stroke's edit at the caret.
    pub fn apply(&mut self, edit: &StenoEdit) {
        // Never delete past the start of the document. Backspaces can exceed
        // what is in front of the caret when the user has deleted
        // steno-produced text by hand.
        let mut start = self.caret.saturating_sub(edit.backspaces);
        while start > 0 && !self.text.is_char_boundary(start) {
            start -= 1;
        }
        let end = self.caret;

        self.raw_ranges = splice_ranges(&self.raw_ranges, start, end, edit.text.len());
        for &(range_start, range_end) in &edit.raw_ranges {
            self.raw_ranges
                .push((start + range_start, start + range_end));
        }
        self.raw_ranges.sort_unstable();

        self.text.replace_range(start..end, &edit.text);
        self.caret = start + edit.text.len();
        self.revision += 1;
    }

    /// Take in text the user edited by hand, keeping the red ranges attached to
    /// the characters they belong to.
    ///
    /// The widget hands back a whole string rather than an edit, so the change
    /// is recovered by diffing. Typed text is never steno, so the changed
    /// region carries no new red.
    pub fn reconcile(&mut self, new_text: &str) {
        if new_text == self.text {
            return;
        }
        self.revision += 1;

        let prefix = common_prefix(&self.text, new_text);
        let suffix = common_suffix(&self.text, new_text, prefix);
        let old_end = self.text.len() - suffix;
        let new_end = new_text.len() - suffix;

        self.raw_ranges = splice_ranges(&self.raw_ranges, prefix, old_end, new_end - prefix);
        self.text = new_text.to_owned();
        self.caret = self.caret.min(self.text.len());
        while self.caret > 0 && !self.text.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
    }
}

/// Work out what changed between two formatter outputs.
///
/// The shared prefix is left alone, so a stroke that only appends produces no
/// backspaces, and a retroactive correction produces exactly enough to reach
/// back to the point where the two differ.
pub fn steno_edit(previous: &Formatted, next: &Formatted) -> StenoEdit {
    let prefix = common_prefix(&previous.text, &next.text);

    let raw_ranges = next
        .raw_ranges
        .iter()
        .filter_map(|&(start, end)| {
            // Ranges before the prefix describe text that did not change, so
            // the document's existing colour there is still right.
            let start = start.max(prefix);
            (end > start).then(|| (start - prefix, end - prefix))
        })
        .collect();

    StenoEdit {
        backspaces: previous.text.len() - prefix,
        backspace_keys: previous.text[prefix..].chars().count(),
        text: next.text[prefix..].to_owned(),
        raw_ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted(text: &str, raw_ranges: Vec<(usize, usize)>) -> Formatted {
        Formatted {
            text: text.to_owned(),
            raw_ranges,
            ..Default::default()
        }
    }

    fn document(text: &str, raw_ranges: Vec<(usize, usize)>, caret: usize) -> Document {
        Document {
            text: text.to_owned(),
            raw_ranges,
            caret,
            ..Default::default()
        }
    }

    /// The red text, for asserting colour landed on the right characters.
    fn red(document: &Document) -> Vec<&str> {
        document
            .raw_ranges
            .iter()
            .map(|&(start, end)| &document.text[start..end])
            .collect()
    }

    /// One backspace keypress deletes one character, not one byte. Driving
    /// keystrokes from the byte count eats extra characters on any accented
    /// text, silently and only sometimes.
    #[test]
    fn backspace_keypresses_are_counted_in_characters_not_bytes() {
        // Three e-acutes: six bytes, three characters.
        let edit = steno_edit(
            &formatted("\u{00E9}\u{00E9}\u{00E9}", vec![]),
            &formatted("", vec![]),
        );
        assert_eq!(edit.backspaces, 6, "bytes, for the in-app document");
        assert_eq!(edit.backspace_keys, 3, "keypresses, for another app");
    }

    #[test]
    fn for_ascii_the_two_backspace_counts_agree() {
        let edit = steno_edit(&formatted("cat", vec![]), &formatted("", vec![]));
        assert_eq!(edit.backspaces, 3);
        assert_eq!(edit.backspace_keys, 3);
    }

    #[test]
    fn appending_produces_no_backspaces() {
        let edit = steno_edit(&formatted("cat", vec![]), &formatted("cat dog", vec![]));
        assert_eq!(edit.backspaces, 0);
        assert_eq!(edit.text, " dog");
    }

    #[test]
    fn a_retroactive_correction_reaches_back_only_as_far_as_it_differs() {
        // "wel come" becoming "welcome": the shared "wel" is left alone.
        let edit = steno_edit(
            &formatted("wel come", vec![]),
            &formatted("welcome", vec![]),
        );
        assert_eq!(edit.backspaces, 5, "should not rewrite the shared prefix");
        assert_eq!(edit.text, "come");
    }

    #[test]
    fn an_edit_carries_the_raw_ranges_of_the_text_it_inserts() {
        let edit = steno_edit(
            &formatted("cat ", vec![]),
            &formatted("cat KAT", vec![(4, 7)]),
        );
        assert_eq!(edit.text, "KAT");
        assert_eq!(
            edit.raw_ranges,
            vec![(0, 3)],
            "relative to the inserted text"
        );
    }

    #[test]
    fn raw_ranges_in_the_unchanged_prefix_are_not_repeated() {
        let edit = steno_edit(
            &formatted("KAT", vec![(0, 3)]),
            &formatted("KAT dog", vec![(0, 3)]),
        );
        assert_eq!(edit.text, " dog");
        assert!(
            edit.raw_ranges.is_empty(),
            "the document already has it red"
        );
    }

    #[test]
    fn applying_at_the_end_appends() {
        let mut doc = document("cat", vec![], 3);
        doc.apply(&StenoEdit {
            backspaces: 0,
            backspace_keys: 0,
            text: " dog".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "cat dog");
        assert_eq!(doc.caret(), 7);
    }

    /// The whole point of the milestone.
    #[test]
    fn applying_mid_sentence_inserts_at_the_caret() {
        let mut doc = document("the dog", vec![], 3);
        doc.apply(&StenoEdit {
            backspaces: 0,
            backspace_keys: 0,
            text: " big".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "the big dog");
        assert_eq!(doc.caret(), 7, "caret follows the inserted text");
    }

    #[test]
    fn inserting_before_red_text_moves_the_red_along_with_it() {
        // "KAT" is red at 4..7; typing at the very start must carry it right.
        let mut doc = document("cat KAT", vec![(4, 7)], 0);
        doc.apply(&StenoEdit {
            backspaces: 0,
            backspace_keys: 0,
            text: "big ".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "big cat KAT");
        assert_eq!(red(&doc), vec!["KAT"], "red must still be on KAT");
    }

    #[test]
    fn backspacing_removes_the_red_range_it_deletes() {
        let mut doc = document("cat KAT", vec![(4, 7)], 7);
        doc.apply(&StenoEdit {
            backspaces: 3,
            backspace_keys: 3,
            text: "dog".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "cat dog");
        assert!(red(&doc).is_empty(), "the red steno was replaced");
    }

    #[test]
    fn an_edit_inside_a_red_range_splits_it() {
        let mut doc = document("KATTKOG", vec![(0, 7)], 3);
        doc.apply(&StenoEdit {
            backspaces: 0,
            backspace_keys: 0,
            text: " ".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "KAT TKOG");
        assert_eq!(red(&doc), vec!["KAT", "TKOG"], "one range became two");
    }

    #[test]
    fn backspaces_cannot_delete_past_the_start_of_the_document() {
        let mut doc = document("ab", vec![], 2);
        doc.apply(&StenoEdit {
            backspaces: 99,
            backspace_keys: 99,
            text: "z".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "z");
        assert_eq!(doc.caret(), 1);
    }

    #[test]
    fn a_stroke_landing_in_the_middle_carries_its_own_red() {
        let mut doc = document("cat dog", vec![], 3);
        doc.apply(&StenoEdit {
            backspaces: 0,
            backspace_keys: 0,
            text: " TPHRPBLG".to_owned(),
            raw_ranges: vec![(1, 9)],
        });
        assert_eq!(doc.text(), "cat TPHRPBLG dog");
        assert_eq!(red(&doc), vec!["TPHRPBLG"]);
    }

    #[test]
    fn typing_by_hand_keeps_red_attached_to_its_characters() {
        let mut doc = document("cat KAT", vec![(4, 7)], 0);
        doc.reconcile("a cat KAT");
        assert_eq!(red(&doc), vec!["KAT"]);
    }

    #[test]
    fn deleting_red_text_by_hand_removes_the_range() {
        let mut doc = document("cat KAT dog", vec![(4, 7)], 0);
        doc.reconcile("cat  dog");
        assert!(red(&doc).is_empty());
    }

    #[test]
    fn reconciling_identical_text_changes_nothing() {
        let mut doc = document("cat KAT", vec![(4, 7)], 2);
        doc.reconcile("cat KAT");
        assert_eq!(doc.caret(), 2, "caret must not jump");
        assert_eq!(red(&doc), vec!["KAT"]);
    }

    #[test]
    fn the_caret_is_clamped_onto_a_character_boundary() {
        // The pound sign is two bytes.
        let mut doc = document("\u{00A3}5", vec![], 0);
        doc.set_caret(1);
        assert_eq!(doc.caret(), 0, "1 splits the pound sign");
        doc.set_caret(99);
        assert_eq!(doc.caret(), 3, "clamped to the end");
    }

    #[test]
    fn multibyte_text_survives_an_insertion_before_it() {
        let mut doc = document("\u{00E9}\u{00E9} KAT", vec![(5, 8)], 0);
        doc.apply(&StenoEdit {
            backspaces: 0,
            backspace_keys: 0,
            text: "x".to_owned(),
            raw_ranges: vec![],
        });
        assert_eq!(doc.text(), "x\u{00E9}\u{00E9} KAT");
        assert_eq!(red(&doc), vec!["KAT"]);
    }

    /// Text widgets speak characters and this document speaks bytes. Getting
    /// this wrong works flawlessly until the first accented character.
    #[test]
    fn character_offsets_convert_to_byte_offsets() {
        // Each e-acute is two bytes, so char 2 is byte 4.
        let doc = document("\u{00E9}\u{00E9}ab", vec![], 0);
        assert_eq!(doc.byte_of_char(0), 0);
        assert_eq!(doc.byte_of_char(1), 2);
        assert_eq!(doc.byte_of_char(2), 4);
        assert_eq!(doc.byte_of_char(3), 5);
        assert_eq!(doc.byte_of_char(99), 6, "past the end clamps");

        assert_eq!(doc.char_of_byte(0), 0);
        assert_eq!(doc.char_of_byte(4), 2);
        assert_eq!(doc.char_of_byte(99), 4, "past the end clamps");
    }

    #[test]
    fn the_caret_round_trips_through_character_offsets() {
        let mut doc = document("\u{00E9}\u{00E9}ab", vec![], 0);
        doc.set_caret_char(2);
        assert_eq!(doc.caret(), 4, "byte offset");
        assert_eq!(doc.caret_char(), 2, "and back again");
    }

    /// A full round trip: write, correct retroactively, then undo.
    #[test]
    fn a_retroactive_correction_applies_at_the_caret() {
        let mut doc = document("I wel", vec![], 5);
        let edit = steno_edit(&formatted("I wel", vec![]), &formatted("I welcome", vec![]));
        doc.apply(&edit);
        assert_eq!(doc.text(), "I welcome");

        // And with text after the caret, the correction must not disturb it.
        let mut doc = document("I wel. Later.", vec![], 5);
        doc.apply(&edit);
        assert_eq!(doc.text(), "I welcome. Later.");
    }
}
