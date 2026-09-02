//! Choices that should outlive a single run.
//!
//! Three things live here: which dictionaries are enabled, what order they are
//! consulted in, and the settings on the Settings screen. It is kept small and
//! separate on purpose: the dictionary files themselves are the user's data and
//! are never touched to record a preference, so a stray checkbox can never
//! corrupt a dictionary.
//!
//! Dictionary state is keyed by file name rather than by position, so it
//! survives adding, removing or reordering dictionaries. A name that is absent
//! from the file falls back to the built-in default (JSON on, Python off),
//! which is what a brand new dictionary should do.
//!
//! ## The file has two shapes
//!
//! Before the Settings screen existed the whole file was one flat
//! `name -> enabled` object. That shape is still read, and is recognised by the
//! absence of a `"dictionaries"` key, so nobody loses their checkboxes by
//! updating. It is rewritten in the new shape on the next save.
//!
//! There is no serde derive in this crate, so both shapes are read and written
//! through `serde_json::Value` by hand. A failure at any point falls back to
//! defaults and logs, because losing a preference is a nuisance and refusing to
//! start is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Seconds between autosaves. Also the floor and ceiling the Settings screen
/// offers, and what a hand-edited file is clamped to.
pub const DEFAULT_AUTOSAVE_SECONDS: u64 = 60;
pub const AUTOSAVE_RANGE: std::ops::RangeInclusive<u64> = 5..=600;

/// How many tape lines to keep. The strip lays out every line each frame, so
/// this bounds per-frame cost rather than memory; see `live::trim_tape`.
pub const DEFAULT_TAPE_LIMIT: usize = 500;
pub const TAPE_RANGE: std::ops::RangeInclusive<usize> = 50..=5000;

pub const DEFAULT_FONT_SIZE: f32 = 18.0;
pub const FONT_RANGE: std::ops::RangeInclusive<f32> = 10.0..=48.0;

/// Everything the Settings screen owns.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub autosave_seconds: u64,
    /// `None` means the default folder beside the executable. A folder chosen
    /// here takes effect at the next start, because the running document is
    /// already open from the old one.
    pub documents_dir: Option<PathBuf>,
    pub tape_limit: usize,
    pub font_size: f32,
    /// Whether typing into other windows is switched on when Pluvialis starts.
    pub output_at_launch: bool,
    /// Checked before anything is recorded, not before it is displayed. This
    /// file would otherwise hold the words she writes whether or not she ever
    /// opened the Stats screen.
    pub record_stats: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            autosave_seconds: DEFAULT_AUTOSAVE_SECONDS,
            documents_dir: None,
            tape_limit: DEFAULT_TAPE_LIMIT,
            font_size: DEFAULT_FONT_SIZE,
            output_at_launch: true,
            record_stats: true,
        }
    }
}

impl Settings {
    /// Pull every value back inside its range. The file is plain JSON in a
    /// folder the user is invited to open, so it can hold anything.
    fn clamped(mut self) -> Self {
        self.autosave_seconds = self
            .autosave_seconds
            .clamp(*AUTOSAVE_RANGE.start(), *AUTOSAVE_RANGE.end());
        self.tape_limit = self
            .tape_limit
            .clamp(*TAPE_RANGE.start(), *TAPE_RANGE.end());
        self.font_size = self.font_size.clamp(*FONT_RANGE.start(), *FONT_RANGE.end());
        self
    }

    pub fn autosave_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.autosave_seconds)
    }
}

/// The whole preference file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// `file name -> enabled`.
    pub enabled: HashMap<String, bool>,
    /// File names in priority order, highest first. Names not listed keep the
    /// order they were found in, after the listed ones.
    pub order: Vec<String>,
    pub settings: Settings,
}

/// Beside the executable, a sibling of the dictionary and document folders. Not
/// inside `dictionaries\`, because a `.json` there would be picked up and loaded
/// as a dictionary. See `paths`.
fn path() -> PathBuf {
    crate::paths::base_dir().join("pluvialis-config.json")
}

/// The saved configuration. Defaults on any problem reading it: a missing or
/// unreadable preference file is not an error worth stopping for.
pub fn load() -> Config {
    load_from(&path())
}

/// Write the configuration. A failure here is logged, not surfaced: losing a
/// preference is a nuisance, not something to interrupt writing over.
pub fn save(config: &Config) {
    save_to(&path(), config);
}

fn load_from(path: &Path) -> Config {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        log::warn!("{} is not valid JSON, using defaults", path.display());
        return Config::default();
    };
    let Some(object) = value.as_object() else {
        return Config::default();
    };

    // The old shape: the whole file was the enabled map. Recognised by what is
    // missing rather than by what it contains, since a dictionary could be
    // named anything.
    if !object.contains_key("dictionaries") {
        return Config {
            enabled: booleans(&value),
            ..Config::default()
        };
    }

    Config {
        enabled: object.get("dictionaries").map(booleans).unwrap_or_default(),
        order: object
            .get("order")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        settings: object.get("settings").map(settings).unwrap_or_default(),
    }
}

