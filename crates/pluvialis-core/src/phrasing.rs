//! Jeff Phrasing: a generative phrase dictionary.
//!
//! This is a native port of `jeff-phrasing.py`. Rather than storing phrases, it
//! splits one stroke into six groups and composes an answer from small tables:
//!
//! ```text
//! (S T K P W H R) (A O) -? (*) (E U) (F) (R P B L G T S D Z)
//!  starter         v1        star v2   f  ender
//! ```
//!
//! The left bank picks the subject, the vowels and star pick the modal and the
//! sentence shape, and the right bank picks the verb. `SWR-RB` is "I ask",
//! `SWRAOEUF` is "I will never have".
//!
//! LONGEST_KEY is 1: every answer comes from a single stroke, which is what
//! makes the answered space finite. `tests/phrasing_fixture.json` holds the
//! output of the Python for all 218,071 enumerated strokes and the test below
//! checks every one of them.

use std::collections::HashMap;
use std::sync::LazyLock;

/// A value tree matching the nested dicts in the Python. Maps are small (at
/// most eight entries) and are keyed by tense or verb form, with `None`
/// standing for the Python default entry.
enum Data {
    Text(String),
    Map(Vec<(Option<&'static str>, Data)>),
}

/// The middle of a phrase: a word plus the verb form it forces on the ender,
/// wrapped in the same tense/form maps as [`Data`].
enum Middle {
    Word(String, Option<&'static str>),
    Map(Vec<(Option<&'static str>, Middle)>),
}

/// The subject of the phrase. `valid_enders` restricts which verbs a subject
/// accepts, which only the two "there" starters need: they collide with a large
/// number of ordinary briefs, so they answer a hand-picked list instead.
struct Starter {
    word: &'static str,
    verb_form: &'static str,
    valid_enders: Option<&'static [&'static str]>,
}

/// The sentence shape. `!` stands for the starter and `*` for the middle word.
struct Structure {
    format: Data,
    /// Whether the middle's verb form overrides the starter's, which is how
    /// "I can go" gets the root "go" instead of the first person "go".
    use_middle_verb_form: bool,
    /// Applied after the middle phrase is built, so it affects only the ender.
    verb_update: Option<&'static str>,
}

struct Ender {
    tense: &'static str,
    verb: Data,
}

// Strokes that mean something else in the user's other dictionaries and must
// not be swallowed by phrasing.
const NON_PHRASE_STROKES: [&str; 8] = [
    "STHR",       // "is there"
    "STHRET",     // "stiletto"
    "STHREUPLT",  // "stimulate"
    "STPHREFPLT", // "investment in"
    "SKPUR",      // "and you're", not "and you run"
    "SKPUL",      // "and you'll", not "and you look"
    "SKPEUT",     // "and it", not "and I have"
    "SKP*",       // {&&}
];

/// The only enders the "there" starters accept.
const THERE_SUFFIXES: [&str; 40] = [
    "", "D", // past tense
    "B", "BT", "BD", "BTD", // be (a)
    "BG", "BGD", // come
    "G", "GD", // go
    "PZ", "PDZ", // happen
    "T", "TD", "TS", "TSDZ", // have (to)
    "LZ", "LZD", // live
    "PL", "PLT", "PLD", "PLTD", // may (have)
    "PBLGS", "PBLGTS", // must (have)
    "PBLGSZ", "PBLGTSDZ", // just
    "RPG", "RPGD", "RPGT", "RPGTD", // need (to)
    "RLG", "RLGD", // really
    "PLS", "PLSZ", "PLTS", "PLTSDZ", // seem (to)
    "Z", "DZ", "TZ", "TDZ", // use (to)
];

const TO_BE_PRESENT: [(Option<&str>, &str); 6] = [
    (None, " are"),
    (Some("root"), " be"),
    (Some("1ps"), " am"),
    (Some("3ps"), " is"),
    (Some("present-participle"), " being"),
    (Some("past-participle"), " been"),
];

const TO_BE_PAST: [(Option<&str>, &str); 6] = [
    (None, " were"),
    (Some("root"), " be"),
    (Some("1ps"), " was"),
    (Some("3ps"), " was"),
    (Some("present-participle"), " being"),
    (Some("past-participle"), " been"),
];

const TO_HAVE_PRESENT: [(Option<&str>, &str); 4] = [
    (None, " have"),
    (Some("3ps"), " has"),
    (Some("present-participle"), " having"),
    (Some("past-participle"), " had"),
];

const TO_HAVE_PAST: [(Option<&str>, &str); 4] = [
    (Some("root"), " have"),
    (None, " had"),
    (Some("present-participle"), " having"),
    (Some("past-participle"), " had"),
];

fn text(s: &str) -> Data {
    Data::Text(s.to_owned())
}

fn map(entries: Vec<(Option<&'static str>, Data)>) -> Data {
    Data::Map(entries)
}

/// Wrap a conjugation table in a fixed prefix and suffix, which is how the
/// Python builds the "to be" and "to have" structures out of one source table.
fn affix(pairs: &[(Option<&'static str>, &'static str)], prefix: &str, suffix: &str) -> Data {
    Data::Map(
        pairs
            .iter()
            .map(|(form, word)| (*form, Data::Text(format!("{prefix}{word}{suffix}"))))
            .collect(),
    )
}

/// A tense pair, the shape every conjugation-derived table takes.
fn by_tense(present: Data, past: Data) -> Data {
    map(vec![(Some("present"), present), (Some("past"), past)])
}

/// A regular present-tense ender: base form, third person singular, and the two
/// participles.
fn present_verb(base: &str, third: &str, ing: &str, past_participle: &str) -> Data {
    map(vec![
        (None, text(base)),
        (Some("3ps"), text(third)),
        (Some("present-participle"), text(ing)),
        (Some("past-participle"), text(past_participle)),
    ])
}

/// A regular past-tense ender. The default entry is the past form; the root is
/// what a modal in the middle asks for, as in "I could ask".
fn past_verb(past: &str, root: &str, ing: &str, past_participle: &str) -> Data {
    map(vec![
        (None, text(past)),
        (Some("root"), text(root)),
        (Some("present-participle"), text(ing)),
        (Some("past-participle"), text(past_participle)),
    ])
}

/// The fallback chain the Python `_lookup_data` walks for a single key: the key
/// itself, then the segment after the first hyphen, then that segment with a
/// leading `b` (blank subject) removed, and finally the default entry.
fn key_candidates(key: &str) -> Vec<Option<&str>> {
    let mut candidates = vec![Some(key)];
    let mut current = key;
    if key.contains('-') {
        current = key.split('-').nth(1).unwrap_or("");
        candidates.push(Some(current));
    }
    if let Some(rest) = current.strip_prefix('b') {
        candidates.push(Some(rest));
    }
    candidates.push(None);
    candidates
}

fn find<'a, T>(entries: &'a [(Option<&'static str>, T)], key: Option<&str>) -> Option<&'a T> {
    entries
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, value)| value)
}

fn step<'a, T>(entries: &'a [(Option<&'static str>, T)], key: &str) -> Option<&'a T> {
    key_candidates(key)
        .into_iter()
        .find_map(|candidate| find(entries, candidate))
}

impl Data {
    /// Descend through the tree, one key per level. A leaf absorbs any
    /// remaining keys, so a table may stop at any depth.
    fn get(&self, keys: &[&str]) -> Option<&Data> {
        let mut node = Some(self);
        for key in keys {
            match node {
                Some(Data::Map(entries)) => node = step(entries, key),
                other => return other,
            }
        }
        node
    }

    fn as_text(&self) -> Option<&str> {
        match self {
            Data::Text(s) => Some(s),
            Data::Map(_) => None,
        }
    }
}

impl Middle {
    fn get(&self, keys: &[&str]) -> Option<&Middle> {
        let mut node = Some(self);
        for key in keys {
            match node {
                Some(Middle::Map(entries)) => node = step(entries, key),
                other => return other,
            }
        }
        node
    }
}

struct Tables {
    starters: HashMap<&'static str, Starter>,
    simple_starters: HashMap<&'static str, Middle>,
    simple_pronouns: HashMap<&'static str, Starter>,
    simple_structures: HashMap<&'static str, Structure>,
    middles: HashMap<&'static str, Middle>,
    structure_exceptions: HashMap<&'static str, Structure>,
    structures: HashMap<&'static str, Structure>,
    enders: HashMap<&'static str, Ender>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| Tables {
    starters: build_starters(),
    simple_starters: build_simple_starters(),
    simple_pronouns: build_simple_pronouns(),
    simple_structures: build_simple_structures(),
    middles: build_middles(),
    structure_exceptions: build_structure_exceptions(),
    structures: build_structures(),
    enders: build_enders(),
});

/// Subjects with no ender restriction, the common case.
fn plain_starters(
    rows: &[(&'static str, &'static str, &'static str)],
) -> HashMap<&'static str, Starter> {
    rows.iter()
        .map(|(key, word, verb_form)| {
            (
                *key,
                Starter {
                    word,
                    verb_form,
                    valid_enders: None,
                },
            )
        })
        .collect()
}

fn build_starters() -> HashMap<&'static str, Starter> {
    let mut m = plain_starters(&[
        ("SWR", "I", "1ps"),
        ("KPWR", "you", "2p"),
        ("KWHR", "he", "3ps"),
        ("SKWHR", "she", "3ps"),
        ("KPWH", "it", "3ps"),
        ("TWR", "we", "1pp"),
        ("TWH", "they", "3pp"),
        ("STKH", "this", "3ps"),
        ("STWH", "that", "3ps"),
        // 'b' marks a blank subject: the structure prints no pronoun.
        ("STKPWHR", "", "b3ps"),
        ("STWR", "", "b3pp"),
    ]);
    for (key, verb_form) in [("STHR", "3ps"), ("STPHR", "3pp")] {
        m.insert(
            key,
            Starter {
                word: "there",
                verb_form,
                valid_enders: Some(&THERE_SUFFIXES),
            },
        );
    }
    m
}

fn build_simple_starters() -> HashMap<&'static str, Middle> {
    [
        ("STHA", " that"),
        ("STPA", " if"),
        ("SWH", " when"),
        ("SWHA", " what"),
        ("SWHR", " where"),
        ("SWHO", " who"),
        ("SWHAO", " why"),
        ("SPWH", " but"),
        ("STPR", " for"),
        ("SKP", " and"),
    ]
    .into_iter()
    .map(|(key, word)| (key, Middle::Word(word.to_owned(), None)))
    .collect()
}

fn build_simple_pronouns() -> HashMap<&'static str, Starter> {
    plain_starters(&[
        ("E", "he", "3ps"),
        ("*E", "she", "3ps"),
        ("U", "you", "2p"),
        ("*U", "they", "3pp"),
        ("EU", "I", "1ps"),
        ("*EU", "we", "1pp"),
        ("*", "it", "3ps"),
    ])
}

fn build_simple_structures() -> HashMap<&'static str, Structure> {
    let mut m = HashMap::new();
    m.insert(
        "",
        Structure {
            format: text("* !"),
            use_middle_verb_form: true,
            verb_update: None,
        },
    );
    m.insert(
        "F",
        Structure {
            format: by_tense(
                affix(&TO_HAVE_PRESENT, "* !", ""),
                affix(&TO_HAVE_PAST, "* !", ""),
            ),
            use_middle_verb_form: true,
            verb_update: Some("past-participle"),
        },
    );
    m
}

fn build_middles() -> HashMap<&'static str, Middle> {
    fn modal(word: &str) -> Middle {
        Middle::Word(word.to_owned(), Some("root"))
    }
    fn tenses(present: Middle, past: Middle) -> Middle {
        Middle::Map(vec![(Some("present"), present), (Some("past"), past)])
    }

    let mut m = HashMap::new();
    // The empty and star middles are the only ones that inflect: "do" has to
    // agree with the subject, the modals do not.
    m.insert(
        "",
        tenses(
            Middle::Map(vec![(None, modal(" do")), (Some("3ps"), modal(" does"))]),
            modal(" did"),
        ),
    );
    m.insert(
        "*",
        tenses(
            Middle::Map(vec![
                (None, modal(" don't")),
                (Some("3ps"), modal(" doesn't")),
            ]),
            modal(" didn't"),
        ),
    );
    m.insert("A", tenses(modal(" can"), modal(" could")));
    m.insert("A*", tenses(modal(" can't"), modal(" couldn't")));
    m.insert("O", tenses(modal(" shall"), modal(" should")));
    m.insert("O*", tenses(modal(" shall not"), modal(" shouldn't")));
    m.insert("AO", tenses(modal(" will"), modal(" would")));
    m.insert("AO*", tenses(modal(" won't"), modal(" wouldn't")));
    m
}

fn build_structure_exceptions() -> HashMap<&'static str, Structure> {
    // Every exception ignores the middle's verb form, so `use_middle_verb_form`
    // is false throughout and each row is just (key, format, verb update).
    let rows: Vec<(&'static str, Data, Option<&'static str>)> = vec![
        ("", text("!"), None),
        // These drop the middle entirely: with no `*` in the format, the modal is
        // not printed and the auxiliary carries the sentence.
        (
            "*E",
            by_tense(
                map(vec![
                    (None, text("! aren't")),
                    (Some("1ps"), text("! am not")),
                    (Some("3ps"), text("! isn't")),
                ]),
                map(vec![
                    (None, text("! weren't")),
                    (Some("1ps"), text("! wasn't")),
                    (Some("3ps"), text("! wasn't")),
                ]),
            ),
            Some("present-participle"),
        ),
        (
            "E",
            by_tense(
                map(vec![
                    (None, text("! are")),
                    (Some("1ps"), text("! am")),
                    (Some("3ps"), text("! is")),
                ]),
                map(vec![
                    (None, text("! were")),
                    (Some("1ps"), text("! was")),
                    (Some("3ps"), text("! was")),
                ]),
            ),
            Some("present-participle"),
        ),
        (
            "*F",
            by_tense(
                map(vec![
                    (None, text("! haven't")),
                    (Some("3ps"), text("! hasn't")),
                ]),
                text("! hadn't"),
            ),
            Some("past-participle"),
        ),
        (
            "F",
            by_tense(
                map(vec![(None, text("! have")), (Some("3ps"), text("! has"))]),
                text("! had"),
            ),
            Some("past-participle"),
        ),
        (
            "*EF",
            by_tense(
                map(vec![
                    (None, text("! haven't been")),
                    (Some("3ps"), text("! hasn't been")),
                ]),
                text("! hadn't been"),
            ),
            Some("present-participle"),
        ),
        (
            "EF",
            by_tense(
                map(vec![
                    (None, text("! have been")),
                    (Some("3ps"), text("! has been")),
                ]),
                text("! had been"),
            ),
            Some("present-participle"),
        ),
        ("EU", text("! still"), None),
        ("EUF", text("! never"), None),
        ("UF", text("! just"), None),
        // Infinitives. These are keyed by the whole left bank plus vowels, so they
        // only fire for the two blank subjects.
        ("STWRU", text("to"), Some("root")),
        ("STWR*U", text("not to"), Some("root")),
        ("STKPWHRU", text("to"), Some("root")),
        ("STKPWHR*U", text("not to"), Some("root")),
    ];

    rows.into_iter()
        .map(|(key, format, verb_update)| {
            (
                key,
                Structure {
                    format,
                    use_middle_verb_form: false,
                    verb_update,
                },
            )
        })
        .collect()
}

fn build_structures() -> HashMap<&'static str, Structure> {
    // "always" replaces the subject slot when there is no subject to print.
    let always = || {
        by_tense(
            map(vec![
                (None, text("* !")),
                (Some("b3ps-root"), text("* always")),
                (Some("b3pp-root"), text("* always")),
            ]),
            map(vec![
                (None, text("* !")),
                (Some("b3ps-root"), text("* always")),
                (Some("b3pp-root"), text("* always")),
            ]),
        )
    };
    let to_be = || {
        by_tense(
            affix(&TO_BE_PRESENT, "!*", ""),
            affix(&TO_BE_PAST, "!*", ""),
        )
    };
    let to_have_been = || {
        by_tense(
            affix(&TO_HAVE_PRESENT, "!*", " been"),
            affix(&TO_HAVE_PAST, "!*", " been"),
        )
    };
    let to_have = || {
        by_tense(
            affix(&TO_HAVE_PRESENT, "!*", ""),
            affix(&TO_HAVE_PAST, "!*", ""),
        )
    };

