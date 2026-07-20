//! Plover-format JSON dictionaries and priority ordered lookup across several
//! of them.
//!
//! Dictionary files are shared with the user's working Plover install and are
//! read in place. Their on disk format is not changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::stroke::{Stroke, StrokeError};

#[derive(Debug, thiserror::Error)]
pub enum DictionaryError {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// One dictionary file.
pub struct Dictionary {
    pub path: PathBuf,
    pub enabled: bool,
    entries: HashMap<Box<[Stroke]>, String>,
    longest_key: usize,
    /// Keys that could not be parsed as steno, kept so they can be reported
    /// rather than silently ignored.
    bad_keys: Vec<(String, StrokeError)>,
}

impl Dictionary {
    /// Load a Plover JSON dictionary.
    ///
    /// Unparseable keys are collected in [`Self::bad_keys`] instead of failing
    /// the load: one malformed entry should not cost the user their whole
    /// dictionary. Callers are expected to surface the count.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DictionaryError> {
        let path = path.as_ref().to_path_buf();
        let text = std::fs::read_to_string(&path).map_err(|source| DictionaryError::Io {
            path: path.clone(),
            source,
        })?;
        let raw: HashMap<String, String> =
            serde_json::from_str(&text).map_err(|source| DictionaryError::Json {
                path: path.clone(),
                source,
            })?;

        let mut entries = HashMap::with_capacity(raw.len());
        let mut longest_key = 0;
        let mut bad_keys = Vec::new();

        for (key, value) in raw {
            match Stroke::parse_outline(&key) {
                Ok(strokes) => {
                    longest_key = longest_key.max(strokes.len());
                    entries.insert(strokes.into_boxed_slice(), value);
                }
                Err(e) => bad_keys.push((key, e)),
            }
        }

        Ok(Dictionary {
            path,
            enabled: true,
            entries,
            longest_key,
            bad_keys,
        })
    }

    pub fn lookup(&self, strokes: &[Stroke]) -> Option<&str> {
        self.entries.get(strokes).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The most strokes in any single entry. This sets how far the translator
    /// has to look back.
    pub fn longest_key(&self) -> usize {
        self.longest_key
    }

    pub fn bad_keys(&self) -> &[(String, StrokeError)] {
        &self.bad_keys
    }

    /// Every outline that maps to the given text, for reverse lookup.
    pub fn reverse_lookup(&self, text: &str) -> Vec<&[Stroke]> {
        self.entries
            .iter()
            .filter(|(_, v)| v.as_str() == text)
            .map(|(k, _)| &**k)
            .collect()
    }
}

/// A dictionary that computes its answers instead of storing them.
///
/// Implemented by the Lua host and by the native jeff-phrasing port. Kept as a
/// trait here so `pluvialis-core` needs no dependency on either: it defines the
/// shape and the callers supply the implementations.
///
/// Outlines arrive already rendered (`["SKP"]`, `["TPHOEU", "TPHOEU"]`) so an
/// implementation sees exactly what a JSON key would have matched.
/// Deliberately not `Send`. The dictionary stack lives on the UI thread and is
/// never moved off it (strokes reach it by channel from the machine thread), and
/// requiring `Send` here would force the Lua host into mlua's `send` feature for
/// no benefit.
pub trait ProgrammaticDictionary {
    /// For the dictionary list.
    fn name(&self) -> String;

    /// How many strokes back this dictionary can answer for.
    fn longest_key(&self) -> usize;

    fn lookup(&self, outlines: &[String]) -> Option<String>;

    fn reverse_lookup(&self, _text: &str) -> Vec<String> {
        Vec::new()
    }

