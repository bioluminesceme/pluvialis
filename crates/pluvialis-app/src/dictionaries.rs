//! The dictionary list and the entry editor, which sit either side of the
//! table on the Dictionary screen.
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
//! Editing happens in one docked strip rather than in the table's own cells.
//! Three reasons, all specific to this program: every write reparses and
//! verifies a 93,000 line file, so it cannot fire per keystroke; changing an
//! outline is a move rather than a set; and the outline field has to be the one
//! unambiguous place that accepts raw steno from the writer, which a grid of a
//! hundred thousand possibly-steno cells would destroy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui;

use pluvialis_core::{
    Dictionary, DictionaryStack, Stroke, move_entry, remove_entries, remove_entry, set_entry,
    swap_entries,
};

/// The entry the editor is working on, once one has been loaded from the table.
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
    /// Which JSON dictionary a brand new entry goes into. Ignored once an
    /// existing entry is loaded, which is edited where it already lives.
    target: usize,
    loaded: Option<Loaded>,
    edit_outline: String,
    /// What the outline field held before it cleared itself on regaining
    /// focus. Kept so clearing cannot silently throw the outline away.
    cleared_outline: String,
    edit_translation: String,
    /// The same, for the word field. Both fields empty themselves when they
    /// take focus so a value can be replaced without deleting it first, and
    /// both keep what they held for the same reason: without it, clicking in
    /// and out leaves a save with nothing to save and no sign of why.
    cleared_translation: String,
    translation_focused: bool,
    outline_focused: bool,
    /// The outcome of the last edit: `Ok` for a success line, `Err` for a
    /// refusal to show in red.
    edit_message: Option<Result<String, String>>,
}

