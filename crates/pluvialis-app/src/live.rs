//! The live type window: the document, the tape strip, and a temporary dev
//! input for driving strokes before a machine exists.
//!
//! The document is a pure function of the translator's history. Every stroke
//! reformats the whole history rather than patching the widget, which is what
//! makes retroactive correction (`WEL` then `KO*PL` becoming "welcome") work
//! without any special case. Formatting 1000 translations costs microseconds.
//!
//! Untranslated strokes are painted red by a custom layouter fed from
//! [`Formatted::raw_ranges`]. The ranges live in the document model, not in a
//! render time buffer, so red survives scrolling and reflow and disappears only
//! when the stroke that produced it is undone.

use std::sync::Arc;

use eframe::egui;
use egui::text::{ByteIndex, LayoutJob, LayoutSection};
use egui::{Color32, FontId, TextFormat};

use pluvialis_core::format::{Formatted, format};
use pluvialis_core::{Delta, Dictionary, DictionaryStack, Stroke, Translation, Translator};
use pluvialis_machine::{MachineEvent, MachineStatus, Scanner, all_machines};

/// Raw steno that found no dictionary entry.
///
/// Two shades because the surrounding text takes its colour from the theme:
/// one red cannot stay readable against both a white and a near black
/// background, and unreadable red steno defeats the whole point of marking it.
const RAW_COLOR_LIGHT: Color32 = Color32::from_rgb(0xC0, 0x2A, 0x1E);
const RAW_COLOR_DARK: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5B);

fn raw_color(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        RAW_COLOR_DARK
    } else {
        RAW_COLOR_LIGHT
    }
}

/// A connected writer. Green reads the same on both themes at this weight.
const CONNECTED_COLOR: Color32 = Color32::from_rgb(0x2E, 0xA0, 0x43);

const DOCUMENT_FONT_SIZE: f32 = 18.0;

/// One line of the tape: what was written, and what it produced.
struct TapeEntry {
    outline: String,
    result: String,
}

pub struct LiveView {
    dictionaries: DictionaryStack,
    translator: Translator,
    formatted: Formatted,

    /// Memoised layout, with the raw colour it was built for so a theme
    /// switch rebuilds it. The layouter runs at least once per frame, so the
    /// colour sections are computed only when the document actually changes.
    /// Without this the cost grows with document length at 60 fps and presents
    /// as vague sluggishness rather than as anything pointing here.
    layout: Option<(Color32, Arc<LayoutJob>)>,

    tape: Vec<TapeEntry>,
    /// How many of `formatted.events` have been logged, so a reformat does not
    /// re-log the whole history every stroke.
    events_logged: usize,

    loaded: Vec<String>,
    load_error: Option<String>,

    /// Kept alive so the machine thread keeps running; dropping it stops the
    /// scan.
    _scanner: Option<Scanner>,
    machine_events: Option<crossbeam_channel::Receiver<MachineEvent>>,
    machine_status: MachineStatus,
}

impl LiveView {
    pub fn new() -> Self {
        let mut view = LiveView {
            dictionaries: DictionaryStack::new(),
            translator: Translator::new(),
            formatted: Formatted::default(),
            layout: None,
            tape: Vec::new(),
            events_logged: 0,
            loaded: Vec::new(),
            load_error: None,
            _scanner: None,
            machine_events: None,
            machine_status: MachineStatus::Searching,
        };
        view.load_dictionaries();
        view
    }

