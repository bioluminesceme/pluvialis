//! What one stroke may emit once the translator's history fills up.
//!
//! The live view reformats the whole history every stroke and diffs the result
//! against the previous one by common prefix. Trimming the history drops the
//! oldest translations, so the formatter's output stops starting where it did
//! and that diff degenerates into "delete the whole session, retype the whole
//! session". Before the fix this happened on the first stroke past the limit
//! and on every stroke after it, sending thousands of backspaces and thousands
//! of characters into whatever window had focus.
//!
//! `Translator::trim_history` therefore has to be called at a point where the
//! caller can absorb the shift silently, and this test pins the order it has to
//! be called in. `LiveView::resync_after_trim` is the real implementation of
//! that order; this reproduces it against the core API so a change to either
//! side gets caught here.

use pluvialis_core::format::format;
use pluvialis_core::translator::HISTORY_LIMIT;
use pluvialis_core::{Dictionary, DictionaryStack, Stroke, Translator, steno_edit};

/// Distinct words, so that dropping the oldest one actually shifts the text.
/// A dictionary of one repeated word hides the bug: the trimmed text still
/// shares a long prefix with the untrimmed one purely by coincidence, so the
/// diff stays small for the wrong reason.
const WORDS: [(&str, &str); 5] = [
    ("KAT", "cat"),
    ("TKOG", "dog"),
    ("PWEURD", "bird"),
    ("TPEURB", "fish"),
    ("HORS", "horse"),
];

fn stack() -> DictionaryStack {
    let path = std::env::temp_dir().join("pluv_history_limit.json");
    let entries: Vec<String> = WORDS
        .iter()
        .map(|(outline, word)| format!("\"{outline}\": \"{word}\""))
        .collect();
    std::fs::write(&path, format!("{{{}}}", entries.join(","))).unwrap();
    let mut stack = DictionaryStack::new();
    stack.push(Dictionary::load(&path).unwrap());
    stack
}

fn strokes() -> Vec<Stroke> {
    WORDS
        .iter()
        .map(|(outline, _)| Stroke::parse_outline(outline).unwrap()[0])
        .collect()
}

/// Well past the limit, so the trimming path runs for hundreds of strokes
/// rather than only being entered once.
const STROKES: usize = HISTORY_LIMIT + 200;

#[test]
fn a_single_stroke_never_rewrites_the_whole_session() {
    let dicts = stack();
    let strokes = strokes();
    let mut translator = Translator::new();

    let mut shadow = format(translator.history());
    let mut worst = (0usize, 0usize, 0usize); // stroke, backspaces, inserted
    let mut runaway = 0usize;

    for n in 1..=STROKES {
        // The order `LiveView::apply` uses. Translating and formatting the
        // stroke, delivering its edit, and only then trimming and rebuilding
        // the shadow off the trimmed history.
        translator.translate(&dicts, strokes[n % strokes.len()]);

        let next = format(translator.history());
        let edit = steno_edit(&shadow, &next);
        shadow = next;

        if translator.trim_history() > 0 {
            shadow = format(translator.history());
        }

        if edit.backspace_keys > 100 {
            runaway += 1;
        }
        if edit.backspace_keys > worst.1 {
            worst = (n, edit.backspace_keys, edit.text.len());
        }
    }

    assert_eq!(
        runaway, 0,
        "{runaway} of {STROKES} strokes rewrote more than 100 characters; \
         worst was stroke #{} with {} backspaces and {} characters retyped",
        worst.0, worst.1, worst.2
    );
}

/// The other half of the contract. If `translate` ever trims on its own again,
/// the caller loses the chance to resync and the runaway comes straight back,
/// so this pins trimming as something only the caller does.
#[test]
fn translate_does_not_trim_on_its_own() {
    let dicts = stack();
    let strokes = strokes();
    let mut translator = Translator::new();

    for n in 1..=STROKES {
        translator.translate(&dicts, strokes[n % strokes.len()]);
    }

    assert_eq!(
        translator.history().len(),
        STROKES,
        "translate trimmed the history by itself"
    );
}

/// Trimming still has to actually bound the history, or the fix would have
/// quietly turned the limit off rather than making it safe.
#[test]
fn trimming_bounds_the_history_and_reports_what_it_dropped() {
    let dicts = stack();
    let strokes = strokes();
    let mut translator = Translator::new();

    for n in 1..=STROKES {
        translator.translate(&dicts, strokes[n % strokes.len()]);
    }

    assert_eq!(translator.trim_history(), STROKES - HISTORY_LIMIT);
    assert_eq!(translator.history().len(), HISTORY_LIMIT);
    // Nothing left to drop, so a second call is a no-op.
    assert_eq!(translator.trim_history(), 0);
}
