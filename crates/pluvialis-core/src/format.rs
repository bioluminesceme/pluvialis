//! Turning translations into the text that actually appears.
//!
//! Dictionary values are not plain text: they carry meta commands controlling
//! spacing, capitalization and attachment. `{^ing}` glues to the previous word
//! with spelling rules applied, `{-|}` capitalizes what follows, `{.}` ends a
//! sentence.
//!
//! Scope is driven by `reference/DICTIONARY-AUDIT.md`: 98.6% of the user's
//! entries are plain text and the meta commands in use are a short, measured
//! list. Anything outside it is recorded in [`Formatted::unknown_metas`]
//! rather than silently dropped, because a dropped meta produces subtly wrong
//! text that is only noticed much later.
//!
//! The whole history is reformatted on each call rather than updated
//! incrementally. Retroactive correction can rewrite arbitrarily far back, and
//! a full pass over a document costs microseconds, so this trades a little
//! work for removing a whole class of stale state bugs.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use fancy_regex::Regex;

use crate::orthography::add_suffix;
use crate::translator::Translation;

/// Matches one word, mirroring Plover's `WORD_RX`.
static WORD_RX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\d+(?:[.,]\d+)+|[\'\w]+[-\w\']*|[^\w\s]+)\s*").expect("word pattern is valid")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    CapFirstWord,
    LowerFirstChar,
    UpperFirstWord,
}

/// Something the formatter produced that is not text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `{#Control_L(Left)}`. Key names are X11 keysyms and need mapping to
    /// virtual key codes at the output layer.
    KeyCombo(String),
    /// `{PLOVER:TOGGLE}` and friends: commands aimed at the application.
    Command(String),
}

/// The result of formatting a run of translations.
#[derive(Debug, Clone, Default)]
pub struct Formatted {
    pub text: String,
    pub events: Vec<Event>,
    /// Byte ranges in `text` produced by untranslated strokes. The live view
    /// paints these red.
    pub raw_ranges: Vec<(usize, usize)>,
    /// Meta commands we do not implement, by exact string.
    pub unknown_metas: BTreeSet<String>,
}

/// One piece of a dictionary value.
#[derive(Debug, PartialEq, Eq)]
enum Atom {
    Text(String),
    Meta(String),
}

/// Split a dictionary value into literal text and `{...}` meta commands,
/// honouring `\{` and `\}` escapes.
fn parse_atoms(value: &str) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let mut text = String::new();
    let mut chars = value.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' if matches!(chars.peek(), Some('{') | Some('}')) => {
                text.push(chars.next().expect("peeked"));
            }
            '{' => {
                if !text.is_empty() {
                    atoms.push(Atom::Text(std::mem::take(&mut text)));
                }
                let mut meta = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' if matches!(chars.peek(), Some('{') | Some('}')) => {
                            meta.push(chars.next().expect("peeked"));
                        }
                        '}' => break,
                        _ => meta.push(c),
                    }
                }
                atoms.push(Atom::Meta(meta));
            }
            _ => text.push(c),
        }
    }
    if !text.is_empty() {
        atoms.push(Atom::Text(text));
    }
    atoms
}

fn capitalize_first_word(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn lower_first_character(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn upper_first_word(s: &str) -> String {
    match WORD_RX.find(s).ok().flatten() {
        Some(m) if m.start() == 0 => s[..m.end()].to_uppercase() + &s[m.end()..],
        _ => s.to_owned(),
    }
}

fn apply_case(text: &str, case: Case) -> String {
    match case {
        Case::CapFirstWord => capitalize_first_word(text),
        Case::LowerFirstChar => lower_first_character(text),
        Case::UpperFirstWord => upper_first_word(text),
    }
}

/// The trailing word of `s`, or empty when `s` ends on a boundary. This is
/// what a following suffix attaches to.
fn rightmost_word(s: &str) -> String {
    match WORD_RX.find_iter(s).filter_map(Result::ok).last() {
        // A trailing space means the word is finished and nothing attaches.
        Some(m) if m.as_str().ends_with(char::is_whitespace) => String::new(),
        Some(m) => m.as_str().to_owned(),
        None => String::new(),
    }
}

fn has_word_boundary(s: &str) -> bool {
    s.chars().any(char::is_whitespace)
}

/// Byte length of the common prefix of two strings, aligned to a char
/// boundary.
fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut len = 0;
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            break;
        }
        len += x.len_utf8();
    }
    len
}

