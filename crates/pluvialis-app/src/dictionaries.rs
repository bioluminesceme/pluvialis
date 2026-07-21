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

use pluvialis_core::{Dictionary, DictionaryStack, Stroke, remove_entry, set_entry};

/// One answer to a lookup, and which file it came from.
struct Hit {
    /// Index into the JSON dictionaries, so clicking a hit can edit it in place.
    dict_index: usize,
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

    // The editor.
    /// Which JSON dictionary a new entry is written to. Existing entries are
    /// edited in the dictionary they already live in.
    target: usize,
    edit_outline: String,
    edit_translation: String,
    /// The outcome of the last edit: `Ok` for a success line, `Err` for a
    /// refusal to show in red.
    edit_message: Option<Result<String, String>>,
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

    /// Returns whether the enabled state or order changed this frame, so the
    /// caller can persist it.
    pub fn ui(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) -> bool {
        ui.add_space(4.0);
        ui.strong("Dictionaries");
        ui.label(
            egui::RichText::new("Priority order, highest first")
                .small()
                .weak(),
        );
        ui.separator();

        let changed = self.list(ui, dictionaries);

        ui.add_space(8.0);
        ui.separator();
        ui.strong("Look up");
        self.lookup(ui, dictionaries);

        ui.add_space(8.0);
        ui.separator();
        ui.strong("Edit");
        self.editor(ui, dictionaries);

        changed
    }

