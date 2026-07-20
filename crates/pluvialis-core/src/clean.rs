//! Removing entries whose keys are not valid steno.
//!
//! These entries are dead weight: Plover cannot reach them either, because
//! nothing you can physically stroke will ever produce the key. See
//! `thingstonote.md` for why they must not be "fixed" by loosening the parser.
//!
//! The rewrite is deliberately line based rather than a reserialization. Both
//! of the user's dictionaries store one entry per line, so dropping lines
//! preserves key order, indentation and every byte of the entries we keep. A
//! reserialize would reformat a 93,000 line file to remove nothing.
//!
//! Nothing is written until the result has been parsed back and checked, and
//! the removed entries are always saved alongside so the edit is reversible.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::stroke::{Stroke, StrokeError};

#[derive(Debug, thiserror::Error)]
pub enum CleanError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// The rewrite produced something that is not the dictionary we intended.
    /// Nothing has been written when this is returned.
    #[error("refusing to write {path}: {reason}")]
    VerificationFailed { path: PathBuf, reason: String },
}

#[derive(Debug)]
pub struct CleanReport {
    pub path: PathBuf,
    pub total_entries: usize,
    /// Removed entries, key to value, so the caller can show examples.
    pub removed: BTreeMap<String, String>,
    pub reasons: BTreeMap<String, StrokeError>,
    /// `None` in a dry run, or when there was nothing to remove.
    pub backup: Option<PathBuf>,
    pub removed_file: Option<PathBuf>,
}

impl CleanReport {
    pub fn removed_count(&self) -> usize {
        self.removed.len()
    }

    pub fn kept_count(&self) -> usize {
        self.total_entries - self.removed.len()
    }
}

/// Find and optionally remove entries whose keys are not valid steno.
///
/// With `dry_run` nothing is written and the report describes what would
/// happen.
pub fn clean_dictionary(path: impl AsRef<Path>, dry_run: bool) -> Result<CleanReport, CleanError> {
    let path = path.as_ref().to_path_buf();
    let text = std::fs::read_to_string(&path).map_err(|source| CleanError::Read {
        path: path.clone(),
        source,
    })?;

    let entries: BTreeMap<String, String> =
        serde_json::from_str(&text).map_err(|source| CleanError::Json {
            path: path.clone(),
            source,
        })?;
    let total_entries = entries.len();

    let mut removed = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    for (key, value) in &entries {
        if let Err(e) = Stroke::parse_outline(key) {
            removed.insert(key.clone(), value.clone());
            reasons.insert(key.clone(), e);
        }
    }

    let mut report = CleanReport {
        path: path.clone(),
        total_entries,
        removed,
        reasons,
        backup: None,
        removed_file: None,
    };

    if report.removed.is_empty() || dry_run {
        return Ok(report);
    }

    let cleaned = strip_keys(&text, &report.removed);

    // Verify before writing. If the rewrite lost or gained anything, the line
    // based assumption did not hold for this file and we must not touch it.
    let reparsed: BTreeMap<String, String> =
        serde_json::from_str(&cleaned).map_err(|e| CleanError::VerificationFailed {
            path: path.clone(),
            reason: format!("result is not valid JSON ({e})"),
        })?;
    let expected = report.kept_count();
    if reparsed.len() != expected {
        return Err(CleanError::VerificationFailed {
            path: path.clone(),
            reason: format!(
                "expected {expected} entries after cleaning, got {}",
                reparsed.len()
            ),
        });
    }
    for (key, value) in &reparsed {
        if entries.get(key) != Some(value) {
            return Err(CleanError::VerificationFailed {
                path: path.clone(),
                reason: format!("entry {key:?} changed value during cleaning"),
            });
        }
    }

    // Everything checks out. Save the original and the removed entries before
    // overwriting, so this is always reversible.
    let stamp = timestamp();
    let backup = sibling(&path, &format!(".backup-{stamp}.json"));
    std::fs::write(&backup, &text).map_err(|source| CleanError::Write {
        path: backup.clone(),
        source,
    })?;

    let removed_file = sibling(&path, &format!(".removed-{stamp}.json"));
    let removed_json =
        serde_json::to_string_pretty(&report.removed).expect("a map of strings always serializes");
    std::fs::write(&removed_file, removed_json).map_err(|source| CleanError::Write {
        path: removed_file.clone(),
        source,
    })?;

    std::fs::write(&path, &cleaned).map_err(|source| CleanError::Write {
        path: path.clone(),
        source,
    })?;

    report.backup = Some(backup);
    report.removed_file = Some(removed_file);
    Ok(report)
}

