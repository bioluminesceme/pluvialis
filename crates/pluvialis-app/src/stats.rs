//! What was written, counted.
//!
//! Four things are recorded, each from the translator's own delta rather than
//! from the tape or the document. The delta says exactly what one stroke added
//! and what it took away, which is the only place the answer is unambiguous:
//! the tape is display text, and the document has the user's own typing mixed
//! in.
//!
//! - **Words written**, the output of every translated entry that appeared.
//! - **Strokes with no entry**, the outline of every untranslated one. This is
//!   the useful list: each row is a dictionary entry waiting to be added.
//! - **Words undone**, taken from what an undo stroke removed. Not "the
//!   previous word": the undo's own delta names what was withdrawn, and after a
//!   multi stroke translation those are different words.
//! - **Writing time and words**, so a lifetime words-per-minute can use the
//!   same definition the status bar uses. `Meter` measures a sixty second
//!   window and is reset whenever a document is opened, which is right for a
//!   live reading and wrong for a total, so the total accumulates here instead.
//!
//! ## Recording is checked before counting, not before showing
//!
//! `pluvialis-stats.json` holds the words she writes, in plain text, beside the
//! executable. With recording off nothing is counted and nothing is written, so
//! turning it off is a real answer to "do not keep this", not a hidden file
//! that merely goes unread. `Stats::clear` deletes the file as well as the
//! counts.
//!
//! ## Writing to disk
//!
//! On a timer and at shutdown, never per stroke: this would otherwise rewrite a
//! JSON file in the middle of the output path, several times a second, at exactly
//! the moment latency matters. Only the top [`KEPT_PER_LIST`] of each list is
//! written, because the tail of a word frequency list is one-count entries that
//! nobody reads and that grow the file without bound.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pluvialis_core::{Delta, Stroke};

/// How many of each list survives a save. A day of steno is a few thousand
/// distinct words, so this keeps everything worth looking at and drops the
/// long tail of things written once.
const KEPT_PER_LIST: usize = 2000;

/// How often the file is rewritten while the program runs.
const SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// A gap longer than this is thinking, not writing. The same three seconds the
/// live meter uses, so the two rates mean the same thing.
const IDLE_SECONDS: f64 = 3.0;

/// How far back the best-minute figure looks.
const PEAK_WINDOW: f64 = 60.0;

/// How much of that minute has to be actual writing before it counts as a best.
/// Without a floor, three quick strokes after a pause read as several hundred
/// words per minute and the record could never be beaten honestly.
const PEAK_MIN_WRITING: f64 = 30.0;

/// A counted list, largest first, as the screens want it.
pub type Ranked = Vec<(String, u64)>;

#[derive(Default)]
pub struct Stats {
    recording: bool,

    words: HashMap<String, u64>,
    untranslated: HashMap<String, u64>,
    undone: HashMap<String, u64>,

    /// Total strokes seen, including undos and untranslated ones.
    strokes: u64,
    /// Words written, and the seconds spent writing them with pauses excluded.
    /// The pair is what makes a rate; either alone is not comparable to
    /// anything.
    total_words: u64,
    writing_seconds: f64,

    /// The best sustained rate ever recorded, over a minute of writing.
    best_wpm: u32,
    /// When the last counted stroke arrived, so the gap to the next can be
    /// measured. `Instant`, not egui's frame time, so nothing has to be plumbed
    /// through the call chain to reach here.
    last_stroke: Option<Instant>,
    /// The last minute of `(when, seconds, words)`, for the best-minute figure.
    recent: VecDeque<(Instant, f64, usize)>,

    /// Whether anything has changed since the last save, so an idle program
    /// does no disk writes.
    dirty: bool,
    last_saved: Option<Instant>,
    /// Where it was loaded from, so the same file is written back. `None` in
    /// tests, which keeps them off the real one.
    path: Option<PathBuf>,
}

impl Stats {
    /// Read the file if recording is on. With recording off the file is left
    /// alone rather than deleted: turning the setting back on should not have
    /// silently cost her the history.
    pub fn load(recording: bool) -> Self {
        let path = crate::paths::base_dir().join("pluvialis-stats.json");
        let mut stats = match recording {
            true => Self::read(&path),
            false => Stats::default(),
        };
        stats.recording = recording;
        stats.path = Some(path);
        stats
    }

    fn read(path: &Path) -> Stats {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Stats::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            log::warn!("{} is not valid JSON, starting the counts over", path.display());
            return Stats::default();
        };