/// A short name for a dictionary, for the list.
pub fn short_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl DictionaryPane {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a field here is waiting to be filled by the writer.
    ///
    /// Read by the router before a batch of strokes is handled, so the whole
    /// batch goes to one place. `accept_raw_outline` stays the authority on
    /// actually taking a stroke.
    pub fn wants_strokes(&self) -> bool {
        self.outline_focused
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

    /// Put an entry from the table into the editor.
    pub fn load_entry(&mut self, path: &Path, outline: &str, word: &str) {
        self.loaded = Some(Loaded {
            path: path.to_path_buf(),
            dictionary: short_name(path),
            outline: outline.to_owned(),
        });
        self.edit_outline = outline.to_owned();
        self.edit_translation = word.to_owned();
        self.cleared_outline.clear();
        self.cleared_translation.clear();
        self.edit_message = None;
    }

    /// Start a new entry for an outline chosen somewhere else, for instance
    /// from the untranslated list on the Stats screen. Not `load_entry`: there
    /// is no entry to load, which is the whole reason she is here.
    pub fn start_new_entry(&mut self, outline: &str) {
        self.loaded = None;
        self.edit_outline = outline.to_owned();
        self.edit_translation.clear();
        self.cleared_outline.clear();
        self.cleared_translation.clear();
        self.edit_message = None;
    }

    /// The dictionary list, with priority order and enable checkboxes.
    ///
    /// Returns whether the enabled state or order changed this frame, so the
    /// caller can persist it.
    pub fn list(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) -> bool {
        ui.add_space(4.0);
        ui.strong("Dictionaries");
        ui.label(
            egui::RichText::new("Priority order, highest first")
                .small()
                .weak(),
        );
        ui.separator();

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

        // Dictionaries that compute their answers rather than storing them
        // (Python). They sit in the same list, because to the user they are
        // dictionaries like any other and she wants one place to switch things
        // on and off. They keep no arrows: they are consulted only after every
        // stored dictionary has missed, so they have no position to move.
        for index in 0..dictionaries.programmatic().len() {
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
                    "A Python dictionary: it works out its answers instead of                      looking them up, so it has no entries to browse or edit.                      Consulted only after every other dictionary has missed,                      which is why it has no position to move.

It runs as                      written and is not sandboxed, the same trust model as                      Plover.",
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("python").small().weak());
                });
            });
        }

        changed
    }

    /// Add, change, move or remove an entry. Writes go to Pluvialis's own
    /// dictionary copies, never the user's Plover folder, and every write backs
    /// up the file first and is verified before it lands; see
    /// `pluvialis_core::edit`.
    ///
    /// Returns whether any dictionary changed, so the caller can rebuild what
    /// it has cached about them.
    pub fn editor(&mut self, ui: &mut egui::Ui, dictionaries: &mut DictionaryStack) -> bool {
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
            return false;
        }
        if self.target >= names.len() {
            self.target = 0;
        }

        let mut changed = false;

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
        let outline_hint = match (&self.loaded, self.cleared_outline.is_empty()) {
            (Some(loaded), _) => loaded.outline.clone(),
            (None, false) => self.cleared_outline.clone(),
            (None, true) => "PHO*EF".to_owned(),
        };

        ui.horizontal(|ui| {
            ui.label("Outline");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.edit_outline)
                    .hint_text(outline_hint)
                    .desired_width(200.0),
            );
            if response.gained_focus() {
                self.outline_regained_focus();
            }
            self.outline_focused = response.has_focus();

            ui.label("Word");
            let word_hint = match self.cleared_translation.is_empty() {
                true => "move".to_owned(),
                false => self.cleared_translation.clone(),
            };
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.edit_translation)
                    .hint_text(word_hint)
                    .desired_width(ui.available_width().max(120.0)),
            );
            if response.gained_focus() {
                self.translation_regained_focus();
            }
            self.translation_focused = response.has_focus();
        });

        // An empty field with an entry loaded means "keep this outline",
        // which is what makes changing only the word possible after the
        // field has cleared itself.
        let has_outline = !self.effective_outline().is_empty();
        let can_save = has_outline && !self.effective_translation().is_empty();

        // The buttons get their own row rather than trailing the fields. On the
        // row they used to share, the bottom panel is already narrowed by the
        // file list and the tape, so a Save button after two text fields was
        // pushed off the right edge on a smaller window: the one control that
        // must never be hard to find.
        ui.horizontal(|ui| match self.loaded.is_some() {
            true => {
                if ui
                    .add_enabled(can_save, egui::Button::new("Save changes"))
                    .on_hover_text("Save the change. A different outline moves the entry.")
                    .clicked()
                {
                    changed |= self.apply_edit(dictionaries, EditKind::Set);
                }
                if ui
                    .add_enabled(has_outline, egui::Button::new("Delete"))
                    .on_hover_text("Remove this outline from the dictionary it lives in")
                    .clicked()
                {
                    changed |= self.apply_edit(dictionaries, EditKind::Remove);
                }
            }
            // Nothing exists yet to delete, so only one button belongs here.
            false => {
                if ui
                    .add_enabled(can_save, egui::Button::new("Add entry"))
                    .on_hover_text(
                        "Add the entry, or change its word if the outline already exists",
                    )
                    .clicked()
                {
                    changed |= self.apply_edit(dictionaries, EditKind::Set);
                }
                if !can_save {
                    ui.label(
                        egui::RichText::new("Fill in both fields to add an entry.")
                            .small()
                            .weak(),
                    );
                }
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

        changed
    }

    /// Exchange the words two outlines write, in one verified write.
    pub fn swap_with(
        &mut self,
        dictionaries: &mut DictionaryStack,
        path: &Path,
        first: &str,
        second: &str,
    ) -> bool {
        match swap_entries(path, first, second) {
            Ok(report) => {
                self.reload(dictionaries, path);
                // The state afterwards, because that is the thing being
                // decided: which outline now writes which word.
                self.edit_message = Some(Ok(format!(
                    "{} now writes {:?}, {} writes {:?}",
                    report.first.0, report.first.1, report.second.0, report.second.1
                )));
                self.edit_translation = report.first.1.clone();
                true
            }
            Err(e) => {
                self.edit_message = Some(Err(e.to_string()));
                false
            }
        }
    }

    /// Remove several entries, one verified write per file.
    ///
    /// Grouped by file rather than looped one at a time: fourteen entries
    /// through `remove_entry` would be fourteen reads, fourteen reparses of a
    /// 93,000 line file and fourteen backups to wade through when undoing.
    pub fn delete_entries(
        &mut self,
        dictionaries: &mut DictionaryStack,
        entries: &[(PathBuf, String)],
    ) -> bool {
        let mut by_file: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        for (path, outline) in entries {
            by_file
                .entry(path.clone())
                .or_default()
                .push(outline.clone());
        }

        let mut removed = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for (path, outlines) in &by_file {
            let borrowed: Vec<&str> = outlines.iter().map(String::as_str).collect();
            match remove_entries(path, &borrowed) {
                Ok(report) => {
                    removed += report.removed.len();
                    self.reload(dictionaries, path);
                }
                Err(e) => failures.push(format!("{}: {e}", short_name(path))),
            }
        }

        // The loaded entry may have been one of them.
        if let Some(loaded) = &self.loaded
            && entries
                .iter()
                .any(|(path, outline)| *path == loaded.path && *outline == loaded.outline)
        {
            self.clear_editor();
        }

        self.edit_message = match failures.is_empty() {
            true => Some(Ok(format!("Removed {removed} entries"))),
            false => Some(Err(failures.join("; "))),
        };
        removed > 0
    }

    /// Clear the outline field whenever it regains focus.
    ///
    /// Strokes append to this field, so whatever is left in it from the last
    /// entry becomes the front of the next outline: click away, click back,
    /// write PHOEF, and the field holds KAT/PHOEF. Clearing loses nothing,
    /// because the entry being edited is remembered in `loaded` and its outline
    /// stays visible as the hint.
    /// Empty the outline field when it takes focus, so a chord can be
    /// rewritten without deleting the old one first. Asked for directly.
    ///
    /// What was there is kept rather than dropped. Without that, clicking into
    /// the field and clicking away again left a new entry with no outline at
    /// all, which greyed out the save button with nothing on screen to say why:
    /// the entry looked entered and was never written. That is exactly how
    /// TKEPT was lost on 2026-09-02.
    /// The word field empties on taking focus, like the outline field, so the
    /// word can be replaced without selecting and deleting it first.
    fn translation_regained_focus(&mut self) {
        if !self.edit_translation.trim().is_empty() {
            self.cleared_translation = self.edit_translation.trim().to_owned();
        }
        self.edit_translation.clear();
        self.edit_message = None;
    }

    fn outline_regained_focus(&mut self) {
        if !self.edit_outline.trim().is_empty() {
            self.cleared_outline = self.edit_outline.trim().to_owned();
        }
        self.edit_outline.clear();
        self.edit_message = None;
    }

    /// Drop the loaded entry and start a blank one.
    fn clear_editor(&mut self) {
        self.loaded = None;
        self.cleared_outline.clear();
        self.cleared_translation.clear();
        self.edit_outline.clear();
        self.edit_translation.clear();
        self.edit_message = None;
    }

    /// The outline the buttons act on: the field, or the loaded entry's own
    /// outline when the field is empty.
    fn effective_outline(&self) -> String {
        let typed = self.edit_outline.trim();
        if !typed.is_empty() {
            return typed.to_owned();
        }
        match &self.loaded {
            // Editing an entry: an empty field means "keep this outline".
            Some(loaded) => loaded.outline.clone(),
            // A new entry: the field may have emptied itself when it regained
            // focus, so fall back to whatever it held before that.
            None => self.cleared_outline.clone(),
        }
    }

    /// The word the buttons act on: the field, or what it held before it
    /// cleared itself.
    fn effective_translation(&self) -> String {
        let typed = self.edit_translation.trim();
        match typed.is_empty() {
            true => self.cleared_translation.clone(),
            false => typed.to_owned(),
        }
    }

    fn apply_edit(&mut self, dictionaries: &mut DictionaryStack, kind: EditKind) -> bool {
        let outline = self.effective_outline();
        let translation = self.effective_translation();
        let loaded = self.loaded.clone();

        // An entry loaded from the table is edited in its own file. Anything
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
                        return false;
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
                    move_entry(&path, &loaded.outline, &outline, &translation)
                        .map(|_| format!("Moved {} to {outline} in {name}", loaded.outline))
                }
                _ => set_entry(&path, &outline, &translation)
                    .map(|_| format!("Saved {outline} to {name}")),
            },
            EditKind::Remove => {
                remove_entry(&path, &outline).map(|_| format!("Removed {outline} from {name}"))
            }
        };

        match result {
            Ok(message) => {
                self.reload(dictionaries, &path);
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
                        // The entry is loaded now, so the loaded outline is
                        // the fallback and the remembered one would shadow it.
                        self.cleared_outline.clear();
                        self.cleared_translation.clear();
                    }
                    EditKind::Remove => self.clear_editor(),
                }
                self.edit_message = Some(Ok(message));
                true
            }
            Err(e) => {
                self.edit_message = Some(Err(e.to_string()));
                false
            }
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

