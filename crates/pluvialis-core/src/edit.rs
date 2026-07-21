//! Adding, changing and removing single dictionary entries.
//!
//! Line based like [`crate::clean`], and for the same reason: both of the
//! user's dictionaries store one entry per line, so editing a line at a time
//! preserves key order, indentation and every untouched entry byte for byte. A
//! reserialize would reformat a 93,000 line file to change one entry.
//!
//! The safety rules are the same too, because these files are the only copy of
//! the user's vocabulary:
//!
//! - The outline is validated as reachable steno before anything is touched. A
//!   key that cannot be stroked is dead weight the moment it is written.
//! - Nothing is written until the result parses back to exactly the dictionary
//!   intended: the one edited entry changed, every other entry identical.
//! - The original is copied to a timestamped sibling first, so any edit undoes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::clean::{leading_json_string, sibling, strip_keys, timestamp};
use crate::stroke::{Stroke, StrokeError};

#[derive(Debug, thiserror::Error)]
pub enum EditError {
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
    #[error("{outline:?} is not a valid steno outline: {source}")]
    InvalidOutline {
        outline: String,
        #[source]
        source: StrokeError,
    },
    #[error("{outline:?} is not in {path}")]
    NotFound { outline: String, path: PathBuf },
    /// The rewrite produced something other than the dictionary intended.
    /// Nothing has been written when this is returned.
    #[error("refusing to write {path}: {reason}")]
    VerificationFailed { path: PathBuf, reason: String },
}

/// What an edit did, enough to report it and to undo it.
#[derive(Debug)]
pub struct EditReport {
    pub path: PathBuf,
    /// The value that was there before, if the entry already existed.
    pub previous: Option<String>,
    /// Where the original was saved. `None` when nothing was written (setting an
    /// entry to the value it already has).
    pub backup: Option<PathBuf>,
}

/// Add a new entry, or change an existing one's translation.
pub fn set_entry(
    path: impl AsRef<Path>,
    outline: &str,
    translation: &str,
) -> Result<EditReport, EditError> {
    let path = path.as_ref().to_path_buf();

    // Reachable steno or nothing. See the module note and `clean`.
    Stroke::parse_outline(outline).map_err(|source| EditError::InvalidOutline {
        outline: outline.to_owned(),
        source,
    })?;

    let text = read(&path)?;
    let entries = parse(&path, &text)?;
    let previous = entries.get(outline).cloned();

    // Already exactly this. Do not rewrite the file or make a backup for a
    // no-op; an autosave-like editor could otherwise spray backups.
    if previous.as_deref() == Some(translation) {
        return Ok(EditReport {
            path,
            previous,
            backup: None,
        });
    }

    let updated = if previous.is_some() {
        replace_value(&text, outline, translation)
            .ok_or_else(|| verification_failed(&path, "could not locate the entry line to replace"))?
    } else {
        insert_entry(&text, outline, translation, entries.is_empty())
            .ok_or_else(|| verification_failed(&path, "could not find where to insert the entry"))?
    };

    let reparsed = parse_verify(&path, &updated)?;
    let expected = if previous.is_some() {
        entries.len()
    } else {
        entries.len() + 1
    };
    verify_untouched(&path, &entries, &reparsed, outline, expected)?;
    if reparsed.get(outline).map(String::as_str) != Some(translation) {
        return Err(verification_failed(
            &path,
            &format!("{outline:?} was not written correctly"),
        ));
    }

    let backup = write_with_backup(&path, &text, &updated)?;
    Ok(EditReport {
        path,
        previous,
        backup: Some(backup),
    })
}

/// Remove an entry. Errors if the outline is not present.
pub fn remove_entry(path: impl AsRef<Path>, outline: &str) -> Result<EditReport, EditError> {
    let path = path.as_ref().to_path_buf();

    let text = read(&path)?;
    let entries = parse(&path, &text)?;
    let Some(previous) = entries.get(outline).cloned() else {
        return Err(EditError::NotFound {
            outline: outline.to_owned(),
            path,
        });
    };

    let mut remove = BTreeMap::new();
    remove.insert(outline.to_owned(), previous.clone());
    let updated = strip_keys(&text, &remove);

    let reparsed = parse_verify(&path, &updated)?;
    verify_untouched(&path, &entries, &reparsed, outline, entries.len() - 1)?;
    if reparsed.contains_key(outline) {
        return Err(verification_failed(
            &path,
            &format!("{outline:?} is still present after removal"),
        ));
    }

    let backup = write_with_backup(&path, &text, &updated)?;
    Ok(EditReport {
        path,
        previous: Some(previous),
        backup: Some(backup),
    })
}