        let counts = |key: &str| -> HashMap<String, u64> {
            value
                .get(key)
                .and_then(|v| v.as_object())
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n)))
                        .collect()
                })
                .unwrap_or_default()
        };
        let number = |key: &str| value.get(key).and_then(serde_json::Value::as_u64);

        Stats {
            words: counts("words"),
            untranslated: counts("untranslated"),
            undone: counts("undone"),
            strokes: number("strokes").unwrap_or(0),
            best_wpm: number("best_wpm").unwrap_or(0) as u32,
            total_words: number("total_words").unwrap_or(0),
            writing_seconds: value
                .get("writing_seconds")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0),
            ..Stats::default()
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Turn recording on or off. Switching off stops counting immediately and
    /// leaves what is already on disk; use [`Stats::clear`] to remove it.
    pub fn set_recording(&mut self, recording: bool) {
        if self.recording == recording {
            return;
        }
        self.recording = recording;
        // Switching on mid session picks up the file rather than counting from
        // zero, so the totals do not jump backwards.
        if recording && let Some(path) = self.path.clone() {
            let mut loaded = Self::read(&path);
            loaded.recording = true;
            loaded.path = Some(path);
            *self = loaded;
        }
    }

    /// Forget everything and delete the file.
    pub fn clear(&mut self) {
        self.words.clear();
        self.untranslated.clear();
        self.undone.clear();
        self.strokes = 0;
        self.total_words = 0;
        self.writing_seconds = 0.0;
        self.best_wpm = 0;
        self.last_stroke = None;
        self.recent.clear();
        self.dirty = false;
        if let Some(path) = &self.path
            && path.exists()
            && let Err(e) = std::fs::remove_file(path)
        {
            log::warn!("could not delete {}: {e}", path.display());
        }
    }

    /// Count one stroke and what it did.
    ///
    /// `undo` is whether this stroke was the undo chord, which is what makes an
    /// otherwise ordinary removal a correction.
    pub fn record(&mut self, delta: &Delta, undo: bool) {
        if !self.recording {
            return;
        }
        self.strokes += 1;
        self.dirty = true;

        // The rate is measured here, from the strokes, rather than from the
        // document's word count. Most of what she writes is typed into another
        // program and never reaches the document at all, so a document based
        // rate reported 22 words against 2,468 strokes. The live meter in the
        // status bar still measures the document, because that is what it is
        // showing; this is the lifetime figure and has to count everything.
        let now = Instant::now();
        // A clock that went backwards, or the first stroke of the run: no gap
        // to measure, so this stroke starts the clock rather than counting.
        let gap = self
            .last_stroke
            .map(|then| now.duration_since(then).as_secs_f64().min(IDLE_SECONDS))
            .unwrap_or(0.0);
        self.last_stroke = Some(now);

        let mut words = 0usize;
        for translation in &delta.added {
            if !translation.is_untranslated() {
                words += crate::meter::count_words(&translation.output());
            }
        }
        if words > 0 {
            self.total_words += words as u64;
            self.writing_seconds += gap;
            self.recent.push_back((now, gap, words));
        }
        while let Some(&(when, _, _)) = self.recent.front() {
            match now.duration_since(when).as_secs_f64() > PEAK_WINDOW {
                true => self.recent.pop_front(),
                false => break,
            };
        }
        self.update_best();

        for translation in &delta.added {
            match translation.is_untranslated() {
                true => bump(&mut self.untranslated, &Stroke::render_outline(&translation.strokes)),
                false => bump(&mut self.words, &translation.output()),
            }
        }

        // What the undo took back, which is the correction. A removal that is
        // not an undo is the formatter replacing its own earlier output, for
        // instance a suffix stroke turning "run" into "running", and counting
        // that as a mistake would put her most fluent writing at the top of the
        // list.
        if undo && let Some(removed) = delta.removed.last() {
            bump(&mut self.undone, &removed.output());
        }
    }

    /// Raise the record if the last minute beat it.
    ///
    /// Only counted once at least [`PEAK_MIN_WRITING`] of the window was spent
    /// writing, so a short burst after a pause cannot set a record that honest
    /// writing can never beat.
    fn update_best(&mut self) {
        let writing: f64 = self.recent.iter().map(|&(_, seconds, _)| seconds).sum();
        if writing < PEAK_MIN_WRITING {
            return;
        }
        let words: usize = self.recent.iter().map(|&(_, _, words)| words).sum();
        let rate = (words as f64 * 60.0 / writing).round() as u32;
        self.best_wpm = self.best_wpm.max(rate);
    }

    /// The best sustained minute ever recorded, or `None` before one has been
    /// written.
    pub fn best_wpm(&self) -> Option<u32> {
        (self.best_wpm > 0).then_some(self.best_wpm)
    }

    pub fn strokes(&self) -> u64 {
        self.strokes
    }

    pub fn total_words(&self) -> u64 {
        self.total_words
    }

    /// Words per minute across everything recorded, or `None` before there is
    /// enough to divide by. Same definition as the status bar: words over time
    /// spent writing, with pauses excluded.
    pub fn words_per_minute(&self) -> Option<u32> {
        if self.writing_seconds < 60.0 {
            return None;
        }
        Some((self.total_words as f64 * 60.0 / self.writing_seconds).round() as u32)
    }

    /// Roughly how long she has spent writing, as recorded.
    pub fn writing_seconds(&self) -> f64 {
        self.writing_seconds
    }

    pub fn top_words(&self, count: usize) -> Ranked {
        ranked(&self.words, count)
    }

    pub fn top_untranslated(&self, count: usize) -> Ranked {
        ranked(&self.untranslated, count)
    }

    pub fn top_undone(&self, count: usize) -> Ranked {
        ranked(&self.undone, count)
    }

    pub fn is_empty(&self) -> bool {
        self.strokes == 0 && self.words.is_empty() && self.untranslated.is_empty()
    }

    /// Write the file if anything changed and the interval has elapsed. Called
    /// every frame; both checks make that cheap.
    pub fn save_if_due(&mut self) {
        if !self.dirty {
            return;
        }
        let due = match self.last_saved {
            None => true,
            Some(when) => when.elapsed() >= SAVE_INTERVAL,
        };
        if due {
            self.save();
        }
    }

    pub fn save(&mut self) {
        if !self.recording || !self.dirty {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        self.last_saved = Some(Instant::now());
        self.dirty = false;

        let document = serde_json::json!({
            "strokes": self.strokes,
            "best_wpm": self.best_wpm,
            "total_words": self.total_words,
            "writing_seconds": self.writing_seconds,
            "words": trimmed(&self.words),
            "untranslated": trimmed(&self.untranslated),
            "undone": trimmed(&self.undone),
        });
        match serde_json::to_string(&document) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    log::warn!("could not save stats to {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("could not serialise stats: {e}"),
        }
    }
}

