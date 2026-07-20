//! Words written, and how fast.
//!
//! **Words are real words**, whitespace separated, not the
//! five-characters-is-a-word convention that typing tests use. Steno dictation
//! speeds are quoted in real words (a court reporting certification at 225 wpm
//! means 225 actual words), so counting the other way would show a number that
//! compares to nothing the user cares about.
//!
//! ## Why the rate is not simply "words in the last minute"
//!
//! A plain rolling average answers "how much did you write recently", which
//! decays to zero the moment you stop and punishes you for thinking. What a
//! writer actually wants to know is **how fast they write when they are
//! writing**, which is also what a dictation speed means.
//!
//! So idle time is excluded. Only intervals in which words appeared count
//! toward elapsed time, and each is capped at [`IDLE_SECONDS`] so a long pause
//! before a word does not inflate the denominator. At any steno pace the gap
//! between words is a fraction of a second, far below the cap, so the cap only
//! ever bites during a genuine pause.
//!
//! The number therefore holds steady while you think rather than collapsing.
//! Because a held number could be mistaken for a live one, [`Meter::is_idle`]
//! reports when writing has stopped and the status bar dims it.

use std::collections::VecDeque;

/// How far back the rate looks.
const WINDOW_SECONDS: f64 = 60.0;

/// A gap longer than this is thinking, not writing.
///
/// Three seconds is above any real inter-word gap: even 20 wpm is a word every
/// three seconds, and steno runs an order of magnitude faster. It is also short
/// enough that a pause is excluded promptly.
const IDLE_SECONDS: f64 = 3.0;

/// Samples closer together than this are not worth keeping. At 60 fps an
/// unthrottled sample per frame would be 3,600 entries a minute, all of them
/// describing the same writing.
const MIN_SAMPLE_GAP: f64 = 0.25;

