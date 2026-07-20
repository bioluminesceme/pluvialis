//! Words written, and how fast.
//!
//! **Words are real words**, whitespace separated, not the five-characters-is-a-word
//! convention that typing tests use. Steno dictation speeds are quoted in real
//! words (a court reporting certification at 225 wpm means 225 actual words), so
//! counting the other way would show the user a number that does not compare to
//! any figure she cares about.
//!
//! The rate is measured over a rolling window rather than the whole session, so
//! it shows current speed and decays when writing stops. A session average would
//! be permanently dragged down by every interruption.

use std::collections::VecDeque;

/// How far back the rate looks. Long enough that a pause to think does not
/// read as zero, short enough to reflect the last minute rather than the hour.
const WINDOW_SECONDS: f64 = 60.0;

/// Samples closer together than this are not worth keeping. At 60 fps an
/// unthrottled sample per frame would be 3,600 entries a minute, all of them
/// describing the same writing.
const MIN_SAMPLE_GAP: f64 = 0.25;

#[derive(Default)]
pub struct Meter {
    /// `(seconds, cumulative words)`, oldest first.
    samples: VecDeque<(f64, usize)>,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the document's word count at a moment in time.
    ///
    /// `now` is egui's frame time, which is monotonic seconds since start.
    pub fn observe(&mut self, now: f64, words: usize) {
        match self.samples.back() {
            // Nothing changed and no time worth recording has passed.
            Some(&(when, count)) if count == words && now - when < MIN_SAMPLE_GAP => {}
            Some(&(when, _)) if now - when < MIN_SAMPLE_GAP => {
                // Word count moved within the throttle window. Replace rather
                // than drop, so a fast burst is not undercounted.
                self.samples.pop_back();
                self.samples.push_back((now, words));
            }
            _ => self.samples.push_back((now, words)),
        }

        while let Some(&(when, _)) = self.samples.front() {
            if now - when > WINDOW_SECONDS {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Words per minute across the window, or `None` before there is enough to
    /// divide by.
    ///
    /// Returns `None` rather than zero when the window is too short to mean
    /// anything: a confident "0 wpm" a tenth of a second after starting is a
    /// worse answer than no answer.
    pub fn words_per_minute(&self) -> Option<u32> {
        let (first, first_words) = *self.samples.front()?;
        let (last, last_words) = *self.samples.back()?;

        let elapsed = last - first;
        if elapsed < 1.0 {
            return None;
        }

        // Deleting a paragraph must not produce a negative rate, and clearing
        // the document must not produce a huge one when text comes back.
        let written = last_words.saturating_sub(first_words);
        Some((written as f64 * 60.0 / elapsed).round() as u32)
    }
}

/// Words in a document: runs of non-whitespace.
///
/// `split_whitespace` rather than `split(' ')`, so newlines and runs of spaces
/// do not each count as a word.
pub fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_runs_of_non_whitespace() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   \n  "), 0);
        assert_eq!(count_words("one"), 1);
        assert_eq!(count_words("one two  three\nfour"), 4);
        // Punctuation attaches to its word rather than becoming one.
        assert_eq!(count_words("Hello, world!"), 2);
    }

    #[test]
    fn there_is_no_rate_until_there_is_enough_to_divide_by() {
        let mut meter = Meter::new();
        assert_eq!(meter.words_per_minute(), None);
        meter.observe(0.0, 0);
        assert_eq!(meter.words_per_minute(), None);
        meter.observe(0.5, 5);
        assert_eq!(meter.words_per_minute(), None, "half a second proves nothing");
    }

    #[test]
    fn a_steady_pace_reads_as_that_pace() {
        let mut meter = Meter::new();
        // 60 words over 60 seconds, sampled once a second.
        for second in 0..=60 {
            meter.observe(second as f64, second as usize);
        }
        assert_eq!(meter.words_per_minute(), Some(60));
    }

    #[test]
    fn a_realistic_steno_pace_is_reported_as_written() {
        let mut meter = Meter::new();
        // 200 wpm for thirty seconds is 100 words.
        meter.observe(0.0, 0);
        meter.observe(30.0, 100);
        assert_eq!(meter.words_per_minute(), Some(200));
    }

    /// Old samples must leave, or the rate becomes a session average by
    /// stealth and never recovers from a pause.
    #[test]
    fn writing_long_ago_stops_counting() {
        let mut meter = Meter::new();
        meter.observe(0.0, 0);
        meter.observe(10.0, 100); // a fast burst
        // Two minutes later, writing slowly.
        for second in 130..=160 {
            meter.observe(second as f64, 100 + (second - 130));
        }
        let rate = meter.words_per_minute().expect("a rate");
        assert!(rate < 100, "the old burst is still counted: {rate}");
    }

    /// Deleting text must not produce a negative or nonsensical rate.
    #[test]
    fn deleting_a_paragraph_does_not_go_negative() {
        let mut meter = Meter::new();
        meter.observe(0.0, 500);
        meter.observe(30.0, 20);
        assert_eq!(meter.words_per_minute(), Some(0));
    }

    /// At 60 fps this is called every frame. It must not grow without bound.
    #[test]
    fn sampling_every_frame_does_not_accumulate_forever() {
        let mut meter = Meter::new();
        let mut now = 0.0;
        for frame in 0..(60 * 120) {
            now += 1.0 / 60.0;
            meter.observe(now, frame / 60);
        }
        assert!(
            meter.samples.len() <= (WINDOW_SECONDS / MIN_SAMPLE_GAP) as usize + 2,
            "kept {} samples",
            meter.samples.len()
        );
    }
}