fn bump(counts: &mut HashMap<String, u64>, key: &str) {
    // A word is trimmed because the formatter carries its leading space, and
    // "the" and " the" are the same word to anyone reading the list.
    let key = key.trim();
    if key.is_empty() {
        return;
    }
    *counts.entry(key.to_owned()).or_insert(0) += 1;
}

/// The `count` largest, then alphabetical so equal counts do not shuffle
/// between frames.
fn ranked(counts: &HashMap<String, u64>, count: usize) -> Ranked {
    let mut all: Vec<(String, u64)> = counts.iter().map(|(k, &v)| (k.clone(), v)).collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    all.truncate(count);
    all
}

/// The part of a list worth keeping on disk.
fn trimmed(counts: &HashMap<String, u64>) -> HashMap<String, u64> {
    if counts.len() <= KEPT_PER_LIST {
        return counts.clone();
    }
    ranked(counts, KEPT_PER_LIST).into_iter().collect()
}

/// The undo chord, the bare star. Recognised here rather than in `apply` so the
/// definition of "a correction" sits next to what it is counted for.
pub fn is_undo(stroke: Stroke) -> bool {
    Stroke::render_outline(&[stroke]) == "*"
}

#[cfg(test)]
mod tests {
    use super::*;
    use pluvialis_core::Translation;

    fn translated(text: &str) -> Translation {
        Translation::for_test(vec![Stroke::parse("TEFT").unwrap()], Some(text.to_owned()))
    }

    fn untranslated(outline: &str) -> Translation {
        Translation::for_test(vec![Stroke::parse(outline).unwrap()], None)
    }

    fn recording() -> Stats {
        Stats {
            recording: true,
            ..Stats::default()
        }
    }

    #[test]
    fn words_are_counted_as_they_are_written() {
        let mut stats = recording();
        for text in ["the", "cat", "the"] {
            stats.record(
                &Delta {
                    removed: Vec::new(),
                    added: vec![translated(text)],
                },
                false,
            );
        }
        assert_eq!(stats.top_words(10), vec![("the".into(), 2), ("cat".into(), 1)]);
        assert_eq!(stats.strokes(), 3);
    }

    #[test]
    fn a_stroke_with_no_entry_is_counted_by_its_outline() {
        let mut stats = recording();
        stats.record(
            &Delta {
                removed: Vec::new(),
                added: vec![untranslated("TKPWHR")],
            },
            false,
        );
        assert_eq!(stats.top_untranslated(10), vec![("TKPWHR".into(), 1)]);
        // It is not a word she wrote, so it must not also inflate that list.
        assert!(stats.top_words(10).is_empty());
    }