/// Every `name: true|false` pair, ignoring anything that is not a boolean.
fn booleans(value: &serde_json::Value) -> HashMap<String, bool> {
    let Some(object) = value.as_object() else {
        return HashMap::new();
    };
    object
        .iter()
        .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
        .collect()
}

fn settings(value: &serde_json::Value) -> Settings {
    let default = Settings::default();
    let Some(object) = value.as_object() else {
        return default;
    };
    let get = |key: &str| object.get(key);

    Settings {
        autosave_seconds: get("autosave_seconds")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(default.autosave_seconds),
        // An empty string is the same as absent, so clearing the folder in the
        // file returns to the default rather than pointing at nothing.
        documents_dir: get("documents_dir")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from),
        tape_limit: get("tape_limit")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(default.tape_limit),
        font_size: get("font_size")
            .and_then(serde_json::Value::as_f64)
            .map(|n| n as f32)
            .unwrap_or(default.font_size),
        output_at_launch: get("output_at_launch")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default.output_at_launch),
        record_stats: get("record_stats")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default.record_stats),
    }
    .clamped()
}

fn save_to(path: &Path, config: &Config) {
    let settings = serde_json::json!({
        "autosave_seconds": config.settings.autosave_seconds,
        "documents_dir": config.settings.documents_dir
            .as_ref()
            .map(|p| p.display().to_string()),
        "tape_limit": config.settings.tape_limit,
        "font_size": config.settings.font_size,
        "output_at_launch": config.settings.output_at_launch,
        "record_stats": config.settings.record_stats,
    });
    let document = serde_json::json!({
        "dictionaries": config.enabled,
        "order": config.order,
        "settings": settings,
    });

    match serde_json::to_string_pretty(&document) {
        Ok(text) => {
            if let Err(e) = std::fs::write(path, text) {
                log::warn!("could not save settings to {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("could not serialise settings: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("pluvialis-config-{tag}-{nanos}.json"))
    }

    #[test]
    fn a_missing_file_reads_as_defaults() {
        let path = temp_file("missing");
        assert_eq!(load_from(&path), Config::default());
    }

    #[test]
    fn garbage_reads_as_defaults_rather_than_panicking() {
        let path = temp_file("garbage");
        std::fs::write(&path, "not json at all").expect("writing the probe");
        assert_eq!(load_from(&path), Config::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_whole_config_round_trips() {
        let path = temp_file("roundtrip");
        let mut config = Config::default();
        config.enabled.insert("cb_dictionary_full.json".into(), true);
        config.enabled.insert("dutch.json".into(), false);
        config.order = vec!["dutch.json".into(), "cb_dictionary_full.json".into()];
        config.settings.autosave_seconds = 30;
        config.settings.tape_limit = 200;
        config.settings.font_size = 22.0;
        config.settings.output_at_launch = false;
        config.settings.record_stats = false;
        config.settings.documents_dir = Some(std::env::temp_dir().join("docs"));

        save_to(&path, &config);
        assert_eq!(load_from(&path), config);

        let _ = std::fs::remove_file(&path);
    }

    /// The shape written before the Settings screen existed. Reading it has to
    /// keep working, or updating Pluvialis silently re-enables every dictionary
    /// the user had turned off.
    #[test]
    fn the_old_flat_shape_is_still_read() {
        let path = temp_file("old-shape");
        std::fs::write(
            &path,
            r#"{"cb_dictionary_full.json": true, "jeff-phrasing.py": false}"#,
        )
        .expect("writing the probe");

        let config = load_from(&path);
        assert_eq!(
            config.enabled.get("cb_dictionary_full.json").copied(),
            Some(true)
        );
        assert_eq!(config.enabled.get("jeff-phrasing.py").copied(), Some(false));
        assert!(config.order.is_empty());
        assert_eq!(config.settings, Settings::default());

        let _ = std::fs::remove_file(&path);
    }

    /// The file sits in a folder the user is told to open, so it can be edited
    /// by hand into something unusable.
    #[test]
    fn a_hand_edited_file_is_clamped_rather_than_obeyed() {
        let path = temp_file("silly");
        std::fs::write(
            &path,
            r#"{"dictionaries": {}, "settings": {"autosave_seconds": 0, "tape_limit": 9999999, "font_size": 400}}"#,
        )
        .expect("writing the probe");

        let settings = load_from(&path).settings;
        assert_eq!(settings.autosave_seconds, *AUTOSAVE_RANGE.start());
        assert_eq!(settings.tape_limit, *TAPE_RANGE.end());
        assert_eq!(settings.font_size, *FONT_RANGE.end());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_setting_the_file_does_not_mention_takes_its_default() {
        let path = temp_file("partial");
        std::fs::write(&path, r#"{"dictionaries": {}, "settings": {"font_size": 20}}"#)
            .expect("writing the probe");

        let settings = load_from(&path).settings;
        assert_eq!(settings.font_size, 20.0);
        assert_eq!(settings.autosave_seconds, DEFAULT_AUTOSAVE_SECONDS);
        assert!(settings.output_at_launch);

        let _ = std::fs::remove_file(&path);
    }
}
