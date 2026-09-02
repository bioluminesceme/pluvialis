//! The Dictionary screen: a table of every entry, searched by substring.
//!
//! The table is virtualised. At 101,419 entries, anything that touches every
//! row on every frame is a dropped frame, so only the rows actually on screen
//! are looked at, and the ordering and filtering live in
//! [`crate::entry_index`] where they are computed once per query.
//!
//! Row heights have to be uniform for `ScrollArea::show_rows` to know where to
//! jump, so no cell may wrap. A wrapped cell throws every scroll offset below
//! it out by the amount it grew, which reads as a rendering bug rather than a
//! layout mistake.

use std::collections::BTreeSet;

use eframe::egui;

use pluvialis_core::Stroke;

use crate::entry_index::{EntryIndex, Query, Sort};
use crate::screens::thousands;

/// What the screen wants done. The screen can see the entries but not write to
/// them, so it says what it wants and the caller, which owns the dictionaries,
/// carries it out.
#[derive(Default, PartialEq, Eq, Debug)]
pub enum Action {
    #[default]
    None,
    /// Put this entry in the editor below.
    Load(u32),
    /// Remove these, grouped by file by the caller.
    Delete(Vec<u32>),
    /// Exchange the words these two write.
    Swap(u32, u32),
}

#[derive(Default)]
pub struct DictionaryScreen {
    search: String,
    sort: Sort,
    descending: bool,
    filter: Option<u16>,
    /// Highlighted rows, by index id rather than table position, so a
    /// selection survives the query changing. The two entries in a swap
    /// usually hold different words and cannot both be on screen at once: one
    /// is found and ticked, then the other is searched for.
    selected: BTreeSet<u32>,
    /// Where a shift-click measures from.
    anchor: Option<u32>,
    search_focused: bool,
    /// Deleting more than one entry asks first. One is the same weight as the
    /// editor's own Delete button, which does not ask.
    confirm_delete: bool,
}