#[derive(Default)]
struct State {
    out: String,
    /// Suppress the space before the next text.
    attach_next: bool,
    next_case: Option<Case>,
    /// Trailing word, for orthography when a suffix arrives.
    last_word: String,
    last_glue: bool,
    /// Set by `{}`, which asks for a plain join with no spelling rules.
    orthography: bool,
    caps_mode: bool,
    space: String,
    upper_carry: bool,
}

impl State {
    fn new() -> Self {
        State {
            attach_next: true, // nothing precedes the first word
            orthography: true,
            space: " ".to_owned(),
            ..Default::default()
        }
    }

    /// Append text, deciding spacing, case and mode. Returns the byte range
    /// of the text itself, excluding any space added before it.
    fn emit(&mut self, text: &str, prev_attach: bool, next_attach: bool) -> (usize, usize) {
        let attach = prev_attach || self.attach_next;

        let mut text = match self.next_case.take() {
            Some(case) => apply_case(text, case),
            None if self.upper_carry && attach => upper_first_word(text),
            None => text.to_owned(),
        };
        if self.caps_mode {
            text = text.to_uppercase();
        }
        self.upper_carry = self.upper_carry && attach && !has_word_boundary(&text);

        if !attach && !self.out.is_empty() {
            self.out.push_str(&self.space);
        }
        let start = self.out.len();
        self.out.push_str(&text);
        let range = (start, self.out.len());

        self.last_word = if attach {
            rightmost_word(&format!("{}{}", self.last_word, text))
        } else {
            rightmost_word(&text)
        };
        self.attach_next = next_attach;
        self.last_glue = false;
        range
    }

    /// Attach a suffix, rewriting the previous word per English spelling.
    fn emit_suffix(&mut self, suffix: &str) {
        if self.last_word.is_empty() || !self.orthography || suffix.trim().is_empty() {
            self.emit(suffix, true, false);
            return;
        }

        let new_word = add_suffix(&self.last_word, suffix);
        let common = common_prefix_len(&self.last_word, &new_word);
        // Remove the part of the previous word that the rule changed.
        let remove = self.last_word.len() - common;
        let keep = self.out.len().saturating_sub(remove);
        self.out.truncate(keep);
        self.out.push_str(&new_word[common..]);

        self.last_word = rightmost_word(&new_word);
        self.attach_next = false;
        self.last_glue = false;
    }

    /// Rewrite the last word in place, for the retroactive case commands.
    fn retro_case(&mut self, case: Case) {
        let word = rightmost_word(&self.out);
        if word.is_empty() {
            return;
        }
        let start = self.out.len() - word.len();
        let cased = apply_case(&word, case);
        self.out.truncate(start);
        self.out.push_str(&cased);
        self.last_word = rightmost_word(&cased);
    }
}

/// Format a run of translations into the text they produce.
pub fn format(translations: &[Translation]) -> Formatted {
    let mut state = State::new();
    let mut result = Formatted::default();

    for translation in translations {
        match &translation.text {
            // Untranslated: the raw steno goes in as an ordinary word, and we
            // record where it landed so the live view can paint it red.
            None => {
                let raw = translation.output();
                let range = state.emit(&raw, false, false);
                result.raw_ranges.push(range);
            }
            Some(value) => {
                for atom in parse_atoms(value) {
                    match atom {
                        Atom::Text(text) => {
                            state.emit(&text, false, false);
                        }
                        Atom::Meta(meta) => apply_meta(&mut state, &mut result, &meta),
                    }
                }
            }
        }
    }

    result.text = state.out;
    result
}

