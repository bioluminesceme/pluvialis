//! Choices that should outlive a single run.
//!
//! Right now that is only which dictionaries are enabled. It is kept small and
//! separate on purpose: the dictionary files themselves are the user's data and
//! are never touched to record a preference, so a stray checkbox can never
//! corrupt a dictionary.
//!
//! The state is keyed by file name rather than by position, so it survives
//! adding, removing or reordering dictionaries. A name that is absent from the
//! file falls back to the built-in default (JSON on, Python off), which is what
//! a brand new dictionary should do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Beside the executable, a sibling of the dictionary and document folders. Not
/// inside `dictionaries\`, because a `.json` there would be picked up and loaded
/// as a dictionary. See `paths`.
fn path() -> PathBuf {
    crate::paths::base_dir().join("pluvialis-config.json")
}

/// The saved enabled state, `file name -> enabled`. An empty map on any problem
/// reading it: a missing or unreadable preference file is not an error worth
/// stopping for, it just means every dictionary takes its default.
pub fn load_enabled() -> HashMap<String, bool> {
    load_enabled_from(&path())
}

/// Write the enabled state. A failure here is logged, not surfaced: losing a
/// preference is a nuisance, not something to interrupt writing over.
pub fn save_enabled(enabled: &HashMap<String, bool>) {
    save_enabled_to(&path(), enabled);
}

fn load_enabled_from(path: &Path) -> HashMap<String, bool> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_enabled_to(path: &Path, enabled: &HashMap<String, bool>) {
    match serde_json::to_string_pretty(enabled) {
        Ok(text) => {
            if let Err(e) = std::fs::write(path, text) {
                log::warn!("could not save dictionary state to {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("could not serialise dictionary state: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("pluvialis-config-{tag}-{millis}.json"))
    }

    #[test]
    fn a_missing_file_reads_as_no_preferences() {
        let path = temp_file("missing");
        assert!(load_enabled_from(&path).is_empty());
    }

    #[test]
    fn garbage_reads_as_no_preferences_rather_than_panicking() {
        let path = temp_file("garbage");
        std::fs::write(&path, "not json at all").expect("writing the probe");
        assert!(load_enabled_from(&path).is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_enabled_state_round_trips() {
        let path = temp_file("roundtrip");
        let mut state = HashMap::new();
        state.insert("cb_dictionary_full.json".to_owned(), true);
        state.insert("dutch.json".to_owned(), false);
        state.insert("jeff-phrasing.py".to_owned(), true);

        save_enabled_to(&path, &state);
        assert_eq!(load_enabled_from(&path), state);

        let _ = std::fs::remove_file(&path);
    }
}