    /// Start the Auto scanner.
    ///
    /// Strokes arrive on the machine thread, but egui only runs when it has a
    /// reason to. The forwarding thread exists to give it one: it wakes the UI
    /// on each event, so the alternative (repainting continuously on the chance
    /// a stroke arrives) is not needed. It also keeps egui out of
    /// `pluvialis-machine`, which has to stay portable.
    pub fn start_machines(&mut self, ctx: &egui::Context) {
        let (machine_tx, machine_rx) = crossbeam_channel::unbounded();
        let (app_tx, app_rx) = crossbeam_channel::unbounded();

        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("pluvialis-wake".to_owned())
            .spawn(move || {
                for event in machine_rx {
                    if app_tx.send(event).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawning the wake thread");

        self._scanner = Some(Scanner::spawn(all_machines(), machine_tx));
        self.machine_events = Some(app_rx);
    }

    /// Drain whatever the machine thread has produced. Called once per frame.
    pub fn pump_machine(&mut self) {
        let Some(events) = &self.machine_events else {
            return;
        };
        // Collect first so the borrow ends before `apply` needs `&mut self`.
        let batch: Vec<MachineEvent> = events.try_iter().collect();

        for event in batch {
            match event {
                MachineEvent::Stroke(stroke) => self.apply(stroke),
                MachineEvent::Status(status) => {
                    log::info!("machine status: {status:?}");
                    self.machine_status = status;
                }
            }
        }
    }

    /// Load the user's real dictionaries, in priority order.
    ///
    /// A missing or broken dictionary is reported in the status bar rather than
    /// preventing startup: a program that refuses to open because one of two
    /// dictionaries moved is worse than one that says so and keeps working.
    fn load_dictionaries(&mut self) {
        for name in crate::cli::DICTIONARIES {
            let path = std::path::Path::new(crate::cli::DICTIONARY_DIR).join(name);
            match Dictionary::load(&path) {
                Ok(dictionary) => {
                    let bad = dictionary.bad_keys().len();
                    if bad > 0 {
                        log::warn!("{name}: {bad} keys are not valid steno and were skipped");
                    }
                    self.loaded
                        .push(format!("{name} ({} entries)", dictionary.len()));
                    self.dictionaries.push(dictionary);
                }
                Err(e) => {
                    log::error!("could not load {}: {e}", path.display());
                    let message = format!("{name}: {e}");
                    match &mut self.load_error {
                        Some(existing) => {
                            existing.push_str("; ");
                            existing.push_str(&message);
                        }
                        None => self.load_error = Some(message),
                    }
                }
            }
        }
    }

    /// Feed one stroke through the translator and rebuild the document.
    ///
    /// This is the seam M4a plugs the machine into: everything below the
    /// translator is already stroke driven, so the dev box and a real writer
    /// enter by the same door.
    pub fn apply(&mut self, stroke: Stroke) {
        let delta = self.translator.translate(&self.dictionaries, stroke);
        self.tape.push(TapeEntry {
            outline: Stroke::render_outline(&[stroke]),
            result: describe(stroke, &delta),
        });
        self.reformat();
    }

    fn reformat(&mut self) {
        self.formatted = format(self.translator.history());
        self.layout = None;

        for meta in &self.formatted.unknown_metas {
            log::warn!("unimplemented meta command {{{meta}}}");
        }

        // Key combos and PLOVER: commands are consumed by the output layer in
        // M5. Until then, seeing them go past confirms they are being parsed
        // out of the text rather than typed into the document.
        if self.formatted.events.len() > self.events_logged {
            for event in &self.formatted.events[self.events_logged..] {
                log::info!("event (not yet dispatched, M5): {event:?}");
            }
        }
        self.events_logged = self.formatted.events.len();
    }

    fn clear(&mut self) {
        self.translator.clear();
        self.tape.clear();
        self.events_logged = 0;
        self.reformat();
    }

    /// The memoised layout job, rebuilt only when the document or theme
    /// changed.
    fn layout_job(&mut self, raw: Color32) -> Arc<LayoutJob> {
        if let Some((color, job)) = &self.layout
            && *color == raw
        {
            return job.clone();
        }
        let job = Arc::new(highlight(&self.formatted, raw));
        self.layout = Some((raw, job.clone()));
        job
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // egui 0.35 unified SidePanel and TopBottomPanel into `Panel`.
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("tape")
            .resizable(true)
            .default_size(210.0)
            .show(ui, |ui| self.tape_strip(ui));
        egui::CentralPanel::default().show(ui, |ui| self.document(ui));
    }

    fn document(&mut self, ui: &mut egui::Ui) {
        let job = self.layout_job(raw_color(ui.visuals()));
        let mut layouter = move |ui: &egui::Ui, _buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = (*job).clone();
            job.wrap.max_width = wrap_width;
            ui.fonts_mut(|f| f.layout_job(job))
        };

        // Read only for now. The document is regenerated from translator
        // history on every stroke, so manual edits would be silently discarded
        // by the next stroke. Editing at the caret is M5, where the router and
        // a real document model arrive together.
        let mut text: &str = &self.formatted.text;

        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_width(f32::INFINITY)
                        .layouter(&mut layouter),
                );
            });
    }

    fn tape_strip(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong("Tape");
            if ui.button("Clear").clicked() {
                self.clear();
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &self.tape {
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(&entry.outline);
                        ui.weak(&entry.result);
                    });
                }
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            // "Searching" is a normal resting state, not a failure, so it is
            // worded and coloured as ordinary status. The whole point of the
            // scanner is that no user action is ever required.
            match &self.machine_status {
                MachineStatus::Searching => {
                    ui.label("Searching for a writer");
                }
                MachineStatus::Connected { machine, port } => {
                    ui.colored_label(CONNECTED_COLOR, format!("{machine} on {port}"));
                }
                MachineStatus::Disconnected { reason } => {
                    ui.label(format!("Writer disconnected ({reason}), searching"));
                }
            }
            ui.separator();
            match &self.load_error {
                Some(error) => {
                    let color = raw_color(ui.visuals());
                    ui.colored_label(color, error);
                }
                None => {
                    ui.label(format!(
                        "{} entries: {}",
                        self.dictionaries.entry_count(),
                        self.loaded.join(", ")
                    ));
                }
            }
        });
        ui.add_space(2.0);
    }
}

/// Describe what one stroke did, for the tape.
fn describe(stroke: Stroke, delta: &Delta) -> String {
    let joined = |translations: &[Translation]| {
        translations
            .iter()
            .map(Translation::output)
            .collect::<Vec<_>>()
            .join(" ")
    };

    if stroke.is_undo() {
        if delta.removed.is_empty() {
            return "nothing to undo".to_owned();
        }
        return match delta.added.is_empty() {
            true => "undo".to_owned(),
            // Undoing a retroactive correction restores what it had replaced.
            false => format!("undo, back to {}", joined(&delta.added)),
        };
    }

    let added = joined(&delta.added);
    if delta.added.iter().all(Translation::is_untranslated) {
        return "no entry".to_owned();
    }
    if delta.removed.is_empty() {
        added
    } else {
        format!("{added}, replacing {}", joined(&delta.removed))
    }
}

