//! Attaching a suffix to a word using English spelling rules.
//!
//! Naive concatenation gives "runing" and "artisticly". The rules in
//! [`orthography_rules`](crate::orthography_rules) fix that, and a word
//! frequency list decides between candidates when more than one rule matches.
//!
//! This mirrors Plover's `plover/orthography.py`. Do not hand roll the English
//! rules: they are a large pile of special cases and getting one wrong shows
//! up as a misspelling mid sentence.

use std::collections::HashMap;
use std::sync::LazyLock;

// fancy-regex rather than the regex crate: one of Plover's rules uses a
// negative look-behind (`(?<![gin]a)r`), which regex does not support. The
// subjects here are single words, so the backtracking engine costs nothing
// that matters.
use fancy_regex::Regex;

use crate::orthography_rules::{ALIASES, RULES};

/// Plover's American English word list: `word frequency` per line, where a
/// lower number means more prominent. Embedded so the program stays a single
/// executable.
static WORD_LIST: &str = include_str!("../assets/american_english_words.txt");

static WORDS: LazyLock<HashMap<&'static str, u32>> = LazyLock::new(|| {
    let mut map = HashMap::with_capacity(340_000);
    for line in WORD_LIST.lines() {
        let Some((word, rank)) = line.rsplit_once(' ') else {
            continue;
        };
        if let Ok(rank) = rank.trim().parse::<u32>() {
            map.insert(word, rank);
        }
    }
    map
});

static COMPILED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|(pattern, replacement)| {
            let regex = Regex::new(pattern).unwrap_or_else(|e| {
                panic!("generated orthography rule {pattern:?} is invalid: {e}")
            });
            (regex, *replacement)
        })
        .collect()
});

fn known_word(word: &str) -> Option<u32> {
    WORDS.get(word).copied()
}

/// Every rule expansion that applies to this word and suffix.
fn candidates_from_rules(word: &str, suffix: &str) -> Vec<String> {
    let subject = format!("{word} ^ {suffix}");
    COMPILED
        .iter()
        .filter_map(|(regex, replacement)| {
            // A regex error here would mean a malformed generated rule, which
            // the LazyLock compile step has already ruled out.
            regex
                .captures(&subject)
                .ok()
                .flatten()
                .map(|caps| {
                    let mut out = String::new();
                    caps.expand(replacement, &mut out);
                    out
                })
                // A rule can match with an empty expansion; ignore those.
                .filter(|s| !s.is_empty())
        })
        .collect()
}

/// Attach `suffix` to `word`, applying English spelling rules.
///
/// Candidates that are real words win over ones that are not, and among real
/// words the most common wins. Only if nothing is recognised do we fall back
/// to an unchecked rule, and finally to plain concatenation.
pub fn add_suffix(word: &str, suffix: &str) -> String {
    // Plover only applies rules to the first space separated piece, so
    // `{^ed} more` keeps the trailing text intact.
    let (suffix, rest) = match suffix.split_once(' ') {
        Some((head, tail)) => (head, Some(tail)),
        None => (suffix, None),
    };

    let expanded = add_suffix_word(word, suffix);
    match rest {
        Some(rest) => format!("{expanded} {rest}"),
        None => expanded,
    }
}

fn add_suffix_word(word: &str, suffix: &str) -> String {
    let simple = format!("{word}{suffix}");
    let mut checked: Vec<String> = Vec::new();

    // Some suffixes are also tried under another spelling, so that
    // "able" can find the "ible" rules.
    if let Some((_, alias)) = ALIASES.iter().find(|(from, _)| *from == suffix) {
        checked.extend(
            candidates_from_rules(word, alias)
                .into_iter()
                .filter(|c| known_word(c).is_some()),
        );
    }

    if known_word(&simple).is_some() {
        checked.push(simple.clone());
    }

    checked.extend(
        candidates_from_rules(word, suffix)
            .into_iter()
            .filter(|c| known_word(c).is_some()),
    );

    if !checked.is_empty() {
        // Sort by prominence. The sort is stable, so equally common
        // candidates keep the order they were added, which is the order of
        // preference above.
        checked.sort_by_key(|c| known_word(c).unwrap_or(u32::MAX));
        return checked.remove(0);
    }

    // Nothing was a recognised word, so take the first rule that applies.
    if let Some(first) = candidates_from_rules(word, suffix).into_iter().next() {
        return first;
    }

    simple
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_the_final_consonant() {
        assert_eq!(add_suffix("run", "ing"), "running");
        assert_eq!(add_suffix("shop", "ed"), "shopped");
    }

    #[test]
    fn drops_silent_e() {
        assert_eq!(add_suffix("write", "ing"), "writing");
        assert_eq!(add_suffix("come", "ing"), "coming");
    }

    #[test]
    fn handles_the_ally_rule() {
        assert_eq!(add_suffix("artistic", "ly"), "artistically");
    }

    #[test]
    fn turns_y_into_i() {
        assert_eq!(add_suffix("happy", "er"), "happier");
        assert_eq!(add_suffix("carry", "ed"), "carried");
    }

    #[test]
    fn plain_joins_are_left_alone() {
        assert_eq!(add_suffix("cat", "s"), "cats");
        assert_eq!(add_suffix("quick", "ly"), "quickly");
    }

    #[test]
    fn unknown_words_still_get_rule_treatment() {
        // Not in the word list, but the doubling rule still applies.
        assert_eq!(add_suffix("zorp", "ing"), "zorping");
    }

    #[test]
    fn trailing_text_after_a_space_is_preserved() {
        assert_eq!(add_suffix("run", "ing fast"), "running fast");
    }

    #[test]
    fn word_list_loaded() {
        assert!(WORDS.len() > 300_000, "word list looks truncated");
        assert!(known_word("running").is_some());
    }
}
