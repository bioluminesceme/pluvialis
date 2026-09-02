//! Temporary command line entry points, used to exercise the core before
//! there is a GUI to do it through.
//!
//! `lookup` is the M1 verification: it loads the real dictionaries and answers
//! from them, reporting load time and lookup latency. Proper subcommands
//! (`convert`, `check`) arrive in M6 and will want a real argument parser.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pluvialis_core::{Dictionary, DictionaryStack, Stroke, Translation, Translator};

/// The dictionaries Pluvialis owns, in priority order, highest first.
///
/// These live in the library rather than her Plover folder; see `library`. The
/// library is created and seeded on first use, so a CLI command run before the
/// GUI has ever started still finds dictionaries.
///
/// `clean --write` writes to these, and that is the point: the file it edits is
/// one nothing else reads.
/// A dictionary's file name, for output that has to fit on a line.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn library_dictionaries() -> Vec<PathBuf> {
    if let Err(e) = crate::library::ensure() {
        eprintln!("could not prepare the dictionary library: {e}");
        return Vec::new();
    }
    crate::library::json_dictionaries()
}

pub fn run(args: &[String]) -> std::process::ExitCode {
    match args.first().map(String::as_str) {
        Some("lookup") => lookup(&args[1..]),
        Some("clean") => clean(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("machine") => machine(&args[1..]),
        Some("import") => import(&args[1..]),
        Some("dictionaries") => list_dictionaries(),
        Some(other) => {
            eprintln!("unknown command {other:?}");
            usage();
            std::process::ExitCode::from(2)
        }
        None => unreachable!("run is only called with at least one argument"),
    }
}

fn usage() {
    eprintln!("usage:");
    eprintln!("  pluvialis lookup <OUTLINE|WORD> [OUTLINE|WORD...]");
    eprintln!("  pluvialis clean [--write] [DICTIONARY...]");
    eprintln!("  pluvialis check [DICTIONARY...]");
    eprintln!("  pluvialis machine [SECONDS]");
    eprintln!("  pluvialis import <FILE> [FILE...]");
    eprintln!("  pluvialis dictionaries");
}

/// Copy dictionaries into the library.
///
/// Pluvialis starts with no dictionaries, so this is how the first one arrives.
/// Dropping the file into the library folder by hand does the same thing; this
/// exists because it can say why a file was refused.
fn import(args: &[String]) -> std::process::ExitCode {
    if args.is_empty() {
        eprintln!("usage: pluvialis import <FILE> [FILE...]");
        eprintln!("accepts Plover .json and .py dictionaries");
        return std::process::ExitCode::from(2);
    }

    let mut failed = false;
    let mut imported_python = false;
    for argument in args {
        match crate::library::import(Path::new(argument)) {
            Ok(destination) => {
                imported_python |= destination.extension().is_some_and(|e| e == "py");
                println!("imported {}", destination.display());
            }
            Err(e) => {
                eprintln!("{e}");
                failed = true;
            }
        }
    }

    if imported_python {
        println!("\nA Python dictionary is loaded switched off. Turn it on in the");
        println!("Dictionaries pane once you have looked at what it does.");
    }

    if failed {
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

/// What is in the library, in the priority order it will be used in.
fn list_dictionaries() -> std::process::ExitCode {
    if let Err(e) = crate::library::ensure() {
        eprintln!("could not prepare the dictionary library: {e}");
        return std::process::ExitCode::FAILURE;
    }

    let json = crate::library::json_dictionaries();
    let python = crate::library::python_dictionaries();

    println!("{}\n", crate::library::dir().display());
    if json.is_empty() && python.is_empty() {
        println!("No dictionaries yet. Add one with:");
        println!("  pluvialis import <FILE>");
        return std::process::ExitCode::SUCCESS;
    }

    for path in &json {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match Dictionary::load(path) {
            Ok(dictionary) => println!("{name:<32} {:>7} entries", dictionary.len()),
            Err(e) => println!("{name:<32} will not load: {e}"),
        }
    }
    for path in &python {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        println!("{name:<32} Python, consulted last, off until enabled");
    }

    std::process::ExitCode::SUCCESS
}

/// Watch the Auto scanner and print everything it reports.
///
/// The machine layer is otherwise only observable through the GUI, which makes
/// a connection problem hard to see and impossible to paste into a bug report.
/// This runs the same scanner the app runs, so what it shows is what the app
/// would do. `RUST_LOG=trace` adds the per-attempt detail.
fn machine(args: &[String]) -> std::process::ExitCode {
    let seconds: u64 = args
        .first()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);

    println!("Watching for steno machines for {seconds}s. Write on the machine to see strokes.\n");

    let (tx, rx) = crossbeam_channel::unbounded();
    let _scanner = pluvialis_machine::Scanner::spawn(pluvialis_machine::all_machines(), tx);

    let started = Instant::now();
    let deadline = std::time::Duration::from_secs(seconds);
    let mut strokes = 0usize;

    while started.elapsed() < deadline {
        let left = deadline.saturating_sub(started.elapsed());
        match rx.recv_timeout(left.min(std::time::Duration::from_millis(250))) {
            Ok(pluvialis_machine::MachineEvent::Status(status)) => {
                let elapsed = started.elapsed().as_secs_f32();
                match status {
                    pluvialis_machine::MachineStatus::Searching => {
                        println!("[{elapsed:5.1}s] searching");
                    }
                    pluvialis_machine::MachineStatus::Connected { machine, port } => {
                        println!("[{elapsed:5.1}s] CONNECTED  {machine} on {port}");
                    }
                    pluvialis_machine::MachineStatus::Disconnected { reason } => {
                        println!("[{elapsed:5.1}s] disconnected: {reason}");
                    }
                }
            }
            Ok(pluvialis_machine::MachineEvent::Stroke(stroke)) => {
                strokes += 1;
                let elapsed = started.elapsed().as_secs_f32();
                println!(
                    "[{elapsed:5.1}s] stroke     {}",
                    Stroke::render_outline(&[stroke])
                );
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("\n{strokes} strokes in {seconds}s.");
    std::process::ExitCode::SUCCESS
}

/// Format every entry and report meta commands the formatter does not
/// implement.
///
/// This is the acceptance test for the formatter: an unimplemented meta
/// produces subtly wrong text rather than an error, so the only way to know
/// the coverage is real is to run every entry through and see what falls out.
fn check(args: &[String]) -> std::process::ExitCode {
    let explicit: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let targets: Vec<PathBuf> = if explicit.is_empty() {
        library_dictionaries()
    } else {
        explicit
    };

    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    let mut checked = 0usize;

    for path in targets {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                return std::process::ExitCode::FAILURE;
            }
        };
        let entries: BTreeMap<String, String> = match serde_json::from_str(&text) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                return std::process::ExitCode::FAILURE;
            }
        };

        let started = Instant::now();
        for (key, value) in &entries {
            checked += 1;
            // Format each entry on its own. Meta coverage does not depend on
            // what came before it.
            let translation = Translation::for_test(Vec::new(), Some(value.clone()));
            let formatted = pluvialis_core::format::format(std::slice::from_ref(&translation));
            for meta in formatted.unknown_metas {
                *unknown.entry(meta.clone()).or_default() += 1;
                examples.entry(meta).or_insert_with(|| key.clone());
            }
        }
        println!(
            "{}: {} entries checked in {:.0?}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            entries.len(),
            started.elapsed()
        );
    }

    println!();
    if unknown.is_empty() {
        println!("{checked} entries checked, every meta command is implemented");
        return std::process::ExitCode::SUCCESS;
    }

    let (known, novel): (Vec<_>, Vec<_>) = unknown
        .iter()
        .partition(|(meta, _)| is_known_unsupported(meta));

    if !known.is_empty() {
        let uses: usize = known.iter().map(|(_, n)| **n).sum();
        println!(
            "{} meta commands unimplemented, {uses} uses. These do not work in Plover \
             on this machine either, so nothing is lost:",
            known.len()
        );
        for (meta, count) in &known {
            let example = examples.get(*meta).map(String::as_str).unwrap_or("");
            println!("  {{{meta}}}  {count} uses, for example in {example:?}");
        }
    }

    if novel.is_empty() {
        println!("\n{checked} entries checked, no unexpected meta commands");
        return std::process::ExitCode::SUCCESS;
    }

    println!("\n{} UNEXPECTED meta commands:", novel.len());
    for (meta, count) in &novel {
        let example = examples.get(*meta).map(String::as_str).unwrap_or("");
        println!("  {{{meta}}}  {count} uses, for example in {example:?}");
    }
    std::process::ExitCode::FAILURE
}

/// Meta commands we do not implement and do not intend to, because the user's
/// own Plover cannot handle them either.
///
/// `{*}`, `{*!}` and `{*?}` are absent from Plover 5.4's meta dispatch table.
/// The rest need plugins (plover-stitching, plover-emoji) that are not
/// installed. Implementing them would mean inventing semantics we have not
/// read, which is how subtly wrong text gets into a document.
fn is_known_unsupported(meta: &str) -> bool {
    matches!(meta, "*" | "*!" | "*?" | ":emoji") || meta.starts_with(":stitch_last_word:")
}

/// Remove entries whose keys are not valid steno.
///
/// Defaults to a dry run. Rewriting dictionaries shared with a working Plover
/// install should be something you ask for, not something that happens as a
/// side effect of running a report.
fn clean(args: &[String]) -> std::process::ExitCode {
    let write = args.iter().any(|a| a == "--write");
    let explicit: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();

    let targets: Vec<PathBuf> = if explicit.is_empty() {
        library_dictionaries()
    } else {
        explicit
    };

    if !write {
        println!("Dry run. Nothing will be written. Add --write to apply.\n");
    }

    let mut failed = false;
    for path in targets {
        match pluvialis_core::clean_dictionary(&path, !write) {
            Ok(report) => {
                let name = report
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                if report.removed_count() == 0 {
                    println!("{name}: {} entries, all keys valid", report.total_entries);
                    continue;
                }

                println!(
                    "{name}: {} of {} entries have keys that are not valid steno",
                    report.removed_count(),
                    report.total_entries
                );
                for (key, value) in report.removed.iter().take(5) {
                    let reason = report
                        .reasons
                        .get(key)
                        .map(|r| r.to_string())
                        .unwrap_or_default();
                    println!("    {key:?} -> {value:?}   ({reason})");
                }
                if report.removed_count() > 5 {
                    println!("    ... and {} more", report.removed_count() - 5);
                }

                match (&report.backup, &report.removed_file) {
                    (Some(backup), Some(removed)) => {
                        println!("  kept {} entries", report.kept_count());
                        println!("  original saved to {}", backup.display());
                        println!("  removed entries saved to {}", removed.display());
                    }
                    _ => println!("  would keep {} entries", report.kept_count()),
                }
            }
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                failed = true;
            }
        }
    }

    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

/// Answer from the real dictionaries, in both directions.
///
/// An argument is looked up as an outline when it parses as one, and as a word
/// always. Both, because plenty of words are also valid steno ("the", "to",
/// "pro"), and answering only one direction hides the other with no sign that
/// it was skipped. Read only: editing entries is the GUI's job, where the
/// dictionary a change lands in is visible.
fn lookup(queries: &[String]) -> std::process::ExitCode {
    if queries.is_empty() {
        eprintln!("usage: pluvialis lookup <OUTLINE|WORD> [OUTLINE|WORD...]");
        eprintln!("example: pluvialis lookup KAT WEL/KO*PL cat");
        return std::process::ExitCode::from(2);
    }

    let started = Instant::now();
    let mut stack = DictionaryStack::new();
    for path in library_dictionaries() {
        let name = file_name(&path);
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
    for query in queries {
        println!();
        println!("{query}");
        let started = Instant::now();
        let parsed = Stroke::parse_outline(query).ok();
        let mut found = false;

        // What the outline means. Every dictionary that has it, not just the
        // one that wins, so a shadowed entry is visible rather than a mystery.
        if let Some(strokes) = &parsed {
            let canonical = Stroke::render_outline(strokes);
            let winner = stack
                .dictionaries()
                .iter()
                .position(|d| d.enabled && d.lookup(strokes).is_some());
            for (index, dictionary) in stack.dictionaries().iter().enumerate() {
                if let Some(value) = dictionary.lookup(strokes) {
                    found = true;
                    println!(
                        "  means  {:<20} {:<40} {}{}",
                        canonical,
                        format!("{value:?}"),
                        file_name(&dictionary.path),
                        match (winner == Some(index), dictionary.enabled) {
                            (true, _) => "",
                            (false, true) => "   (shadowed)",
                            (false, false) => "   (disabled)",
                        }
                    );
                }
            }
        }

        // Every way the query can be written. Runs whether or not it parsed as
        // steno, so a word that is also an outline still shows its strokes.
        let mut written: Vec<(String, String, String)> = Vec::new();
        for dictionary in stack.dictionaries() {
            for (outline, value) in dictionary.reverse_lookup(query) {
                written.push((
                    Stroke::render_outline(outline),
                    value.to_owned(),
                    file_name(&dictionary.path),
                ));
            }
        }
        // Shortest outline first: the brief is what a writer wants to learn.
        written.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then(a.0.cmp(&b.0)));
        for (outline, value, dictionary) in &written {
            found = true;
            println!(
                "  write  {outline:<20} {:<40} {dictionary}",
                format!("{value:?}")
            );
        }

        let elapsed = started.elapsed();
        if !found {
            println!("  no entry, and nothing is written that way");
            failed = true;
        }
        println!("  (searched in {elapsed:.0?})");

        // What the translator makes of it stroke by stroke, which is where
        // retroactive correction becomes visible. Only meaningful for steno.
        if let Some(strokes) = &parsed {
            let mut translator = Translator::new();
            for stroke in strokes {
                translator.translate(&stack, *stroke);
            }
            println!("  translated: {:?}", translator.text());
        }
    }

    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