    // The star is only meaningful in some of these shapes, so several pairs
    // deliberately map to the same structure.
    let rows: Vec<(&'static str, Data, Option<&'static str>)> = vec![
        ("", text("!*"), None),
        ("*", text("!*"), None),
        ("*E", to_be(), Some("present-participle")),
        ("E", to_be(), Some("present-participle")),
        ("*EF", to_have_been(), Some("present-participle")),
        ("EF", to_have_been(), Some("present-participle")),
        ("*F", to_have(), Some("past-participle")),
        ("F", to_have(), Some("past-participle")),
        ("*EU", text("! still*"), None),
        ("EU", text("!* still"), None),
        ("*EUF", text("!* even"), None),
        ("EUF", text("!* never"), None),
        ("*U", always(), None),
        ("U", always(), None),
        ("*UF", text("! just*"), None),
        ("UF", text("!* just"), None),
    ];

    rows.into_iter()
        .map(|(key, format, verb_update)| {
            (
                key,
                Structure {
                    format,
                    use_middle_verb_form: true,
                    verb_update,
                },
            )
        })
        .collect()
}

fn build_enders() -> HashMap<&'static str, Ender> {
    let mut m = HashMap::new();
    let mut add = |key: &'static str, tense: &'static str, verb: Data| {
        m.insert(key, Ender { tense, verb });
    };

    add("", "present", text(""));
    add("D", "past", text(""));

    add(
        "RB",
        "present",
        present_verb(" ask", " asks", " asking", " asked"),
    );
    add(
        "RBD",
        "past",
        past_verb(" asked", " ask", " asking", " asked"),
    );

    add("B", "present", affix(&TO_BE_PRESENT, "", ""));
    add("BT", "present", affix(&TO_BE_PRESENT, "", " a"));
    add("BD", "past", affix(&TO_BE_PAST, "", ""));
    add("BTD", "past", affix(&TO_BE_PAST, "", " a"));

    add(
        "RPBG",
        "present",
        present_verb(" become", " becomes", " becoming", " become"),
    );
    add(
        "RPBGT",
        "present",
        present_verb(" become a", " becomes a", " becoming a", " become a"),
    );
    add(
        "RPBGD",
        "past",
        past_verb(" became", " become", " becoming", " become"),
    );
    add(
        "RPBGTD",
        "past",
        past_verb(" became a", " become a", " becoming a", " become a"),
    );

    add(
        "BL",
        "present",
        present_verb(" believe", " believes", " believing", " believed"),
    );
    add(
        "BLT",
        "present",
        present_verb(
            " believe that",
            " believes that",
            " believing that",
            " believed that",
        ),
    );
    add(
        "BLD",
        "past",
        past_verb(" believed", " believe", " believing", " believed"),
    );
    add(
        "BLTD",
        "past",
        past_verb(
            " believed that",
            " believe that",
            " believing that",
            " believed that",
        ),
    );

    add(
        "RBLG",
        "present",
        present_verb(" call", " calls", " calling", " called"),
    );
    add(
        "RBLGD",
        "past",
        past_verb(" called", " call", " calling", " called"),
    );

    // Auxiliaries are bare strings: they take no inflection and so do not
    // combine with the middle or the structures.
    add("BGS", "present", text(" can"));
    add("BGSZ", "past", text(" could"));

    add(
        "RZ",
        "present",
        present_verb(" care", " cares", " caring", " cared"),
    );
    add(
        "RDZ",
        "past",
        past_verb(" cared", " care", " caring", " cared"),
    );

    add(
        "PBGZ",
        "present",
        present_verb(" change", " changes", " changing", " changed"),
    );
    add(
        "PBGDZ",
        "past",
        past_verb(" changed", " change", " changing", " changed"),
    );

    add(
        "BG",
        "present",
        present_verb(" come", " comes", " coming", " come"),
    );
    add(
        "BGT",
        "present",
        present_verb(" come to", " comes to", " coming to", " come to"),
    );
    add(
        "BGD",
        "past",
        past_verb(" came", " come", " coming", " come"),
    );
    add(
        "BGTD",
        "past",
        past_verb(" came to", " come to", " coming to", " come to"),
    );

    add(
        "RBGZ",
        "present",
        present_verb(" consider", " considers", " considering", " considered"),
    );
    add(
        "RBGDZ",
        "past",
        past_verb(" considered", " consider", " considering", " considered"),
    );

    add(
        "RP",
        "present",
        present_verb(" do", " does", " doing", " done"),
    );
    add(
        "RPT",
        "present",
        present_verb(" do it", " does it", " doing it", " done it"),
    );
    add("RPD", "past", past_verb(" did", " do", " doing", " done"));
    add(
        "RPTD",
        "past",
        past_verb(" did it", " do it", " doing it", " done it"),
    );

    add(
        "PGS",
        "present",
        present_verb(" expect", " expects", " expecting", " expected"),
    );
    add(
        "PGTS",
        "present",
        present_verb(
            " expect that",
            " expects that",
            " expecting that",
            " expected that",
        ),
    );
    add(
        "PGSZ",
        "past",
        past_verb(" expected", " expect", " expecting", " expected"),
    );
    add(
        "PGTSDZ",
        "past",
        past_verb(
            " expected that",
            " expect that",
            " expecting that",
            " expected that",
        ),
    );

    add(
        "LT",
        "present",
        present_verb(" feel", " feels", " feeling", " felt"),
    );
    add(
        "LTS",
        "present",
        present_verb(" feel like", " feels like", " feeling like", " felt like"),
    );
    add(
        "LTD",
        "past",
        past_verb(" felt", " feel", " feeling", " felt"),
    );
    add(
        "LTSDZ",
        "past",
        past_verb(" felt like", " feel like", " feeling like", " felt like"),
    );

    add(
        "PBLG",
        "present",
        present_verb(" find", " finds", " finding", " found"),
    );
    add(
        "PBLGT",
        "present",
        present_verb(" find that", " finds that", " finding that", " found that"),
    );
    add(
        "PBLGD",
        "past",
        past_verb(" found", " find", " finding", " found"),
    );
    add(
        "PBLGTD",
        "past",
        past_verb(" found that", " find that", " finding that", " found that"),
    );

    add(
        "RG",
        "present",
        present_verb(" forget", " forgets", " forgetting", " forgotten"),
    );
    add(
        "RGT",
        "present",
        present_verb(
            " forget to",
            " forgets to",
            " forgetting to",
            " forgotten to",
        ),
    );
    add(
        "RGD",
        "past",
        past_verb(" forgot", " forget", " forgetting", " forgotten"),
    );
    add(
        "RGTD",
        "past",
        past_verb(
            " forgot to",
            " forget to",
            " forgetting to",
            " forgotten to",
        ),
    );

    add(
        "GS",
        "present",
        present_verb(" get", " gets", " getting", " got"),
    );
    add(
        "GTS",
        "present",
        present_verb(" get to", " gets to", " getting to", " got to"),
    );
    add("GSZ", "past", past_verb(" got", " get", " getting", " got"));
    add(
        "GTSDZ",
        "past",
        past_verb(" got to", " get to", " getting to", " got to"),
    );

    add(
        "GZ",
        "present",
        present_verb(" give", " gives", " giving", " given"),
    );
    add(
        "GDZ",
        "past",
        past_verb(" gave", " give", " giving", " given"),
    );

    add(
        "G",
        "present",
        present_verb(" go", " goes", " going", " gone"),
    );
    add(
        "GT",
        "present",
        present_verb(" go to", " goes to", " going to", " gone to"),
    );
    add("GD", "past", past_verb(" went", " go", " going", " gone"));
    add(
        "GTD",
        "past",
        past_verb(" went to", " go to", " going to", " gone to"),
    );

    add(
        "T",
        "present",
        present_verb(" have", " has", " having", " had"),
    );
    add(
        "TS",
        "present",
        present_verb(" have to", " has to", " having to", " had to"),
    );
    add("TD", "past", past_verb(" had", " have", " having", " had"));
    add(
        "TSDZ",
        "past",
        past_verb(" had to", " have to", " having to", " had to"),
    );

    add(
        "PZ",
        "present",
        present_verb(" happen", " happens", " happening", " happened"),
    );
    add(
        "PDZ",
        "past",
        past_verb(" happened", " happen", " happening", " happened"),
    );

    add(
        "PG",
        "present",
        present_verb(" hear", " hears", " hearing", " heard"),
    );
    add(
        "PGT",
        "present",
        present_verb(" hear that", " hears that", " hearing that", " heard that"),
    );
    add(
        "PGD",
        "past",
        past_verb(" heard", " hear", " hearing", " heard"),
    );
    add(
        "PGTD",
        "past",
        past_verb(" heard that", " hear that", " hearing that", " heard that"),
    );

    add(
        "RPS",
        "present",
        present_verb(" hope", " hopes", " hoping", " hoped"),
    );
    add(
        "RPTS",
        "present",
        present_verb(" hope to", " hopes to", " hoping to", " hoped to"),
    );
    add(
        "RPSZ",
        "past",
        past_verb(" hoped", " hope", " hoping", " hoped"),
    );
    add(
        "RPTSDZ",
        "past",
        past_verb(" hoped to", " hope to", " hoping to", " hoped to"),
    );

    add(
        "PLG",
        "present",
        present_verb(" imagine", " imagines", " imagining", " imagined"),
    );
    add(
        "PLGT",
        "present",
        present_verb(
            " imagine that",
            " imagines that",
            " imagining that",
            " imagined that",
        ),
    );
    add(
        "PLGD",
        "past",
        past_verb(" imagined", " imagine", " imagining", " imagined"),
    );
    add(
        "PLGTD",
        "past",
        past_verb(
            " imagined that",
            " imagine that",
            " imagining that",
            " imagined that",
        ),
    );

    add("PBLGSZ", "present", text(" just"));
    add("PBLGTSDZ", "past", text(" just"));

    add(
        "PBGS",
        "present",
        present_verb(" keep", " keeps", " keeping", " kept"),
    );
    add(
        "PBGSZ",
        "past",
        past_verb(" kept", " keep", " keeping", " kept"),
    );

    add(
        "PB",
        "present",
        present_verb(" know", " knows", " knowing", " known"),
    );
    add(
        "PBT",
        "present",
        present_verb(" know that", " knows that", " knowing that", " known that"),
    );
    add(
        "PBD",
        "past",
        past_verb(" knew", " know", " knowing", " known"),
    );
    add(
        "PBTD",
        "past",
        past_verb(" knew that", " know that", " knowing that", " known that"),
    );

    add(
        "RPBS",
        "present",
        present_verb(" learn", " learns", " learning", " learned"),
    );
    add(
        "RPBTS",
        "present",
        present_verb(" learn to", " learns to", " learning to", " learned to"),
    );
    add(
        "RPBSZ",
        "past",
        past_verb(" learned", " learn", " learning", " learned"),
    );
    add(
        "RPBTSDZ",
        "past",
        past_verb(" learned to", " learn to", " learning to", " learned to"),
    );

    add(
        "LGZ",
        "present",
        present_verb(" leave", " leaves", " leaving", " left"),
    );
    add(
        "LGDZ",
        "past",
        past_verb(" left", " leave", " leaving", " left"),
    );

    add(
        "LS",
        "present",
        present_verb(" let", " lets", " letting", " let"),
    );
    // "let" has no root entry, unlike every other past ender, so "I could let"
    // falls through to the default " let" rather than a root form.
    add(
        "LSZ",
        "past",
        map(vec![
            (None, text(" let")),
            (Some("present-participle"), text(" letting")),
            (Some("past-participle"), text(" let")),
        ]),
    );

    add(
        "BLG",
        "present",
        present_verb(" like", " likes", " liking", " liked"),
    );
    add(
        "BLGT",
        "present",
        present_verb(" like to", " likes to", " liking to", " liked to"),
    );
    add(
        "BLGD",
        "past",
        past_verb(" liked", " like", " liking", " liked"),
    );
    add(
        "BLGTD",
        "past",
        past_verb(" liked to", " like to", " liking to", " liked to"),
    );

    add(
        "LZ",
        "present",
        present_verb(" live", " lives", " living", " lived"),
    );
    add(
        "LDZ",
        "past",
        past_verb(" lived", " live", " living", " lived"),
    );

    add(
        "L",
        "present",
        present_verb(" look", " looks", " looking", " looked"),
    );
    add(
        "LD",
        "past",
        past_verb(" looked", " look", " looking", " looked"),
    );

    add(
        "LG",
        "present",
        present_verb(" love", " loves", " loving", " loved"),
    );
    add(
        "LGT",
        "present",
        present_verb(" love to", " loves to", " loving to", " loved to"),
    );
    add(
        "LGD",
        "past",
        past_verb(" loved", " love", " loving", " loved"),
    );
    add(
        "LGTD",
        "past",
        past_verb(" loved to", " love to", " loving to", " loved to"),
    );

    add(
        "RPBL",
        "present",
        present_verb(" make", " makes", " making", " made"),
    );
    add(
        "RPBLT",
        "present",
        present_verb(" make a", " makes a", " making a", " made a"),
    );
    add(
        "RPBLD",
        "past",
        past_verb(" made", " make", " making", " made"),
    );
    add(
        "RPBLTD",
        "past",
        past_verb(" made a", " make a", " making a", " made a"),
    );

    add("PL", "present", text(" may"));
    add("PLT", "present", text(" may be"));
    add("PLD", "past", text(" might"));
    add("PLTD", "past", text(" might be"));

    add(
        "PBL",
        "present",
        present_verb(" mean", " means", " meaning", " meant"),
    );
    add(
        "PBLT",
        "present",
        present_verb(" mean to", " means to", " meaning to", " meant to"),
    );
    add(
        "PBLD",
        "past",
        past_verb(" meant", " mean", " meaning", " meant"),
    );
    add(
        "PBLTD",
        "past",
        past_verb(" meant to", " mean to", " meaning to", " meant to"),
    );

    add(
        "PBLS",
        "present",
        present_verb(" mind", " minds", " minding", " minded"),
    );
    add(
        "PBLSZ",
        "past",
        past_verb(" minded", " mind", " minding", " minded"),
    );

    add(
        "PLZ",
        "present",
        present_verb(" move", " moves", " moving", " moved"),
    );
    add(
        "PLDZ",
        "past",
        past_verb(" moved", " move", " moving", " moved"),
    );

    add("PBLGS", "present", text(" must"));
    add("PBLGTS", "present", text(" must be"));

    add(
        "RPG",
        "present",
        present_verb(" need", " needs", " needing", " needed"),
    );
    add(
        "RPGT",
        "present",
        present_verb(" need to", " needs to", " needing to", " needed to"),
    );
    add(
        "RPGD",
        "past",
        past_verb(" needed", " need", " needing", " needed"),
    );
    add(
        "RPGTD",
        "past",
        past_verb(" needed to", " need to", " needing to", " needed to"),
    );

    add(
        "PS",
        "present",
        present_verb(" put", " puts", " putting", " put"),
    );
    add(
        "PTS",
        "present",
        present_verb(" put it", " puts it", " putting it", " put it"),
    );
    add("PSZ", "past", past_verb(" put", " put", " putting", " put"));
    add(
        "PTSDZ",
        "past",
        past_verb(" put it", " put it", " putting it", " put it"),
    );

    add(
        "RS",
        "present",
        present_verb(" read", " reads", " reading", " read"),
    );
    add(
        "RSZ",
        "past",
        past_verb(" read", " read", " reading", " read"),
    );

    add("RLG", "present", text(" really"));
    add("RLGD", "past", text(" really"));

    add(
        "RL",
        "present",
        present_verb(" recall", " recalls", " recalling", " recalled"),
    );
    add(
        "RLD",
        "past",
        past_verb(" recalled", " recall", " recalling", " recalled"),
    );

    add(
        "RLS",
        "present",
        present_verb(" realize", " realizes", " realizing", " realized"),
    );
    add(
        "RLTS",
        "present",
        present_verb(
            " realize that",
            " realizes that",
            " realizing that",
            " realized that",
        ),
    );
    add(
        "RLSZ",
        "past",
        past_verb(" realized", " realize", " realizing", " realized"),
    );
    add(
        "RLTSDZ",
        "past",
        past_verb(
            " realized that",
            " realize that",
            " realizing that",
            " realized that",
        ),
    );

    add(
        "RPL",
        "present",
        present_verb(" remember", " remembers", " remembering", " remembered"),
    );
    add(
        "RPLT",
        "present",
        present_verb(
            " remember that",
            " remembers that",
            " remembering that",
            " remembered that",
        ),
    );
    add(
        "RPLD",
        "past",
        past_verb(" remembered", " remember", " remembering", " remembered"),
    );
    add(
        "RPLTD",
        "past",
        past_verb(
            " remembered that",
            " remember that",
            " remembering that",
            " remembered that",
        ),
    );

    add(
        "RPLS",
        "present",
        present_verb(" remain", " remains", " remaining", " remained"),
    );
    add(
        "RPLSZ",
        "past",
        past_verb(" remained", " remain", " remaining", " remained"),
    );

    add(
        "R",
        "present",
        present_verb(" run", " runs", " running", " run"),
    );
    add("RD", "past", past_verb(" ran", " run", " running", " run"));

    add(
        "BS",
        "present",
        present_verb(" say", " says", " saying", " said"),
    );
    add(
        "BTS",
        "present",
        present_verb(" say that", " says that", " saying that", " said that"),
    );
    add(
        "BSZ",
        "past",
        past_verb(" said", " say", " saying", " said"),
    );
    add(
        "BTSDZ",
        "past",
        past_verb(" said that", " say that", " saying that", " said that"),
    );

    add(
        "S",
        "present",
        present_verb(" see", " sees", " seeing", " seen"),
    );
    add("SZ", "past", past_verb(" saw", " see", " seeing", " seen"));

    add(
        "BLS",
        "present",
        present_verb(" set", " sets", " setting", " set"),
    );
    add(
        "BLSZ",
        "past",
        past_verb(" set", " set", " setting", " set"),
    );

    add(
        "PLS",
        "present",
        present_verb(" seem", " seems", " seeming", " seemed"),
    );
    add(
        "PLTS",
        "present",
        present_verb(" seem to", " seems to", " seeming to", " seemed to"),
    );
    add(
        "PLSZ",
        "past",
        past_verb(" seemed", " seem", " seeming", " seemed"),
    );
    add(
        "PLTSDZ",
        "past",
        past_verb(" seemed to", " seem to", " seeming to", " seemed to"),
    );

    add("RBL", "present", text(" shall"));
    add("RBLD", "past", text(" should"));

    add(
        "RBZ",
        "present",
        present_verb(" show", " shows", " showing", " shown"),
    );
    add(
        "RBDZ",
        "past",
        past_verb(" showed", " show", " showing", " shown"),
    );

    add(
        "RBT",
        "present",
        present_verb(" take", " takes", " taking", " taken"),
    );
    add(
        "RBTD",
        "past",
        past_verb(" took", " take", " taking", " taken"),
    );

    add(
        "RLT",
        "present",
        present_verb(" tell", " tells", " telling", " told"),
    );
    add(
        "RLTD",
        "past",
        past_verb(" told", " tell", " telling", " told"),
    );

    add(
        "PBG",
        "present",
        present_verb(" think", " thinks", " thinking", " thought"),
    );
    add(
        "PBGT",
        "present",
        present_verb(
            " think that",
            " thinks that",
            " thinking that",
            " thought that",
        ),
    );
    add(
        "PBGD",
        "past",
        past_verb(" thought", " think", " thinking", " thought"),
    );
    add(
        "PBGTD",
        "past",
        past_verb(
            " thought that",
            " think that",
            " thinking that",
            " thought that",
        ),
    );

    add(
        "RT",
        "present",
        present_verb(" try", " tries", " trying", " tried"),
    );
    add(
        "RTS",
        "present",
        present_verb(" try to", " tries to", " trying to", " tried to"),
    );
    add(
        "RTD",
        "past",
        past_verb(" tried", " try", " trying", " tried"),
    );
    add(
        "RTSDZ",
        "past",
        past_verb(" tried to", " try to", " trying to", " tried to"),
    );

    add(
        "RPB",
        "present",
        present_verb(
            " understand",
            " understands",
            " understanding",
            " understood",
        ),
    );
    add(
        "RPBT",
        "present",
        present_verb(
            " understand the",
            " understands the",
            " understanding the",
            " understood the",
        ),
    );
    add(
        "RPBD",
        "past",
        past_verb(
            " understood",
            " understand",
            " understanding",
            " understood",
        ),
    );
    add(
        "RPBTD",
        "past",
        past_verb(
            " understood the",
            " understand the",
            " understanding the",
            " understood the",
        ),
    );

    add(
        "Z",
        "present",
        present_verb(" use", " uses", " using", " used"),
    );
    add("DZ", "past", past_verb(" used", " use", " using", " used"));
    add("TZ", "present", text(" used to"));
    add("TDZ", "past", text(" used to"));

    add(
        "P",
        "present",
        present_verb(" want", " wants", " wanting", " wanted"),
    );
    add(
        "PT",
        "present",
        present_verb(" want to", " wants to", " wanting to", " wanted to"),
    );
    add(
        "PD",
        "past",
        past_verb(" wanted", " want", " wanting", " wanted"),
    );
    add(
        "PTD",
        "past",
        past_verb(" wanted to", " want to", " wanting to", " wanted to"),
    );

    add("RBGS", "present", text(" will"));
    add("RBGSZ", "past", text(" would"));

    add(
        "RBS",
        "present",
        present_verb(" wish", " wishes", " wishing", " wished"),
    );
    add(
        "RBTS",
        "present",
        present_verb(" wish to", " wishes to", " wishing to", " wished to"),
    );
    add(
        "RBSZ",
        "past",
        past_verb(" wished", " wish", " wishing", " wished"),
    );
    add(
        "RBTSDZ",
        "past",
        past_verb(" wished to", " wish to", " wishing to", " wished to"),
    );

    add(
        "RBG",
        "present",
        present_verb(" work", " works", " working", " worked"),
    );
    add(
        "RBGT",
        "present",
        present_verb(" work on", " works on", " working on", " worked on"),
    );
    add(
        "RBGD",
        "past",
        past_verb(" worked", " work", " working", " worked"),
    );
    add(
        "RBGTD",
        "past",
        past_verb(" worked on", " work on", " working on", " worked on"),
    );
    m
}