impl DictionaryScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the search box is waiting to be filled by the writer.
    pub fn wants_strokes(&self) -> bool {
        self.search_focused
    }

    /// Write a chord straight into the search box, so an outline can be looked
    /// up by stroking it rather than spelling it out on the keyboard.
    pub fn accept_raw_outline(&mut self, stroke: Stroke) -> bool {
        if !self.search_focused {
            return false;
        }
        let outline = Stroke::render_outline(&[stroke]);
        if !self.search.trim().is_empty() && !self.search.ends_with('/') {
            self.search.push('/');
        }
        self.search.push_str(&outline);
        true
    }

    /// Forget the selection. The ids are positions in the index, so anything
    /// that rebuilds it invalidates them.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.confirm_delete = false;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, index: &mut EntryIndex) -> Action {
        self.search_box(ui, index);

        index.refresh(&Query {
            text: self.search.clone(),
            sort: self.sort,
            descending: self.descending,
            dictionary: self.filter,
        });

        ui.add_space(4.0);
        let matches = index.total_matches();
        let total = index.total_entries();
        let label = match self.search.trim().is_empty() && self.filter.is_none() {
            true => format!("{} entries", thousands(total as u64)),
            false => format!(
                "{} of {} entries",
                thousands(matches as u64),
                thousands(total as u64)
            ),
        };
        ui.label(egui::RichText::new(label).weak());

        ui.add_space(2.0);
        self.header(ui);
        ui.separator();

        // The action bar keeps its space whatever the table does, so it never
        // slides off the bottom of a long list.
        const ACTION_BAR: f32 = 34.0;
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
        let widths = columns(ui.available_width());
        let table_height = (ui.available_height() - ACTION_BAR).max(60.0);

        let mut action = self.table(ui, index, row_height, table_height, &widths);

        ui.separator();
        let bar = self.action_bar(ui, index);
        if bar != Action::None {
            action = bar;
        }
        action
    }

    fn search_box(&mut self, ui: &mut egui::Ui, index: &EntryIndex) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search outlines and words")
                    .desired_width(320.0),
            );
            self.search_focused = response.has_focus();
            if ui.button("Clear").clicked() {
                self.search.clear();
            }

            ui.separator();
            ui.label("Show");
            if ui.selectable_label(self.filter.is_none(), "all").clicked() {
                self.filter = None;
            }
            // Collected first: the chips borrow the index while drawing.
            let names: Vec<(u16, String)> = index
                .dictionaries()
                .map(|(id, name)| (id, name.to_owned()))
                .collect();
            for (id, name) in names {
                if ui.selectable_label(self.filter == Some(id), name).clicked() {
                    self.filter = match self.filter == Some(id) {
                        true => None,
                        false => Some(id),
                    };
                }
            }
        });
    }

    fn table(
        &mut self,
        ui: &mut egui::Ui,
        index: &EntryIndex,
        row_height: f32,
        height: f32,
        widths: &Columns,
    ) -> Action {
        let rows = index.rows();
        if rows.is_empty() {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Nothing matches that.").weak());
            ui.allocate_space(egui::vec2(0.0, (height - 40.0).max(0.0)));
            return Action::None;
        }

        // Read once, outside the row loop: reading modifiers per row would ask
        // egui the same question thirty times a frame.
        let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        let mut clicked: Option<u32> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(height)
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for position in range {
                    let Some(id) = rows.get(position) else {
                        continue;
                    };
                    let Some(entry) = index.entry(*id) else {
                        continue;
                    };
                    let selected = self.selected.contains(id);
                    if row(
                        ui,
                        entry,
                        index.name(entry.dictionary),
                        widths,
                        selected,
                        row_height,
                    ) {
                        clicked = Some(*id);
                    }
                }
            });

        let Some(id) = clicked else {
            return Action::None;
        };
        self.confirm_delete = false;

        if shift && let Some(anchor) = self.anchor {
            self.select_range(rows, anchor, id);
            return Action::None;
        }
        if ctrl {
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
            self.anchor = Some(id);
            return Action::None;
        }

        // A plain click replaces the selection and opens the entry, which is
        // what a single click on a row is for.
        self.selected.clear();
        self.selected.insert(id);
        self.anchor = Some(id);
        Action::Load(id)
    }

    /// Select everything between two ids, in the order the table is showing,
    /// not the order the index stores.
    fn select_range(&mut self, rows: &[u32], anchor: u32, id: u32) {
        let (Some(from), Some(to)) = (
            rows.iter().position(|row| *row == anchor),
            rows.iter().position(|row| *row == id),
        ) else {
            // The anchor has been filtered out of view. Start again from here
            // rather than selecting something arbitrary.
            self.selected.clear();
            self.selected.insert(id);
            self.anchor = Some(id);
            return;
        };
        let (from, to) = (from.min(to), from.max(to));
        self.selected.clear();
        self.selected.extend(&rows[from..=to]);
    }

    fn action_bar(&mut self, ui: &mut egui::Ui, index: &EntryIndex) -> Action {
        let mut action = Action::None;
        let count = self.selected.len();

        ui.horizontal(|ui| {
            if count == 0 {
                ui.label(
                    egui::RichText::new(
                        "Click an entry to edit it. Ctrl-click or shift-click to pick more.",
                    )
                    .weak()
                    .small(),
                );
                return;
            }

            ui.label(format!("{count} selected"));

            let only = (count == 1)
                .then(|| self.selected.iter().copied().next())
                .flatten();
            if ui
                .add_enabled(only.is_some(), egui::Button::new("Edit"))
                .on_hover_text("Load this entry into the editor below")
                .clicked()
                && let Some(id) = only
            {
                action = Action::Load(id);
            }

            // Deleting one is the same weight as the editor's own Delete, which
            // does not ask. Deleting a batch is worth a second look, because
            // undoing it means finding the backup by hand.
            match self.confirm_delete {
                false => {
                    let label = match count {
                        1 => "Delete".to_owned(),
                        n => format!("Delete {n}"),
                    };
                    if ui
                        .button(label)
                        .on_hover_text("Remove from the dictionary each entry lives in")
                        .clicked()
                    {
                        match count {
                            1 => action = Action::Delete(self.selected.iter().copied().collect()),
                            _ => self.confirm_delete = true,
                        }
                    }
                }
                true => {
                    ui.label(
                        egui::RichText::new(format!("Delete {count} entries?"))
                            .color(ui.visuals().error_fg_color),
                    );
                    if ui.button("Yes, delete").clicked() {
                        action = Action::Delete(self.selected.iter().copied().collect());
                        self.confirm_delete = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = false;
                    }
                }
            }

            let pair = self.selectable_pair(index);
            let hint = match (count, &pair) {
                (2, None) => "Both entries must be in the same dictionary.",
                (2, Some(_)) => "Exchange the words these two entries write",
                _ => "Select exactly two entries to swap them",
            };
            if ui
                .add_enabled(pair.is_some(), egui::Button::new("Swap"))
                .on_hover_text(hint)
                .clicked()
                && let Some((a, b)) = pair
            {
                action = Action::Swap(a, b);
            }

            if ui
                .button("Clear")
                .on_hover_text("Deselect everything")
                .clicked()
            {
                self.selected.clear();
                self.anchor = None;
                self.confirm_delete = false;
            }
        });

        action
    }

    /// The two entries a swap would act on, when there are exactly two and they
    /// live in the same file.
    ///
    /// Same file only, because a swap across two files cannot be one verified
    /// write, and two writes can half succeed and leave both entries holding
    /// the same word.
    fn selectable_pair(&self, index: &EntryIndex) -> Option<(u32, u32)> {
        let mut ids = self.selected.iter().copied();
        let (a, b) = (ids.next()?, ids.next()?);
        if ids.next().is_some() {
            return None;
        }
        let first = index.entry(a)?;
        let second = index.entry(b)?;
        (first.dictionary == second.dictionary).then_some((a, b))
    }

    /// Clickable column titles. Clicking the column already sorted on reverses
    /// it, which is what every table does and what people try first.
    fn header(&mut self, ui: &mut egui::Ui) {
        let widths = columns(ui.available_width());
        ui.horizontal(|ui| {
            for (title, sort, width) in [
                ("Outline", Sort::Outline, widths.outline),
                ("Word", Sort::Word, widths.word),
                ("Dictionary", Sort::Dictionary, widths.dictionary),
            ] {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, ui.spacing().interact_size.y),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let active = self.sort == sort;
                        let arrow = match (active, self.descending) {
                            (true, false) => " ^",
                            (true, true) => " v",
                            (false, _) => "",
                        };
                        let text = egui::RichText::new(format!("{title}{arrow}")).strong();
                        if ui
                            .add(egui::Label::new(text).sense(egui::Sense::click()))
                            .clicked()
                        {
                            match active {
                                true => self.descending = !self.descending,
                                false => {
                                    self.sort = sort;
                                    self.descending = false;
                                }
                            }
                        }
                    },
                );
            }
        });
    }
}

