//! The live type window: the editable document, the tape strip, and the status
//! bar.
//!
//! Up to M4b the document was a pure function of the translator's history, and
//! every stroke replaced the whole text. That made retroactive correction free
//! but allowed no caret: steno could only land at the end, and anything typed
//! by hand was discarded by the next stroke.
//!
//! Since M5 the formatter is unchanged and still formats the entire history,
//! but its output is a *shadow* of what steno has produced rather than the
//! document itself. Each stroke diffs the previous shadow against the new one
//! to get "delete this much, insert this", and [`Document`] applies that edit
//! at the caret. So retroactive correction still works, and the user can put
//! the caret mid sentence and write there, or type by hand, without the two
//! fighting.
//!
//! Untranslated strokes are painted red by a custom layouter fed from the
//! document's raw ranges. Those ranges live in the document model, not in a
//! render time buffer, and every edit shifts, trims or splits them, so red
//! stays attached to the characters it belongs to through insertions,
//! deletions and reflow.

use std::sync::Arc;

use eframe::egui;
use egui::text::{ByteIndex, LayoutJob, LayoutSection};
use egui::{Color32, FontId, TextFormat};

use pluvialis_core::format::{Event, Formatted, format};
use pluvialis_core::{
    Delta, Dictionary, DictionaryStack, Document, Stroke, Translation, Translator, steno_edit,
};
use pluvialis_core::document::StenoEdit;
use pluvialis_machine::{MachineEvent, MachineStatus, Scanner, all_machines};