struct Parts<'a> {
    starter: &'a str,
    v1: &'a str,
    star: &'a str,
    v2: &'a str,
    f: &'a str,
    ender: &'a str,
}

/// Consume, in order, whichever of `letters` come next. Each letter may appear
/// at most once, which is what makes the split unambiguous without backtracking.
fn take_ordered<'a>(stroke: &'a str, pos: &mut usize, letters: &str) -> &'a str {
    let bytes = stroke.as_bytes();
    let start = *pos;
    for want in letters.bytes() {
        if *pos < bytes.len() && bytes[*pos] == want {
            *pos += 1;
        }
    }
    &stroke[start..*pos]
}

/// Split a stroke into the six groups of the Python `PARTS_MATCHER`. Every
/// group is optional and the Python uses `re.match`, so this never fails and
/// any trailing characters are ignored, exactly as there.
fn split_parts(stroke: &str) -> Parts<'_> {
    let mut pos = 0;
    let starter = take_ordered(stroke, &mut pos, "STKPWHR");
    let v1 = take_ordered(stroke, &mut pos, "AO");
    // The hyphen is optional here and nowhere else: it separates the banks when
    // no center key already does.
    if stroke.as_bytes().get(pos) == Some(&b'-') {
        pos += 1;
    }
    let star = take_ordered(stroke, &mut pos, "*");
    let v2 = take_ordered(stroke, &mut pos, "EU");
    let f = take_ordered(stroke, &mut pos, "F");
    let ender = take_ordered(stroke, &mut pos, "RPBLGTSDZ");
    Parts {
        starter,
        v1,
        star,
        v2,
        f,
        ender,
    }
}