#[derive(Clone, Copy)]
enum EditKind {
    Set,
    Remove,
}

#[cfg(test)]
mod focus_tests {
    use super::*;

    /// The outline field empties when it regains focus, so a chord can be
    /// rewritten without deleting the old one first. What it held must survive
    /// that, or a new entry ends up with no outline, the save button greys out
    /// with nothing on screen to say why, and the entry is silently never
    /// written. TKEPT was lost exactly this way on 2026-09-02.
    #[test]
    fn clicking_back_into_the_outline_field_does_not_lose_a_new_entry() {
        let mut pane = DictionaryPane::new();
        pane.edit_outline = "TKEPT".to_owned();
        pane.edit_translation = "department".to_owned();

        pane.outline_regained_focus();

        assert!(pane.edit_outline.is_empty(), "the field clears, as asked");
        assert_eq!(
            pane.effective_outline(),
            "TKEPT",
            "but the outline is still what would be saved"
        );
    }

    /// Writing a new chord after the field clears replaces the old one rather
    /// than being ignored in favour of it.
    #[test]
    fn a_freshly_written_outline_wins_over_the_remembered_one() {
        let mut pane = DictionaryPane::new();
        pane.edit_outline = "TKEPT".to_owned();
        pane.outline_regained_focus();
        pane.edit_outline = "TKPAERT".to_owned();

        assert_eq!(pane.effective_outline(), "TKPAERT");
    }