#[derive(Default)]
pub struct Meter {
    /// `(seconds, cumulative words)`, oldest first.
    samples: VecDeque<(f64, usize)>,
    /// When the word count last went up.
    last_wrote: Option<f64>,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the document's word count at a moment in time.
    ///
    /// `now` is egui's frame time: monotonic seconds since the program started.
    pub fn observe(&mut self, now: f64, words: usize) {
        if let Some(&(_, previous)) = self.samples.back()
            && words > previous
        {
            self.last_wrote = Some(now);
        }

        match self.samples.back() {
            // Nothing changed and no time worth recording has passed.
            Some(&(when, count)) if count == words && now - when < MIN_SAMPLE_GAP => {}
            Some(&(when, _)) if now - when < MIN_SAMPLE_GAP => {
                // The count moved within the throttle window. Replace rather
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

    /// Has writing stopped? Callers use this to show the rate as stale rather
    /// than current.
    pub fn is_idle(&self, now: f64) -> bool {
        match self.last_wrote {
            None => true,
            Some(when) => now - when > IDLE_SECONDS,
        }
    }

    /// Words per minute while writing, or `None` before there is enough to
    /// divide by.
    ///
    /// `None` rather than zero when the window is too short to mean anything: a
    /// confident "0 wpm" a tenth of a second after starting is a worse answer
    /// than no answer.
    pub fn words_per_minute(&self) -> Option<u32> {
        let mut writing = 0.0f64;
        let mut written = 0usize;

        for (&(before, was), &(after, is)) in self.samples.iter().zip(self.samples.iter().skip(1)) {
            // Deleting must not produce a negative rate, and an interval with
            // no new words is not writing, so it contributes no time.
            let added = is.saturating_sub(was);
            if added == 0 {
                continue;
            }
            writing += (after - before).min(IDLE_SECONDS);
            written += added;
        }

        if writing < 1.0 {
            return None;
        }
        Some((written as f64 * 60.0 / writing).round() as u32)
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
        assert_eq!(
            meter.words_per_minute(),
            None,
            "half a second proves nothing"
        );
    }

    /// Writing steadily, sampled the way the UI samples it.
    #[test]
    fn a_steady_pace_reads_as_that_pace() {
        let mut meter = Meter::new();
        let mut now = 0.0;
        // 120 wpm is a word every half second.
        for word in 0..=120 {
            meter.observe(now, word);
            now += 0.5;
        }
        assert_eq!(meter.words_per_minute(), Some(120));
    }

    /// The point of the whole design: thinking must not read as slow writing.
    #[test]
    fn a_pause_to_think_does_not_lower_the_rate() {
        let mut meter = Meter::new();
        let mut now = 0.0;
        let mut words = 0;

        // Twenty words at 120 wpm.
        for _ in 0..20 {
            words += 1;
            meter.observe(now, words);
            now += 0.5;
        }
        let before = meter.words_per_minute().expect("a rate");

        // Fifteen seconds of staring out of the window, sampled all the while.
        for _ in 0..60 {
            now += 0.25;
            meter.observe(now, words);
        }
        let after = meter.words_per_minute().expect("a rate");

        assert_eq!(
            before, after,
            "the pause changed the rate from {before} to {after}"
        );
        assert!(meter.is_idle(now), "not writing, so it should read as idle");
    }

    #[test]
    fn writing_again_after_a_pause_stops_reading_as_idle() {
        let mut meter = Meter::new();
        meter.observe(0.0, 0);
        meter.observe(10.0, 1);
        assert!(!meter.is_idle(10.0));
        assert!(meter.is_idle(20.0));
        meter.observe(20.5, 2);
        assert!(!meter.is_idle(20.5));
    }

    #[test]
    fn nothing_written_yet_reads_as_idle() {
        let meter = Meter::new();
        assert!(meter.is_idle(0.0));
    }

    /// Old samples must leave, or the rate becomes a session average by stealth.
    #[test]
    fn writing_long_ago_stops_counting() {
        let mut meter = Meter::new();
        let mut now = 0.0;

        // A fast burst: 40 words at 240 wpm.
        for word in 1..=40 {
            now += 0.25;
            meter.observe(now, word);
        }
        // Two minutes later, writing at a gentler pace.
        now += 120.0;
        for word in 41..=80 {
            now += 1.0;
            meter.observe(now, word);
        }

        let rate = meter.words_per_minute().expect("a rate");
        assert_eq!(rate, 60, "the old burst is still being counted");
    }

    /// Deleting text must not produce a negative or nonsensical rate.
    #[test]
    fn deleting_a_paragraph_does_not_go_negative() {
        let mut meter = Meter::new();
        meter.observe(0.0, 500);
        meter.observe(1.0, 400);
        meter.observe(2.0, 300);
        // No interval added words, so there is nothing to divide.
        assert_eq!(meter.words_per_minute(), None);
    }

    /// Deleting mid session must not corrupt the rate of what follows.
    #[test]
    fn a_deletion_does_not_distort_later_writing() {
        let mut meter = Meter::new();
        let mut now = 0.0;

        for word in 1..=20 {
            now += 0.5;
            meter.observe(now, word);
        }
        // Delete half of it.
        now += 0.5;
        meter.observe(now, 10);
        // Carry on at the same pace.
        for word in 11..=30 {
            now += 0.5;
            meter.observe(now, word);
        }

        assert_eq!(meter.words_per_minute(), Some(120));
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

#[cfg(test)]
mod cost {
    use super::*;

    /// What counting words every frame would cost if it were not cached, on a
    /// document far longer than a day's writing.
    ///
    /// This is why `Document::revision` exists: the status bar recounts only
    /// when the text changes, so the figure below is paid per edit rather than
    /// sixty times a second.
    #[test]
    #[ignore = "measurement, not a pass/fail test"]
    fn counting_words_is_measured_rather_than_assumed() {
        // 45,000 words, roughly a 90 page transcript.
        let text = "the quick brown fox jumps over a lazy dog ".repeat(5_000);
        let words = count_words(&text);

        let started = std::time::Instant::now();
        let rounds = 200;
        let mut total = 0usize;
        for _ in 0..rounds {
            total += count_words(&text);
        }
        let each = started.elapsed() / rounds;

        println!("{words} words, {} bytes", text.len());
        println!("one count: {each:?}");
        println!(
            "if it ran every frame at 60 fps: {:.2}% of one core",
            each.as_secs_f64() * 60.0 * 100.0
        );
        assert_eq!(total, words * rounds as usize);
    }
}