fn apply_meta(state: &mut State, result: &mut Formatted, meta: &str) {
    // Order matters and follows Plover's dispatch table.
    //
    // The macro name must be non-empty and cannot start with a colon, per
    // Plover's `:([^:]+):?(.*)`. Without that guard `{:}` is swallowed here
    // instead of falling through to the colon punctuation below.
    if let Some(rest) = meta.strip_prefix(':')
        && !rest.is_empty()
        && !rest.starts_with(':')
    {
        return apply_macro(state, result, rest);
    }
    if let Some(command) = meta.strip_prefix("PLOVER:") {
        result.events.push(Event::Command(command.to_owned()));
        return;
    }
    if let Some(combo) = meta.strip_prefix('#') {
        result.events.push(Event::KeyCombo(combo.to_owned()));
        return;
    }
    if let Some(mode) = meta.strip_prefix("MODE:") {
        return apply_mode(state, result, mode);
    }

    match meta {
        // Punctuation attaches to the previous word. Sentence enders also
        // capitalize what comes next.
        "," | ";" | ":" => {
            state.emit(meta, true, false);
        }
        "." | "!" | "?" => {
            state.emit(meta, true, false);
            state.next_case = Some(Case::CapFirstWord);
        }
        "-|" => state.next_case = Some(Case::CapFirstWord),
        ">" => state.next_case = Some(Case::LowerFirstChar),
        "<" => {
            state.next_case = Some(Case::UpperFirstWord);
            state.upper_carry = true;
        }
        "*-|" => state.retro_case(Case::CapFirstWord),
        "*>" => state.retro_case(Case::LowerFirstChar),
        "*<" => state.retro_case(Case::UpperFirstWord),
        "$" => state.last_word.clear(),
        // An empty meta asks for a plain join with no spelling rules.
        "" => {
            state.attach_next = true;
            state.orthography = false;
        }
        _ => {
            if let Some(text) = meta.strip_prefix('&') {
                // Glue joins to adjacent glue, which is how fingerspelling
                // and numbers run together.
                let attach = state.last_glue;
                state.emit(text, attach, false);
                state.last_glue = true;
                return;
            }
            if let Some(rest) = strip_carry_capitalize(meta) {
                let (text, prev, next) = rest;
                // Carry capitalization keeps a pending capital across the
                // inserted text, as in {~|"}.
                let pending = state.next_case;
                state.emit(&text, prev, next);
                state.next_case = pending;
                return;
            }
            if meta.starts_with('^') || meta.ends_with('^') {
                return apply_attach(state, meta);
            }
            // Nothing recognised it. Record the exact string rather than
            // guessing at semantics we have not verified.
            result.unknown_metas.insert(meta.to_owned());
        }
    }
}

/// `{^text}`, `{text^}`, `{^text^}`, `{^}`.
fn apply_attach(state: &mut State, meta: &str) {
    let begin = meta.starts_with('^');
    let end = meta.ends_with('^') && meta.len() > 1;

    let body = {
        let mut b = meta;
        if begin {
            b = &b[1..];
        }
        if end && !b.is_empty() {
            b = &b[..b.len() - 1];
        }
        b
    };

    if body.is_empty() {
        // A bare `{^}` just suppresses the next space.
        state.attach_next = true;
        return;
    }

    // A suffix attaches with spelling rules; a prefix or infix does not.
    if begin && !end {
        state.emit_suffix(body);
    } else {
        state.emit(body, begin, end);
    }
}

/// `{~|text}` with optional attach flags. Returns the text and its attachment.
fn strip_carry_capitalize(meta: &str) -> Option<(String, bool, bool)> {
    let begin = meta.starts_with('^');
    let rest = if begin { &meta[1..] } else { meta };
    let rest = rest.strip_prefix("~|")?;
    let end = rest.ends_with('^');
    let body = if end { &rest[..rest.len() - 1] } else { rest };
    Some((body.to_owned(), begin, end))
}

