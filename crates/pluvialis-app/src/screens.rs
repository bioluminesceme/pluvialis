//! The four screens, and the one rule that decides where a stroke goes.
//!
//! Screens are mutually exclusive: only one occupies the middle of the window
//! at a time. That is a change from the earlier layout, where the dictionaries
//! were a narrow panel beside the always-visible document, and it is what makes
//! room for a table of a hundred thousand entries.
//!
//! It also means the visible screen is now an input to stroke routing, which it
//! never was before, so [`sink`] is kept here as a small pure function with its
//! own tests rather than buried in the middle of the view.

use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Screen {
    #[default]
    Home,
    Dictionary,
    Settings,
    Stats,
}

impl Screen {
    pub fn all() -> [Screen; 4] {
        [
            Screen::Home,
            Screen::Dictionary,
            Screen::Settings,
            Screen::Stats,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Screen::Home => "Home",
            Screen::Dictionary => "Dictionary",
            Screen::Settings => "Settings",
            Screen::Stats => "Stats",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Screen::Home => "Write here, and watch the tape",
            Screen::Dictionary => "Browse, search and edit your dictionaries",
            Screen::Settings => "Not built yet",
            Screen::Stats => "Not built yet",
        }
    }

    /// Whether the tape strip is worth its width on this screen.
    ///
    /// The tape is the only place a stroke is visible once it has gone
    /// somewhere unexpected, so it stays on the Dictionary screen even though
    /// the document is not showing there.
    pub fn shows_tape(self) -> bool {
        matches!(self, Screen::Home | Screen::Dictionary)
    }
}

/// Where a batch of strokes goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sink {
    /// Typed into whatever program has focus, as real keystrokes.
    OtherWindow,
    /// Translated into the Home document.
    #[default]
    Document,
    /// Straight into a focused text field as raw steno, untranslated.
    Field,
}

/// Decide where the next batch of strokes goes.
///
/// The unfocused case is answered first and without consulting the screen. That
/// ordering is the whole rule: this program exists so that steno keeps reaching
/// the program the user is actually typing into, and which screen happens to be
/// showing in a window she is not looking at has nothing to do with it. Making
/// the screen relevant there would break the one thing the app must never get
/// wrong.
///
/// Strokes are never dropped. On Settings and Stats, where nothing claims them,
/// they still reach the document, and the tape says so. A stroke that landed in
/// the document is visible the moment Home is opened and the undo chord takes
/// it back; a stroke that was silently discarded is gone.
pub fn sink(focused: bool, screen: Screen, field_wants_strokes: bool) -> Sink {
    if !focused {
        return Sink::OtherWindow;
    }
    match screen {
        Screen::Dictionary if field_wants_strokes => Sink::Field,
        _ => Sink::Document,
    }
}

/// The row of screen buttons across the top of the window.
pub fn nav_bar(ui: &mut egui::Ui, screen: &mut Screen) {
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        for candidate in Screen::all() {
            let selected = *screen == candidate;
            if ui
                .selectable_label(selected, candidate.label())
                .on_hover_text(candidate.hint())
                .clicked()
            {
                *screen = candidate;
            }
        }
    });
    ui.add_space(3.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_reaches_another_program_from_every_screen() {
        // The rule the app exists for. If this ever fails, steno stops
        // arriving in the program she is writing into.
        for screen in Screen::all() {
            assert_eq!(sink(false, screen, false), Sink::OtherWindow);
            assert_eq!(
                sink(false, screen, true),
                Sink::OtherWindow,
                "a focused field in a window that is not focused claims nothing"
            );
        }
    }

    #[test]
    fn a_focused_home_screen_writes_into_the_document() {
        assert_eq!(sink(true, Screen::Home, false), Sink::Document);
    }

    #[test]
    fn a_focused_dictionary_field_takes_the_stroke() {
        assert_eq!(sink(true, Screen::Dictionary, true), Sink::Field);
    }

    #[test]
    fn strokes_are_never_dropped_on_a_screen_that_wants_none() {
        assert_eq!(sink(true, Screen::Dictionary, false), Sink::Document);
        assert_eq!(sink(true, Screen::Settings, false), Sink::Document);
        assert_eq!(sink(true, Screen::Stats, false), Sink::Document);
    }

    #[test]
    fn the_tape_follows_the_screens_where_strokes_are_the_subject() {
        assert!(Screen::Home.shows_tape());
        assert!(Screen::Dictionary.shows_tape());
        assert!(!Screen::Settings.shows_tape());
        assert!(!Screen::Stats.shows_tape());
    }
}