    fn list(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) -> bool {
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
        changed
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

        // What a clicked result should load into the editor below:
        // (dictionary to target, outline, translation). Collected here and
        // applied after the loop, since the loop only borrows the results.
        let mut fill: Option<(Option<usize>, String, String)> = None;

        // Capped so the editor below stays on screen; the results scroll within.
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if !self.forward.is_empty() {
                    ui.add_space(4.0);
                    for hit in &self.forward {
                        ui.horizontal_wrapped(|ui| {
                            let value = egui::RichText::new(format!("{:?}", hit.value));
                            // The winning entry is what the translator uses;
                            // the rest are shadowed by priority order.
                            let value = match hit.winning {
                                true => value.strong(),
                                false => value.weak(),
                            };
                            if ui
                                .add(egui::Label::new(value).sense(egui::Sense::click()))
                                .on_hover_text("Click to load into the editor below")
                                .clicked()
                            {
                                fill = Some((
                                    Some(hit.dict_index),
                                    self.query.clone(),
                                    hit.value.clone(),
                                ));
                            }
                            ui.label(egui::RichText::new(&hit.dictionary).small().weak());
                        });
                    }
                }

                if !self.reverse.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Written as").small().weak());
                    for outline in &self.reverse {
                        if ui
                            .add(
                                egui::Label::new(egui::RichText::new(outline).monospace())
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Click to load into the editor below")
                            .clicked()
                        {
                            fill = Some((None, outline.clone(), self.query.clone()));
                        }
                    }
                }
            });

        if let Some((target, outline, translation)) = fill {
            if let Some(index) = target {
                self.target = index;
            }
            self.edit_outline = outline;
            self.edit_translation = translation;
            self.edit_message = None;
        }
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
                for (index, dictionary) in dictionaries.dictionaries().iter().enumerate() {
                    if let Some(value) = dictionary.lookup(&strokes) {
                        self.forward.push(Hit {
                            dict_index: index,
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

    /// Add, change or remove an entry. Writes go to Pluvialis's own dictionary
    /// copies, never the user's Plover folder, and every write backs up the
    /// file first and is verified before it lands; see `pluvialis_core::edit`.
    fn editor(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) {
        let names: Vec<String> = dictionaries
            .dictionaries()
            .iter()
            .map(|d| short_name(&d.path))
            .collect();
        if names.is_empty() {
            ui.label(
                egui::RichText::new("No editable dictionaries. Add a JSON one first.")
                    .small()
                    .weak(),
            );
            return;
        }
        if self.target >= names.len() {
            self.target = 0;
        }

        ui.horizontal(|ui| {
            ui.label("New entries in");
            egui::ComboBox::from_id_salt("edit-target")
                .selected_text(names[self.target].clone())
                .show_ui(ui, |ui| {
                    for (index, name) in names.iter().enumerate() {
                        ui.selectable_value(&mut self.target, index, name);
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Outline");
            ui.add(
                egui::TextEdit::singleline(&mut self.edit_outline)
                    .hint_text("PHO*EF")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Word   ");
            ui.add(
                egui::TextEdit::singleline(&mut self.edit_translation)
                    .hint_text("move")
                    .desired_width(f32::INFINITY),
            );
        });

        ui.horizontal(|ui| {
            let has_outline = !self.edit_outline.trim().is_empty();
            let can_save = has_outline && !self.edit_translation.is_empty();
            if ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .on_hover_text("Add the entry, or change its word if the outline already exists")
                .clicked()
            {
                self.apply_edit(dictionaries, EditKind::Set);
            }
            if ui
                .add_enabled(has_outline, egui::Button::new("Delete"))
                .on_hover_text("Remove this outline from the dictionary it lives in")
                .clicked()
            {
                self.apply_edit(dictionaries, EditKind::Remove);
            }
        });

        match &self.edit_message {
            Some(Ok(text)) => {
                ui.label(egui::RichText::new(text).small());
            }
            Some(Err(text)) => {
                ui.colored_label(ui.visuals().error_fg_color, egui::RichText::new(text).small());
            }
            None => {}
        }
    }

    fn apply_edit(&mut self, dictionaries: &mut DictionaryStack, kind: EditKind) {
        let outline = self.edit_outline.trim().to_owned();

        // Editing an existing entry writes to whichever dictionary it is in;
        // a new entry goes to the chosen target. Find the entry by parsing the
        // outline and seeing which dictionary answers.
        let owning = Stroke::parse_outline(&outline).ok().and_then(|strokes| {
            dictionaries
                .dictionaries()
                .iter()
                .position(|d| d.lookup(&strokes).is_some())
        });
        let index = match kind {
            // A delete must target the dictionary that actually holds it.
            EditKind::Remove => match owning {
                Some(index) => index,
                None => {
                    self.edit_message =
                        Some(Err(format!("{outline} is not in any editable dictionary")));
                    return;
                }
            },
            EditKind::Set => owning.unwrap_or(self.target),
        };

        let path = dictionaries.dictionaries()[index].path.clone();
        let name = short_name(&path);

        let result = match kind {
            EditKind::Set => set_entry(&path, &outline, &self.edit_translation).map(|_| {
                format!("Saved {outline} to {name}")
            }),
            EditKind::Remove => remove_entry(&path, &outline).map(|_| {
                format!("Removed {outline} from {name}")
            }),
        };

        match result {
            Ok(message) => {
                self.reload(dictionaries, index, &path);
                self.invalidate();
                self.edit_message = Some(Ok(message));
            }
            Err(e) => self.edit_message = Some(Err(e.to_string())),
        }
    }

    /// Reload one dictionary from disk after editing it, so the change is live
    /// immediately, keeping its enabled state and its place in the priority
    /// order.
    fn reload(&mut self, dictionaries: &mut DictionaryStack, index: usize, path: &std::path::Path) {
        match Dictionary::load(path) {
            Ok(mut reloaded) => {
                reloaded.enabled = dictionaries.dictionaries()[index].enabled;
                dictionaries.dictionaries_mut()[index] = reloaded;
            }
            Err(e) => {
                // The file on disk is the edited one and is correct; only the
                // in-memory copy is now stale, so say so rather than pretend.
                self.edit_message = Some(Err(format!(
                    "edited on disk, but reloading failed, restart to see it: {e}"
                )));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum EditKind {
    Set,
    Remove,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dictionary_is_named_by_its_file_rather_than_its_whole_path() {
        let path = std::path::Path::new(r"C:\Users\you\AppData\Local\plover\plover\cb.json");
        assert_eq!(short_name(path), "cb.json");
    }
}
