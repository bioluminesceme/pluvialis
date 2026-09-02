//! The dictionary pane: priority order, enable and disable, and lookup.
//!
//! Priority is the list order, highest first, and it is load bearing: three of
//! the outlines captured from the user's writer exist in both her dictionaries
//! and resolve differently depending on which sits on top (`SKP` is "and" in
//! the English one and "en" in the Dutch one). So reordering is a real editing
//! operation, not decoration.
//!
//! Reordering and enabling change only this session; persisting them is a
//! separate step. Editing entries writes through `pluvialis_core::edit`.
//!
//! A lookup answers both questions at once: what an outline means, and every
//! way a word can be written. Both directions run for every query, because a
//! great many words are also valid steno ("the", "to", "pro", "hat"), and
//! answering only the direction the query happens to parse as hides the other
//! one with no sign that it was skipped.

use std::path::{Path, PathBuf};

use eframe::egui;

use pluvialis_core::{Dictionary, DictionaryStack, Stroke, move_entry, remove_entry, set_entry};

/// One entry a lookup found, and the file it lives in.
struct Hit {
    /// Which file to edit. A path rather than an index, because the list can be
    /// reordered between finding the entry and clicking it.
    path: PathBuf,
    dictionary: String,
    /// Canonical rendering. The file may spell its key differently (`TK-LS` for
    /// `TKLS`); `pluvialis_core::edit` resolves that when it writes.
    outline: String,
    value: String,
    /// Whether this is the entry the translator would actually use for these
    /// strokes. The others are shadowed by priority order, or disabled.
    winning: bool,
}

/// The entry the editor is working on, once one has been loaded from a result.
///
/// Held separately from the text fields so the fields can be edited, or
/// cleared, without losing which entry is being changed. That is what makes
/// changing an outline a move rather than an add.
#[derive(Clone)]
struct Loaded {
    path: PathBuf,
    dictionary: String,
    /// The outline as it was when the entry was loaded, which is what a move
    /// starts from.
    outline: String,
}

#[derive(Default)]
pub struct DictionaryPane {
    query: String,
    /// Recomputed only when the query or the stack changes, since a reverse
    /// lookup walks every entry in every dictionary.
    forward: Vec<Hit>,
    reverse: Vec<Hit>,
    last_query: Option<String>,
    parse_error: Option<String>,

    // The editor.
    /// Which JSON dictionary a brand new entry goes into. Ignored once an
    /// existing entry is loaded, which is edited where it already lives.
    target: usize,
    loaded: Option<Loaded>,
    edit_outline: String,
    edit_translation: String,
    outline_focused: bool,
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

/// Whether `path` is the dictionary the translator answers these strokes from.
///
/// Not "holds the same text as the winner": two dictionaries can hold the same
/// translation, and only one of them is the one being used.
fn is_winner(dictionaries: &DictionaryStack, path: &Path, strokes: &[Stroke]) -> bool {
    dictionaries
        .dictionaries()
        .iter()
        .find(|d| d.enabled && d.lookup(strokes).is_some())
        .is_some_and(|d| d.path == path)
}

impl DictionaryPane {
    pub fn new() -> Self {
        Self::default()
    }

    /// Force the next frame to recompute, after the stack changed underneath.
    fn invalidate(&mut self) {
        self.last_query = None;
    }

    pub fn accept_raw_outline(&mut self, stroke: Stroke) -> bool {
        if !self.outline_focused {
            return false;
        }

        let outline = Stroke::render_outline(&[stroke]);
        if !self.edit_outline.trim().is_empty() && !self.edit_outline.ends_with('/') {
            self.edit_outline.push('/');
        }
        self.edit_outline.push_str(&outline);
        self.edit_message = None;
        true
    }