pub struct Columns {
    outline: f32,
    word: f32,
    dictionary: f32,
}

/// Fixed widths, computed once per frame. Proportional so the table uses the
/// window, with floors so the outline column never collapses to nothing on a
/// narrow window.
fn columns(available: f32) -> Columns {
    let usable = (available - 24.0).max(240.0);
    Columns {
        outline: (usable * 0.28).max(90.0),
        word: (usable * 0.47).max(110.0),
        dictionary: (usable * 0.25).max(80.0),
    }
}

/// One row. Returns whether it was clicked.
fn row(
    ui: &mut egui::Ui,
    entry: &crate::entry_index::Entry,
    dictionary: &str,
    widths: &Columns,
    selected: bool,
    height: f32,
) -> bool {
    let fill = match selected {
        true => ui.visuals().selection.bg_fill,
        false => egui::Color32::TRANSPARENT,
    };

    let response = egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(2, 0))
        .show(ui, |ui| {
            ui.set_min_height(height);
            ui.horizontal(|ui| {
                cell(
                    ui,
                    widths.outline,
                    egui::RichText::new(&entry.outline).monospace(),
                );
                cell(ui, widths.word, egui::RichText::new(&entry.word));
                cell(
                    ui,
                    widths.dictionary,
                    egui::RichText::new(dictionary).weak().small(),
                );
            });
        })
        .response;

    response.interact(egui::Sense::click()).clicked()
}