fn determine_parts<'a>(
    tables: &'a Tables,
    stroke: &str,
) -> Option<(&'a Starter, &'a Middle, &'a Structure, &'a Ender)> {
    if NON_PHRASE_STROKES.contains(&stroke) {
        return None;
    }

    let parts = split_parts(stroke);
    let ender = tables.enders.get(parts.ender)?;

    // Short form: a conjunction on the left bank and a pronoun in the vowels,
    // as in SKPE for "and he". The pronoun takes the subject slot and the
    // conjunction takes the middle slot.
    let conjunction_key = format!("{}{}", parts.starter, parts.v1);
    let pronoun_key = format!("{}{}", parts.star, parts.v2);
    if let (Some(conjunction), Some(pronoun)) = (
        tables.simple_starters.get(conjunction_key.as_str()),
        tables.simple_pronouns.get(pronoun_key.as_str()),
    ) {
        let structure = tables
            .simple_structures
            .get(parts.f)
            .expect("SIMPLE_STRUCTURES covers both values of the F group");
        return Some((pronoun, conjunction, structure, ender));
    }

    let starter = tables.starters.get(parts.starter)?;
    if let Some(valid) = starter.valid_enders
        && !valid.contains(&parts.ender)
    {
        return None;
    }

    let middle = tables
        .middles
        .get(format!("{}{}", parts.v1, parts.star).as_str())
        .expect("MIDDLES covers every A/O and star combination");

    // Exceptions are tried with the left bank included first, so a shape can be
    // overridden for one specific subject, then without it.
    let full_key = format!(
        "{}{}{}{}{}",
        parts.starter, parts.v1, parts.star, parts.v2, parts.f
    );
    let shape_key = &full_key[parts.starter.len()..];
    let structure = match tables.structure_exceptions.get(full_key.as_str()) {
        Some(structure) => structure,
        None => match tables.structure_exceptions.get(shape_key) {
            Some(structure) => structure,
            None => tables
                .structures
                .get(format!("{}{}{}", parts.star, parts.v2, parts.f).as_str())
                .expect("STRUCTURES covers every star, E/U and F combination"),
        },
    };

    Some((starter, middle, structure, ender))
}

