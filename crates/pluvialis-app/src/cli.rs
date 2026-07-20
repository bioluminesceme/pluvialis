//! Temporary command line entry points, used to exercise the core before
//! there is a GUI to do it through.
//!
//! `lookup` is the M1 verification: it loads the real dictionaries and answers
//! from them, reporting load time and lookup latency. Proper subcommands
//! (`convert`, `check`) arrive in M6 and will want a real argument parser.

use std::path::PathBuf;
use std::time::Instant;

use pluvialis_core::{Dictionary, DictionaryStack, Stroke, Translator};

/// Where the user's dictionaries live. These are shared in place with her
/// working Plover install and are never copied or modified.
const DICTIONARY_DIR: &str = r"C:\Users\Corien\AppData\Local\plover\plover";

/// Priority order, highest first, mirroring her plover.cfg. `jeff-phrasing.py`
/// is a Python programmatic dictionary and is not loadable here; it is
/// replaced by a native implementation in M6.
const DICTIONARIES: [&str; 2] = ["cb_dictionary_full.json", "corien-dutch.json"];

pub fn run(args: &[String]) -> std::process::ExitCode {
    match args.first().map(String::as_str) {
        Some("lookup") => lookup(&args[1..]),
        Some(other) => {
            eprintln!("unknown command {other:?}");
            eprintln!("usage: pluvialis lookup <OUTLINE> [OUTLINE...]");
            std::process::ExitCode::from(2)
        }
        None => unreachable!("run is only called with at least one argument"),
    }
}

fn lookup(outlines: &[String]) -> std::process::ExitCode {
    if outlines.is_empty() {
        eprintln!("usage: pluvialis lookup <OUTLINE> [OUTLINE...]");
        eprintln!("example: pluvialis lookup KAT WEL/KO*PL");
        return std::process::ExitCode::from(2);
    }

    let started = Instant::now();
    let mut stack = DictionaryStack::new();
    for name in DICTIONARIES {
        let path = PathBuf::from(DICTIONARY_DIR).join(name);
        match Dictionary::load(&path) {
            Ok(dictionary) => {
                let bad = dictionary.bad_keys().len();
                println!(
                    "loaded {:<28} {:>7} entries, longest key {:>2}{}",
                    name,
                    dictionary.len(),
                    dictionary.longest_key(),
                    if bad > 0 {
                        format!(", {bad} unparseable keys")
                    } else {
                        String::new()
                    }
                );
                // Unparseable keys are a data problem worth seeing, not
                // something to hide behind a count.
                for (key, error) in dictionary.bad_keys().iter().take(5) {
                    println!("    skipped {key:?}: {error}");
                }
                stack.push(dictionary);
            }
            Err(e) => {
                eprintln!("could not load {}: {e}", path.display());
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    println!(
        "\n{} entries across {} dictionaries, longest key {}, loaded in {:.0?}",
        stack.entry_count(),
        stack.dictionaries().len(),
        stack.longest_key(),
        started.elapsed()
    );

    let mut failed = false;
    for outline in outlines {
        println!();
        let strokes = match Stroke::parse_outline(outline) {
            Ok(strokes) => strokes,
            Err(e) => {
                println!("{outline}: not valid steno: {e}");
                failed = true;
                continue;
            }
        };

        let canonical = Stroke::render_outline(&strokes);
        let started = Instant::now();
        let hit = stack.lookup(&strokes);
        let elapsed = started.elapsed();

        match hit {
            Some(text) => println!("{canonical} -> {text:?}   ({elapsed:?})"),
            None => println!("{canonical} -> no entry   ({elapsed:?})"),
        }

        // Also show what the translator makes of it stroke by stroke, which is
        // where retroactive correction becomes visible.
        let mut translator = Translator::new();
        for stroke in &strokes {
            translator.translate(&stack, *stroke);
        }
        println!("    translated: {:?}", translator.text());
    }

    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
