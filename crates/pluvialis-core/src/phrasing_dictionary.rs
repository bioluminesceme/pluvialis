//! jeff-phrasing as an entry in the dictionary list.
//!
//! The translation tables live in [`crate::phrasing`], ported from the Python
//! and checked against it over 218,071 outlines. This is only the wrapper that
//! lets the stack consult them.

use crate::dictionary::ProgrammaticDictionary;
use crate::phrasing;

pub struct PhrasingDictionary {
    enabled: bool,
}

impl Default for PhrasingDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl PhrasingDictionary {
    pub fn new() -> Self {
        PhrasingDictionary { enabled: true }
    }
}

impl ProgrammaticDictionary for PhrasingDictionary {
    fn name(&self) -> String {
        "jeff-phrasing (built in)".to_owned()
    }

    fn longest_key(&self) -> usize {
        phrasing::longest_key()
    }

    fn lookup(&self, outlines: &[String]) -> Option<String> {
        // Single stroke only. The Python's matcher runs on a prefix, so it
        // would answer a two stroke outline by quietly ignoring the second
        // stroke; the port refuses instead, and this guard means the case never
        // arises in the first place.
        match outlines {
            [one] => phrasing::lookup(one),
            _ => None,
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DictionaryStack, Stroke};

    fn stack() -> DictionaryStack {
        let mut stack = DictionaryStack::new();
        stack.push_programmatic(Box::new(PhrasingDictionary::new()));
        stack
    }

    #[test]
    fn a_phrasing_outline_resolves_through_the_stack() {
        let stack = stack();
        let outline = Stroke::parse_outline("KPWH").expect("valid steno");
        assert_eq!(stack.lookup_owned(&outline), Some("it".to_owned()));
    }

    #[test]
    fn it_widens_the_lookback_by_its_longest_key() {
        assert_eq!(stack().longest_key(), phrasing::longest_key());
    }

    #[test]
    fn disabling_it_stops_it_answering() {
        let mut stack = stack();
        stack.programmatic_mut()[0].set_enabled(false);
        let outline = Stroke::parse_outline("KPWH").expect("valid steno");
        assert_eq!(stack.lookup_owned(&outline), None);
    }

    /// Multi stroke outlines never reach the tables.
    #[test]
    fn a_multi_stroke_outline_is_refused() {
        let dictionary = PhrasingDictionary::new();
        assert_eq!(
            dictionary.lookup(&["KPWH".to_owned(), "KPWH".to_owned()]),
            None
        );
        assert_eq!(dictionary.lookup(&[]), None);
    }
}
