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

use eframe::egui;

use pluvialis_core::Stroke;

use crate::entry_index::{EntryIndex, Query, Sort};

/// What the screen wants done. The screen can see the entries but not write to
/// them, so it says what it wants and the caller, which owns the dictionaries,
/// carries it out.
#[derive(Default)]
pub enum Action {
    #[default]
    None,
    /// Put this entry in the editor below.
    Load(u32),
}

#[derive(Default)]
pub struct DictionaryScreen {
    search: String,
    sort: Sort,
    descending: bool,
    filter: Option<u16>,
    /// The row highlighted and loaded in the editor, by index id.
    selected: Option<u32>,
    search_focused: bool,
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

    /// Point the table at one entry, for callers that changed it elsewhere.
    pub fn select(&mut self, id: Option<u32>) {
        self.selected = id;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, index: &mut EntryIndex) -> Action {
        let mut action = Action::None;

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
            let all = self.filter.is_none();
            if ui.selectable_label(all, "all").clicked() {
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

        index.refresh(&Query {
            text: self.search.clone(),
            sort: self.sort,
            descending: self.descending,
            dictionary: self.filter,
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let matches = index.total_matches();
            let total = index.total_entries();
            let label = match self.search.trim().is_empty() && self.filter.is_none() {
                true => format!("{} entries", thousands(total)),
                false => format!("{} of {} entries", thousands(matches), thousands(total)),
            };
            ui.label(egui::RichText::new(label).weak());
        });

        ui.add_space(2.0);
        self.header(ui);
        ui.separator();

        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
        let widths = columns(ui.available_width());
        // Immutable from here down: the query has already been answered.
        let index: &EntryIndex = index;
        let rows = index.rows();

        if rows.is_empty() {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Nothing matches that.").weak());
            return action;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for position in range {
                    let Some(id) = rows.get(position) else {
                        continue;
                    };
                    let Some(entry) = index.entry(*id) else {
                        continue;
                    };
                    let selected = self.selected == Some(*id);
                    if row(
                        ui,
                        entry,
                        index.name(entry.dictionary),
                        &widths,
                        selected,
                        row_height,
                    ) {
                        self.selected = Some(*id);
                        action = Action::Load(*id);
                    }
                }
            });

        action
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

struct Columns {
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
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped_for_reading() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(101_419), "101,419");
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
}
