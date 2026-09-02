//! Stats: what has been written, counted.
//!
//! The numbers come from `stats`, which counts from the translator's own delta
//! as each stroke is applied. Nothing is computed here; this only draws.
//!
//! The strokes with no entry are the point of the screen. Each row has an
//! **Add entry** button that opens the Dictionary screen with the outline
//! already in the editor, which turns a statistic into a dictionary entry in one
//! click. That is why this screen and the Dictionary screen were designed
//! together.

use eframe::egui;

use crate::screens::thousands;
use crate::stats::{Ranked, Stats};

/// How many rows each list shows. Enough to be worth reading, short enough that
/// the three lists fit side by side without scrolling.
const ROWS: usize = 15;

/// Draw the screen. Returns an outline she asked to add a dictionary entry for,
/// which the caller turns into a screen change.
pub fn ui(ui: &mut egui::Ui, stats: &Stats) -> Option<String> {
    let mut add: Option<String> = None;

    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(10.0);
            ui.heading("Stats");
            ui.add_space(8.0);

            if !stats.is_recording() {
                ui.label("Counting is switched off, so nothing is being recorded.");
                ui.label(
                    egui::RichText::new("Settings has the switch that turns it back on.")
                        .small()
                        .weak(),
                );
                return;
            }
            if stats.is_empty() {
                ui.label("Nothing counted yet. Write something and it will show up here.");
                return;
            }

            totals(ui, stats);
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(10.0);

            add = lists(ui, stats);
        });
    });

    add
}

fn totals(ui: &mut egui::Ui, stats: &Stats) {
    ui.horizontal(|ui| {
        let rate = match stats.words_per_minute() {
            Some(wpm) => format!("{wpm}"),
            // Not zero. A confident 0 wpm before there is enough to divide by
            // is a worse answer than no answer, which is the same rule the
            // status bar follows.
            None => "-".to_owned(),
        };
        figure(ui, &rate, "words per minute");
        let best = match stats.best_wpm() {
            Some(wpm) => format!("{wpm}"),
            None => "-".to_owned(),
        };
        figure(ui, &best, "best minute");
        figure(ui, &thousands(stats.total_words()), "words written");
        figure(ui, &thousands(stats.strokes()), "strokes");
        figure(ui, &duration(stats.writing_seconds()), "spent writing");
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Both rates count every word you write, including what goes into other programs, and exclude the time spent thinking. Best minute is the fastest full minute ever recorded. It only counts once at least half that minute was spent writing, so a short burst cannot set a record you could never beat.",
        )
        .small()
        .weak(),
    );
}

/// One big number with its label under it.
fn figure(ui: &mut egui::Ui, value: &str, label: &str) {
    ui.allocate_ui(egui::vec2(150.0, 52.0), |ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(value).size(26.0).strong());
            ui.label(egui::RichText::new(label).small().weak());
        });
    });
}

fn lists(ui: &mut egui::Ui, stats: &Stats) -> Option<String> {
    let mut add = None;

    ui.columns(3, |columns| {
        list(
            &mut columns[0],
            "Written most",
            "The words you write most often. The easier stroke should belong to \
             the word higher up this list.",
            &stats.top_words(ROWS),
        );

        list(
            &mut columns[1],
            "Undone most",
            "What you took back with the undo stroke. A word near the top is one \
             worth a second look, either in the dictionary or in the fingers.",
            &stats.top_undone(ROWS),
        );

        add = untranslated(&mut columns[2], &stats.top_untranslated(ROWS));
    });

    add
}

fn list(ui: &mut egui::Ui, title: &str, explanation: &str, rows: &Ranked) {
    ui.strong(title);
    ui.label(egui::RichText::new(explanation).small().weak());
    ui.add_space(6.0);

    if rows.is_empty() {
        ui.label(egui::RichText::new("Nothing yet.").small().weak());
        return;
    }
    for (text, count) in rows {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(count.to_string()).small().weak());
            ui.label(text);
        });
    }
}

fn untranslated(ui: &mut egui::Ui, rows: &Ranked) -> Option<String> {
    let mut add = None;

    ui.strong("No entry yet");
    ui.label(
        egui::RichText::new(
            "Strokes that found nothing, so they came out as red steno. Add is \
             the one to use: it opens the Dictionary screen with the outline \
             already filled in.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    if rows.is_empty() {
        ui.label(
            egui::RichText::new("Nothing yet. Every stroke found an entry.")
                .small()
                .weak(),
        );
        return None;
    }

    for (outline, count) in rows {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(count.to_string()).small().weak());
            ui.label(egui::RichText::new(outline).monospace());
            if ui
                .small_button("Add")
                .on_hover_text(format!("Start a dictionary entry for {outline}"))
                .clicked()
            {
                add = Some(outline.clone());
            }
        });
    }

    add
}

/// Writing time, as hours and minutes. Seconds only while it is under a minute,
/// because "2h 14m" is the readable form and "8078s" is not.
fn duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    match (hours, minutes) {
        (0, 0) => format!("{total}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_time_reads_as_hours_and_minutes() {
        assert_eq!(duration(0.0), "0s");
        assert_eq!(duration(45.0), "45s");
        assert_eq!(duration(90.0), "1m");
        assert_eq!(duration(3600.0), "1h 0m");
        assert_eq!(duration(8078.0), "2h 14m");
    }
}
