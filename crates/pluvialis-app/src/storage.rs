//! Saving documents: autosave, versioned snapshots, and crash recovery.
//!
//! Someone writing at speed can produce a great deal of text between thoughts
//! about saving, so nothing here depends on the user remembering to. The
//! document is saved on a timer and on losing focus, and every save that
//! differs from the last also writes a timestamped snapshot.
//!
//! Versioning without requiring git to be installed: snapshots are files under
//! `.pluvialis-history/<document>/`, thinned by age rather than count, so
//! recent work is recoverable minute by minute while a month of history still
//! costs little.
//!
//! Crash recovery uses the same snapshots. A marker file exists while the
//! program is running and is removed on a clean exit, so finding one at startup
//! means the last run ended badly and there may be work to recover.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often to autosave when the text has changed.
pub const DEFAULT_AUTOSAVE: Duration = Duration::from_secs(60);

const HISTORY_DIR: &str = ".pluvialis-history";
const RUNNING_MARKER: &str = ".pluvialis-running";

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StorageError + '_ {
    move |source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Milliseconds since the epoch, which is what snapshots are named by.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One saved version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub path: PathBuf,
    /// Milliseconds since the epoch, taken from the file name so the list can
    /// be built without reading any file metadata.
    pub at: u64,
}

/// How long ago, in words.
///
/// Relative rather than absolute because it answers the question actually being
/// asked of a history list ("how far back does this go") without needing a date
/// formatting dependency for a handful of labels.
pub fn how_long_ago(at: u64, now: u64) -> String {
    const MINUTE: u64 = 60 * 1000;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let age = now.saturating_sub(at);
    let plural = |n: u64, unit: &str| match n {
        1 => format!("1 {unit} ago"),
        n => format!("{n} {unit}s ago"),
    };

    match age {
        a if a < MINUTE => "just now".to_owned(),
        a if a < HOUR => plural(a / MINUTE, "minute"),
        a if a < DAY => plural(a / HOUR, "hour"),
        a => plural(a / DAY, "day"),
    }
}

/// Milliseconds since the epoch, for callers building a history view.
pub fn now() -> u64 {
    now_millis()
}

/// Parse `1784541960123.md` back into its timestamp.
fn snapshot_time(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.parse().ok()
}

/// Which snapshots to keep, given the newest first.
///
/// Thinned by age, not by count: everything from the last day, then one per
/// hour for a week, then one per day. Someone who wrote all morning can step
/// back through it minute by minute, while a year of history stays small.
///
/// The newest is always kept, whatever the arithmetic says, so a fresh snapshot
/// can never be discarded by the pass that just wrote it.
pub fn snapshots_to_keep(snapshots: &[Snapshot], now: u64) -> Vec<bool> {
    const HOUR: u64 = 60 * 60 * 1000;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;

    let mut keep = vec![false; snapshots.len()];
    let mut last_kept_bucket: Option<(u8, u64)> = None;

    for (index, snapshot) in snapshots.iter().enumerate() {
        let age = now.saturating_sub(snapshot.at);

        // Bucket by age band, then by position within that band, so one
        // snapshot survives per hour or per day as appropriate.
        let (band, bucket) = if age <= DAY {
            (0u8, snapshot.at) // every one
        } else if age <= WEEK {
            (1u8, snapshot.at / HOUR)
        } else {
            (2u8, snapshot.at / DAY)
        };

        let wanted = match last_kept_bucket {
            Some((last_band, last_bucket)) => (band, bucket) != (last_band, last_bucket),
            None => true,
        };

        if wanted || index == 0 {
            keep[index] = true;
            last_kept_bucket = Some((band, bucket));
        }
    }

    keep
}

pub struct Storage {
    documents_dir: PathBuf,
    /// The file the document is saved to, once it has a name.
    current: Option<PathBuf>,
    /// Whether the user has chosen a location (Save As), as opposed to the
    /// default untitled file. A plain Save on an unnamed document prompts.
    named: bool,
    /// What was last written, so an unchanged document costs no disk writes and
    /// produces no duplicate snapshots.
    last_saved: String,
    pub autosave_interval: Duration,
}

