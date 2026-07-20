//! Mapping the keys a machine reports onto the 23 keys of the steno system.
//!
//! Machines do not report steno keys. They report *their own* keys, and the two
//! differ in ways that matter. A Gemini PR keyboard has a split S, so it sends
//! `S1-` or `S2-` depending on which half was struck; it has four star keys;
//! its number bar is twelve separate segments. All of those collapse onto one
//! steno key each.
//!
//! This collapse belongs here and nowhere else. Doing it inside a protocol
//! decoder looks simpler and is how every later machine ends up subtly wrong,
//! because the next protocol has a different set of duplicates and no place to
//! put them. A decoder's job ends at "which physical keys were struck".
//!
//! Verified against real hardware: the Peregrine's `SKP` chord arrives as
//! `S2- K- P-`, not `S- K- P-`.

use std::collections::HashMap;

use pluvialis_core::{Stroke, StrokeError};

/// Which steno key each machine key produces.
///
/// A machine key absent from the map is deliberately unbound, which is how
/// `Fn`, `pwr` and the reserved keys stay inert rather than becoming stray
/// steno.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: HashMap<&'static str, &'static str>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a machine key to a steno key. Many machine keys may share one
    /// steno key; that is the point.
    pub fn bind(&mut self, machine_key: &'static str, steno_key: &'static str) -> &mut Self {
        self.bindings.insert(machine_key, steno_key);
        self
    }

    pub fn get(&self, machine_key: &str) -> Option<&'static str> {
        self.bindings.get(machine_key).copied()
    }

    /// Translate machine keys into a stroke, dropping unbound keys.
    ///
    /// Returns `None` when nothing bound was struck, which happens for a chord
    /// of only unmapped keys and is not an error.
    pub fn stroke(&self, machine_keys: &[&str]) -> Result<Option<Stroke>, StrokeError> {
        let steno: Vec<&str> = machine_keys
            .iter()
            .filter_map(|key| self.get(key))
            .collect();

        if steno.is_empty() {
            return Ok(None);
        }
        Stroke::from_keys(steno).map(Some)
    }

    /// The default Gemini PR binding.
    ///
    /// Every key maps to itself except the duplicates: both halves of S, all
    /// four stars, and all twelve number bar segments. `Fn`, `pwr`, `res1` and
    /// `res2` are left unbound, matching Plover's default.
    pub fn gemini_pr() -> Self {
        let mut keymap = Keymap::new();

        keymap.bind("S1-", "S-").bind("S2-", "S-");
        for star in ["*1", "*2", "*3", "*4"] {
            keymap.bind(star, "*");
        }
        for number in [
            "#1", "#2", "#3", "#4", "#5", "#6", "#7", "#8", "#9", "#A", "#B", "#C",
        ] {
            keymap.bind(number, "#");
        }
        // The rest are named identically to their steno keys.
        for key in [
            "T-", "K-", "P-", "W-", "H-", "R-", "A-", "O-", "-E", "-U", "-F", "-R", "-P", "-B",
            "-L", "-G", "-T", "-S", "-D", "-Z",
        ] {
            keymap.bind(key, key);
        }

        keymap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_halves_of_the_split_s_produce_the_same_steno_key() {
        let keymap = Keymap::gemini_pr();
        assert_eq!(keymap.get("S1-"), Some("S-"));
        assert_eq!(keymap.get("S2-"), Some("S-"));
    }

    #[test]
    fn all_four_stars_and_all_twelve_number_segments_collapse() {
        let keymap = Keymap::gemini_pr();
        for star in ["*1", "*2", "*3", "*4"] {
            assert_eq!(keymap.get(star), Some("*"), "{star}");
        }
        for number in [
            "#1", "#2", "#3", "#4", "#5", "#6", "#7", "#8", "#9", "#A", "#B", "#C",
        ] {
            assert_eq!(keymap.get(number), Some("#"), "{number}");
        }
    }

    #[test]
    fn unassigned_machine_keys_stay_unbound() {
        let keymap = Keymap::gemini_pr();
        for key in ["Fn", "pwr", "res1", "res2"] {
            assert_eq!(keymap.get(key), None, "{key}");
        }
    }

    /// The exact chord captured from the user's Peregrine.
    #[test]
    fn the_real_skp_chord_becomes_the_skp_stroke() {
        let keymap = Keymap::gemini_pr();
        let stroke = keymap.stroke(&["S2-", "K-", "P-"]).unwrap().unwrap();
        assert_eq!(Stroke::render_outline(&[stroke]), "SKP");
    }

    /// The other captured chord.
    #[test]
    fn the_real_eu_chord_becomes_the_eu_stroke() {
        let keymap = Keymap::gemini_pr();
        let stroke = keymap.stroke(&["-E", "-U"]).unwrap().unwrap();
        assert_eq!(Stroke::render_outline(&[stroke]), "EU");
    }

    #[test]
    fn pressing_both_halves_of_s_at_once_is_still_one_s() {
        let keymap = Keymap::gemini_pr();
        let stroke = keymap.stroke(&["S1-", "S2-"]).unwrap().unwrap();
        assert_eq!(Stroke::render_outline(&[stroke]), "S");
    }

    #[test]
    fn a_chord_of_only_unbound_keys_produces_nothing() {
        let keymap = Keymap::gemini_pr();
        assert_eq!(keymap.stroke(&["Fn", "pwr"]).unwrap(), None);
        assert_eq!(keymap.stroke(&[]).unwrap(), None);
    }

    #[test]
    fn unbound_keys_are_dropped_from_an_otherwise_valid_chord() {
        let keymap = Keymap::gemini_pr();
        let stroke = keymap.stroke(&["Fn", "K-", "A-", "-T"]).unwrap().unwrap();
        assert_eq!(Stroke::render_outline(&[stroke]), "KAT");
    }

    #[test]
    fn the_number_key_reaches_the_stroke() {
        let keymap = Keymap::gemini_pr();
        // #1 collapses to #, and with S- that renders as the digit 1.
        let stroke = keymap.stroke(&["#1", "S1-"]).unwrap().unwrap();
        assert_eq!(Stroke::render_outline(&[stroke]), "1");
    }
}