/// Where an output batch goes.
///
/// Decided by window focus at the moment the batch is produced, and each batch
/// goes to exactly one of these. Never both: that is what makes double typing
/// impossible by construction rather than an intermittent bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// Pluvialis has focus, so steno lands in the document at the caret.
    Document,
    /// Something else has focus, so steno is typed as real keystrokes.
    OtherWindow,
}

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

    /// The editable text the user sees. Steno lands at its caret.
    document: Document,

    /// The formatter's last output.
    ///
    /// Not the document: it is a shadow of what steno alone has produced, kept
    /// so the next stroke can be diffed against it. Keeping them separate is
    /// what lets the user edit the document by hand without the next stroke
    /// undoing their edit, since the diff is computed against this rather than
    /// against what is on screen.
    shadow: Formatted,

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

    /// Whether our own window had focus as of this frame.
    focused: bool,
    /// Where the previous batch went, so a batch that crosses the boundary does
    /// not try to backspace into the other destination's text.
    last_destination: Destination,
    /// The tray toggle. With this off, steno still translates and still shows
    /// on the tape, but nothing is typed anywhere.
    output_enabled: bool,

    dictionary_pane: crate::dictionaries::DictionaryPane,
    show_dictionaries: bool,

    storage: crate::storage::Storage,
    last_autosave: std::time::Instant,
    save_error: Option<String>,
    /// Snapshots offered after an unclean exit, newest first. Empty once the
    /// user has answered.
    recovery: Vec<crate::storage::Snapshot>,
    #[cfg(windows)]
    keyboard: pluvialis_output::Keyboard,

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
            document: Document::new(),
            shadow: Formatted::default(),
            layout: None,
            tape: Vec::new(),
            events_logged: 0,
            loaded: Vec::new(),
            load_error: None,
            focused: true,
            last_destination: Destination::Document,
            output_enabled: true,
            dictionary_pane: crate::dictionaries::DictionaryPane::new(),
            show_dictionaries: false,
            storage: crate::storage::Storage::new(documents_dir()),
            last_autosave: std::time::Instant::now(),
            save_error: None,
            recovery: Vec::new(),
            #[cfg(windows)]
            keyboard: pluvialis_output::Keyboard::new(),
            _scanner: None,
            machine_events: None,
            machine_status: MachineStatus::Searching,
        };
        view.load_dictionaries();
        view.begin_session();
        view
    }

    /// Set the current document going and find out whether the last run
    /// crashed.
    fn begin_session(&mut self) {
        self.storage
            .set_current(self.storage.documents_dir().join("untitled.md"));

        match self.storage.begin_session() {
            Ok(false) => {}
            Ok(true) => {
                // A marker left behind means the previous run did not exit
                // cleanly. Offer what was saved rather than restoring it
                // silently: the user may well prefer the blank page.
                self.recovery = self.storage.snapshots();
                if !self.recovery.is_empty() {
                    log::warn!(
                        "the last session ended without a clean exit, {} snapshots available",
                        self.recovery.len()
                    );
                }
            }
            Err(e) => {
                log::error!("could not open the documents folder: {e}");
                self.save_error = Some(e.to_string());
            }
        }
    }

    /// Save if the text has changed and the interval has elapsed.
    ///
    /// Called every frame; the interval and the dirty check make that cheap.
    fn autosave(&mut self) {
        if self.last_autosave.elapsed() < self.storage.autosave_interval {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        self.save_now();
    }

    fn save_now(&mut self) {
        match self.storage.save(self.document.text()) {
            Ok(true) => {
                self.save_error = None;
                log::debug!("saved");
            }
            Ok(false) => {}
            Err(e) => {
                log::error!("could not save: {e}");
                self.save_error = Some(e.to_string());
            }
        }
    }

    /// Save and clear the running marker, so the next start knows this was a
    /// clean exit.
    pub fn shutdown(&mut self) {
        self.save_now();
        self.storage.end_session();
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
    ///
    /// Focus is sampled here, before any stroke is handled, so a whole batch is
    /// routed by the focus that was true when it arrived rather than by
    /// whatever happens to be true partway through.
    pub fn pump_machine(&mut self, ctx: &egui::Context) {
        let was_focused = self.focused;
        self.focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));

        // Losing focus usually means the user has gone to another program,
        // which is exactly when unsaved work is most likely to be forgotten
        // about.
        if was_focused && !self.focused {
            self.save_now();
        }
        self.autosave();

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

        self.load_python_dictionaries();
    }

    /// Find Plover Python dictionaries and load them **disabled**.
    ///
    /// Disabled on purpose. The user asked to be able to import any Python
    /// dictionary and enable or disable it, and separately said she is not sure
    /// she wants jeff-phrasing yet. Loading them off satisfies both: they are
    /// there in the list with a checkbox, and nothing about her writing changes
    /// until she ticks one. Do not flip this default without asking.
    fn load_python_dictionaries(&mut self) {
        let directory = std::path::Path::new(crate::cli::DICTIONARY_DIR);
        let Ok(entries) = std::fs::read_dir(directory) else {
            log::warn!("could not scan {} for dictionaries", directory.display());
            return;
        };

        let mut found: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "py"))
            .collect();
        found.sort();

        for path in found {
            match pluvialis_python::PythonDictionary::load(&path) {
                Ok(mut dictionary) => {
                    use pluvialis_core::ProgrammaticDictionary;
                    dictionary.set_enabled(false);
                    log::info!("found Python dictionary {} (disabled)", dictionary.name());
                    self.dictionaries.push_programmatic(Box::new(dictionary));
                }
                // A dictionary that will not load is worth reporting by name,
                // but must not stop the others or the program.
                Err(e) => log::warn!("could not load {}: {e}", path.display()),
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

    /// Reformat the whole stroke history and fold the change into the document
    /// at the caret.
    ///
    /// The formatter still works on the entire history, which is what makes
    /// retroactive correction (`WEL` then `KO*PL` becoming "welcome") fall out
    /// for free. What changed at M5 is that its output is diffed against the
    /// previous output rather than replacing the document, so the resulting
    /// edit can be applied wherever the caret happens to be.
    fn reformat(&mut self) {
        let next = format(self.translator.history());
        let mut edit = steno_edit(&self.shadow, &next);

        let destination = self.destination();
        if !edit.is_empty() {
            self.deliver(&mut edit, destination);
        }
        self.shadow = next;
        self.dispatch_events(destination);

        for meta in &self.shadow.unknown_metas {
            log::warn!("unimplemented meta command {{{meta}}}");
        }

        // Key combos and PLOVER: commands are consumed by the output layer in
        // M5. Until then, seeing them go past confirms they are being parsed
        // out of the text rather than typed into the document.
    }

    fn destination(&self) -> Destination {
        match self.focused {
            true => Destination::Document,
            false => Destination::OtherWindow,
        }
    }

    /// Perform key combos and application commands produced by this batch.
    ///
    /// Combos are only sent when another window has focus. `{#Control_L(Left)}`
    /// means "press these keys in whatever you are typing into"; synthesising
    /// it while Pluvialis itself is focused would send it to our own text
    /// widget, which is not what any dictionary entry means by it.
    fn dispatch_events(&mut self, destination: Destination) {
        if self.shadow.events.len() <= self.events_logged {
            return;
        }
        let fresh: Vec<Event> = self.shadow.events[self.events_logged..].to_vec();
        self.events_logged = self.shadow.events.len();

        for event in fresh {
            match event {
                Event::KeyCombo(spec) => {
                    if destination != Destination::OtherWindow || !self.output_enabled {
                        log::debug!("key combo {{#{spec}}} ignored: not typing into another window");
                        continue;
                    }
                    match pluvialis_output::parse_combo(&spec) {
                        // An unknown key name is reported by name rather than
                        // dropped, so a dictionary entry that cannot work is
                        // discoverable instead of merely inert.
                        Err(e) => log::warn!("key combo {{#{spec}}}: {e}"),
                        Ok(chords) => {
                            #[cfg(windows)]
                            if let Err(e) = self.keyboard.send_combo(&chords) {
                                log::warn!("could not send key combo {{#{spec}}}: {e}");
                            }
                            #[cfg(not(windows))]
                            let _ = chords;
                        }
                    }
                }
                Event::Command(command) => {
                    log::info!("application command {{PLOVER:{command}}} is not implemented");
                }
            }
        }
    }

    /// Send one batch to exactly one destination.
    fn deliver(&mut self, edit: &mut StenoEdit, destination: Destination) {
        // A correction's backspaces refer to text the *previous* batch wrote.
        // If that went somewhere else, deleting here would eat characters this
        // program never wrote, in someone else's document. Drop them and keep
        // only the insertion.
        if destination != self.last_destination && (edit.backspaces > 0 || edit.backspace_keys > 0)
        {
            log::debug!(
                "focus changed mid correction, dropping {} backspaces rather than \
                 deleting text in the other destination",
                edit.backspace_keys
            );
            edit.backspaces = 0;
            edit.backspace_keys = 0;
        }
        self.last_destination = destination;

        match destination {
            Destination::Document => {
                self.document.apply(edit);
                self.layout = None;
            }
            Destination::OtherWindow => {
                if !self.output_enabled {
                    return;
                }
                #[cfg(windows)]
                if let Err(e) = self.keyboard.send_edit(edit.backspace_keys, &edit.text) {
                    log::warn!("could not type into the focused window: {e}");
                }
            }
        }
    }

    fn clear(&mut self) {
        self.translator.clear();
        self.tape.clear();
        self.events_logged = 0;
        self.document.clear();
        self.shadow = Formatted::default();
        self.layout = None;
    }

    /// The memoised layout job, rebuilt only when the document or theme
    /// changed.
    fn layout_job(&mut self, raw: Color32) -> Arc<LayoutJob> {
        if let Some((color, job)) = &self.layout
            && *color == raw
        {
            return job.clone();
        }
        let job = Arc::new(highlight(
            self.document.text(),
            self.document.raw_ranges(),
            raw,
        ));
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

        if self.show_dictionaries {
            egui::Panel::left("dictionaries")
                .resizable(true)
                .default_size(260.0)
                .show(ui, |ui| {
                    self.dictionary_pane.ui(ui, &mut self.dictionaries);
                });
        }

        egui::CentralPanel::default().show(ui, |ui| self.document(ui));
        self.recovery_prompt(ui);
    }

    fn document(&mut self, ui: &mut egui::Ui) {
        let job = self.layout_job(raw_color(ui.visuals()));
        let mut layouter = move |ui: &egui::Ui, _buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = (*job).clone();
            job.wrap.max_width = wrap_width;
            ui.fonts_mut(|f| f.layout_job(job))
        };

        // The widget edits its own copy; the document is the source of truth
        // and takes the result back below.
        let mut text = self.document.text().to_owned();

        // Follow the end only while the caret is actually there, which is the
        // common case of writing forwards. Sticking unconditionally would drag
        // the view to the bottom every stroke and make working mid document
        // impossible; never sticking would stop the text following the user as
        // they write.
        let caret_at_end = self.document.caret() == self.document.text().len();

        let output = egui::ScrollArea::vertical()
            .stick_to_bottom(caret_at_end)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::TextEdit::multiline(&mut text)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;

        // Typing, pasting and deleting all arrive as a changed string rather
        // than as an edit, so the document recovers the change by diffing and
        // keeps the red ranges attached to their characters.
        if text != self.document.text() {
            self.document.reconcile(&text);
            self.layout = None;
        }

        // egui counts characters, the document counts bytes.
        if let Some(cursor) = output.cursor_range {
            self.document.set_caret_char(cursor.primary.index.0);
        }
    }

    /// Offer the newest snapshot after an unclean exit.
    ///
    /// Offered, never applied automatically: restoring on top of a blank page
    /// the user meant to start with would be its own kind of data loss.
    fn recovery_prompt(&mut self, ui: &mut egui::Ui) {
        if self.recovery.is_empty() {
            return;
        }

        let mut restore = false;
        let mut dismiss = false;

        egui::Window::new("Recover unsaved work")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label("The last session ended without closing properly.");
                ui.label(format!(
                    "{} saved versions are available.",
                    self.recovery.len()
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Restore the newest").clicked() {
                        restore = true;
                    }
                    if ui.button("Start fresh").clicked() {
                        dismiss = true;
                    }
                });
            });

        if restore && let Some(snapshot) = self.recovery.first().cloned() {
            match self.storage.read_snapshot(&snapshot) {
                Ok(text) => {
                    // Straight into the document: recovered text is ordinary
                    // text with no steno history behind it, so it carries no
                    // red and the shadow stays empty.
                    self.document.reconcile(&text);
                    self.document.set_caret(text.len());
                    self.layout = None;
                    log::info!("recovered {} characters", text.len());
                }
                Err(e) => {
                    log::error!("could not read the snapshot: {e}");
                    self.save_error = Some(e.to_string());
                }
            }
        }
        if restore || dismiss {
            self.recovery.clear();
        }
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

            // Where the next stroke will go, which is decided by focus and is
            // otherwise invisible. Worth showing, because "why is nothing
            // appearing" is almost always this.
            let toggle = ui.checkbox(&mut self.output_enabled, "Type into other windows");
            toggle.on_hover_text(
                "When another window has focus, steno is typed into it as real keystrokes.\n\
                 With this off, strokes still translate and still show on the tape, but \
                 nothing is typed anywhere.",
            );

            ui.separator();
            ui.toggle_value(&mut self.show_dictionaries, "Dictionaries")
                .on_hover_text("Priority order, enable and disable, and lookup");

            ui.separator();
            match &self.load_error {
                Some(error) => {
                    let color = raw_color(ui.visuals());
                    ui.colored_label(color, error);
                }
                None => {
                    ui.label(format!("{} entries", self.dictionaries.entry_count()));
                }
            }
        });
        ui.add_space(2.0);
    }
}