/// A fixed width cell that truncates. Truncation rather than wrapping is what
/// keeps every row the same height, which `show_rows` requires.
fn cell(ui: &mut egui::Ui, width: f32, text: egui::RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.available_height()),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

/// `101419` reads as noise at a glance; `101,419` does not.
#[cfg(test)]
mod tests {
    use super::*;
    use pluvialis_core::{Dictionary, DictionaryStack};
    use std::path::PathBuf;

    fn temp_dict(name: &str, json: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pluvialis-screen-{name}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, json).unwrap();
        path
    }

    fn index_of(paths: &[&std::path::Path]) -> EntryIndex {
        let mut stack = DictionaryStack::new();
        for path in paths {
            stack.push(Dictionary::load(path).unwrap());
        }
        let mut index = EntryIndex::new();
        index.rebuild(&stack);
        index.refresh(&Query::default());
        index
    }

    /// The id of the entry with this outline, whichever order the index built.
    fn id_of(index: &EntryIndex, outline: &str) -> u32 {
        index
            .rows()
            .iter()
            .copied()
            .find(|id| index.entry(*id).map(|e| e.outline.as_str()) == Some(outline))
            .expect("outline is in the index")
    }

    #[test]
    fn a_focused_search_box_takes_raw_steno() {
        let mut screen = DictionaryScreen {
            search_focused: true,
            ..DictionaryScreen::new()
        };
        let stroke = Stroke::parse_outline("KAT").unwrap()[0];

        assert!(screen.accept_raw_outline(stroke));
        assert_eq!(screen.search, "KAT");
    }

    #[test]
    fn raw_steno_builds_a_multi_stroke_outline_in_the_search_box() {
        let mut screen = DictionaryScreen {
            search: "WEL".to_owned(),
            search_focused: true,
            ..DictionaryScreen::new()
        };
        let stroke = Stroke::parse_outline("KO*PL").unwrap()[0];

        assert!(screen.accept_raw_outline(stroke));
        assert_eq!(screen.search, "WEL/KO*PL");
    }

    #[test]
    fn an_unfocused_search_box_refuses_raw_steno() {
        let mut screen = DictionaryScreen::new();
        let stroke = Stroke::parse_outline("KAT").unwrap()[0];

        assert!(!screen.accept_raw_outline(stroke));
        assert_eq!(screen.search, "");
    }

    #[test]
    fn the_outline_column_never_collapses_on_a_narrow_window() {
        let narrow = columns(200.0);
        assert!(narrow.outline >= 90.0);
        let wide = columns(1400.0);
        assert!(wide.word > wide.outline, "the word gets the most room");
    }

    #[test]
    fn a_shift_click_selects_the_range_the_table_is_showing() {
        // Not the order the index stores. The rows here are deliberately not
        // in id order.
        let rows = [7u32, 3, 9, 1, 5];
        let mut screen = DictionaryScreen::new();

        screen.select_range(&rows, 3, 1);

        let picked: Vec<u32> = screen.selected.iter().copied().collect();
        assert_eq!(picked, [1, 3, 9], "3, 9 and 1 as displayed, sorted by id");
    }

    #[test]
    fn a_shift_click_works_in_either_direction() {
        let rows = [7u32, 3, 9, 1, 5];
        let mut screen = DictionaryScreen::new();

        screen.select_range(&rows, 1, 3);

        let picked: Vec<u32> = screen.selected.iter().copied().collect();
        assert_eq!(picked, [1, 3, 9]);
    }

    #[test]
    fn a_shift_click_past_a_filtered_out_anchor_starts_again() {
        let rows = [7u32, 3, 9];
        let mut screen = DictionaryScreen::new();

        // 42 is no longer on screen, so there is no range to measure.
        screen.select_range(&rows, 42, 9);

        let picked: Vec<u32> = screen.selected.iter().copied().collect();
        assert_eq!(picked, [9]);
        assert_eq!(screen.anchor, Some(9));
    }

    #[test]
    fn two_entries_in_one_dictionary_can_be_swapped() {
        let dict = temp_dict("pair", "{\n\"KAT\": \"cat\",\n\"KAERT\": \"cart\"\n}\n");
        let index = index_of(&[&dict]);
        let (a, b) = (id_of(&index, "KAT"), id_of(&index, "KAERT"));

        let mut screen = DictionaryScreen::new();
        screen.selected.extend([a, b]);

        let pair = screen.selectable_pair(&index).unwrap();
        assert_eq!(pair, (a.min(b), a.max(b)));
    }

    #[test]
    fn two_entries_in_different_dictionaries_cannot_be_swapped() {
        let english = temp_dict("pairen", "{\n\"KAT\": \"cat\"\n}\n");
        let dutch = temp_dict("pairnl", "{\n\"KAT\": \"kat\"\n}\n");
        let index = index_of(&[&english, &dutch]);

        let mut screen = DictionaryScreen::new();
        screen.selected.extend(index.rows().iter().copied());
        assert_eq!(screen.selected.len(), 2);

        assert!(
            screen.selectable_pair(&index).is_none(),
            "a swap across two files cannot be one verified write"
        );
    }

    #[test]
    fn a_swap_needs_exactly_two() {
        let dict = temp_dict(
            "three",
            "{\n\"KAT\": \"cat\",\n\"KAERT\": \"cart\",\n\"TKOG\": \"dog\"\n}\n",
        );
        let index = index_of(&[&dict]);
        let mut screen = DictionaryScreen::new();

        assert!(screen.selectable_pair(&index).is_none(), "none selected");

        screen.selected.insert(id_of(&index, "KAT"));
        assert!(screen.selectable_pair(&index).is_none(), "one selected");

        screen.selected.insert(id_of(&index, "KAERT"));
        assert!(screen.selectable_pair(&index).is_some());

        screen.selected.insert(id_of(&index, "TKOG"));
        assert!(
            screen.selectable_pair(&index).is_none(),
            "a third selection must not swap an arbitrary pair"
        );
    }

    #[test]
    fn a_swap_is_not_limited_to_star_variants() {
        // KAT and TKOG share no keys at all. The user asked for this
        // explicitly: any two entries in one file can be swapped.
        let dict = temp_dict("unrelated", "{\n\"KAT\": \"cat\",\n\"TKOG\": \"dog\"\n}\n");
        let index = index_of(&[&dict]);
        let mut screen = DictionaryScreen::new();
        screen
            .selected
            .extend([id_of(&index, "KAT"), id_of(&index, "TKOG")]);

        assert!(screen.selectable_pair(&index).is_some());
    }

    #[test]
    fn clearing_the_selection_also_drops_a_pending_delete() {
        let mut screen = DictionaryScreen::new();
        screen.selected.extend([1, 2]);
        screen.anchor = Some(1);
        screen.confirm_delete = true;

        screen.clear_selection();

        assert!(screen.selected.is_empty());
        assert_eq!(screen.anchor, None);
        assert!(!screen.confirm_delete, "a stale confirm must not survive");
    }
}
