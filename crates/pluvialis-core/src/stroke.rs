//! Steno strokes for the English Stenotype system.
//!
//! A stroke is a chord: the set of keys pressed together. It is stored as a
//! bitmask over the 23 keys in canonical steno order, which makes equality,
//! hashing and dictionary lookup cheap.

use std::fmt;

/// The 23 keys of English Stenotype, in canonical order. Bit `i` of a
/// [`Stroke`] corresponds to `KEYS[i]`.
pub const KEYS: [&str; 23] = [
    "#", //
    "S-", "T-", "K-", "P-", "W-", "H-", "R-", //
    "A-", "O-", //
    "*",  //
    "-E", "-U", //
    "-F", "-R", "-P", "-B", "-L", "-G", "-T", "-S", "-D", "-Z",
];

const NUMBER: u32 = 1 << 0;
const STAR: u32 = 1 << 10;

/// Index of the first right bank key (`-F`). An explicit hyphen in a stroke
/// string means "everything after this is right bank".
const RIGHT_BANK_START: usize = 13;

/// `A- O- * -E -U`. These sit in the middle of the steno order, so their
/// presence already tells you which side later keys are on and no hyphen is
/// needed. Plover calls these the implicit hyphen keys.
const CENTER_MASK: u32 = 0b1_1111 << 8;

/// Which key index each character can mean. Ambiguous letters appear on both
/// banks, and parsing resolves them by position: a stroke's keys always appear
/// in canonical order, so the first candidate at or after the current position
/// wins.
fn candidates(c: char) -> Option<&'static [usize]> {
    Some(match c {
        'S' => &[1, 20],
        'T' => &[2, 19],
        'K' => &[3],
        'P' => &[4, 15],
        'W' => &[5],
        'H' => &[6],
        'R' => &[7, 14],
        'A' => &[8],
        'O' => &[9],
        '*' => &[10],
        'E' => &[11],
        'U' => &[12],
        'F' => &[13],
        'B' => &[16],
        'L' => &[17],
        'G' => &[18],
        'D' => &[21],
        'Z' => &[22],
        _ => return None,
    })
}

/// Digits imply the number key plus one specific letter key.
/// `S-`=1 `T-`=2 `P-`=3 `H-`=4 `A-`=5 `O-`=0 `-F`=6 `-P`=7 `-L`=8 `-T`=9
fn digit_key(c: char) -> Option<usize> {
    Some(match c {
        '1' => 1,
        '2' => 2,
        '3' => 4,
        '4' => 6,
        '5' => 8,
        '0' => 9,
        '6' => 13,
        '7' => 15,
        '8' => 17,
        '9' => 19,
        _ => return None,
    })
}

/// The digit a key renders as when the number key is pressed, if any.
fn key_digit(index: usize) -> Option<char> {
    Some(match index {
        1 => '1',
        2 => '2',
        4 => '3',
        6 => '4',
        8 => '5',
        9 => '0',
        13 => '6',
        15 => '7',
        17 => '8',
        19 => '9',
        _ => return None,
    })
}