/// Where documents and their history live.
///
/// Next to the executable's project folder rather than in AppData, so the user
/// can find, back up and edit them with ordinary tools.
fn documents_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(r"F:\Steno\Pluvialis\documents")
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
fn highlight(text: &str, raw_ranges: &[(usize, usize)], raw_color: Color32) -> LayoutJob {
    let font = FontId::proportional(DOCUMENT_FONT_SIZE);

    let mut job = LayoutJob {
        text: text.to_owned(),
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
    for &(start, end) in raw_ranges {
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

    /// A document's worth of text and its red ranges.
    fn formatted(text: &str, raw_ranges: Vec<(usize, usize)>) -> (String, Vec<(usize, usize)>) {
        (text.to_owned(), raw_ranges)
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
        let job = highlight(&f.0, &f.1, RED);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].format.color, Color32::PLACEHOLDER);
        assert_eq!(job.sections[1].format.color, RED);
        let red = &job.sections[1].byte_range;
        assert_eq!(&f.0[red.start.0..red.end.0], "KAT");
        assert_eq!(job.sections[2].format.color, Color32::PLACEHOLDER);
    }

    #[test]
    fn text_with_no_raw_steno_is_one_section() {
        let f = formatted("hello world", Vec::new());
        let job = highlight(&f.0, &f.1, RED);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, Color32::PLACEHOLDER);
    }

    #[test]
    fn empty_document_produces_no_sections() {
        let empty = formatted("", Vec::new());
        let job = highlight(&empty.0, &empty.1, RED);
        assert!(job.sections.is_empty());
    }

    #[test]
    fn a_document_that_is_entirely_raw_steno_is_one_red_section() {
        let f = formatted("KAT", vec![(0, 3)]);
        let job = highlight(&f.0, &f.1, RED);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, RED);
    }

    #[test]
    fn adjacent_raw_ranges_do_not_produce_an_empty_section_between_them() {
        let f = formatted("KATTKOG", vec![(0, 3), (3, 7)]);
        let job = highlight(&f.0, &f.1, RED);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 2);
        assert!(job.sections.iter().all(|s| s.format.color == RED));
    }

    #[test]
    fn stale_ranges_are_dropped_rather_than_panicking() {
        // Past the end, backwards, and overlapping the previous range: all
        // reachable if formatting rewrote earlier text.
        let f = formatted("short", vec![(0, 2), (1, 3), (4, 99), (3, 3)]);
        let job = highlight(&f.0, &f.1, RED);
        assert_covers(&job, &f.0);
    }

    #[test]
    fn a_range_splitting_a_character_is_ignored() {
        // The pound sign is two bytes, so 1 is not a character boundary.
        let f = formatted("\u{00A3}5", vec![(1, 2)]);
        let job = highlight(&f.0, &f.1, RED);
        assert_covers(&job, &f.0);
        assert!(job.sections.iter().all(|s| s.format.color != RED));
    }
}