    fn is_enabled(&self) -> bool;
    fn set_enabled(&mut self, enabled: bool);
}

/// Several dictionaries searched in priority order.
///
/// Index 0 has the highest priority, matching how the list is displayed. The
/// first enabled dictionary with a match wins, and disabled ones are skipped
/// without being unloaded.
#[derive(Default)]
pub struct DictionaryStack {
    dictionaries: Vec<Dictionary>,
    /// Consulted only after every JSON dictionary has missed.
    ///
    /// Deliberately last rather than interleaved by priority. Being unable to
    /// shadow an existing entry means adding one can only add behaviour, never
    /// change what the user's existing outlines already do, which is the safe
    /// default while nobody has said where they belong. Interleaving is a real
    /// feature and is noted in PLAN.md.
    programmatic: Vec<Box<dyn ProgrammaticDictionary>>,
}

impl DictionaryStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, dictionary: Dictionary) {
        self.dictionaries.push(dictionary);
    }

    pub fn dictionaries(&self) -> &[Dictionary] {
        &self.dictionaries
    }

    pub fn dictionaries_mut(&mut self) -> &mut Vec<Dictionary> {
        &mut self.dictionaries
    }

    pub fn push_programmatic(&mut self, dictionary: Box<dyn ProgrammaticDictionary>) {
        self.programmatic.push(dictionary);
    }

    pub fn programmatic(&self) -> &[Box<dyn ProgrammaticDictionary>] {
        &self.programmatic
    }

    pub fn programmatic_mut(&mut self) -> &mut Vec<Box<dyn ProgrammaticDictionary>> {
        &mut self.programmatic
    }

    /// Look up an outline, JSON dictionaries first.
    ///
    /// Returns an owned string rather than a borrow because a programmatic
    /// dictionary computes its answer and has nothing to lend.
    pub fn lookup_owned(&self, strokes: &[Stroke]) -> Option<String> {
        if let Some(value) = self.lookup(strokes) {
            return Some(value.to_owned());
        }

        if self.programmatic.is_empty() {
            return None;
        }
        // Rendering costs an allocation, so it is done once and only when a
        // programmatic dictionary might actually be consulted.
        let outlines: Vec<String> = strokes
            .iter()
            .map(|stroke| Stroke::render_outline(std::slice::from_ref(stroke)))
            .collect();

        self.programmatic
            .iter()
            .filter(|d| d.is_enabled())
            .find_map(|d| d.lookup(&outlines))
    }

    /// JSON dictionaries only. Kept for callers that want a borrow.
    pub fn lookup(&self, strokes: &[Stroke]) -> Option<&str> {
        self.dictionaries
            .iter()
            .filter(|d| d.enabled)
            .find_map(|d| d.lookup(strokes))
    }

    /// The longest key across enabled dictionaries, which bounds how far back
    /// the translator looks. Zero when nothing is loaded.
    pub fn longest_key(&self) -> usize {
        let json = self
            .dictionaries
            .iter()
            .filter(|d| d.enabled)
            .map(Dictionary::longest_key)
            .max()
            .unwrap_or(0);

        let programmatic = self
            .programmatic
            .iter()
            .filter(|d| d.is_enabled())
            .map(|d| d.longest_key())
            .max()
            .unwrap_or(0);

        json.max(programmatic)
    }

    pub fn entry_count(&self) -> usize {
        self.dictionaries
            .iter()
            .filter(|d| d.enabled)
            .map(Dictionary::len)
            .sum()
    }
}

#[cfg(test)]
mod programmatic_tests {
    use super::*;

    /// Answers only SWR, so it is easy to tell apart from a JSON hit.
    struct Fake {
        enabled: bool,
    }

    impl ProgrammaticDictionary for Fake {
        fn name(&self) -> String {
            "fake".to_owned()
        }
        fn longest_key(&self) -> usize {
            3
        }
        fn lookup(&self, outlines: &[String]) -> Option<String> {
            (outlines == ["SWR"]).then(|| "computed".to_owned())
        }
        fn is_enabled(&self) -> bool {
            self.enabled
        }
        fn set_enabled(&mut self, enabled: bool) {
            self.enabled = enabled;
        }
    }

    fn stack(enabled: bool) -> DictionaryStack {
        let mut stack = DictionaryStack::new();
        stack.push_programmatic(Box::new(Fake { enabled }));
        stack
    }

    #[test]
    fn a_programmatic_dictionary_answers_when_the_json_ones_miss() {
        let stack = stack(true);
        let outline = Stroke::parse_outline("SWR").unwrap();
        assert_eq!(
            stack.lookup_owned(&outline),
            Some("computed".to_owned())
        );
    }

