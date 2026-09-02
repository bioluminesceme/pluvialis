//! An independent check on the RTF/CRE reader.
//!
//! These inputs were written separately from the module's own tests, on the
//! principle that a suite written alongside the code shares its blind spots.
//! They are deliberately close to what a real dictionary contains rather than
//! to what the parser finds convenient.

use pluvialis_core::rtfcre;

/// The shape every RTF/CRE dictionary starts with.
fn wrap(body: &str) -> String {
    format!("{{\\rtf1\\ansi\\cxrev100\\cxdict{body}}}")
}

#[test]
fn a_plain_entry_survives_the_round_trip() {
    let entries = rtfcre::parse(&wrap(r"{\*\cxs KAT}cat")).expect("parsing");
    assert_eq!(entries, vec![("KAT".to_owned(), "cat".to_owned())]);
}

#[test]
fn several_entries_keep_their_order_and_multi_stroke_keys() {
    let entries = rtfcre::parse(&wrap(
        r"{\*\cxs WEL}well{\*\cxs WEL/KO*PL}welcome{\*\cxs TKOG}dog",
    ))
    .expect("parsing");
    assert_eq!(
        entries,
        vec![
            ("WEL".to_owned(), "well".to_owned()),
            ("WEL/KO*PL".to_owned(), "welcome".to_owned()),
            ("TKOG".to_owned(), "dog".to_owned()),
        ]
    );
}

/// Attachment in all four positions, which is the bulk of what a real
/// dictionary uses beyond plain text.
#[test]
fn delete_space_becomes_the_matching_attachment_meta() {
    let cases = [
        (r"{\*\cxs -G}\cxds ing", "{^ing}"),
        (r"{\*\cxs KAUPB}counter\cxds ", "{counter^}"),
        // A bare attach. This is the form Plover's writer emits for `{^}`
        // (`rtfcre_dict.py`, the `{\^\^?}` rule): one `\cxds` in a group, not
        // two in a row. Two in a row is `{^}{^}` in Plover too, because the
        // parser looks at the token after `\cxds` and rewinds when it is not
        // text. Checked against `rtfcre_parse.py` after this test first
        // asserted otherwise and the parser turned out to be right.
        (r"{\*\cxs *EU}{\cxds}", "{^}"),
    ];
    for (body, want) in cases {
        let entries = rtfcre::parse(&wrap(body)).expect("parsing");
        assert_eq!(entries.len(), 1, "{body}");
        assert_eq!(entries[0].1, want, "{body}");
    }
}

/// The case the port called out as the reason escapes merge into the
/// surrounding run: if `caf\'e9` split into two text tokens, the trailing
/// `\cxds` would attach to the wrong one and this would come back as a prefix.
#[test]
fn an_escape_in_the_middle_does_not_break_infix_detection() {
    let entries = rtfcre::parse(&wrap(r"{\*\cxs KAF}\cxds caf\'e9\cxds ")).expect("parsing");
    assert_eq!(entries[0].1, "{^caf\u{e9}^}");
}

#[test]
fn cp1252_high_bytes_are_not_confused_with_latin_1() {
    // 0x93 is a left double quote in cp1252 and a control character in
    // Latin-1. Getting this wrong is silent and only shows up as mojibake.
    let entries = rtfcre::parse(&wrap(r"{\*\cxs KWOET}\'93")).expect("parsing");
    assert_eq!(entries[0].1, "\u{201c}");
}

#[test]
fn a_unicode_escape_resolves_and_skips_its_fallback() {
    let entries = rtfcre::parse(&wrap(r"{\*\cxs KWRO}\u233 e")).expect("parsing");
    assert_eq!(entries[0].1, "\u{e9}");
}

/// A literal brace must not reach the formatter looking like a meta command.
#[test]
fn a_literal_brace_is_escaped_for_the_formatter() {
    let entries = rtfcre::parse(&wrap(r"{\*\cxs PWRAS}\{")).expect("parsing");
    assert_eq!(entries[0].1, r"\{");

    // And the escaped form survives a real parse by the formatter as one
    // literal character rather than opening a meta command.
    let formatted = pluvialis_core::format::format(&[pluvialis_core::Translation::for_test(
        vec![pluvialis_core::Stroke::parse("PWRAS").expect("valid steno")],
        Some(entries[0].1.clone()),
    )]);
    assert!(formatted.text.contains('{'), "{:?}", formatted.text);
}

#[test]
fn unrecognised_ignorable_groups_are_skipped_rather_than_emitted() {
    let entries = rtfcre::parse(&wrap(r"{\*\cxs KAT}cat{\*\cxcomment a note}")).expect("parsing");
    assert_eq!(entries, vec![("KAT".to_owned(), "cat".to_owned())]);
}

#[test]
fn a_file_that_is_not_rtf_is_an_error_rather_than_an_empty_dictionary() {
    // The dangerous failure is returning Ok(vec![]), which reads as "imported
    // fine, zero entries" instead of "this is the wrong file".
    for bad in [
        "",
        "just some text",
        "{\\rtf1\\ansi}",        // RTF, but not a dictionary
        "{\\rtf1\\ansi\\cxdict", // truncated
    ] {
        let result = rtfcre::parse(bad);
        if let Ok(entries) = &result {
            assert!(
                entries.is_empty(),
                "{bad:?} parsed as {entries:?}, which is worse than an error"
            );
        }
    }
}

#[test]
fn every_key_it_returns_parses_as_real_steno() {
    let entries = rtfcre::parse(&wrap(
        r"{\*\cxs KAT}cat{\*\cxs WEL/KO*PL}welcome{\*\cxs -G}\cxds ing",
    ))
    .expect("parsing");
    for (outline, _) in &entries {
        pluvialis_core::Stroke::parse_outline(outline)
            .unwrap_or_else(|error| panic!("{outline:?} is not steno: {error}"));
    }
}
