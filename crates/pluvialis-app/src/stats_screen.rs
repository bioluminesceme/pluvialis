//! Stats. Not built yet.
//!
//! Nothing is recorded for this yet. The meter holds a sixty second rolling
//! window and is reset whenever a document is opened, the tape is five hundred
//! lines of already-formatted text, and no stroke anywhere carries a timestamp.
//! So this screen needs new recording before it can show anything, which is
//! described in `PLAN.md`.

use eframe::egui;

const PLANNED: &str = "Planned: average words per minute over a day and over all time, the words \
                       you write most, the words you undo and rewrite most, and the strokes that \
                       have no entry at all.";

const THE_USEFUL_ONE: &str = "The last of those is the useful one. Every stroke with no entry \
                              gets a button that opens the Dictionary screen with the outline \
                              already filled in.";

const NOTHING_YET: &str = "Nothing is being recorded yet, so there is nothing to show and \
                           nothing has been written to disk.";

pub fn ui(ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(12.0);
        ui.heading("Stats");
        ui.add_space(6.0);
        ui.label("Not built yet.");
        ui.add_space(10.0);
        ui.label(egui::RichText::new(PLANNED).weak());
        ui.add_space(6.0);
        ui.label(egui::RichText::new(THE_USEFUL_ONE).weak());
        ui.add_space(6.0);
        ui.label(egui::RichText::new(NOTHING_YET).weak());
    });
}
