//! Longest match translation with retroactive correction.
//!
//! Steno is written one stroke at a time but dictionary entries can span
//! several strokes, and you cannot know which until the later strokes arrive.
//! So each new stroke may turn out to belong to a longer entry that supersedes
//! what was already emitted. When that happens the earlier output is withdrawn
//! and replaced. Plover behaves the same way, and it is why steno output
//! sometimes visibly rewrites itself as you write.
//!
//! Example: `WEL` alone translates to "well". When `KO*PL` follows and
//! `WEL/KO*PL` is in the dictionary as "welcome", the "well" is removed and
//! "welcome" takes its place.

use crate::dictionary::DictionaryStack;
use crate::stroke::Stroke;

/// One unit of translated output: the strokes that produced it, the text they
/// produced, and whatever it displaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    pub strokes: Vec<Stroke>,
    /// `None` means no dictionary matched, so this shows as raw steno. In the
    /// live view these are the entries that render red.
    pub text: Option<String>,
    /// The translations this one replaced, kept so undo can put them back.
    replaced: Vec<Translation>,
}

impl Translation {
    /// What this translation contributes to the document: the dictionary text,
    /// or the raw steno when nothing matched.
    pub fn output(&self) -> String {
        match &self.text {
            Some(text) => text.clone(),
            None => Stroke::render_outline(&self.strokes),
        }
    }

    /// True when no dictionary matched these strokes.
    pub fn is_untranslated(&self) -> bool {
        self.text.is_none()
    }

    /// Build a translation directly. For tests that exercise formatting
    /// without going through a dictionary.
    pub fn for_test(strokes: Vec<Stroke>, text: Option<String>) -> Self {
        Translation {
            strokes,
            text,
            replaced: Vec::new(),
        }
    }
}

/// What changed as a result of one stroke.
///
/// `removed` translations were withdrawn, `added` ones took their place. Both
/// can be empty (an undo at the start of a session changes nothing).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delta {
    pub removed: Vec<Translation>,
    pub added: Vec<Translation>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty()
    }
}

/// How many translations to retain. Only the most recent `longest_key` of them
/// can affect a new match, so this is far more than correctness needs; it
/// exists to bound memory and to give undo somewhere to go.
pub const HISTORY_LIMIT: usize = 1000;