/// Build the coloured layout for the document.
///
/// Ranges are validated rather than trusted: formatting can rewrite earlier
/// text (a suffix rule turning "run" into "running" truncates and re-appends),
/// so a range recorded earlier can end up stale. A stale range that splits a
/// UTF-8 character would panic inside the layouter, which is a poor way to find
/// out about it.
fn highlight(formatted: &Formatted, raw_color: Color32) -> LayoutJob {
    let text = &formatted.text;
    let font = FontId::proportional(DOCUMENT_FONT_SIZE);

    let mut job = LayoutJob {
        text: text.clone(),
        break_on_newline: true,
        ..Default::default()
    };

    let section = |job: &mut LayoutJob, range: std::ops::Range<usize>, raw: bool| {
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(range.start)..ByteIndex(range.end),
            format: TextFormat {
                font_id: font.clone(),
                // PLACEHOLDER tells the painter to substitute the theme's
                // normal text colour, so ordinary text follows light or dark
                // mode without us tracking it.
                color: if raw { raw_color } else { Color32::PLACEHOLDER },
                ..Default::default()
            },
        });
    };

    let mut cursor = 0usize;
    for &(start, end) in &formatted.raw_ranges {
        if start < cursor || end > text.len() || start >= end {
            continue;
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            log::warn!("raw range {start}..{end} is not on a character boundary, ignoring");
            continue;
        }
        if start > cursor {
            section(&mut job, cursor..start, false);
        }
        section(&mut job, start..end, true);
        cursor = end;
    }
    if cursor < text.len() {
        section(&mut job, cursor..text.len(), false);
    }

    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which red is used is a theme question; these tests are about where it
    /// lands.
    const RED: Color32 = RAW_COLOR_LIGHT;

    fn formatted(text: &str, raw_ranges: Vec<(usize, usize)>) -> Formatted {
        Formatted {
            text: text.to_owned(),
            raw_ranges,
            ..Default::default()
        }
    }

    /// The sections must tile the text exactly, or characters go missing on
    /// screen without any error.
    fn assert_covers(job: &LayoutJob, text: &str) {
        let mut at = 0;
        for section in &job.sections {
            assert_eq!(section.byte_range.start.0, at, "gap or overlap in sections");
            at = section.byte_range.end.0;
        }
        assert_eq!(at, text.len(), "sections do not reach the end");
    }

    #[test]
    fn raw_ranges_are_painted_red_and_the_rest_is_not() {
        let f = formatted("cat KAT dog", vec![(4, 7)]);
        let job = highlight(&f, RED);
        assert_covers(&job, &f.text);
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].format.color, Color32::PLACEHOLDER);
        assert_eq!(job.sections[1].format.color, RED);
        let red = &job.sections[1].byte_range;
        assert_eq!(&f.text[red.start.0..red.end.0], "KAT");
        assert_eq!(job.sections[2].format.color, Color32::PLACEHOLDER);
    }

    #[test]
    fn text_with_no_raw_steno_is_one_section() {
        let f = formatted("hello world", Vec::new());
        let job = highlight(&f, RED);
        assert_covers(&job, &f.text);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, Color32::PLACEHOLDER);
    }

    #[test]
    fn empty_document_produces_no_sections() {
        let job = highlight(&formatted("", Vec::new()), RED);
        assert!(job.sections.is_empty());
    }

    #[test]
    fn a_document_that_is_entirely_raw_steno_is_one_red_section() {
        let f = formatted("KAT", vec![(0, 3)]);
        let job = highlight(&f, RED);
        assert_covers(&job, &f.text);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, RED);
    }

    #[test]
    fn adjacent_raw_ranges_do_not_produce_an_empty_section_between_them() {
        let f = formatted("KATTKOG", vec![(0, 3), (3, 7)]);
        let job = highlight(&f, RED);
        assert_covers(&job, &f.text);
        assert_eq!(job.sections.len(), 2);
        assert!(job.sections.iter().all(|s| s.format.color == RED));
    }

    #[test]
    fn stale_ranges_are_dropped_rather_than_panicking() {
        // Past the end, backwards, and overlapping the previous range: all
        // reachable if formatting rewrote earlier text.
        let f = formatted("short", vec![(0, 2), (1, 3), (4, 99), (3, 3)]);
        let job = highlight(&f, RED);
        assert_covers(&job, &f.text);
    }

    #[test]
    fn a_range_splitting_a_character_is_ignored() {
        // The pound sign is two bytes, so 1 is not a character boundary.
        let f = formatted("\u{00A3}5", vec![(1, 2)]);
        let job = highlight(&f, RED);
        assert_covers(&job, &f.text);
        assert!(job.sections.iter().all(|s| s.format.color != RED));
    }
}
