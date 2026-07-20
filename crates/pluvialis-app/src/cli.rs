//! Temporary command line entry points, used to exercise the core before
//! there is a GUI to do it through.
//!
//! `lookup` is the M1 verification: it loads the real dictionaries and answers
//! from them, reporting load time and lookup latency. Proper subcommands
//! (`convert`, `check`) arrive in M6 and will want a real argument parser.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use pluvialis_core::{Dictionary, DictionaryStack, Stroke, Translation, Translator};

/// Where the user's dictionaries live. These are shared in place with her
/// working Plover install and are never copied or modified.
pub(crate) const DICTIONARY_DIR: &str = r"C:\Users\Corien\AppData\Local\plover\plover";

/// Priority order, highest first, mirroring her plover.cfg. Only the JSON ones
/// are listed here. `jeff-phrasing.py` and any other Python dictionary in the
/// same directory are discovered separately by the GUI and loaded disabled.
pub(crate) const DICTIONARIES: [&str; 2] = ["cb_dictionary_full.json", "corien-dutch.json"];

pub fn run(args: &[String]) -> std::process::ExitCode {
    match args.first().map(String::as_str) {
        Some("lookup") => lookup(&args[1..]),
        Some("clean") => clean(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("machine") => machine(&args[1..]),
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
    eprintln!("  pluvialis lookup <OUTLINE> [OUTLINE...]");
    eprintln!("  pluvialis clean [--write] [DICTIONARY...]");
    eprintln!("  pluvialis check [DICTIONARY...]");
    eprintln!("  pluvialis machine [SECONDS]");
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
        DICTIONARIES
            .iter()
            .map(|name| PathBuf::from(DICTIONARY_DIR).join(name))
            .collect()
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
        DICTIONARIES
            .iter()
            .map(|name| PathBuf::from(DICTIONARY_DIR).join(name))
            .collect()
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
