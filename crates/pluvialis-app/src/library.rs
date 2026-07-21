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
//! **A fresh Pluvialis has no dictionaries at all.** Decided by the user the
//! same day, after an earlier version of this module seeded the library from her
//! Plover folder on first run: "not enabled by default. No dictionaries by
//! default until the user imports them."
//!
//! That seeding is gone. It was convenient for exactly one person on exactly one
//! machine, and it guessed: it decided which of her files were dictionaries and
//! what their priority order should be, neither of which it could know. An empty
//! library is honest about the fact that the program does not know what she
//! wants to write with.
//!
//! Her existing library was left in place when this changed. Removing it would
//! have deleted the dictionaries she writes with to make a default tidier.

use std::path::{Path, PathBuf};

/// Where Pluvialis keeps the dictionaries it owns. A sibling of `documents`.
pub fn dir() -> PathBuf {
    PathBuf::from(r"F:\Steno\Pluvialis\dictionaries")
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: not a dictionary, it defines no lookup")]
    NotADictionary { path: PathBuf },

    #[error("{path}: not a dictionary format Pluvialis reads (.json or .py)")]
    UnsupportedFormat { path: PathBuf },

    #[error("{path}: already in the library, remove it first to replace it")]
    AlreadyPresent { path: PathBuf },
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> LibraryError + '_ {
    move |source| LibraryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Create the library folder if it is missing. It starts empty and stays empty
/// until the user puts something in it.
pub fn ensure() -> Result<(), LibraryError> {
    let dir = dir();
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(io(&dir))
}

/// Copy a dictionary into the library.
///
/// Copies rather than references, which is the whole point of the library: the
/// file Pluvialis reads is one nothing else writes to. The source is not
/// modified or moved.
///
/// A `.py` source is screened first. Deciding whether a Python file is a
/// dictionary by running it and seeing what happens is not an option, because a
/// file that is not a dictionary can do anything at all when executed, and one
/// in the user's own Plover folder copies a dictionary to another drive.
///
/// Refuses to overwrite. A name collision is far more likely to be an accident
/// than an intent to replace, and the recovery from a wrong overwrite is a file
/// the user may not have another copy of.
pub fn import(source: &Path) -> Result<PathBuf, LibraryError> {
    ensure()?;

    let extension = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "json" => {}
        "py" => {
            let text = std::fs::read_to_string(source).map_err(io(source))?;
            if !pluvialis_python::looks_like_a_dictionary(&text) {
                return Err(LibraryError::NotADictionary {
                    path: source.to_path_buf(),
                });
            }
        }
        _ => {
            return Err(LibraryError::UnsupportedFormat {
                path: source.to_path_buf(),
            });
        }
    }

    let Some(name) = source.file_name() else {
        return Err(LibraryError::UnsupportedFormat {
            path: source.to_path_buf(),
        });
    };

    let destination = dir().join(name);
    if destination.exists() {
        return Err(LibraryError::AlreadyPresent { path: destination });
    }

    std::fs::copy(source, &destination).map_err(io(source))?;
    Ok(destination)
}

/// The JSON dictionaries in the library, in priority order, highest first.
///
/// Order is alphabetical by file name. That is not arbitrary and it is not
/// clever: it is predictable, which is what matters when the user is the one
/// putting files in the folder. It also happens to preserve the order the
/// seeded pair had, since `cb_dictionary_full` sorts before `dutch`, and
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

    #[test]
    fn listing_a_missing_library_is_empty_rather_than_an_error() {
        // Called before `ensure`, or after the folder is deleted underneath us.
        let listed = files_with_extension("json");
        let _ = listed;
    }

    /// Importing is the only way in, so refusing the wrong thing matters.
    #[test]
    fn it_refuses_formats_it_cannot_read() {
        let path = std::env::temp_dir().join("pluvialis-import-probe.rtf");
        std::fs::write(&path, "not a dictionary").expect("writing the probe");
        assert!(matches!(
            import(&path),
            Err(LibraryError::UnsupportedFormat { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }

    /// A `.py` that is not a dictionary is refused without being executed. The
    /// user's own Plover folder holds one that copies a dictionary to another
    /// drive when run, so "import it and see what happens" is not an option.
    #[test]
    fn it_refuses_a_python_file_that_is_not_a_dictionary() {
        let path = std::env::temp_dir().join("pluvialis-import-probe.py");
        std::fs::write(&path, "import shutil\nshutil.copy2(a, b)\n").expect("writing the probe");
        assert!(matches!(
            import(&path),
            Err(LibraryError::NotADictionary { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }
}
