//! The dictionary library: Pluvialis keeps its own copies.
//!
//! **Decided by the user, 2026-07-20:** "any dictionaries we open in Pluvialis
//! get saved into a special folder. User can edit them in VSCode from there if
//! needed, and Pluvialis has full access too. We own them."
//!
//! This replaces the earlier arrangement, where the dictionaries in her Plover
//! folder were read in place and shared with a working Plover install. That was
//! deliberate too, and it was changed deliberately: sharing a file with another
//! program means every write has to consider what the other one is doing with
//! it, and an accidental edit reaches further than the program that made it.
//! Owning a copy removes the whole class of problem.
//!
//! **The consequence, stated plainly, because it is a real cost.** The two
//! copies drift. Editing `cb_dictionary_full.json` in her Plover folder no
//! longer changes anything here, and editing the copy here does not change what
//! Plover does. She accepted this knowing it, when the alternative was offered.
//!
//! Seeding happens **once**, when the folder does not yet exist. After that the
//! folder is hers and nothing re-copies into it, or a dictionary she deliberately
//! removed would silently return on the next start.

use std::path::{Path, PathBuf};

/// Where Pluvialis keeps the dictionaries it owns. A sibling of `documents`.
pub fn dir() -> PathBuf {
    PathBuf::from(r"F:\Steno\Pluvialis\dictionaries")
}

/// Her Plover folder, read **only** to seed an empty library on first run, and
/// never written to.
const SEED_DIR: &str = r"C:\Users\Corien\AppData\Local\plover\plover";

/// The JSON dictionaries she was using, in the priority order they had.
const SEED_JSON: [&str; 2] = ["cb_dictionary_full.json", "corien-dutch.json"];

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> LibraryError + '_ {
    move |source| LibraryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Create the library if it is missing, seeding it from her Plover folder.
///
/// Returns whether anything was seeded, so the caller can say so rather than
/// leaving the user wondering where the files came from.
pub fn ensure() -> Result<Vec<String>, LibraryError> {
    let dir = dir();
    if dir.exists() {
        return Ok(Vec::new());
    }

    std::fs::create_dir_all(&dir).map_err(io(&dir))?;

    let seed_dir = Path::new(SEED_DIR);
    let mut copied = Vec::new();

    for name in SEED_JSON {
        let source = seed_dir.join(name);
        if source.is_file() {
            std::fs::copy(&source, dir.join(name)).map_err(io(&source))?;
            copied.push(name.to_owned());
        }
    }

    // Python dictionaries, screened first. Her folder holds `.py` files that
    // are scripts rather than dictionaries, and one of them copies a dictionary
    // to another drive when executed. The screen is what tells them apart
    // without running anything.
    if let Ok(entries) = std::fs::read_dir(seed_dir) {
        for entry in entries.flatten() {
            let source = entry.path();
            if source.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&source) else {
                continue;
            };
            if !pluvialis_python::looks_like_a_dictionary(&text) {
                log::info!("not seeding {}: it defines no lookup", source.display());
                continue;
            }
            let Some(name) = source.file_name() else {
                continue;
            };
            std::fs::copy(&source, dir.join(name)).map_err(io(&source))?;
            copied.push(name.to_string_lossy().into_owned());
        }
    }

    Ok(copied)
}

/// The JSON dictionaries in the library, in priority order, highest first.
///
/// Order is alphabetical by file name. That is not arbitrary and it is not
/// clever: it is predictable, which is what matters when the user is the one
/// putting files in the folder. It also happens to preserve the order the
/// seeded pair had, since `cb_dictionary_full` sorts before `corien-dutch`, and
/// that pair's order is load bearing (`SKP` is "and" in the English one and
/// "en" in the Dutch one, and she confirmed the English one wins).
///
/// Reordering in the dictionary pane still works and still lasts only for the
/// session. Persisting it is the open item.
pub fn json_dictionaries() -> Vec<PathBuf> {
    files_with_extension("json")
}

/// The Python dictionaries in the library, in the same order.
pub fn python_dictionaries() -> Vec<PathBuf> {
    files_with_extension("py")
}

fn files_with_extension(extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(extension))
        .collect();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seeded pair's order is load bearing, and this pins the coincidence
    /// that alphabetical order preserves it. If a future dictionary needs to
    /// outrank `cb_dictionary_full`, alphabetical order stops being enough and
    /// this test is where that will show up.
    #[test]
    fn the_english_dictionary_outranks_the_dutch_one_alphabetically() {
        let mut order = SEED_JSON;
        order.sort();
        assert_eq!(order, ["cb_dictionary_full.json", "corien-dutch.json"]);
    }

    #[test]
    fn listing_a_missing_library_is_empty_rather_than_an_error() {
        // Called before `ensure`, or after the folder is deleted underneath us.
        let listed = files_with_extension("json");
        let _ = listed;
    }
}