/// `{:name:argument}` plugin style metas.
fn apply_macro(state: &mut State, result: &mut Formatted, rest: &str) {
    let (name, argument) = match rest.split_once(':') {
        Some((name, argument)) => (name, argument),
        None => (rest, ""),
    };

    let case = |argument: &str| match argument {
        "cap_first_word" => Some(Case::CapFirstWord),
        "lower_first_char" => Some(Case::LowerFirstChar),
        "upper_first_word" => Some(Case::UpperFirstWord),
        _ => None,
    };

    match name {
        "case" => match case(argument) {
            Some(c) => state.next_case = Some(c),
            None => {
                result.unknown_metas.insert(format!(":case:{argument}"));
            }
        },
        "retro_case" => match case(argument) {
            Some(c) => state.retro_case(c),
            None => {
                result
                    .unknown_metas
                    .insert(format!(":retro_case:{argument}"));
            }
        },
        // Stitching joins letters with a separator, as in F-B-I.
        "stitch" => {
            let attach = state.last_glue;
            let text = if attach {
                format!("-{argument}")
            } else {
                argument.to_owned()
            };
            state.emit(&text, attach, false);
            state.last_glue = true;
        }
        _ => {
            result.unknown_metas.insert(format!(":{rest}"));
        }
    }
}

