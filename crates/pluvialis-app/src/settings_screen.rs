//! Settings.
//!
//! Everything here is written to `pluvialis-config.json` beside the executable
//! the moment it changes, so there is no Save button and nothing to lose by
//! closing the window. See `config`.
//!
//! Two settings cannot take effect until the next start, and say so on screen
//! rather than only in a comment here: the documents folder, because the
//! current document is already open from the old one, and whether output starts
//! switched on, because that is a question about starting.
//!
//! The dictionary priority order is not on this screen. It is edited where the
//! dictionaries are, on the Dictionary screen, and is saved from there.

use std::path::Path;

use eframe::egui;

use crate::config::{AUTOSAVE_RANGE, FONT_RANGE, Settings, TAPE_RANGE};
use crate::stats::Stats;

/// Draw the screen. Returns whether anything changed, so the caller can write
/// the file and apply what needs applying.
pub fn ui(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    documents_dir: &Path,
    stats: &mut Stats,
) -> bool {
    let mut changed = false;

    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(10.0);
            ui.heading("Settings");
            ui.add_space(10.0);

            changed |= writing(ui, settings);
            ui.add_space(14.0);
            changed |= saving(ui, settings, documents_dir);
            ui.add_space(14.0);
            changed |= output(ui, settings);
            ui.add_space(14.0);
            changed |= statistics(ui, settings, stats);
            ui.add_space(14.0);
            where_things_live(ui, documents_dir);
        });
    });

    changed
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.strong(text);
    ui.add_space(4.0);
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().weak());
}

fn writing(ui: &mut egui::Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    heading(ui, "Writing");

    ui.horizontal(|ui| {
        ui.label("Document text size");
        changed |= ui
            .add(
                egui::Slider::new(&mut settings.font_size, FONT_RANGE)
                    .step_by(1.0)
                    .suffix(" pt"),
            )
            .changed();
    });
    note(ui, "The size of the text on the Home screen. Takes effect at once.");

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Tape lines kept");
        changed |= ui
            .add(egui::Slider::new(&mut settings.tape_limit, TAPE_RANGE).step_by(50.0))
            .changed();
    });
    note(
        ui,
        "How much of the tape stays scrollable. Every kept line is laid out each \
         frame, so a very long tape costs speed rather than memory.",
    );

    changed
}

fn saving(ui: &mut egui::Ui, settings: &mut Settings, documents_dir: &Path) -> bool {
    let mut changed = false;
    heading(ui, "Saving");

    ui.horizontal(|ui| {
        ui.label("Autosave every");
        changed |= ui
            .add(
                egui::Slider::new(&mut settings.autosave_seconds, AUTOSAVE_RANGE)
                    .suffix(" seconds"),
            )
            .changed();
    });
    note(
        ui,
        "Only when the text has changed. An unchanged document is never written.",
    );

    ui.add_space(8.0);
    ui.label("Documents folder");
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(documents_dir.display().to_string())
                .monospace()
                .small(),
        );
    });
    ui.horizontal(|ui| {
        if ui
            .button("Change...")
            .on_hover_text("Choose where documents and their history are saved")
            .clicked()
            && let Some(folder) = rfd::FileDialog::new()
                .set_title("Where should Pluvialis save documents?")
                .set_directory(documents_dir)
                .pick_folder()
        {
            settings.documents_dir = Some(folder);
            changed = true;
        }
        if settings.documents_dir.is_some()
            && ui
                .button("Use the default")
                .on_hover_text("The documents folder beside pluvialis-app.exe")
                .clicked()
        {
            settings.documents_dir = None;
            changed = true;
        }
        open_folder_button(ui, documents_dir);
    });
    note(
        ui,
        "Applies the next time Pluvialis starts. Nothing is moved: the documents \
         already saved stay where they are.",
    );

    changed
}

fn output(ui: &mut egui::Ui, settings: &mut Settings) -> bool {
    heading(ui, "Typing into other windows");
    let changed = ui
        .checkbox(
            &mut settings.output_at_launch,
            "Start with typing switched on",
        )
        .changed();
    note(
        ui,
        "The switch in the status bar is what turns it on and off while working. \
         This only decides where that switch starts.",
    );
    changed
}

fn statistics(ui: &mut egui::Ui, settings: &mut Settings, stats: &mut Stats) -> bool {
    heading(ui, "Statistics");

    let mut recording = settings.record_stats;
    let changed = ui
        .checkbox(&mut recording, "Count what I write")
        .on_hover_text("Feeds the Stats screen. Off means nothing is counted at all.")
        .changed();
    if changed {
        settings.record_stats = recording;
        stats.set_recording(recording);
    }

    note(
        ui,
        "This is counted on your machine and written to pluvialis-stats.json \
         beside the program. It holds the words you write, so switching this off \
         stops the counting itself rather than only hiding the screen.",
    );

    ui.add_space(6.0);
    if ui
        .add_enabled(!stats.is_empty(), egui::Button::new("Delete what has been counted"))
        .on_hover_text("Forget every count and delete pluvialis-stats.json")
        .clicked()
    {
        stats.clear();
    }

    changed
}

fn where_things_live(ui: &mut egui::Ui, documents_dir: &Path) {
    let base = crate::paths::base_dir();
    heading(ui, "Where things are kept");
    note(
        ui,
        "Everything is beside the program in plain files, so it can be found, \
         backed up and edited with ordinary tools.",
    );
    ui.add_space(4.0);
    for (what, where_) in [
        ("Dictionaries", base.join("dictionaries")),
        ("Documents", documents_dir.to_path_buf()),
        ("Settings", base.join("pluvialis-config.json")),
        ("Statistics", base.join("pluvialis-stats.json")),
    ] {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{what}:")).small());
            ui.label(
                egui::RichText::new(where_.display().to_string())
                    .monospace()
                    .small()
                    .weak(),
            );
        });
    }
    ui.add_space(4.0);
    open_folder_button(ui, &base);
}

/// Open a folder in the file manager. Windows only, because that is the only
/// platform Pluvialis runs on today and there is no portable way to do it.
fn open_folder_button(ui: &mut egui::Ui, folder: &Path) {
    #[cfg(windows)]
    if ui.button("Open folder").clicked()
        && let Err(e) = std::process::Command::new("explorer")
            .arg(folder)
            .spawn()
    {
        log::warn!("could not open {}: {e}", folder.display());
    }
    #[cfg(not(windows))]
    let _ = (ui, folder);
}