    /// Returns whether the enabled state or order changed this frame, so the
    /// caller can persist it.
    pub fn ui(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) -> bool {
        self.outline_focused = false;
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
                ui.label(label)
                    .on_hover_text(format!("{}\n{entries} entries", entry.path.display()));

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

        // Which entry a clicked row should load into the editor. Collected here
        // and applied after the loop, since the loop only borrows the results.
        let mut fill: Option<Loaded> = None;
        let mut fill_value = String::new();

        // Capped so the editor below stays on screen; the results scroll within.
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if !self.forward.is_empty() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Means").small().weak());
                    for hit in &self.forward {
                        if row(ui, hit) {
                            fill = Some(hit.into());
                            fill_value = hit.value.clone();
                        }
                    }
                }

                if !self.reverse.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("Written as ({})", self.reverse.len()))
                            .small()
                            .weak(),
                    );
                    for hit in &self.reverse {
                        if row(ui, hit) {
                            fill = Some(hit.into());
                            fill_value = hit.value.clone();
                        }
                    }
                }
            });

        if let Some(loaded) = fill {
            self.edit_outline = loaded.outline.clone();
            self.edit_translation = fill_value;
            self.loaded = Some(loaded);
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

        // What the outline means, when the query is one. Most queries that are
        // not steno fail here, which is expected: "cat" is a perfectly good
        // reverse lookup and a hopeless outline.
        let parsed = Stroke::parse_outline(query).ok();
        if let Some(strokes) = &parsed {
            let outline = Stroke::render_outline(strokes);
            for dictionary in dictionaries.dictionaries() {
                if let Some(value) = dictionary.lookup(strokes) {
                    self.forward.push(Hit {
                        path: dictionary.path.clone(),
                        dictionary: short_name(&dictionary.path),
                        outline: outline.clone(),
                        value: value.to_owned(),
                        winning: is_winner(dictionaries, &dictionary.path, strokes),
                    });
                }
            }
        }

        // Every way the query can be written. Runs whether or not the query
        // parsed as steno, so a word that is also a valid outline still shows
        // its strokes.
        for dictionary in dictionaries.dictionaries() {
            for (outline, value) in dictionary.reverse_lookup(query) {
                self.reverse.push(Hit {
                    path: dictionary.path.clone(),
                    dictionary: short_name(&dictionary.path),
                    outline: Stroke::render_outline(outline),
                    value: value.to_owned(),
                    winning: is_winner(dictionaries, &dictionary.path, outline),
                });
            }
        }

        // Entries that match the query exactly come first, then shortest
        // outline: the brief is what the user wants to learn, and an entry that
        // differs in capitalisation is a weaker answer than one that does not.
        self.reverse.sort_by(|a, b| {
            (a.value != query)
                .cmp(&(b.value != query))
                .then(a.outline.len().cmp(&b.outline.len()))
                .then(a.outline.cmp(&b.outline))
        });

        if self.forward.is_empty() && self.reverse.is_empty() {
            self.parse_error = Some(match parsed {
                Some(_) => "No entry for that outline, and nothing is written that way".to_owned(),
                None => "Not valid steno, and no entry produces that text".to_owned(),
            });
        }
    }

    /// Add, change, move or remove an entry. Writes go to Pluvialis's own
    /// dictionary copies, never the user's Plover folder, and every write backs
    /// up the file first and is verified before it lands; see
    /// `pluvialis_core::edit`.
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

        // A loaded entry is edited where it already lives, so there is nothing
        // to choose. Only a new entry needs a destination.
        match self.loaded.clone() {
            Some(loaded) => {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Editing {} in {}",
                            loaded.outline, loaded.dictionary
                        ))
                        .small(),
                    );
                    if ui
                        .button("New")
                        .on_hover_text("Stop editing this entry and start a new one")
                        .clicked()
                    {
                        self.clear_editor();
                    }
                });
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label("New entry in");
                    egui::ComboBox::from_id_salt("edit-target")
                        .selected_text(names[self.target].clone())
                        .show_ui(ui, |ui| {
                            for (index, name) in names.iter().enumerate() {
                                ui.selectable_value(&mut self.target, index, name);
                            }
                        });
                });
            }
        }

        // The outline being changed from, shown as the hint once the field is
        // cleared, so it is never a mystery what a save would keep.
        let outline_hint = match &self.loaded {
            Some(loaded) => loaded.outline.clone(),
            None => "PHO*EF".to_owned(),
        };

        ui.horizontal(|ui| {
            ui.label("Outline");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.edit_outline)
                    .hint_text(outline_hint)
                    .desired_width(f32::INFINITY),
            );
            if response.gained_focus() {
                self.outline_regained_focus();
            }
            self.outline_focused = response.has_focus();
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
            // An empty field with an entry loaded means "keep this outline",
            // which is what makes changing only the word possible after the
            // field has cleared itself.
            let has_outline = !self.edit_outline.trim().is_empty() || self.loaded.is_some();
            let can_save = has_outline && !self.edit_translation.is_empty();
            let save_hint = match &self.loaded {
                Some(_) => "Save the change. A different outline moves the entry.",
                None => "Add the entry, or change its word if the outline already exists",
            };
            if ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .on_hover_text(save_hint)
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
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    egui::RichText::new(text).small(),
                );
            }
            None => {}
        }
    }

    /// Clear the outline field whenever it regains focus.
    ///
    /// Strokes append to this field, so whatever is left in it from the last
    /// entry becomes the front of the next outline: click away, click back,
    /// write PHOEF, and the field holds KAT/PHOEF. Clearing loses nothing,
    /// because the entry being edited is remembered in `loaded` and its outline
    /// stays visible as the hint.
    fn outline_regained_focus(&mut self) {
        self.edit_outline.clear();
        self.edit_message = None;
    }

    /// Drop the loaded entry and start a blank one.
    fn clear_editor(&mut self) {
        self.loaded = None;
        self.edit_outline.clear();
        self.edit_translation.clear();
        self.edit_message = None;
    }

    /// The outline the buttons act on: the field, or the loaded entry's own
    /// outline when the field is empty.
    fn effective_outline(&self) -> String {
        let typed = self.edit_outline.trim();
        match (typed.is_empty(), &self.loaded) {
            (true, Some(loaded)) => loaded.outline.clone(),
            _ => typed.to_owned(),
        }
    }

    fn apply_edit(&mut self, dictionaries: &mut DictionaryStack, kind: EditKind) {
        let outline = self.effective_outline();
        let loaded = self.loaded.clone();

        // An entry loaded from a result is edited in its own file. Anything
        // else is a new entry, and goes where the combo says, so adding an
        // outline that already exists elsewhere can deliberately shadow it.
        let path = match (&loaded, kind) {
            (Some(loaded), _) => loaded.path.clone(),
            (None, EditKind::Set) => dictionaries.dictionaries()[self.target].path.clone(),
            // A delete with nothing loaded has to find the entry first.
            (None, EditKind::Remove) => {
                let owning = Stroke::parse_outline(&outline).ok().and_then(|strokes| {
                    dictionaries
                        .dictionaries()
                        .iter()
                        .find(|d| d.lookup(&strokes).is_some())
                });
                match owning {
                    Some(dictionary) => dictionary.path.clone(),
                    None => {
                        self.edit_message =
                            Some(Err(format!("{outline} is not in any editable dictionary")));
                        return;
                    }
                }
            }
        };
        let name = short_name(&path);

        let result = match kind {
            EditKind::Set => match &loaded {
                // Changing the outline of an existing entry moves it, in one
                // write. Adding the new one and deleting the old separately
                // would leave the entry duplicated if the second write failed.
                Some(loaded) if loaded.outline != outline => {
                    move_entry(&path, &loaded.outline, &outline, &self.edit_translation)
                        .map(|_| format!("Moved {} to {outline} in {name}", loaded.outline))
                }
                _ => set_entry(&path, &outline, &self.edit_translation)
                    .map(|_| format!("Saved {outline} to {name}")),
            },
            EditKind::Remove => {
                remove_entry(&path, &outline).map(|_| format!("Removed {outline} from {name}"))
            }
        };

        match result {
            Ok(message) => {
                self.reload(dictionaries, &path);
                self.invalidate();
                match kind {
                    // Keep editing the entry that was just saved, under
                    // whatever outline it now has, so a second save is another
                    // change to it rather than a new entry beside it.
                    EditKind::Set => {
                        self.loaded = Some(Loaded {
                            path,
                            dictionary: name,
                            outline: outline.clone(),
                        });
                        self.edit_outline = outline;
                    }
                    EditKind::Remove => self.clear_editor(),
                }
                self.edit_message = Some(Ok(message));
            }
            Err(e) => self.edit_message = Some(Err(e.to_string())),
        }
    }

    /// Reload one dictionary from disk after editing it, so the change is live
    /// immediately, keeping its enabled state and its place in the priority
    /// order.
    fn reload(&mut self, dictionaries: &mut DictionaryStack, path: &Path) {
        let Some(index) = dictionaries
            .dictionaries()
            .iter()
            .position(|d| d.path == path)
        else {
            return;
        };
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

impl From<&Hit> for Loaded {
    fn from(hit: &Hit) -> Self {
        Loaded {
            path: hit.path.clone(),
            dictionary: hit.dictionary.clone(),
            outline: hit.outline.clone(),
        }
    }
}

/// One result row. Returns whether it was clicked.
///
/// The outline and the word are both clickable, because either is a reasonable
/// thing to aim at when the intent is "edit that one".
fn row(ui: &mut egui::Ui, hit: &Hit) -> bool {
    let mut clicked = false;
    ui.horizontal_wrapped(|ui| {
        let outline = egui::RichText::new(&hit.outline).monospace();
        let value = egui::RichText::new(format!("{:?}", hit.value));
        let (outline, value) = match hit.winning {
            true => (outline.strong(), value.strong()),
            false => (outline.weak(), value.weak()),
        };
        let hover = match hit.winning {
            true => "Click to edit this entry",
            false => "Click to edit this entry. Shadowed: a higher dictionary answers first.",
        };

        clicked |= ui
            .add(egui::Label::new(outline).sense(egui::Sense::click()))
            .on_hover_text(hover)
            .clicked();
        clicked |= ui
            .add(egui::Label::new(value).sense(egui::Sense::click()))
            .on_hover_text(hover)
            .clicked();
        ui.label(egui::RichText::new(&hit.dictionary).small().weak());
    });
    clicked
}

#[derive(Clone, Copy)]
enum EditKind {
    Set,
    Remove,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn temp_dict(name: &str, json: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pluvialis-pane-{name}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, json).unwrap();
        path
    }

    fn read_map(path: &Path) -> BTreeMap<String, String> {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn stack(paths: &[&Path]) -> DictionaryStack {
        let mut stack = DictionaryStack::new();
        for path in paths {
            stack.push(Dictionary::load(path).unwrap());
        }
        stack
    }

    #[test]
    fn a_dictionary_is_named_by_its_file_rather_than_its_whole_path() {
        let path = std::path::Path::new(r"C:\Users\you\AppData\Local\plover\plover\cb.json");
        assert_eq!(short_name(path), "cb.json");
    }

    #[test]
    fn saving_an_outline_that_exists_elsewhere_uses_the_selected_dictionary() {
        let high = temp_dict("high", "{\n\"KAT\": \"cat\"\n}\n");
        let selected = temp_dict("selected", "{\n}\n");
        let mut stack = stack(&[&high, &selected]);

        let mut pane = DictionaryPane {
            target: 1,
            edit_outline: "KAT".to_owned(),
            edit_translation: "kitten".to_owned(),
            ..DictionaryPane::new()
        };

        pane.apply_edit(&mut stack, EditKind::Set);

        assert_eq!(read_map(&high)["KAT"], "cat");
        assert_eq!(read_map(&selected)["KAT"], "kitten");
        assert!(matches!(pane.edit_message, Some(Ok(_))));
    }

    #[test]
    fn a_focused_outline_field_accepts_raw_steno() {
        let mut pane = DictionaryPane {
            outline_focused: true,
            ..DictionaryPane::new()
        };
        let stroke = Stroke::parse_outline("KAT").unwrap()[0];

        assert!(pane.accept_raw_outline(stroke));
        assert_eq!(pane.edit_outline, "KAT");
    }

    #[test]
    fn raw_steno_appends_as_a_multi_stroke_outline() {
        let mut pane = DictionaryPane {
            edit_outline: "WEL".to_owned(),
            outline_focused: true,
            ..DictionaryPane::new()
        };
        let stroke = Stroke::parse_outline("KO*PL").unwrap()[0];

        assert!(pane.accept_raw_outline(stroke));
        assert_eq!(pane.edit_outline, "WEL/KO*PL");
    }

    #[test]
    fn an_unfocused_outline_field_refuses_raw_steno() {
        let mut pane = DictionaryPane::new();
        let stroke = Stroke::parse_outline("KAT").unwrap()[0];

        assert!(!pane.accept_raw_outline(stroke));
        assert_eq!(pane.edit_outline, "");
    }

    #[test]
    fn regaining_focus_empties_the_outline_field() {
        let mut pane = DictionaryPane {
            edit_outline: "KAT".to_owned(),
            ..DictionaryPane::new()
        };
        pane.outline_regained_focus();
        assert_eq!(pane.edit_outline, "");
    }

    #[test]
    fn clearing_the_field_does_not_lose_the_entry_being_edited() {
        let dict = temp_dict("keepsloaded", "{\n\"KAT\": \"cat\"\n}\n");
        let mut pane = DictionaryPane {
            loaded: Some(Loaded {
                path: dict.clone(),
                dictionary: short_name(&dict),
                outline: "KAT".to_owned(),
            }),
            edit_outline: "KAT".to_owned(),
            ..DictionaryPane::new()
        };

        pane.outline_regained_focus();

        assert_eq!(pane.edit_outline, "");
        assert_eq!(
            pane.effective_outline(),
            "KAT",
            "the move still knows its start"
        );
    }

    #[test]
    fn a_word_that_is_also_valid_steno_is_answered_both_ways() {
        // "TO" parses as steno, so the old pane answered only the forward
        // direction and never said how the word itself is written.
        let dict = temp_dict("bothways", "{\n\"TO\": \"toe\",\n\"TOU\": \"to\"\n}\n");
        let mut pane = DictionaryPane {
            query: "TO".to_owned(),
            ..DictionaryPane::new()
        };

        pane.recompute(&stack(&[&dict]));

        assert_eq!(pane.forward.len(), 1, "TO is an outline meaning toe");
        assert_eq!(pane.forward[0].value, "toe");
        assert_eq!(pane.reverse.len(), 1, "and the word to is written TOU");
        assert_eq!(pane.reverse[0].outline, "TOU");
        assert!(pane.parse_error.is_none());
    }

    #[test]
    fn capitalisation_does_not_hide_an_outline_and_exact_matches_rank_first() {
        let dict = temp_dict("case", "{\n\"THE\": \"The\",\n\"-T\": \"the\"\n}\n");
        let mut pane = DictionaryPane {
            query: "the".to_owned(),
            ..DictionaryPane::new()
        };

        pane.recompute(&stack(&[&dict]));

        assert_eq!(pane.reverse.len(), 2);
        assert_eq!(pane.reverse[0].value, "the", "exact match first");
        assert_eq!(pane.reverse[1].value, "The");
    }

    #[test]
    fn every_outline_for_a_word_is_listed_shortest_first() {
        let dict = temp_dict(
            "allstrokes",
            "{\n\"KAT\": \"cat\",\n\"KAERT\": \"cat\",\n\"KAT/KAT\": \"cat\"\n}\n",
        );
        let mut pane = DictionaryPane {
            query: "cat".to_owned(),
            ..DictionaryPane::new()
        };

        pane.recompute(&stack(&[&dict]));

        let outlines: Vec<&str> = pane.reverse.iter().map(|h| h.outline.as_str()).collect();
        assert_eq!(outlines, ["KAT", "KAERT", "KAT/KAT"]);
    }

    #[test]
    fn the_same_word_in_two_dictionaries_is_listed_once_per_dictionary() {
        // Both are real, separately editable entries. Collapsing them would
        // hide one of the two files the user might need to change.
        let high = temp_dict("dupehigh", "{\n\"KAT\": \"cat\"\n}\n");
        let low = temp_dict("dupelow", "{\n\"KAT\": \"cat\"\n}\n");
        let mut pane = DictionaryPane {
            query: "cat".to_owned(),
            ..DictionaryPane::new()
        };

        pane.recompute(&stack(&[&high, &low]));

        assert_eq!(pane.reverse.len(), 2);
        assert!(pane.reverse[0].winning, "the higher dictionary answers");
        assert!(!pane.reverse[1].winning, "the lower one is shadowed");
    }

    #[test]
    fn changing_the_outline_of_a_loaded_entry_moves_it() {
        let dict = temp_dict("moveit", "{\n\"KAT\": \"cat\",\n\"TKOG\": \"dog\"\n}\n");
        let mut stack = stack(&[&dict]);
        let mut pane = DictionaryPane {
            loaded: Some(Loaded {
                path: dict.clone(),
                dictionary: short_name(&dict),
                outline: "KAT".to_owned(),
            }),
            edit_outline: "KAERT".to_owned(),
            edit_translation: "cat".to_owned(),
            ..DictionaryPane::new()
        };

        pane.apply_edit(&mut stack, EditKind::Set);

        let map = read_map(&dict);
        assert_eq!(map.len(), 2, "moved, not duplicated");
        assert!(!map.contains_key("KAT"));
        assert_eq!(map["KAERT"], "cat");
        assert_eq!(map["TKOG"], "dog");
        assert_eq!(pane.loaded.as_ref().unwrap().outline, "KAERT");
    }

    #[test]
    fn a_loaded_entry_is_saved_where_it_lives_not_where_the_combo_points() {
        let owning = temp_dict("owning", "{\n\"KAT\": \"cat\"\n}\n");
        let other = temp_dict("other", "{\n}\n");
        // The combo points at the second dictionary, which is where a new
        // entry would go. This one is not new.
        let mut stack = stack(&[&owning, &other]);
        let mut pane = DictionaryPane {
            target: 1,
            loaded: Some(Loaded {
                path: owning.clone(),
                dictionary: short_name(&owning),
                outline: "KAT".to_owned(),
            }),
            edit_outline: "KAT".to_owned(),
            edit_translation: "kitten".to_owned(),
            ..DictionaryPane::new()
        };

        pane.apply_edit(&mut stack, EditKind::Set);

        assert_eq!(read_map(&owning)["KAT"], "kitten");
        assert!(read_map(&other).is_empty());
    }

    #[test]
    fn deleting_a_loaded_entry_clears_the_editor() {
        let dict = temp_dict("delloaded", "{\n\"KAT\": \"cat\"\n}\n");
        let mut stack = stack(&[&dict]);
        let mut pane = DictionaryPane {
            loaded: Some(Loaded {
                path: dict.clone(),
                dictionary: short_name(&dict),
                outline: "KAT".to_owned(),
            }),
            edit_translation: "cat".to_owned(),
            ..DictionaryPane::new()
        };

        pane.apply_edit(&mut stack, EditKind::Remove);

        assert!(read_map(&dict).is_empty());
        assert!(pane.loaded.is_none());
        assert_eq!(pane.edit_outline, "");
        assert!(matches!(pane.edit_message, Some(Ok(_))));
    }

    #[test]
    fn an_edit_is_visible_in_the_stack_without_a_restart() {
        let dict = temp_dict("livereload", "{\n\"KAT\": \"cat\"\n}\n");
        let mut stack = stack(&[&dict]);
        let mut pane = DictionaryPane {
            edit_outline: "KAT".to_owned(),
            edit_translation: "kitten".to_owned(),
            ..DictionaryPane::new()
        };

        pane.apply_edit(&mut stack, EditKind::Set);

        let strokes = Stroke::parse_outline("KAT").unwrap();
        assert_eq!(stack.lookup(&strokes), Some("kitten"));
    }
}