fn apply_mode(state: &mut State, result: &mut Formatted, mode: &str) {
    if let Some(space) = mode.strip_prefix("SET_SPACE:") {
        state.space = space.to_owned();
        return;
    }
    match mode {
        "CAPS" => state.caps_mode = true,
        "RESET" | "RESET_CASE" => state.caps_mode = false,
        _ => {
            result.unknown_metas.insert(format!("MODE:{mode}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::Stroke;

    /// Build translations directly, so these tests cover formatting only.
    fn t(text: Option<&str>) -> Translation {
        Translation::for_test(vec![Stroke::parse("KAT").unwrap()], text.map(str::to_owned))
    }

    fn render(values: &[Option<&str>]) -> String {
        let translations: Vec<Translation> = values.iter().map(|v| t(*v)).collect();
        format(&translations).text
    }

    #[test]
    fn plain_words_are_joined_with_spaces() {
        assert_eq!(render(&[Some("hello"), Some("world")]), "hello world");
    }

    #[test]
    fn no_leading_space_at_the_start() {
        assert_eq!(render(&[Some("hello")]), "hello");
    }

    #[test]
    fn suffix_attaches_with_spelling_rules() {
        assert_eq!(render(&[Some("run"), Some("{^ing}")]), "running");
        assert_eq!(render(&[Some("write"), Some("{^ing}")]), "writing");
        assert_eq!(render(&[Some("cat"), Some("{^s}")]), "cats");
    }

    #[test]
    fn prefix_attaches_to_what_follows() {
        assert_eq!(render(&[Some("{re^}"), Some("do")]), "redo");
    }

    #[test]
    fn infix_attaches_on_both_sides() {
        assert_eq!(render(&[Some("in"), Some("{^-^}"), Some("law")]), "in-law");
    }

    #[test]
    fn bare_attach_suppresses_one_space() {
        assert_eq!(render(&[Some("foo"), Some("{^}"), Some("bar")]), "foobar");
    }

    #[test]
    fn punctuation_attaches_and_capitalizes() {
        assert_eq!(
            render(&[Some("hello"), Some("{.}"), Some("world")]),
            "hello. World"
        );
        assert_eq!(render(&[Some("one"), Some("{,}"), Some("two")]), "one, two");
    }

    #[test]
    fn explicit_case_commands() {
        assert_eq!(render(&[Some("{-|}"), Some("hello")]), "Hello");
        assert_eq!(render(&[Some("{>}"), Some("Hello")]), "hello");
        assert_eq!(render(&[Some("{<}"), Some("hello")]), "HELLO");
    }

    #[test]
    fn retroactive_case_rewrites_the_previous_word() {
        assert_eq!(render(&[Some("hello"), Some("{*-|}")]), "Hello");
        assert_eq!(render(&[Some("Hello"), Some("{*>}")]), "hello");
    }

    #[test]
    fn glue_joins_to_adjacent_glue_only() {
        // Fingerspelling runs together, but a normal word does not join it.
        assert_eq!(render(&[Some("{&f}"), Some("{&b}"), Some("{&i}")]), "fbi");
        assert_eq!(
            render(&[Some("say"), Some("{&a}"), Some("word")]),
            "say a word"
        );
    }

    #[test]
    fn stitching_joins_with_hyphens() {
        assert_eq!(
            render(&[
                Some("{:stitch:F}"),
                Some("{:stitch:B}"),
                Some("{:stitch:I}")
            ]),
            "F-B-I"
        );
    }

    #[test]
    fn plugin_case_macros() {
        assert_eq!(
            render(&[Some("{:case:cap_first_word}"), Some("hello")]),
            "Hello"
        );
        assert_eq!(
            render(&[Some("hello"), Some("{:retro_case:upper_first_word}")]),
            "HELLO"
        );
    }

    #[test]
    fn caps_mode_applies_until_reset() {
        assert_eq!(
            render(&[
                Some("{MODE:CAPS}"),
                Some("loud"),
                Some("{MODE:RESET}"),
                Some("quiet")
            ]),
            "LOUD quiet"
        );
    }

    #[test]
    fn untranslated_strokes_appear_as_raw_steno_and_are_marked() {
        let translations = vec![t(Some("cat")), t(None)];
        let formatted = format(&translations);
        assert_eq!(formatted.text, "cat KAT");
        assert_eq!(formatted.raw_ranges, vec![(4, 7)]);
        // The recorded range must actually cover the raw text.
        let (from, to) = formatted.raw_ranges[0];
        assert_eq!(&formatted.text[from..to], "KAT");
    }

    #[test]
    fn key_combos_and_commands_become_events_not_text() {
        let formatted = format(&[t(Some("{#Control_L(Left)}")), t(Some("{PLOVER:TOGGLE}"))]);
        assert_eq!(formatted.text, "");
        assert_eq!(
            formatted.events,
            vec![
                Event::KeyCombo("Control_L(Left)".to_owned()),
                Event::Command("TOGGLE".to_owned()),
            ]
        );
    }

    #[test]
    fn bare_colon_is_punctuation_not_an_empty_macro() {
        // Plover's macro pattern requires a non-empty name, so `{:}` falls
        // through to the punctuation rule.
        assert_eq!(render(&[Some("hello"), Some("{:}")]), "hello:");
        let formatted = format(&[t(Some("{:}"))]);
        assert!(formatted.unknown_metas.is_empty());
    }

    #[test]
    fn unrecognized_metas_are_recorded_never_dropped_silently() {
        let formatted = format(&[t(Some("{*!}")), t(Some("{NONSENSE}"))]);
        assert!(formatted.unknown_metas.contains("*!"));
        assert!(formatted.unknown_metas.contains("NONSENSE"));
    }

    #[test]
    fn escaped_braces_are_literal_text() {
        assert_eq!(
            parse_atoms(r"\{literal\}"),
            vec![Atom::Text("{literal}".to_owned())]
        );
    }

    #[test]
    fn atoms_split_text_and_meta() {
        assert_eq!(
            parse_atoms("{-|}hello{^ing}"),
            vec![
                Atom::Meta("-|".to_owned()),
                Atom::Text("hello".to_owned()),
                Atom::Meta("^ing".to_owned()),
            ]
        );
    }
}
