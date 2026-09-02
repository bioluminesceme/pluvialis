//! Settings. Not built yet.
//!
//! What lands here is listed in `PLAN.md`: autosave interval, documents folder,
//! tape length, document font size, whether output is on at launch, and the
//! dictionary priority order, which is not saved today.

use eframe::egui;

const PLANNED: &str = "Planned: autosave interval, where documents are saved, how much tape to \
                       keep, document font size, whether typing into other windows starts \
                       switched on, and the dictionary priority order.";

const KNOWN_BUG: &str = "Priority order is the one that matters. Reordering the dictionaries \
                         works, but the order is not saved, so it is lost when Pluvialis \
                         restarts.";

pub fn ui(ui: &mut egui::Ui) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(12.0);
        ui.heading("Settings");
        ui.add_space(6.0);
        ui.label("Not built yet.");
        ui.add_space(10.0);
        ui.label(egui::RichText::new(PLANNED).weak());
        ui.add_space(6.0);
        ui.label(egui::RichText::new(KNOWN_BUG).weak());
    });
}