/// The letter a key renders as, without its side marker.
fn key_letter(index: usize) -> char {
    KEYS[index]
        .chars()
        .find(|c| *c != '-')
        .expect("every key has a letter")
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StrokeError {
    #[error("empty stroke")]
    Empty,
    #[error("invalid character {0:?} in stroke {1:?}")]
    InvalidChar(char, String),
    #[error("key {0:?} is out of steno order in stroke {1:?}")]
    OutOfOrder(char, String),
}

/// A single steno chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Stroke(u32);

impl Stroke {
    /// Parse one stroke in RTF/CRE notation, for example `KAT`, `TK-LS`, `*`
    /// or `1-9`.
    ///
    /// Resolution relies on steno order: within a stroke, keys always appear
    /// left to right in the order of [`KEYS`]. So for an ambiguous letter we
    /// take the first candidate at or after the position reached so far, which
    /// is what makes `KAT` mean `K- A- -T` and `TK-LS` mean `T- K- -L -S`.
    pub fn parse(s: &str) -> Result<Self, StrokeError> {
        let mut bits: u32 = 0;
        // Next key index we are allowed to use. Keys must be consumed in
        // canonical order.
        let mut pos: usize = 0;

        for c in s.chars() {
            match c {
                // The number key may appear anywhere in the stroke
                // (FERAL_NUMBER_KEY in Plover), so it does not move `pos`.
                '#' => {
                    bits |= NUMBER;
                }
                '-' => {
                    // Explicit hyphen: everything after it is right bank.
                    pos = pos.max(RIGHT_BANK_START);
                }
                _ => {
                    let index = if let Some(index) = digit_key(c) {
                        bits |= NUMBER;
                        index
                    } else {
                        let options = candidates(c)
                            .ok_or_else(|| StrokeError::InvalidChar(c, s.to_owned()))?;
                        *options
                            .iter()
                            .find(|i| **i >= pos)
                            .ok_or_else(|| StrokeError::OutOfOrder(c, s.to_owned()))?
                    };
                    if index < pos {
                        return Err(StrokeError::OutOfOrder(c, s.to_owned()));
                    }
                    bits |= 1 << index;
                    pos = index + 1;
                }
            }
        }

        if bits == 0 {
            return Err(StrokeError::Empty);
        }
        Ok(Stroke(bits))
    }

    /// Parse a full steno outline, one or more strokes separated by `/`.
    pub fn parse_outline(s: &str) -> Result<Vec<Stroke>, StrokeError> {
        if s.is_empty() {
            return Err(StrokeError::Empty);
        }
        s.split('/').map(Stroke::parse).collect()
    }

    /// Render an outline back to `/`-separated notation.
    pub fn render_outline(strokes: &[Stroke]) -> String {
        strokes
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// True if this stroke is the undo chord: the star key alone.
    pub fn is_undo(self) -> bool {
        self.0 == STAR
    }

    pub fn has_number_key(self) -> bool {
        self.0 & NUMBER != 0
    }

    /// The raw bitmask. Bit `i` corresponds to `KEYS[i]`.
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Build a stroke from a bitmask, for machine decoders.
    pub fn from_bits(bits: u32) -> Result<Self, StrokeError> {
        if bits == 0 {
            return Err(StrokeError::Empty);
        }
        Ok(Stroke(bits))
    }

    /// Build a stroke from key names such as `S-`, `*`, `-Z`. Unknown names
    /// are reported rather than ignored.
    pub fn from_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<Self, StrokeError> {
        let mut bits = 0u32;
        for key in keys {
            let index = KEYS
                .iter()
                .position(|k| *k == key)
                .ok_or_else(|| StrokeError::InvalidChar('?', key.to_owned()))?;
            bits |= 1 << index;
        }
        Stroke::from_bits(bits)
    }
}

impl fmt::Display for Stroke {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let numbered = self.0 & NUMBER != 0;
        let has_center = self.0 & CENTER_MASK != 0;
        let mut out = String::with_capacity(8);
        let mut used_digit = false;
        let mut wrote_right = false;

        for index in 1..KEYS.len() {
            if self.0 & (1 << index) == 0 {
                continue;
            }
            // A hyphen is only needed to show that we have crossed to the
            // right bank, and only when no center key already showed it.
            if index >= RIGHT_BANK_START && !has_center && !wrote_right {
                out.push('-');
            }
            if index >= RIGHT_BANK_START {
                wrote_right = true;
            }
            match key_digit(index).filter(|_| numbered) {
                Some(digit) => {
                    out.push(digit);
                    used_digit = true;
                }
                None => out.push(key_letter(index)),
            }
        }

        // When digits were substituted the number key is implied by them.
        // Otherwise it has to be shown, as in `#K` or a bare `#`.
        if numbered && !used_digit {
            out.insert(0, '#');
        }
        f.write_str(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(s: &str) -> String {
        Stroke::parse(s).unwrap().to_string()
    }

    #[test]
    fn resolves_ambiguous_letters_by_position() {
        // The T must be the right bank one, because A- comes before it.
        assert_eq!(Stroke::parse("KAT").unwrap().to_string(), "KAT");
        // Both S keys in one stroke.
        assert_eq!(roundtrip("SAS"), "SAS");
        // Without a center key, the hyphen decides.
        assert_eq!(roundtrip("TK-LS"), "TK-LS");
    }

    #[test]
    fn left_bank_only_needs_no_hyphen() {
        assert_eq!(roundtrip("STKPWHR"), "STKPWHR");
    }

    #[test]
    fn right_bank_only_keeps_its_hyphen() {
        assert_eq!(roundtrip("-Z"), "-Z");
        assert_eq!(roundtrip("-FRPBLGTSDZ"), "-FRPBLGTSDZ");
    }

    #[test]
    fn center_keys_make_the_hyphen_unnecessary() {
        assert_eq!(roundtrip("AOEU"), "AOEU");
        assert_eq!(roundtrip("KAEUT"), "KAEUT");
        // A star alone is enough to place the following keys.
        assert_eq!(roundtrip("KA*T"), "KA*T");
    }

    #[test]
    fn star_alone_is_the_undo_stroke() {
        assert!(Stroke::parse("*").unwrap().is_undo());
        assert!(!Stroke::parse("KA*T").unwrap().is_undo());
        assert!(!Stroke::parse("KAT").unwrap().is_undo());
    }

    #[test]
    fn digits_imply_the_number_key() {
        let one = Stroke::parse("1").unwrap();
        assert!(one.has_number_key());
        assert_eq!(one, Stroke::parse("#S").unwrap());
        assert_eq!(one.to_string(), "1");

        // 1234 is #STPH, and 0 is O-.
        assert_eq!(
            Stroke::parse("1234").unwrap(),
            Stroke::parse("#STPH").unwrap()
        );
        assert_eq!(Stroke::parse("0").unwrap(), Stroke::parse("#O").unwrap());
        // Right bank digits keep the hyphen.
        assert_eq!(roundtrip("-6"), "-6");
        assert_eq!(Stroke::parse("-6").unwrap(), Stroke::parse("#-F").unwrap());
    }

    #[test]
    fn number_key_shows_when_no_digit_substitution_applies() {
        // K- has no digit, so the number key has to be written out.
        assert_eq!(roundtrip("#K"), "#K");
        assert_eq!(roundtrip("#"), "#");
    }

    #[test]
    fn number_key_may_appear_anywhere() {
        // FERAL_NUMBER_KEY: these are all the same stroke.
        let expected = Stroke::parse("#K").unwrap();
        assert_eq!(Stroke::parse("K#").unwrap(), expected);
    }

    #[test]
    fn outlines_split_on_slash() {
        let outline = Stroke::parse_outline("WEL/KO*PL").unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(Stroke::render_outline(&outline), "WEL/KO*PL");
    }

    #[test]
    fn rejects_invalid_input() {
        assert_eq!(Stroke::parse(""), Err(StrokeError::Empty));
        assert!(matches!(
            Stroke::parse("KQT"),
            Err(StrokeError::InvalidChar('Q', _))
        ));
        // Out of steno order: the right bank S cannot be followed by K-.
        assert!(matches!(
            Stroke::parse("ASK"),
            Err(StrokeError::OutOfOrder('K', _))
        ));
    }

    #[test]
    fn from_keys_matches_parsing() {
        let by_keys = Stroke::from_keys(["K-", "A-", "-T"]).unwrap();
        assert_eq!(by_keys, Stroke::parse("KAT").unwrap());
    }
}