#[derive(Default)]
pub struct Translator {
    history: Vec<Translation>,
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn history(&self) -> &[Translation] {
        &self.history
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Feed one stroke and return what changed.
    pub fn translate(&mut self, dictionaries: &DictionaryStack, stroke: Stroke) -> Delta {
        if stroke.is_undo() {
            return self.undo();
        }

        let longest_key = dictionaries.longest_key().max(1);

        // Work out how many previous translations we could absorb without the
        // combined outline exceeding the longest dictionary key. Absorbing
        // more previous translations means a longer outline, so trying these
        // from the back gives longest match first.
        let mut absorb_options = vec![0usize];
        let mut total = 1; // the incoming stroke
        for (count, previous) in self.history.iter().rev().enumerate() {
            total += previous.strokes.len();
            if total > longest_key {
                break;
            }
            absorb_options.push(count + 1);
        }

        for &absorb in absorb_options.iter().rev() {
            let split = self.history.len() - absorb;
            let mut outline: Vec<Stroke> = self.history[split..]
                .iter()
                .flat_map(|t| t.strokes.iter().copied())
                .collect();
            outline.push(stroke);

            // lookup_owned so programmatic dictionaries (Python)
            // are consulted too, after the JSON ones miss.
            if let Some(text) = dictionaries.lookup_owned(&outline) {
                let replaced: Vec<Translation> = self.history.drain(split..).collect();
                let translation = Translation {
                    strokes: outline,
                    text: Some(text),
                    replaced: replaced.clone(),
                };
                self.history.push(translation.clone());
                return Delta {
                    removed: replaced,
                    added: vec![translation],
                };
            }
        }

        // Nothing matched, so the stroke stands alone as raw steno.
        let translation = Translation {
            strokes: vec![stroke],
            text: None,
            replaced: Vec::new(),
        };
        self.history.push(translation.clone());
        Delta {
            removed: Vec::new(),
            added: vec![translation],
        }
    }

    /// Undo the most recent translation, restoring whatever it had replaced.
    ///
    /// Undoing a retroactive correction puts the shorter translations back, so
    /// `WEL` `KO*PL` `*` leaves "well" rather than nothing.
    pub fn undo(&mut self) -> Delta {
        let Some(last) = self.history.pop() else {
            return Delta::default();
        };
        let restored = last.replaced.clone();
        self.history.extend(restored.iter().cloned());
        Delta {
            removed: vec![last],
            added: restored,
        }
    }

    /// The whole document as plain text.
    ///
    /// M1 joins translations with single spaces. Real spacing, capitalization
    /// and attachment arrive with the formatter in M2, so meta commands such
    /// as `{^ing}` still appear literally here.
    pub fn text(&self) -> String {
        self.history
            .iter()
            .map(Translation::output)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Drop the oldest translations down to [`HISTORY_LIMIT`], and report how
    /// many went. Returns 0 when nothing was dropped.
    ///
    /// **Deliberately not called by [`Translator::translate`].** Trimming
    /// changes where the formatter's output starts, and anything diffing
    /// consecutive `format(history)` results by common prefix (which is what
    /// [`crate::steno_edit`] does) will read that as "delete everything, retype
    /// everything". The caller must therefore trim at a point where it can
    /// absorb the shift silently: format and emit for the stroke first, then
    /// trim, then recompute its own copy of the formatter output without
    /// emitting the difference. See `LiveView::resync_after_trim`.
    pub fn trim_history(&mut self) -> usize {
        if self.history.len() <= HISTORY_LIMIT {
            return 0;
        }
        let excess = self.history.len() - HISTORY_LIMIT;
        self.history.drain(..excess);
        excess
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::Dictionary;
    use std::io::Write;
    use std::path::PathBuf;

    fn stack_from(json: &str, name: &str) -> DictionaryStack {
        let path: PathBuf = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        let mut stack = DictionaryStack::new();
        stack.push(Dictionary::load(&path).unwrap());
        stack
    }

    fn write(translator: &mut Translator, dicts: &DictionaryStack, outline: &str) {
        for stroke in Stroke::parse_outline(outline).unwrap() {
            translator.translate(dicts, stroke);
        }
    }

    #[test]
    fn translates_single_strokes() {
        let dicts = stack_from(r#"{"KAT": "cat", "TKOG": "dog"}"#, "pluv_tr_single.json");
        let mut t = Translator::new();
        write(&mut t, &dicts, "KAT/TKOG");
        assert_eq!(t.text(), "cat dog");
    }

    #[test]
    fn unmatched_strokes_stay_as_raw_steno() {
        let dicts = stack_from(r#"{"KAT": "cat"}"#, "pluv_tr_raw.json");
        let mut t = Translator::new();
        write(&mut t, &dicts, "KAT/TPHOG");

        assert_eq!(t.text(), "cat TPHOG");
        assert!(t.history()[1].is_untranslated());
        assert!(!t.history()[0].is_untranslated());
    }

    #[test]
    fn raw_steno_is_rendered_canonically_not_as_typed() {
        // This outline has no center key, so a hyphen is required to show
        // where the right bank starts. Raw steno in the live view will
        // therefore not always look like what was written.
        let dicts = stack_from(r#"{"KAT": "cat"}"#, "pluv_tr_raw_render.json");
        let mut t = Translator::new();
        write(&mut t, &dicts, "TPHRPBLG");
        assert_eq!(t.text(), "TPHR-PBLG");
    }

    #[test]
    fn later_strokes_retroactively_replace_earlier_output() {
        let dicts = stack_from(
            r#"{"WEL": "well", "WEL/KO*PL": "welcome"}"#,
            "pluv_tr_retro.json",
        );
        let mut t = Translator::new();

        write(&mut t, &dicts, "WEL");
        assert_eq!(t.text(), "well");

        // The second stroke completes a longer entry, so "well" is withdrawn.
        let delta = t.translate(&dicts, Stroke::parse("KO*PL").unwrap());
        assert_eq!(t.text(), "welcome");
        assert_eq!(delta.removed.len(), 1);
        assert_eq!(delta.removed[0].output(), "well");
        assert_eq!(delta.added[0].output(), "welcome");
    }

    #[test]
    fn multi_stroke_entry_forms_even_when_prefix_is_unknown() {
        // No entry for "TPHO" alone, so it starts as raw steno and is then
        // absorbed when the outline completes.
        let dicts = stack_from(r#"{"TPHO/THEUPBG": "nothing"}"#, "pluv_tr_absorb.json");
        let mut t = Translator::new();

        write(&mut t, &dicts, "TPHO");
        assert_eq!(t.text(), "TPHO");

        write(&mut t, &dicts, "THEUPBG");
        assert_eq!(t.text(), "nothing");
        assert_eq!(t.history().len(), 1);
    }

    #[test]
    fn undo_removes_the_last_translation() {
        let dicts = stack_from(r#"{"KAT": "cat", "TKOG": "dog"}"#, "pluv_tr_undo.json");
        let mut t = Translator::new();
        write(&mut t, &dicts, "KAT/TKOG");

        t.translate(&dicts, Stroke::parse("*").unwrap());
        assert_eq!(t.text(), "cat");

        t.translate(&dicts, Stroke::parse("*").unwrap());
        assert_eq!(t.text(), "");
    }

    #[test]
    fn undo_restores_what_a_retroactive_match_replaced() {
        let dicts = stack_from(
            r#"{"WEL": "well", "WEL/KO*PL": "welcome"}"#,
            "pluv_tr_undo_retro.json",
        );
        let mut t = Translator::new();
        write(&mut t, &dicts, "WEL/KO*PL");
        assert_eq!(t.text(), "welcome");

        // Undoing the correction must bring "well" back, not leave nothing.
        t.translate(&dicts, Stroke::parse("*").unwrap());
        assert_eq!(t.text(), "well");
    }

    #[test]
    fn undo_on_empty_history_is_harmless() {
        let dicts = stack_from(r#"{"KAT": "cat"}"#, "pluv_tr_undo_empty.json");
        let mut t = Translator::new();
        let delta = t.translate(&dicts, Stroke::parse("*").unwrap());
        assert!(delta.is_empty());
        assert_eq!(t.text(), "");
    }

    #[test]
    fn undo_removes_raw_steno_too() {
        let dicts = stack_from(r#"{"KAT": "cat"}"#, "pluv_tr_undo_raw.json");
        let mut t = Translator::new();
        write(&mut t, &dicts, "KAT/TPHOG");
        assert_eq!(t.text(), "cat TPHOG");

        t.translate(&dicts, Stroke::parse("*").unwrap());
        assert_eq!(t.text(), "cat");
    }

    #[test]
    fn longest_match_wins_over_shorter_ones() {
        let dicts = stack_from(
            r#"{"A": "a", "A/B": "ab", "A/B/K": "abk"}"#,
            "pluv_tr_longest.json",
        );
        let mut t = Translator::new();
        write(&mut t, &dicts, "A/B/K");
        assert_eq!(t.text(), "abk");
        assert_eq!(t.history().len(), 1);
    }
}
