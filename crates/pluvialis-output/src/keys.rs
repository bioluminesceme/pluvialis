//! Key combo names and the nesting they use.
//!
//! Dictionary key combos look like `{#Control_L(Left)}` or
//! `{#Control_L(Shift(Left))}`, and the names are **X11 keysyms**, not Win32
//! names: `Control_L`, `Page_Down`, `BackSpace`. They need mapping to virtual
//! key codes.
//!
//! They also genuinely nest. `Control_L(Shift(Left))` means hold Control, hold
//! Shift, press Left, so this needs a real parser rather than a split on
//! parentheses. A sequence inside a modifier (`Control_L(a b)`) presses each
//! key in turn with the modifier still held.
//!
//! Portable on purpose: only the sending half is Windows specific, so a Linux
//! output layer reuses this untouched. The table covers the 44 distinct names
//! measured across the user's dictionaries, plus the obvious neighbours.

use crate::OutputError;

/// A virtual key, and whether it needs the extended-key flag.
///
/// The arrow cluster, navigation keys and Delete are "extended" on a PC
/// keyboard. Some applications read the flag and behave oddly without it, so it
/// travels with the code rather than being inferred later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub vk: u16,
    pub extended: bool,
}

impl Key {
    const fn plain(vk: u16) -> Self {
        Key {
            vk,
            extended: false,
        }
    }
    const fn extended(vk: u16) -> Self {
        Key { vk, extended: true }
    }
}

/// One keypress, with whatever is held down for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Vec<Key>,
    pub key: Key,
}

/// X11 keysym to virtual key.
///
/// Matched case insensitively for multi-character names. The dictionaries
/// contain both `Return` and `return`, and X11 only defines the first, so a
/// case sensitive lookup would drop one of them for no benefit to anyone.
/// Single characters stay case sensitive and are handled separately.
#[rustfmt::skip]
const NAMED: &[(&str, Key)] = &[
    // Modifiers. The side specific codes are used where the name says a side,
    // since that is what the keysym means.
    ("control_l",  Key::plain(0xA2)),  ("control_r", Key::plain(0xA3)),
    ("control",    Key::plain(0x11)),  ("ctrl",      Key::plain(0x11)),
    ("shift_l",    Key::plain(0xA0)),  ("shift_r",   Key::plain(0xA1)),
    ("shift",      Key::plain(0x10)),
    ("alt_l",      Key::plain(0xA4)),  ("alt_r",     Key::plain(0xA5)),
    ("alt",        Key::plain(0x12)),  ("meta_l",    Key::plain(0xA4)),
    ("super_l",    Key::plain(0x5B)),  ("super_r",   Key::plain(0x5C)),

    // Navigation. All extended.
    ("left",  Key::extended(0x25)), ("up",        Key::extended(0x26)),
    ("right", Key::extended(0x27)), ("down",      Key::extended(0x28)),
    ("home",  Key::extended(0x24)), ("end",       Key::extended(0x23)),
    ("page_up", Key::extended(0x21)), ("page_down", Key::extended(0x22)),
    ("prior", Key::extended(0x21)), ("next",      Key::extended(0x22)),
    ("insert", Key::extended(0x2D)), ("delete",   Key::extended(0x2E)),

    // Editing and whitespace.
    ("backspace", Key::plain(0x08)), ("tab",    Key::plain(0x09)),
    ("return",    Key::plain(0x0D)), ("enter",  Key::plain(0x0D)),
    ("escape",    Key::plain(0x1B)), ("esc",    Key::plain(0x1B)),
    ("space",     Key::plain(0x20)),

    ("f1", Key::plain(0x70)), ("f2",  Key::plain(0x71)), ("f3",  Key::plain(0x72)),
    ("f4", Key::plain(0x73)), ("f5",  Key::plain(0x74)), ("f6",  Key::plain(0x75)),
    ("f7", Key::plain(0x76)), ("f8",  Key::plain(0x77)), ("f9",  Key::plain(0x78)),
    ("f10", Key::plain(0x79)), ("f11", Key::plain(0x7A)), ("f12", Key::plain(0x7B)),
];

/// Look up one key name.
pub fn key_for(name: &str) -> Option<Key> {
    // A single character is a literal key: its virtual key code is the
    // uppercase ASCII value, which is how Win32 numbers the letter and digit
    // keys.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphanumeric()
    {
        return Some(Key::plain(c.to_ascii_uppercase() as u16));
    }

    let lowered = name.to_ascii_lowercase();
    NAMED
        .iter()
        .find(|(candidate, _)| *candidate == lowered)
        .map(|(_, key)| *key)
}

/// Parse the inside of a `{#...}` combo into the keypresses it means.
pub fn parse_combo(spec: &str) -> Result<Vec<Chord>, OutputError> {
    let tokens: Vec<char> = spec.chars().collect();
    let mut at = 0;
    let mut held: Vec<Key> = Vec::new();
    let mut out: Vec<Chord> = Vec::new();

    parse_sequence(spec, &tokens, &mut at, &mut held, &mut out, 0)?;

    if at != tokens.len() {
        // A stray ')' with nothing open.
        return Err(OutputError::MalformedCombo(spec.to_owned()));
    }
    Ok(out)
}