/// Drop the lines defining the given keys, then repair the trailing comma if
/// the last entry was one of them.
fn strip_keys(text: &str, remove: &BTreeMap<String, String>) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut kept: Vec<&str> = Vec::new();

    for line in text.lines() {
        match leading_json_string(line) {
            Some(key) if remove.contains_key(&key) => continue,
            _ => kept.push(line),
        }
    }

    // JSON forbids a trailing comma before the closing brace, so if the final
    // entry line was removed the new final entry has to lose its comma.
    if let Some(index) = kept
        .iter()
        .rposition(|line| leading_json_string(line).is_some())
    {
        let trimmed = kept[index].trim_end();
        if let Some(without) = trimmed.strip_suffix(',') {
            kept[index] = without;
        }
    }

    let mut out = kept.join(newline);
    if text.ends_with('\n') {
        out.push_str(newline);
    }
    out
}

/// Extract the first JSON string on a line, which for an entry line is its
/// key. Returns `None` for structural lines such as `{` and `}`.
fn leading_json_string(line: &str) -> Option<String> {
    let rest = line.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut key = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => key.push(chars.next()?),
            '"' => {
                // Only an entry line, not a bare string, has a colon next.
                return chars.as_str().trim_start().starts_with(':').then_some(key);
            }
            _ => key.push(c),
        }
    }
    None
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "dictionary".to_owned());
    path.with_file_name(format!("{stem}{suffix}"))
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn finds_invalid_keys_without_writing_in_dry_run() {
        let path = temp(
            "pluv_clean_dry.json",
            "{\n\"KAT\": \"cat\",\n\"WEU*UF\": \"broken\"\n}\n",
        );
        let before = std::fs::read_to_string(&path).unwrap();

        let report = clean_dictionary(&path, true).unwrap();
        assert_eq!(report.removed_count(), 1);
        assert!(report.removed.contains_key("WEU*UF"));
        assert!(report.backup.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn removes_bad_lines_and_keeps_formatting() {
        let path = temp(
            "pluv_clean_write.json",
            "{\n  \"KAT\": \"cat\",\n  \"WEU*UF\": \"broken\",\n  \"TKOG\": \"dog\"\n}\n",
        );
        let report = clean_dictionary(&path, false).unwrap();

        assert_eq!(report.removed_count(), 1);
        assert_eq!(report.kept_count(), 2);

        let after = std::fs::read_to_string(&path).unwrap();
        // The surviving lines keep their exact original text, indentation
        // included.
        assert_eq!(after, "{\n  \"KAT\": \"cat\",\n  \"TKOG\": \"dog\"\n}\n");

        // And the removal is reversible.
        let backup = std::fs::read_to_string(report.backup.unwrap()).unwrap();
        assert!(backup.contains("WEU*UF"));
        let removed = std::fs::read_to_string(report.removed_file.unwrap()).unwrap();
        assert!(removed.contains("broken"));
    }

    #[test]
    fn repairs_the_trailing_comma_when_the_last_entry_goes() {
        let path = temp(
            "pluv_clean_last.json",
            "{\n  \"KAT\": \"cat\",\n  \"WEU*UF\": \"broken\"\n}\n",
        );
        clean_dictionary(&path, false).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "{\n  \"KAT\": \"cat\"\n}\n");
        // The real check: it still parses.
        let parsed: BTreeMap<String, String> = serde_json::from_str(&after).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn clean_file_is_left_completely_untouched() {
        let contents = "{\n  \"KAT\": \"cat\"\n}\n";
        let path = temp("pluv_clean_noop.json", contents);
        let report = clean_dictionary(&path, false).unwrap();

        assert_eq!(report.removed_count(), 0);
        assert!(report.backup.is_none(), "no backup for an unchanged file");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn values_containing_braces_and_quotes_survive() {
        let path = temp(
            "pluv_clean_meta.json",
            "{\n  \"KAT\": \"{^ing}\",\n  \"WEU*UF\": \"broken\",\n  \"TKOG\": \"say \\\"hi\\\"\"\n}\n",
        );
        clean_dictionary(&path, false).unwrap();

        let parsed: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed["KAT"], "{^ing}");
        assert_eq!(parsed["TKOG"], "say \"hi\"");
    }
}