    /// The word field clears on focus like the outline field does, and must
    /// keep what it held for the same reason: a save with an empty word is
    /// refused, so losing it silently disables the button with nothing on
    /// screen to explain it.
    #[test]
    fn clicking_back_into_the_word_field_does_not_lose_it() {
        let mut pane = DictionaryPane::new();
        pane.edit_outline = "TKEPT".to_owned();
        pane.edit_translation = "department".to_owned();

        pane.translation_regained_focus();

        assert!(
            pane.edit_translation.is_empty(),
            "the field clears, as asked"
        );
        assert_eq!(pane.effective_translation(), "department");
    }

    #[test]
    fn a_freshly_typed_word_wins_over_the_remembered_one() {
        let mut pane = DictionaryPane::new();
        pane.edit_translation = "department".to_owned();
        pane.translation_regained_focus();
        pane.edit_translation = "depart".to_owned();

        assert_eq!(pane.effective_translation(), "depart");
    }

    /// Both fields cleared and neither retyped still saves what was entered.
    #[test]
    fn clearing_both_fields_still_saves_the_entry() {
        let mut pane = DictionaryPane::new();
        pane.edit_outline = "TKEPT".to_owned();
        pane.edit_translation = "department".to_owned();
        pane.outline_regained_focus();
        pane.translation_regained_focus();

        assert_eq!(pane.effective_outline(), "TKEPT");
        assert_eq!(pane.effective_translation(), "department");
    }