    #[test]
    fn a_disabled_programmatic_dictionary_is_skipped() {
        let stack = stack(false);
        let outline = Stroke::parse_outline("SWR").unwrap();
        assert_eq!(stack.lookup_owned(&outline), None);
    }

    #[test]
    fn it_widens_the_stroke_window_the_translator_looks_back_over() {
        // Its longest_key must count, or long entries silently never match.
        assert_eq!(stack(true).longest_key(), 3);
        assert_eq!(stack(false).longest_key(), 0, "disabled does not count");
    }

    #[test]
    fn an_outline_it_does_not_know_is_still_a_miss() {
        let stack = stack(true);
        let outline = Stroke::parse_outline("KAT").unwrap();
        assert_eq!(stack.lookup_owned(&outline), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_dict(name: &str, json: &str) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        path
    }

    fn outline(s: &str) -> Vec<Stroke> {
        Stroke::parse_outline(s).unwrap()
    }

    #[test]
    fn loads_and_looks_up() {
        let path = write_dict(
            "pluvialis_test_basic.json",
            r#"{"KAT": "cat", "WEL/KO*PL": "welcome"}"#,
        );
        let dict = Dictionary::load(&path).unwrap();

        assert_eq!(dict.len(), 2);
        assert_eq!(dict.longest_key(), 2);
        assert_eq!(dict.lookup(&outline("KAT")), Some("cat"));
        assert_eq!(dict.lookup(&outline("WEL/KO*PL")), Some("welcome"));
        assert_eq!(dict.lookup(&outline("TPHOT")), None);
        assert!(dict.bad_keys().is_empty());
    }

    #[test]
    fn keys_are_matched_after_normalization() {
        // The stored key uses a redundant hyphen; lookup by the canonical form
        // still has to find it.
        let path = write_dict("pluvialis_test_norm.json", r#"{"TK-LS": "tools"}"#);
        let dict = Dictionary::load(&path).unwrap();
        assert_eq!(dict.lookup(&outline("TK-LS")), Some("tools"));
    }

    #[test]
    fn malformed_keys_are_reported_not_fatal() {
        let path = write_dict(
            "pluvialis_test_bad.json",
            r#"{"KAT": "cat", "QQQ": "nonsense"}"#,
        );
        let dict = Dictionary::load(&path).unwrap();
        assert_eq!(dict.len(), 1);
        assert_eq!(dict.bad_keys().len(), 1);
        assert_eq!(dict.bad_keys()[0].0, "QQQ");
    }

    #[test]
    fn priority_order_decides_and_disabled_are_skipped() {
        let high = write_dict("pluvialis_test_high.json", r#"{"KAT": "feline"}"#);
        let low = write_dict("pluvialis_test_low.json", r#"{"KAT": "cat", "TO": "to"}"#);

        let mut stack = DictionaryStack::new();
        stack.push(Dictionary::load(&high).unwrap());
        stack.push(Dictionary::load(&low).unwrap());

        // First enabled hit wins.
        assert_eq!(stack.lookup(&outline("KAT")), Some("feline"));
        // Falls through to the lower dictionary when the first misses.
        assert_eq!(stack.lookup(&outline("TO")), Some("to"));

        stack.dictionaries_mut()[0].enabled = false;
        assert_eq!(stack.lookup(&outline("KAT")), Some("cat"));
    }

    #[test]
    fn longest_key_ignores_disabled_dictionaries() {
        let short = write_dict("pluvialis_test_short.json", r#"{"KAT": "cat"}"#);
        let long = write_dict("pluvialis_test_long.json", r#"{"A/B/K": "abk"}"#);

        let mut stack = DictionaryStack::new();
        stack.push(Dictionary::load(&short).unwrap());
        stack.push(Dictionary::load(&long).unwrap());
        assert_eq!(stack.longest_key(), 3);

        stack.dictionaries_mut()[1].enabled = false;
        assert_eq!(stack.longest_key(), 1);
    }
}