/// Look up one outline, for example `SWR-RB`. Returns `None` if this dictionary
/// does not answer it.
pub fn lookup(outline: &str) -> Option<String> {
    // LONGEST_KEY is 1, so a multi-stroke outline is never ours. The Python
    // would match the first stroke and silently ignore the rest, because its
    // regex only has to match a prefix.
    if outline.contains('/') {
        return None;
    }

    let tables = &*TABLES;
    let (starter, middle, structure, ender) = determine_parts(tables, outline)?;

    let tense = ender.tense;
    let mut verb_form = starter.verb_form;

    let (middle_word, middle_verb_form) = match middle.get(&[tense, verb_form]) {
        Some(Middle::Word(word, form)) => (word, *form),
        // Unreachable: every MIDDLES branch bottoms out in a word. Refusing the
        // stroke keeps this total rather than panicking on a table edit.
        _ => return None,
    };

    let original_verb_form = verb_form;
    if structure.use_middle_verb_form
        && let Some(form) = middle_verb_form
        && !form.is_empty()
    {
        verb_form = form;
    }

    let combined = format!("{original_verb_form}-{verb_form}");
    let format = structure
        .format
        .get(&[tense, &combined])
        .and_then(Data::as_text)?;
    // `*` is the middle and only ever appears once; `!` is the subject and can
    // appear more than once, so it is replaced everywhere.
    let phrase = format
        .replacen('*', middle_word, 1)
        .replace('!', starter.word);

    if let Some(update) = structure.verb_update {
        verb_form = update;
    }
    let ending = ender.verb.get(&[verb_form]).and_then(Data::as_text)?;

    Some(phrase + ending)
}