/// Check the count is right and that every entry other than the edited one is
/// byte-for-byte what it was.
fn verify_untouched(
    path: &Path,
    original: &BTreeMap<String, String>,
    reparsed: &BTreeMap<String, String>,
    edited: &str,
    expected_len: usize,
) -> Result<(), EditError> {
    if reparsed.len() != expected_len {
        return Err(verification_failed(
            path,
            &format!("expected {expected_len} entries, got {}", reparsed.len()),
        ));
    }
    for (key, value) in reparsed {
        if key == edited {
            continue;
        }
        if original.get(key) != Some(value) {
            return Err(verification_failed(
                path,
                &format!("entry {key:?} changed unexpectedly"),
            ));
        }
    }
    Ok(())
}

/// Replace only the value on the line defining `key`, keeping the indentation,
/// the key text and the trailing comma exactly.
fn replace_value(text: &str, key: &str, new_value: &str) -> Option<String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;

    for line in text.lines() {
        if !replaced
            && leading_json_string(line).as_deref() == Some(key)
            && let Some(new_line) = replace_line_value(line, new_value)
        {
            lines.push(new_line);
            replaced = true;
            continue;
        }
        lines.push(line.to_owned());
    }

    if !replaced {
        return None;
    }
    let mut out = lines.join(newline);
    if text.ends_with('\n') {
        out.push_str(newline);
    }
    Some(out)
}

/// Swap the value string of a single entry line, leaving everything else on the
/// line untouched.
fn replace_line_value(line: &str, new_value: &str) -> Option<String> {
    let (start, end) = value_span(line)?;
    let encoded = serde_json::to_string(new_value).ok()?;
    Some(format!("{}{encoded}{}", &line[..start], &line[end..]))
}

/// Byte range of the value string token on an entry line, including its quotes.
fn value_span(line: &str) -> Option<(usize, usize)> {
    let mut chars = line.char_indices().peekable();

    let skip_ws = |chars: &mut std::iter::Peekable<std::str::CharIndices>| {
        while let Some(&(_, c)) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
    };
    // The key string.
    skip_ws(&mut chars);
    if !matches!(chars.next(), Some((_, '"'))) {
        return None;
    }
    loop {
        match chars.next()? {
            (_, '\\') => {
                chars.next()?;
            }
            (_, '"') => break,
            _ => {}
        }
    }
    // The colon.
    skip_ws(&mut chars);
    if !matches!(chars.next(), Some((_, ':'))) {
        return None;
    }
    // The value string.
    skip_ws(&mut chars);
    let start = match chars.next()? {
        (i, '"') => i,
        _ => return None,
    };
    loop {
        match chars.next()? {
            (_, '\\') => {
                chars.next()?;
            }
            // The closing quote is one ASCII byte, so the value ends after it.
            (i, '"') => return Some((start, i + 1)),
            _ => {}
        }
    }
}

/// Insert a new entry as the first one, right after the opening brace, so the
/// trailing-comma rule (no comma before `}`) is only ever a concern when the
/// dictionary was empty.
fn insert_entry(text: &str, key: &str, value: &str, was_empty: bool) -> Option<String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();

    let brace = lines
        .iter()
        .position(|line| line.trim_start().starts_with('{'))?;
    let indent = detect_indent(&lines);
    let comma = if was_empty { "" } else { "," };
    let entry = format!(
        "{indent}{}: {}{comma}",
        serde_json::to_string(key).ok()?,
        serde_json::to_string(value).ok()?
    );
    lines.insert(brace + 1, entry);

    let mut out = lines.join(newline);
    if text.ends_with('\n') {
        out.push_str(newline);
    }
    Some(out)
}

/// The indentation existing entries use, so an inserted line matches them.
fn detect_indent(lines: &[String]) -> String {
    lines
        .iter()
        .find(|line| leading_json_string(line).is_some())
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_default()
}