fn parse_sequence(
    spec: &str,
    tokens: &[char],
    at: &mut usize,
    held: &mut Vec<Key>,
    out: &mut Vec<Chord>,
    depth: usize,
) -> Result<(), OutputError> {
    // Deep enough to be a malformed value rather than anything meaningful.
    if depth > 16 {
        return Err(OutputError::MalformedCombo(spec.to_owned()));
    }

    loop {
        while *at < tokens.len() && tokens[*at].is_whitespace() {
            *at += 1;
        }
        if *at >= tokens.len() || tokens[*at] == ')' {
            return Ok(());
        }

        let start = *at;
        while *at < tokens.len()
            && !tokens[*at].is_whitespace()
            && tokens[*at] != '('
            && tokens[*at] != ')'
        {
            *at += 1;
        }
        if start == *at {
            return Err(OutputError::MalformedCombo(spec.to_owned()));
        }

        let name: String = tokens[start..*at].iter().collect();
        let key = key_for(&name).ok_or_else(|| OutputError::UnknownKey(name.clone()))?;

        if *at < tokens.len() && tokens[*at] == '(' {
            // A name with a bracket after it is held down for what is inside.
            *at += 1;
            held.push(key);
            parse_sequence(spec, tokens, at, held, out, depth + 1)?;
            held.pop();

            if *at >= tokens.len() || tokens[*at] != ')' {
                return Err(OutputError::MalformedCombo(spec.to_owned()));
            }
            *at += 1;
        } else {
            out.push(Chord {
                modifiers: held.clone(),
                key,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(spec: &str) -> Vec<Chord> {
        parse_combo(spec).expect(spec)
    }

    fn vks(chords: &[Chord]) -> Vec<(Vec<u16>, u16)> {
        chords
            .iter()
            .map(|c| (c.modifiers.iter().map(|k| k.vk).collect(), c.key.vk))
            .collect()
    }

    #[test]
    fn a_bare_key_is_one_press_with_nothing_held() {
        assert_eq!(vks(&combo("Left")), vec![(vec![], 0x25)]);
    }

    #[test]
    fn a_space_separated_list_presses_each_in_turn() {
        assert_eq!(
            vks(&combo("Home End")),
            vec![(vec![], 0x24), (vec![], 0x23)]
        );
    }

    /// The most common shape in the user's dictionaries, 59 uses.
    #[test]
    fn a_modifier_is_held_for_what_is_inside_its_brackets() {
        assert_eq!(vks(&combo("Control_L(Left)")), vec![(vec![0xA2], 0x25)]);
    }

    /// Real nesting, which is why this is a parser and not a split on '('.
    #[test]
    fn modifiers_nest() {
        assert_eq!(
            vks(&combo("Control_L(Shift(Left))")),
            vec![(vec![0xA2, 0x10], 0x25)]
        );
    }

    #[test]
    fn a_sequence_inside_a_modifier_keeps_it_held_throughout() {
        assert_eq!(
            vks(&combo("Control_L(a b)")),
            vec![(vec![0xA2], 0x41), (vec![0xA2], 0x42)]
        );
    }

    #[test]
    fn a_modifier_stops_applying_after_its_brackets_close() {
        assert_eq!(
            vks(&combo("Control_L(a) b")),
            vec![(vec![0xA2], 0x41), (vec![], 0x42)]
        );
    }

    #[test]
    fn single_letters_use_the_uppercase_ascii_virtual_key() {
        assert_eq!(vks(&combo("c")), vec![(vec![], 0x43)]);
        assert_eq!(vks(&combo("C")), vec![(vec![], 0x43)]);
        assert_eq!(vks(&combo("5")), vec![(vec![], 0x35)]);
    }

    /// The dictionaries contain both spellings and X11 defines only `Return`.
    #[test]
    fn named_keys_are_matched_case_insensitively() {
        assert_eq!(key_for("Return"), key_for("return"));
        assert_eq!(key_for("Page_Down"), key_for("page_down"));
    }

    /// Applications misbehave without the extended flag on these.
    #[test]
    fn navigation_keys_are_marked_extended() {
        for name in ["Left", "Right", "Up", "Down", "Home", "End", "Delete"] {
            assert!(key_for(name).expect(name).extended, "{name}");
        }
        assert!(!key_for("a").expect("a").extended);
        assert!(!key_for("Shift").expect("Shift").extended);
    }

    #[test]
    fn every_key_name_used_in_the_real_dictionaries_resolves() {
        // Measured across cb_dictionary_full.json and dutch.json:
        // 201 combos, 44 distinct names.
        let measured = [
            "Control_L",
            "Left",
            "Right",
            "Shift",
            "Up",
            "Down",
            "Home",
            "End",
            "Control",
            "Delete",
            "Page_Down",
            "Page_Up",
            "Tab",
            "Super_L",
            "Alt_L",
            "F11",
            "F3",
            "Escape",
            "v",
            "k",
            "w",
            "c",
            "BackSpace",
            "return",
            "Return",
            "e",
            "F5",
            "F9",
            "F8",
            "F2",
            "F4",
            "F6",
            "F7",
            "F12",
            "F10",
            "F1",
            "a",
            "y",
            "f",
            "p",
            "n",
            "x",
            "s",
            "z",
        ];
        for name in measured {
            assert!(key_for(name).is_some(), "{name} does not resolve");
        }
    }

    #[test]
    fn an_unknown_name_is_reported_rather_than_silently_dropped() {
        assert!(matches!(
            parse_combo("Frobnicate"),
            Err(OutputError::UnknownKey(_))
        ));
    }

    #[test]
    fn unbalanced_brackets_are_rejected() {
        assert!(parse_combo("Control_L(Left").is_err());
        assert!(parse_combo("Control_L(Left))").is_err());
        assert!(parse_combo(")").is_err());
    }

    #[test]
    fn an_empty_combo_produces_nothing() {
        assert!(combo("").is_empty());
        assert!(combo("   ").is_empty());
    }
}