impl Storage {
    pub fn new(documents_dir: impl Into<PathBuf>) -> Self {
        Storage {
            documents_dir: documents_dir.into(),
            current: None,
            named: false,
            last_saved: String::new(),
            autosave_interval: DEFAULT_AUTOSAVE,
        }
    }

    pub fn documents_dir(&self) -> &Path {
        &self.documents_dir
    }

    pub fn current(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    /// The current file's name for display, e.g. `lecture.md`.
    pub fn current_file_name(&self) -> Option<String> {
        self.current
            .as_deref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Whether the user has picked a location, so a plain Save can write
    /// straight to it rather than having to ask where.
    pub fn is_named(&self) -> bool {
        self.named
    }

    /// Point autosave and Save at the default untitled file. Leaves the
    /// document unnamed, so the first Save still prompts for a real location.
    pub fn set_current(&mut self, path: impl Into<PathBuf>) {
        self.current = Some(path.into());
        self.named = false;
        // Unknown contents until the next save, so force one.
        self.last_saved = String::new();
    }

    /// Point autosave, Save and the version history at a user chosen file.
    /// Everything for the document then lives beside it; see `history_dir`.
    pub fn choose_target(&mut self, path: impl Into<PathBuf>) {
        self.current = Some(path.into());
        self.named = true;
        // A different file, whose contents we have not written, so force a save.
        self.last_saved = String::new();
    }

    /// Whether `text` differs from what is on disk.
    pub fn is_dirty(&self, text: &str) -> bool {
        text != self.last_saved
    }

    /// Where a document's snapshots live: beside the file itself, so saving to
    /// a folder the user picked takes the history there too rather than leaving
    /// it in the default documents folder. Falls back to the documents folder
    /// for a path with no parent (a bare file name).
    fn history_dir(&self, document: &Path) -> PathBuf {
        let name = document
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_owned());
        let base = document
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        match base {
            Some(base) => base.join(HISTORY_DIR).join(name),
            None => self.documents_dir.join(HISTORY_DIR).join(name),
        }
    }

    /// Write the document and, if it changed, a snapshot.
    ///
    /// Does nothing when the text matches what is already saved, so the timer
    /// can fire as often as it likes.
    pub fn save(&mut self, text: &str) -> Result<bool, StorageError> {
        if !self.is_dirty(text) {
            return Ok(false);
        }

        let path = match &self.current {
            Some(path) => path.clone(),
            None => {
                let path = self.documents_dir.join("untitled.md");
                self.current = Some(path.clone());
                path
            }
        };

        // The target's own folder, not the documents folder: a document saved
        // elsewhere must not resurrect the default one.
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        // UTF-8 throughout, which write_all of a &str guarantees.
        std::fs::write(&path, text).map_err(io(&path))?;

        let history = self.history_dir(&path);
        std::fs::create_dir_all(&history).map_err(io(&history))?;
        let snapshot = history.join(format!("{}.md", now_millis()));
        std::fs::write(&snapshot, text).map_err(io(&snapshot))?;

        self.last_saved = text.to_owned();
        self.prune(&history)?;
        Ok(true)
    }

    /// Every snapshot for the current document, newest first.
    pub fn snapshots(&self) -> Vec<Snapshot> {
        let Some(current) = &self.current else {
            return Vec::new();
        };
        let history = self.history_dir(current);

        let Ok(entries) = std::fs::read_dir(&history) else {
            return Vec::new();
        };

        let mut snapshots: Vec<Snapshot> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                snapshot_time(&path).map(|at| Snapshot { path, at })
            })
            .collect();

        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.at));
        snapshots
    }

    fn prune(&self, history: &Path) -> Result<(), StorageError> {
        let mut snapshots: Vec<Snapshot> = match std::fs::read_dir(history) {
            Ok(entries) => entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    snapshot_time(&path).map(|at| Snapshot { path, at })
                })
                .collect(),
            Err(_) => return Ok(()),
        };
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.at));

        let keep = snapshots_to_keep(&snapshots, now_millis());
        for (snapshot, keep) in snapshots.iter().zip(keep) {
            if !keep {
                // A snapshot that will not delete is not worth failing a save
                // over: the text is already safely written.
                if let Err(e) = std::fs::remove_file(&snapshot.path) {
                    log::debug!("could not prune {}: {e}", snapshot.path.display());
                }
            }
        }
        Ok(())
    }

    pub fn read_snapshot(&self, snapshot: &Snapshot) -> Result<String, StorageError> {
        std::fs::read_to_string(&snapshot.path).map_err(io(&snapshot.path))
    }

    // Crash recovery.

    fn marker(&self) -> PathBuf {
        self.documents_dir.join(RUNNING_MARKER)
    }

    /// Note that we are running, and report whether the previous run crashed.
    ///
    /// A marker left behind means the last run did not exit cleanly, so there
    /// may be unsaved work in the snapshots.
    pub fn begin_session(&self) -> Result<bool, StorageError> {
        std::fs::create_dir_all(&self.documents_dir).map_err(io(&self.documents_dir))?;
        let marker = self.marker();
        let crashed = marker.exists();
        std::fs::write(&marker, "running").map_err(io(&marker))?;
        Ok(crashed)
    }

    /// Record a clean exit.
    pub fn end_session(&self) {
        let marker = self.marker();
        if let Err(e) = std::fs::remove_file(&marker) {
            log::debug!("could not clear the running marker: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 60 * 60 * 1000;
    const DAY: u64 = 24 * HOUR;

    fn snapshot(at: u64) -> Snapshot {
        Snapshot {
            path: PathBuf::from(format!("{at}.md")),
            at,
        }
    }

    /// Count survivors, given absolute timestamps newest first.
    ///
    /// Deliberately takes absolute times rather than ages: the buckets are
    /// aligned to real hour and day boundaries, so a run of snapshots defined
    /// by age can straddle a boundary and keep two. Tests that want to pin down
    /// "one per hour" have to say which hour.
    fn kept(at: &[u64], now: u64) -> usize {
        let mut snapshots: Vec<Snapshot> = at.iter().map(|&at| snapshot(at)).collect();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.at));
        snapshots_to_keep(&snapshots, now)
            .into_iter()
            .filter(|&keep| keep)
            .count()
    }

    #[test]
    fn ages_are_described_in_words() {
        let now = 100 * DAY;
        assert_eq!(how_long_ago(now, now), "just now");
        assert_eq!(how_long_ago(now - 30_000, now), "just now");
        assert_eq!(how_long_ago(now - 60_000, now), "1 minute ago");
        assert_eq!(how_long_ago(now - 5 * 60_000, now), "5 minutes ago");
        assert_eq!(how_long_ago(now - HOUR, now), "1 hour ago");
        assert_eq!(how_long_ago(now - 3 * HOUR, now), "3 hours ago");
        assert_eq!(how_long_ago(now - DAY, now), "1 day ago");
        assert_eq!(how_long_ago(now - 9 * DAY, now), "9 days ago");
    }

    /// A snapshot dated in the future must not underflow into "584 million
    /// years ago", which is what subtracting the wrong way round produces.
    #[test]
    fn a_future_timestamp_is_described_as_just_now() {
        let now = 100 * DAY;
        assert_eq!(how_long_ago(now + 5000, now), "just now");
    }

    #[test]
    fn a_snapshot_filename_round_trips_to_its_timestamp() {
        assert_eq!(
            snapshot_time(Path::new("1784541960123.md")),
            Some(1784541960123)
        );
        assert_eq!(snapshot_time(Path::new("notatimestamp.md")), None);
        assert_eq!(snapshot_time(Path::new("untitled.md")), None);
    }

    /// Recent work must be recoverable minute by minute.
    #[test]
    fn everything_from_the_last_day_is_kept() {
        let now = 100 * DAY;
        let at: Vec<u64> = (0..20).map(|i| now - i * 60 * 1000).collect();
        assert_eq!(kept(&at, now), 20, "all within a day");
    }

    #[test]
    fn the_last_week_is_thinned_to_one_an_hour() {
        let now = 100 * DAY;
        // Six snapshots inside the single hour starting at day 98, which is two
        // days old: past the keep-everything window, inside the week.
        let hour = 98 * DAY;
        let at: Vec<u64> = (0..6).map(|i| hour + i * 10 * 60 * 1000).collect();
        assert_eq!(kept(&at, now), 1, "one per hour");
    }

    /// Two snapshots either side of an hour boundary are two hours as far as
    /// the policy is concerned, however close together they are.
    #[test]
    fn an_hour_boundary_separates_snapshots_even_when_they_are_minutes_apart() {
        let now = 100 * DAY;
        let hour = 98 * DAY;
        let at = [hour - 60_000, hour + 60_000];
        assert_eq!(kept(&at, now), 2);
    }

    #[test]
    fn older_than_a_week_is_thinned_to_one_a_day() {
        let now = 100 * DAY;
        // Five snapshots inside the single day at day 70, a month old.
        let day = 70 * DAY;
        let at: Vec<u64> = (0..5).map(|i| day + i * HOUR).collect();
        assert_eq!(kept(&at, now), 1, "one per day");
    }

    /// The pass that writes a snapshot must never then delete it.
    #[test]
    fn the_newest_snapshot_is_always_kept() {
        let now = 100 * DAY;
        let snapshots = vec![snapshot(now)];
        assert_eq!(snapshots_to_keep(&snapshots, now), vec![true]);

        // Even when it is somehow dated in the future.
        let snapshots = vec![snapshot(now + 5000)];
        assert_eq!(snapshots_to_keep(&snapshots, now), vec![true]);
    }

    #[test]
    fn an_empty_history_is_handled() {
        assert!(snapshots_to_keep(&[], 0).is_empty());
    }

    #[test]
    fn saving_writes_the_document_and_a_snapshot() {
        let dir = std::env::temp_dir().join(format!("pluvialis-test-{}", now_millis()));
        let mut storage = Storage::new(&dir);

        assert!(storage.save("hello").expect("save"), "first save writes");
        let document = storage.current().expect("named").to_path_buf();
        assert_eq!(std::fs::read_to_string(&document).unwrap(), "hello");
        assert_eq!(storage.snapshots().len(), 1);

        assert!(
            !storage.save("hello").expect("save"),
            "unchanged text writes nothing"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choosing_a_target_names_the_document_and_saves_there() {
        let dir = std::env::temp_dir().join(format!("pluvialis-target-{}", now_millis()));
        let elsewhere = dir.join("elsewhere");
        let mut storage = Storage::new(&dir);

        assert!(!storage.is_named(), "starts unnamed");
        assert_eq!(storage.current_file_name(), None);

        let target = elsewhere.join("lecture.md");
        storage.choose_target(&target);
        assert!(storage.is_named(), "choosing a target names it");
        assert_eq!(storage.current_file_name().as_deref(), Some("lecture.md"));

        assert!(storage.save("notes").expect("save"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "notes");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Saving to a folder the user picked must take the version history there,
    /// not leave it in the default documents folder.
    #[test]
    fn history_lives_beside_the_chosen_file() {
        let documents = std::env::temp_dir().join(format!("pluvialis-docs-{}", now_millis()));
        let chosen = documents.join("chosen");
        let mut storage = Storage::new(&documents);

        let target = chosen.join("lecture.md");
        storage.choose_target(&target);
        assert!(storage.save("first").expect("save"));
        assert!(
            storage.save("second").expect("save"),
            "changed text snapshots"
        );

        let history = chosen.join(HISTORY_DIR).join("lecture");
        assert!(history.is_dir(), "history sits beside the chosen file");
        assert_eq!(storage.snapshots().len(), 2);

        let stray = documents.join(HISTORY_DIR);
        assert!(
            !stray.exists(),
            "nothing written under the documents folder"
        );

        let _ = std::fs::remove_dir_all(&documents);
    }

    #[test]
    fn a_missing_marker_means_the_last_run_exited_cleanly() {
        let dir = std::env::temp_dir().join(format!("pluvialis-marker-{}", now_millis()));
        let storage = Storage::new(&dir);

        assert!(!storage.begin_session().expect("begin"), "first run");
        assert!(
            storage.begin_session().expect("begin"),
            "marker still there means the previous run crashed"
        );

        storage.end_session();
        assert!(!storage.begin_session().expect("begin"), "cleared on exit");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
