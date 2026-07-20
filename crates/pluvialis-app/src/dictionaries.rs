//! The dictionary pane: priority order, enable and disable, and lookup.
//!
//! Priority is the list order, highest first, and it is load bearing: three of
//! the outlines captured from the user's writer exist in both her dictionaries
//! and resolve differently depending on which sits on top (`SKP` is "and" in
//! the English one and "en" in the Dutch one). So reordering is a real editing
//! operation, not decoration.
//!
//! Nothing here writes to the dictionary files. Reordering and enabling change
//! only this session; persisting them is a separate step, and editing entries
//! is deliberately not implemented yet.

use eframe::egui;

use pluvialis_core::{DictionaryStack, Stroke};

/// One answer to a lookup, and which file it came from.
struct Hit {
    dictionary: String,
    value: String,
    /// Whether this is the one the translator would actually use.
    winning: bool,
}

#[derive(Default)]
pub struct DictionaryPane {
    query: String,
    /// Recomputed only when the query or the stack changes, since a reverse
    /// lookup walks every entry in every dictionary.
    forward: Vec<Hit>,
    reverse: Vec<String>,
    last_query: Option<String>,
    parse_error: Option<String>,
}

/// A short name for a dictionary, for the list.
fn short_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl DictionaryPane {
    pub fn new() -> Self {
        Self::default()
    }

    /// Force the next frame to recompute, after the stack changed underneath.
    fn invalidate(&mut self) {
        self.last_query = None;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) {
        ui.add_space(4.0);
        ui.strong("Dictionaries");
        ui.label(
            egui::RichText::new("Priority order, highest first")
                .small()
                .weak(),
        );
        ui.separator();

        self.list(ui, dictionaries);

        ui.add_space(8.0);
        ui.separator();
        ui.strong("Look up");
        self.lookup(ui, dictionaries);
    }

    fn list(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) {
        let count = dictionaries.dictionaries().len();
        // Applied after the loop: reordering mid iteration would renumber the
        // rows being drawn.
        let mut swap: Option<(usize, usize)> = None;
        let mut changed = false;

        for index in 0..count {
            ui.horizontal(|ui| {
                let entry = &mut dictionaries.dictionaries_mut()[index];
                if ui.checkbox(&mut entry.enabled, "").changed() {
                    changed = true;
                }

                let name = short_name(&entry.path);
                let entries = entry.len();
                let enabled = entry.enabled;

                let label = egui::RichText::new(name);
                let label = if enabled { label } else { label.weak() };
                ui.label(label).on_hover_text(format!(
                    "{}\n{entries} entries",
                    entry.path.display()
                ));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(index + 1 < count, egui::Button::new("v").small())
                        .on_hover_text("Lower priority")
                        .clicked()
                    {
                        swap = Some((index, index + 1));
                    }
                    if ui
                        .add_enabled(index > 0, egui::Button::new("^").small())
                        .on_hover_text("Higher priority")
                        .clicked()
                    {
                        swap = Some((index, index - 1));
                    }
                });
            });
        }

        if let Some((a, b)) = swap {
            dictionaries.dictionaries_mut().swap(a, b);
            changed = true;
        }

        // Dictionaries that compute their answers (Python). Consulted only
        // after every JSON one has missed, so they sit below the list rather
        // than in it, and they are off until the user turns them on.
        let programmatic = dictionaries.programmatic().len();
        if programmatic > 0 {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Programmatic, consulted last")
                    .small()
                    .weak(),
            );

            for index in 0..programmatic {
                ui.horizontal(|ui| {
                    let entry = &mut dictionaries.programmatic_mut()[index];
                    let mut enabled = entry.is_enabled();
                    if ui.checkbox(&mut enabled, "").changed() {
                        entry.set_enabled(enabled);
                        changed = true;
                    }
                    let label = egui::RichText::new(entry.name());
                    let label = if enabled { label } else { label.weak() };
                    ui.label(label).on_hover_text(
                        "Runs as written. A Python dictionary is not sandboxed, \
                         which is the same trust model as Plover.",
                    );
                });
            }
        }

        if changed {
            self.invalidate();
        }
    }

    fn lookup(&mut self, ui: &mut egui::Ui, dictionaries: &DictionaryStack) {
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("KAT, or a word")
                .desired_width(f32::INFINITY),
        );

        if response.changed() || self.last_query.as_deref() != Some(self.query.as_str()) {
            self.recompute(dictionaries);
        }

        if let Some(error) = &self.parse_error {
            ui.label(egui::RichText::new(error).small().weak());
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !self.forward.is_empty() {
                    ui.add_space(4.0);
                    for hit in &self.forward {
                        ui.horizontal_wrapped(|ui| {
                            let value = egui::RichText::new(format!("{:?}", hit.value));
                            // The winning entry is what the translator uses;
                            // the rest are shadowed by priority order.
                            ui.label(match hit.winning {
                                true => value.strong(),
                                false => value.weak(),
                            });
                            ui.label(
                                egui::RichText::new(&hit.dictionary)
                                    .small()
                                    .weak(),
                            );
                        });
                    }
                }

                if !self.reverse.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Written as").small().weak());
                    for outline in &self.reverse {
                        ui.monospace(outline);
                    }
                }
            });
    }

    fn recompute(&mut self, dictionaries: &DictionaryStack) {
        self.last_query = Some(self.query.clone());
        self.forward.clear();
        self.reverse.clear();
        self.parse_error = None;

        let query = self.query.trim();
        if query.is_empty() {
            return;
        }

        // An outline, if it parses as one. Most queries that are not steno fail
        // here, which is expected and not worth reporting: "cat" is a perfectly
        // good reverse lookup and a hopeless outline.
        match Stroke::parse_outline(query) {
            Ok(strokes) => {
                let winner = dictionaries.lookup(&strokes);
                for dictionary in dictionaries.dictionaries() {
                    if let Some(value) = dictionary.lookup(&strokes) {
                        self.forward.push(Hit {
                            dictionary: short_name(&dictionary.path),
                            value: value.to_owned(),
                            winning: dictionary.enabled && winner == Some(value),
                        });
                    }
                }
                if self.forward.is_empty() {
                    self.parse_error = Some("No entry for that outline".to_owned());
                }
            }
            Err(_) => {
                // Reverse lookup covers the "how do I write this word" case.
                for dictionary in dictionaries.dictionaries() {
                    for outline in dictionary.reverse_lookup(query) {
                        let rendered = Stroke::render_outline(outline);
                        if !self.reverse.contains(&rendered) {
                            self.reverse.push(rendered);
                        }
                    }
                }
                if self.reverse.is_empty() {
                    self.parse_error =
                        Some("Not valid steno, and no entry produces that text".to_owned());
                }
            }
        }

        // Shortest first: the brief is what the user wants to learn.
        self.reverse.sort_by_key(|outline| outline.len());
        self.reverse.truncate(20);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dictionary_is_named_by_its_file_rather_than_its_whole_path() {
        let path = std::path::Path::new(r"C:\Users\Corien\AppData\Local\plover\plover\cb.json");
        assert_eq!(short_name(path), "cb.json");
    }
}