fn read(path: &Path) -> Result<String, EditError> {
    std::fs::read_to_string(path).map_err(|source| EditError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn parse(path: &Path, text: &str) -> Result<BTreeMap<String, String>, EditError> {
    serde_json::from_str(text).map_err(|source| EditError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_verify(path: &Path, text: &str) -> Result<BTreeMap<String, String>, EditError> {
    serde_json::from_str(text)
        .map_err(|e| verification_failed(path, &format!("result is not valid JSON ({e})")))
}

fn write_with_backup(path: &Path, original: &str, updated: &str) -> Result<PathBuf, EditError> {
    let backup = sibling(path, &format!(".backup-{}.json", timestamp()));
    std::fs::write(&backup, original).map_err(|source| EditError::Write {
        path: backup.clone(),
        source,
    })?;
    std::fs::write(path, updated).map_err(|source| EditError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(backup)
}

fn verification_failed(path: &Path, reason: &str) -> EditError {
    EditError::VerificationFailed {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pluv-edit-{name}-{}.json", timestamp()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        let _ = name;
        path
    }

    fn read_map(path: &Path) -> BTreeMap<String, String> {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn adds_a_new_entry_as_the_first_line() {
        let path = temp("add", "{\n\"KAT\": \"cat\"\n}\n");
        let report = set_entry(&path, "PHO*EF", "move").unwrap();

        assert!(report.previous.is_none());
        assert!(report.backup.is_some());
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "{\n\"PHO*EF\": \"move\",\n\"KAT\": \"cat\"\n}\n");
        let map = read_map(&path);
        assert_eq!(map["PHO*EF"], "move");
        assert_eq!(map["KAT"], "cat");
    }

    #[test]
    fn changes_an_existing_value_and_keeps_the_rest_of_the_line() {
        let path = temp("change", "{\n  \"KAT\": \"cat\",\n  \"TKOG\": \"dog\"\n}\n");
        let report = set_entry(&path, "KAT", "kitten").unwrap();

        assert_eq!(report.previous.as_deref(), Some("cat"));
        let after = std::fs::read_to_string(&path).unwrap();
        // Indent, key and comma preserved; only the value changed.
        assert_eq!(after, "{\n  \"KAT\": \"kitten\",\n  \"TKOG\": \"dog\"\n}\n");
    }

    #[test]
    fn setting_an_entry_to_its_current_value_writes_nothing() {
        let contents = "{\n\"KAT\": \"cat\"\n}\n";
        let path = temp("noop", contents);
        let report = set_entry(&path, "KAT", "cat").unwrap();

        assert!(report.backup.is_none(), "no backup for a no-op");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn removes_an_entry_and_backs_it_up() {
        let path = temp("remove", "{\n\"KAT\": \"cat\",\n\"TKOG\": \"dog\"\n}\n");
        let report = remove_entry(&path, "KAT").unwrap();

        assert_eq!(report.previous.as_deref(), Some("cat"));
        let map = read_map(&path);
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("KAT"));
        // Reversible.
        let backup = std::fs::read_to_string(report.backup.unwrap()).unwrap();
        assert!(backup.contains("KAT"));
    }

    #[test]
    fn removing_the_last_entry_repairs_the_trailing_comma() {
        let path = temp("removelast", "{\n\"KAT\": \"cat\",\n\"TKOG\": \"dog\"\n}\n");
        remove_entry(&path, "TKOG").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\n\"KAT\": \"cat\"\n}\n");
    }

    #[test]
    fn a_value_with_quotes_and_meta_is_encoded_correctly() {
        let path = temp("meta", "{\n\"KAT\": \"cat\"\n}\n");
        set_entry(&path, "TKOG", "say \"hi\" {^ing}").unwrap();
        let map = read_map(&path);
        assert_eq!(map["TKOG"], "say \"hi\" {^ing}");
    }

    #[test]
    fn an_invalid_outline_is_refused_before_the_file_is_touched() {
        let contents = "{\n\"KAT\": \"cat\"\n}\n";
        let path = temp("bad", contents);
        assert!(matches!(
            set_entry(&path, "not valid steno!!", "x"),
            Err(EditError::InvalidOutline { .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }

    #[test]
    fn removing_a_missing_entry_is_an_error_not_a_silent_success() {
        let path = temp("missing", "{\n\"KAT\": \"cat\"\n}\n");
        assert!(matches!(
            remove_entry(&path, "TKOG"),
            Err(EditError::NotFound { .. })
        ));
    }

    #[test]
    fn adding_the_first_ever_entry_needs_no_trailing_comma() {
        let path = temp("empty", "{\n}\n");
        set_entry(&path, "KAT", "cat").unwrap();
        assert_eq!(read_map(&path).len(), 1);
    }
}