    /// Starting a new entry forgets the previous one entirely.
    #[test]
    fn clearing_the_editor_forgets_the_remembered_outline() {
        let mut pane = DictionaryPane::new();
        pane.edit_outline = "TKEPT".to_owned();
        pane.outline_regained_focus();
        pane.clear_editor();

        assert_eq!(pane.effective_outline(), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dict(name: &str, json: &str) -> PathBuf {
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
        let path = Path::new(r"C:\Users\you\AppData\Local\plover\plover\cb.json");
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

        assert!(pane.apply_edit(&mut stack, EditKind::Set));

        assert_eq!(read_map(&high)["KAT"], "cat");
        assert_eq!(read_map(&selected)["KAT"], "kitten");
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
        let mut pane = DictionaryPane::new();
        pane.load_entry(&dict, "KAT", "cat");

        pane.outline_regained_focus();

        assert_eq!(pane.edit_outline, "");
        assert_eq!(
            pane.effective_outline(),
            "KAT",
            "the move still knows its start"
        );
    }

    #[test]
    fn changing_the_outline_of_a_loaded_entry_moves_it() {
        let dict = temp_dict("moveit", "{\n\"KAT\": \"cat\",\n\"TKOG\": \"dog\"\n}\n");
        let mut stack = stack(&[&dict]);
        let mut pane = DictionaryPane::new();
        pane.load_entry(&dict, "KAT", "cat");
        pane.edit_outline = "KAERT".to_owned();

        assert!(pane.apply_edit(&mut stack, EditKind::Set));

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
        let mut stack = stack(&[&owning, &other]);

        // The combo points at the second dictionary, which is where a new
        // entry would go. This one is not new.
        let mut pane = DictionaryPane {
            target: 1,
            ..DictionaryPane::new()
        };
        pane.load_entry(&owning, "KAT", "cat");
        pane.edit_translation = "kitten".to_owned();

        assert!(pane.apply_edit(&mut stack, EditKind::Set));

        assert_eq!(read_map(&owning)["KAT"], "kitten");
        assert!(read_map(&other).is_empty());
    }

    #[test]
    fn deleting_a_loaded_entry_clears_the_editor() {
        let dict = temp_dict("delloaded", "{\n\"KAT\": \"cat\"\n}\n");
        let mut stack = stack(&[&dict]);
        let mut pane = DictionaryPane::new();
        pane.load_entry(&dict, "KAT", "cat");

        assert!(pane.apply_edit(&mut stack, EditKind::Remove));

        assert!(read_map(&dict).is_empty());
        assert!(pane.loaded.is_none());
        assert_eq!(pane.edit_outline, "");
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

        assert!(pane.apply_edit(&mut stack, EditKind::Set));

        let strokes = Stroke::parse_outline("KAT").unwrap();
        assert_eq!(stack.lookup(&strokes), Some("kitten"));
    }

    #[test]
    fn swapping_a_star_variant_exchanges_the_two_words() {
        // The whole point: KA*T is harder to write than KAT, so the word she
        // uses more should own the plain outline.
        let dict = temp_dict("starswap", "{\n\"KAT\": \"cart\",\n\"KA*T\": \"cat\"\n}\n");
        let mut stack = stack(&[&dict]);
        let mut pane = DictionaryPane::new();
        pane.load_entry(&dict, "KAT", "cart");

        assert!(pane.swap_with(&mut stack, &dict, "KAT", "KA*T"));

        let map = read_map(&dict);
        assert_eq!(map["KAT"], "cat");
        assert_eq!(map["KA*T"], "cart");
        assert_eq!(pane.edit_translation, "cat", "the editor follows the swap");
        assert!(matches!(pane.edit_message, Some(Ok(_))));
    }

    #[test]
    fn a_swap_across_two_dictionaries_is_refused() {
        let english = temp_dict("swapen", "{\n\"KAT\": \"cat\"\n}\n");
        let dutch = temp_dict("swapnl", "{\n\"KA*T\": \"kat\"\n}\n");
        let mut stack = stack(&[&english, &dutch]);
        let mut pane = DictionaryPane::new();
        pane.load_entry(&english, "KAT", "cat");

        // KA*T is not in the English file, so the write finds nothing to swap.
        assert!(!pane.swap_with(&mut stack, &english, "KAT", "KA*T"));

        assert_eq!(read_map(&english)["KAT"], "cat");
        assert_eq!(read_map(&dutch)["KA*T"], "kat");
        assert!(matches!(pane.edit_message, Some(Err(_))));
    }
}