    /// The distinction the whole "undone" list rests on. Retroactive
    /// correction removes and re-adds constantly while writing well, so only
    /// the undo chord counts.
    #[test]
    fn only_an_undo_counts_as_a_correction() {
        let mut stats = recording();
        let replaced = Delta {
            removed: vec![translated("run")],
            added: vec![translated("running")],
        };
        stats.record(&replaced, false);
        assert!(stats.top_undone(10).is_empty());

        let undone = Delta {
            removed: vec![translated("running")],
            added: Vec::new(),
        };
        stats.record(&undone, true);
        assert_eq!(stats.top_undone(10), vec![("running".into(), 1)]);
    }

    #[test]
    fn the_bare_star_is_the_undo_chord() {
        assert!(is_undo(Stroke::parse("*").unwrap()));
        assert!(!is_undo(Stroke::parse("KA*T").unwrap()));
        assert!(!is_undo(Stroke::parse("KAT").unwrap()));
    }

    /// The off switch has to stop the counting, not just the showing. This is
    /// the test that makes the privacy claim in the module comment true.
    #[test]
    fn nothing_is_counted_while_recording_is_off() {
        let mut stats = Stats::default();
        assert!(!stats.is_recording());
        stats.record(
            &Delta {
                removed: Vec::new(),
                added: vec![translated("the")],
            },
            false,
        );
        assert!(stats.top_words(10).is_empty());
        assert_eq!(stats.strokes(), 0);
        assert_eq!(stats.total_words(), 0);
    }

    #[test]
    fn the_rate_uses_writing_time_rather_than_wall_clock() {
        let mut stats = recording();
        // Two minutes of writing, 240 words.
        stats.writing_seconds = 120.0;
        stats.total_words = 240;
        assert_eq!(stats.words_per_minute(), Some(120));
    }

    #[test]
    fn there_is_no_rate_until_there_is_enough_to_divide_by() {
        let mut stats = recording();
        stats.writing_seconds = 10.0;
        stats.total_words = 30;
        assert_eq!(stats.words_per_minute(), None);
    }

    /// A burst must not set a record that honest writing can never beat.
    #[test]
    fn a_short_burst_does_not_set_a_best_minute() {
        let mut stats = recording();
        let now = Instant::now();
        // Ten words in two seconds is 300 wpm, and proves nothing.
        stats.recent.push_back((now, 2.0, 10));
        stats.update_best();
        assert_eq!(stats.best_wpm(), None);
    }

    #[test]
    fn a_sustained_minute_sets_the_best() {
        let mut stats = recording();
        let now = Instant::now();
        // Forty seconds of writing, 80 words: 120 wpm.
        stats.recent.push_back((now, 40.0, 80));
        stats.update_best();
        assert_eq!(stats.best_wpm(), Some(120));

        // A slower minute afterwards does not lower the record.
        stats.recent.clear();
        stats.recent.push_back((now, 40.0, 40));
        stats.update_best();
        assert_eq!(stats.best_wpm(), Some(120));
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut stats = recording();
        stats.record(
            &Delta {
                removed: Vec::new(),
                added: vec![translated("the")],
            },
            false,
        );
        stats.writing_seconds = 120.0;
        stats.total_words = 240;
        stats.best_wpm = 150;
        stats.clear();
        assert!(stats.is_empty());
        assert_eq!(stats.words_per_minute(), None);
        assert_eq!(stats.best_wpm(), None);
    }

    #[test]
    fn only_the_top_of_a_list_is_kept_on_disk() {
        let mut counts = HashMap::new();
        for n in 0..KEPT_PER_LIST + 500 {
            counts.insert(format!("word{n}"), n as u64);
        }
        let kept = trimmed(&counts);
        assert_eq!(kept.len(), KEPT_PER_LIST);
        // The ones dropped are the ones written least.
        assert!(kept.contains_key(&format!("word{}", KEPT_PER_LIST + 499)));
        assert!(!kept.contains_key("word0"));
    }

    #[test]
    fn a_ranked_list_is_largest_first_then_alphabetical() {
        let mut counts = HashMap::new();
        counts.insert("beta".to_owned(), 2);
        counts.insert("alpha".to_owned(), 2);
        counts.insert("gamma".to_owned(), 5);
        assert_eq!(
            ranked(&counts, 10),
            vec![
                ("gamma".to_owned(), 5),
                ("alpha".to_owned(), 2),
                ("beta".to_owned(), 2),
            ]
        );
    }

    /// The formatter carries a leading space on most words, so counting the
    /// raw output would list "the" and " the" as two different words.
    #[test]
    fn the_space_a_word_carries_is_not_part_of_the_word() {
        let mut stats = recording();
        stats.record(
            &Delta {
                removed: Vec::new(),
                added: vec![translated(" the")],
            },
            false,
        );
        stats.record(
            &Delta {
                removed: Vec::new(),
                added: vec![translated("the")],
            },
            false,
        );
        assert_eq!(stats.top_words(10), vec![("the".into(), 2)]);
    }
}