/// The longest outline this dictionary can answer, in strokes.
pub fn longest_key() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every outline the Python answers, plus every outline it refuses, as
    /// enumerated by `dump_phrasing.py`. This is the whole point of the port: a
    /// table this large cannot be checked by hand-picked cases.
    #[test]
    fn matches_the_python_on_every_enumerated_stroke() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/phrasing_fixture.json");
        let raw = std::fs::read_to_string(path).expect("phrasing fixture is missing");
        let expected: BTreeMap<String, Option<String>> =
            serde_json::from_str(&raw).expect("phrasing fixture is not valid JSON");

        assert!(
            expected.len() > 200_000,
            "fixture looks truncated: {} entries",
            expected.len()
        );

        let mut answered = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for (outline, want) in &expected {
            if want.is_some() {
                answered += 1;
            }
            let got = lookup(outline);
            if got.as_deref() != want.as_deref() {
                failures.push(format!("{outline}: python {want:?}, rust {got:?}"));
            }
        }

        assert!(
            answered > 170_000,
            "fixture has too few answered strokes: {answered}"
        );
        assert!(
            failures.is_empty(),
            "{} of {} outlines differ, first few:\n{}",
            failures.len(),
            expected.len(),
            failures
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn refuses_multi_stroke_outlines() {
        assert_eq!(lookup("SWR"), Some("I".to_owned()));
        assert_eq!(lookup("SWR/SWR"), None);
        assert_eq!(longest_key(), 1);
    }
}
