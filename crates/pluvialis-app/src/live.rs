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

use pluvialis_core::document::StenoEdit;
use pluvialis_core::format::{Event, Formatted, format};
use pluvialis_core::{
    Delta, Dictionary, DictionaryStack, Document, Stroke, Translation, Translator, steno_edit,
};
use pluvialis_machine::{MachineEvent, MachineStatus, Scanner, all_machines};

use crate::screens::Screen;

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



/// One line of the tape: what was written, and what it produced.
struct TapeEntry {
    outline: String,
    result: String,
}

/// Drop the oldest tape lines once there are more than `limit`.
///
/// The strip is an `egui::ScrollArea`, and `show` lays out every child each
/// frame whether or not it is on screen, so an uncapped tape costs steadily
/// more per frame the longer the session runs. The strip sticks to the bottom,
/// so the lines dropped are always the ones that have scrolled out of reach.
///
/// Dropping from the front is safe here in a way it is not for the
/// translator's history: nothing reads the tape by index or diffs it against
/// anything, it is only iterated for display and cleared wholesale. Compare
/// `LiveView::resync_after_trim`, where trimming the front of the *history*
/// shifts what the formatter produces and has to be absorbed deliberately.
fn trim_tape(tape: &mut Vec<TapeEntry>, limit: usize) {
    if tape.len() > limit {
        let excess = tape.len() - limit;
        tape.drain(..excess);
    }
}

pub struct LiveView {
    dictionaries: DictionaryStack,
    translator: Translator,

    /// The editable text the user sees. Steno lands at its caret.
    document: Document,

    /// Words written and how fast, for the status bar.
    meter: crate::meter::Meter,

    /// The word count, and the document revision it was counted at.
    ///
    /// The status bar draws every frame and counting words is linear in the
    /// document: 204us at 45,000 words, which is 1.2% of a core at 60 fps and
    /// grows as the day goes on. Comparing the revision instead costs nothing,
    /// so the count is paid once per edit.
    words: (u64, usize),

    /// The document's caret has moved for a reason the text widget does not
    /// know about, so the widget's own cursor must be told rather than asked.
    ///
    /// The widget tracks a cursor of its own and only updates it in response to
    /// clicks and keystrokes. A stroke changes the text behind its back, so its
    /// cursor stays where it was, and reading it back would undo the move the
    /// stroke just made. That is not hypothetical: it put every new word
    /// *before* the previous one, turning "I can" into "can I".
    push_caret: bool,

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
    layout: Option<(Color32, f32, Arc<LayoutJob>)>,

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
    /// The window this program last typed into, and how many characters it put
    /// there that have not been erased again. Together they decide whether a
    /// correction's backspaces may be sent: only into the same window, and only
    /// as far back as this program's own text goes.
    typed_target: Option<isize>,
    typed_chars: usize,
    /// The tray toggle. With this off, steno still translates and still shows
    /// on the tape, but nothing is typed anywhere.
    output_enabled: bool,

    /// Enabled state, priority order and every setting, as loaded at start
    /// and written back whenever one of them changes.
    config: crate::config::Config,
    /// The stats being recorded this run. Recording is checked before anything
    /// is counted, not before it is shown.
    stats: crate::stats::Stats,
    /// Whether the Settings screen is waiting for the second click that
    /// actually deletes the counts.
    confirm_stats_reset: bool,

    dictionary_pane: crate::dictionaries::DictionaryPane,
    dictionary_screen: crate::dictionary_screen::DictionaryScreen,
    /// Every entry, flattened for the table. Rebuilt only when the entries
    /// themselves change, never on a redraw.
    entry_index: crate::entry_index::EntryIndex,
    /// Where the current batch of strokes goes, decided once in
    /// `pump_machine` and read by `apply`. See `crate::screens::sink`.
    sink: crate::screens::Sink,

    storage: crate::storage::Storage,
    last_autosave: std::time::Instant,
    save_error: Option<String>,
    /// Snapshots offered after an unclean exit, newest first. Empty once the
    /// user has answered.
    recovery: Vec<crate::storage::Snapshot>,
    show_history: bool,
    /// Listed when the window opens rather than every frame, since it reads the
    /// directory.
    history: Vec<crate::storage::Snapshot>,
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
        let config = crate::config::load();
        let stats = crate::stats::Stats::load(config.settings.record_stats);
        let documents = config
            .settings
            .documents_dir
            .clone()
            .unwrap_or_else(documents_dir);

        let mut view = LiveView {
            dictionaries: DictionaryStack::new(),
            translator: Translator::new(),
            document: Document::new(),
            meter: crate::meter::Meter::new(),
            words: (0, 0),
            push_caret: false,
            shadow: Formatted::default(),
            layout: None,
            tape: Vec::new(),
            events_logged: 0,
            loaded: Vec::new(),
            load_error: None,
            focused: true,
            last_destination: Destination::Document,
            typed_target: None,
            typed_chars: 0,
            output_enabled: config.settings.output_at_launch,
            config,
            stats,
            confirm_stats_reset: false,
            dictionary_pane: crate::dictionaries::DictionaryPane::new(),
            dictionary_screen: crate::dictionary_screen::DictionaryScreen::new(),
            entry_index: crate::entry_index::EntryIndex::new(),
            sink: crate::screens::Sink::Document,
            storage: crate::storage::Storage::new(documents),
            last_autosave: std::time::Instant::now(),
            save_error: None,
            recovery: Vec::new(),
            show_history: false,
            history: Vec::new(),
            #[cfg(windows)]
            keyboard: pluvialis_output::Keyboard::new(),
            _scanner: None,
            machine_events: None,
            machine_status: MachineStatus::Searching,
        };
        view.storage.autosave_interval = view.config.settings.autosave_interval();
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

    /// Write the counts if any have changed and the interval has elapsed.
    /// Never per stroke: this sits in the output path.
    fn save_stats(&mut self) {
        self.stats.save_if_due();
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

    /// Pick a file and folder, point autosave there, and save at once.
    ///
    /// The blocking dialog holds the UI thread while it is open, which is what
    /// a desktop Save As does; strokes that arrive meanwhile are queued by the
    /// machine thread and applied when it returns.
    fn save_as(&mut self) {
        // Owned copy so the dialog does not borrow storage while we build it.
        let current = self.storage.current().map(|path| path.to_path_buf());

        let mut dialog = rfd::FileDialog::new()
            .add_filter("Markdown", &["md"])
            .add_filter("Text", &["txt"]);
        if let Some(current) = &current {
            if let Some(parent) = current.parent().filter(|p| !p.as_os_str().is_empty()) {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = current.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
        }

        if let Some(path) = dialog.save_file() {
            self.storage.choose_target(path);
            self.save_now();
        }
    }

    /// Open an existing document, saving the current one first.
    ///
    /// The opened text has no steno behind it, so it is loaded as plain
    /// document content and writing continues from the end of it. Saving and
    /// autosave then target the opened file.
    fn open(&mut self) {
        // Owned so the dialog does not borrow storage while we build it.
        let start_dir = self
            .storage
            .current()
            .and_then(|path| path.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf());

        let mut dialog = rfd::FileDialog::new()
            .add_filter("Text and Markdown", &["md", "txt"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = start_dir {
            dialog = dialog.set_directory(dir);
        }

        let Some(path) = dialog.pick_file() else {
            return;
        };

        match std::fs::read_to_string(&path) {
            Ok(text) => {
                // Do not lose whatever is on screen when switching documents.
                self.save_now();
                self.load_document(text, path);
            }
            Err(e) => {
                log::error!("could not open {}: {e}", path.display());
                self.save_error = Some(format!("could not open {}: {e}", path.display()));
            }
        }
    }

    /// Replace the live document with opened text and retarget saving at it.
    fn load_document(&mut self, text: String, path: std::path::PathBuf) {
        // No steno stands behind opened text, so start the translator and its
        // shadow fresh: the next stroke must append to the loaded text, not try
        // to reconcile it against the previous document's steno output.
        self.translator = Translator::new();
        self.shadow = Formatted::default();
        self.events_logged = 0;
        self.tape.clear();
        self.meter = crate::meter::Meter::new();

        self.document.clear();
        self.document.reconcile(&text);
        // Writing carries on from the end of what was opened.
        let end = self.document.text().len();
        self.document.set_caret(end);
        self.push_caret = true;
        self.layout = None;
        self.words = (
            self.document.revision(),
            crate::meter::count_words(self.document.text()),
        );

        // Save and autosave here from now on, and write a baseline snapshot so
        // the state as opened is itself recoverable.
        self.storage.choose_target(path);
        self.save_now();
    }

    /// The top bar: open or save the live document, or choose where it lives.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if ui
                .button("Open...")
                .on_hover_text("Open an existing document and continue writing in it")
                .clicked()
            {
                self.open();
            }
            // An unnamed document has nowhere chosen to save to, so Save asks
            // first rather than quietly writing the default untitled file.
            if ui
                .button("Save")
                .on_hover_text("Write the document to disk now")
                .clicked()
            {
                if self.storage.is_named() {
                    self.save_now();
                } else {
                    self.save_as();
                }
            }
            if ui
                .button("Save As...")
                .on_hover_text("Choose a file and folder; autosaves and history then go there too")
                .clicked()
            {
                self.save_as();
            }

            ui.separator();
            let name = self
                .storage
                .current_file_name()
                .unwrap_or_else(|| "untitled.md".to_owned());
            if self.storage.is_named() {
                ui.label(name)
                    .on_hover_text("The file this document saves to");
            } else {
                ui.weak(format!("{name} (location not chosen)"))
                    .on_hover_text("Autosaving to the default documents folder until you Save As");
            }
        });
        ui.add_space(2.0);
    }

    /// Save and clear the running marker, so the next start knows this was a
    /// clean exit.
    pub fn shutdown(&mut self) {
        self.stats.save();
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
    pub fn pump_machine(&mut self, ctx: &egui::Context, screen: crate::screens::Screen) {
        let was_focused = self.focused;
        self.focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let field_wants =
            self.dictionary_pane.wants_strokes() || self.dictionary_screen.wants_strokes();
        self.sink = crate::screens::sink(self.focused, screen, field_wants);

        // Losing focus usually means the user has gone to another program,
        // which is exactly when unsaved work is most likely to be forgotten
        // about.
        if was_focused && !self.focused {
            self.save_now();
        }
        self.autosave();
        self.save_stats();

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
        // The library, not her Plover folder. Seeded from it on first run and
        // owned by Pluvialis afterwards; see `library`.
        if let Err(e) = crate::library::ensure() {
            log::error!("could not prepare the dictionary library: {e}");
            self.load_error = Some(format!("dictionary library: {e}"));
            return;
        }

        for path in crate::library::json_dictionaries() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
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
        self.rebuild_entry_index();
        self.apply_saved_enabled();
    }

    /// Find Plover Python dictionaries and load them **disabled**.
    ///
    /// Disabled on purpose. The user asked to be able to import any Python
    /// dictionary and enable or disable it, and separately said she is not sure
    /// she wants jeff-phrasing yet. Loading them off satisfies both: they are
    /// there in the list with a checkbox, and nothing about her writing changes
    /// until she ticks one. Do not flip this default without asking.
    fn load_python_dictionaries(&mut self) {
        for path in crate::library::python_dictionaries() {
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

    /// Import dictionary files the user picked and add them to the live stack.
    ///
    /// Appends rather than rebuilding, so session reordering and any Python
    /// dictionary already enabled survive adding another. A new JSON dictionary
    /// arrives at the lowest priority and can be moved up in the pane. Copying
    /// into the library, validating, and refusing duplicates is `library::import`;
    /// the original file is not moved or changed.
    fn add_dictionaries(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Dictionaries", &["json", "py"])
            .add_filter("All files", &["*"])
            .pick_files()
        else {
            return;
        };

        let mut errors = Vec::new();
        for file in &files {
            match crate::library::import(file) {
                Ok(destination) => self.load_added(&destination, &mut errors),
                Err(e) => errors.push(e.to_string()),
            }
        }

        // Leave any earlier load error in place on success: the new dictionary
        // appearing in the pane is the confirmation that the add worked.
        if !errors.is_empty() {
            self.load_error = Some(errors.join("; "));
        }

        // A dictionary re-added after being removed keeps its remembered state.
        self.apply_saved_enabled();
    }

    /// Set each dictionary's enabled state and priority order from what was
    /// saved last run, keyed by file name. A dictionary the file does not
    /// mention keeps its default, which is on for JSON and off for Python, and
    /// sorts after the ones that are listed.
    fn apply_saved_enabled(&mut self) {
        let saved = &self.config.enabled;
        for entry in self.dictionaries.dictionaries_mut() {
            if let Some(name) = entry.path.file_name()
                && let Some(&enabled) = saved.get(name.to_string_lossy().as_ref())
            {
                entry.enabled = enabled;
            }
        }
        for entry in self.dictionaries.programmatic_mut() {
            if let Some(&enabled) = saved.get(&entry.name()) {
                entry.set_enabled(enabled);
            }
        }
        self.apply_saved_order();
    }

    /// Put the dictionaries back in the priority order she left them in.
    ///
    /// Order is what decides which dictionary wins when two define the same
    /// outline, so losing it on restart silently changes what her strokes
    /// write. It was lost until 2026-09-02: only the enabled flags were saved.
    ///
    /// A name that is not in the saved list sorts last, keeping the order it
    /// was found in, which is what a dictionary added since the last save
    /// should do.
    fn apply_saved_order(&mut self) {
        if self.config.order.is_empty() {
            return;
        }
        // Stable, so unlisted dictionaries keep the order they arrived in
        // rather than being shuffled among themselves.
        self.dictionaries
            .dictionaries_mut()
            .sort_by_key(|d| priority_rank(&self.config.order, &d.path));
    }

    /// Record which dictionaries are enabled and in what order, so both survive
    /// a restart.
    fn save_dictionary_state(&mut self) {
        let mut state = std::collections::HashMap::new();
        let mut order = Vec::new();
        for entry in self.dictionaries.dictionaries() {
            if let Some(name) = entry.path.file_name() {
                let name = name.to_string_lossy().into_owned();
                state.insert(name.clone(), entry.enabled);
                order.push(name);
            }
        }
        for entry in self.dictionaries.programmatic() {
            state.insert(entry.name(), entry.is_enabled());
        }
        self.config.enabled = state;
        self.config.order = order;
        crate::config::save(&self.config);
    }

    /// Load one just-imported dictionary into the running stack.
    fn load_added(&mut self, path: &std::path::Path, errors: &mut Vec<String>) {
        use pluvialis_core::ProgrammaticDictionary;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        match path.extension().and_then(|e| e.to_str()) {
            // Off on arrival, like the Python dictionaries found at startup: it
            // is unsandboxed code, so the user enables it once she has looked at
            // what it does.
            Some("py") => match pluvialis_python::PythonDictionary::load(path) {
                Ok(mut dictionary) => {
                    dictionary.set_enabled(false);
                    self.dictionaries.push_programmatic(Box::new(dictionary));
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            },
            _ => match Dictionary::load(path) {
                Ok(dictionary) => {
                    let bad = dictionary.bad_keys().len();
                    if bad > 0 {
                        log::warn!("{name}: {bad} keys are not valid steno and were skipped");
                    }
                    self.loaded
                        .push(format!("{name} ({} entries)", dictionary.len()));
                    self.dictionaries.push(dictionary);
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            },
        }
    }

    /// Feed one stroke through the translator and rebuild the document.
    ///
    /// This is the seam M4a plugs the machine into: everything below the
    /// translator is already stroke driven, so the dev box and a real writer
    /// enter by the same door.
    pub fn apply(&mut self, stroke: Stroke) {
        let outline = Stroke::render_outline(&[stroke]);
        if self.sink == crate::screens::Sink::Field
            && (self.dictionary_pane.accept_raw_outline(stroke)
                || self.dictionary_screen.accept_raw_outline(stroke))
        {
            self.tape.push(TapeEntry {
                outline,
                result: "dictionary field".to_owned(),
            });
            trim_tape(&mut self.tape, self.config.settings.tape_limit);
            return;
        }

        let delta = self.translator.translate(&self.dictionaries, stroke);
        self.stats.record(&delta, crate::stats::is_undo(stroke));
        self.tape.push(TapeEntry {
            outline,
            result: describe(stroke, &delta),
        });
        trim_tape(&mut self.tape, self.config.settings.tape_limit);
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

        self.resync_after_trim();
    }

    /// Bound the translator's history, then bring the shadow back into step
    /// with it **without emitting the difference**.
    ///
    /// Trimming drops the oldest translations, so the formatter's output stops
    /// starting where it did. `steno_edit` skips only a common prefix, so a
    /// shadow left over from the untrimmed history diffs against the trimmed
    /// one as "delete the whole session, retype the whole session": at the
    /// 1000 translation limit that is around 5,000 backspaces and 5,000
    /// characters, per stroke, for the rest of the session. Sent through
    /// `SendInput` that is roughly 20,000 events per stroke, which arrives in
    /// whatever window has focus as the entire session's text and locks the
    /// machine up. If focus had just changed, `deliver` drops the backspaces
    /// and only the text lands, so it reads as a pure dump.
    ///
    /// So the order matters and is the whole fix: the stroke's own edit is
    /// formatted and delivered above, *then* the history is trimmed, *then*
    /// the shadow is rebuilt here. Trimming is internal bookkeeping and must
    /// never reach the output. `Translator::trim_history` is not called by
    /// `translate` for this reason.
    fn resync_after_trim(&mut self) {
        if self.translator.trim_history() == 0 {
            return;
        }
        self.shadow = format(self.translator.history());
        // The trim only removed events from the front, so every event left in
        // the rebuilt shadow has already been dispatched. Without this the
        // counter would sit past the shortened list and `dispatch_events`
        // would never fire another key combo for the rest of the session.
        self.events_logged = self.shadow.events.len();
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
                        log::debug!(
                            "key combo {{#{spec}}} ignored: not typing into another window"
                        );
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
    /// Send one batch to exactly one destination.
    ///
    /// A correction's backspaces erase text the *previous* batch wrote, so
    /// sending them anywhere else would delete characters this program never
    /// wrote, in someone else's document. What decides it is not "did focus
    /// change", which was too blunt and threw away every first undo after
    /// switching windows, but "is this the window I typed that text into, and
    /// does my own text reach back that far".
    fn deliver(&mut self, edit: &mut StenoEdit, destination: Destination) {
        match destination {
            Destination::Document => {
                // The document keeps its own text, so the only unsafe case is a
                // correction arriving straight after a batch that went to
                // another window.
                if self.last_destination != Destination::Document {
                    edit.backspaces = 0;
                    edit.backspace_keys = 0;
                }
                self.document.apply(edit);
                self.layout = None;
                self.push_caret = true;
            }
            Destination::OtherWindow => {
                if !self.output_enabled {
                    // Nothing was typed, so nothing there is ours to erase.
                    self.typed_chars = 0;
                    self.last_destination = destination;
                    return;
                }

                let target = pluvialis_output::foreground_window();
                if target.is_none() || target != self.typed_target {
                    // A window this program has not written in. Its text
                    // belongs to the user, so the insertion goes in and the
                    // backspaces do not.
                    self.typed_target = target;
                    self.typed_chars = 0;
                }

                let allowed = erasable(edit.backspace_keys, self.typed_chars);
                if allowed < edit.backspace_keys {
                    log::debug!(
                        "holding back {} backspaces: only {} characters here are ours",
                        edit.backspace_keys - allowed,
                        self.typed_chars
                    );
                }
                edit.backspace_keys = allowed;
                edit.backspaces = allowed;
                self.typed_chars = self.typed_chars - allowed + edit.text.chars().count();

                #[cfg(windows)]
                if let Err(e) = self.keyboard.send_edit(edit.backspace_keys, &edit.text) {
                    log::warn!("could not type into the focused window: {e}");
                }
            }
        }
        self.last_destination = destination;
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
    fn layout_job(&mut self, raw: Color32, font_size: f32) -> Arc<LayoutJob> {
        if let Some((color, size, job)) = &self.layout
            && *color == raw
            && *size == font_size
        {
            return job.clone();
        }
        let job = Arc::new(highlight(
            self.document.text(),
            self.document.raw_ranges(),
            raw,
            font_size,
        ));
        self.layout = Some((raw, font_size, job.clone()));
        job
    }

    /// Everything around the screen: the Home toolbar, the status bar and the
    /// tape.
    ///
    /// Panels have to be added before the central area, so the shell calls this
    /// before it draws whichever screen is showing.
    pub fn chrome(&mut self, ui: &mut egui::Ui, screen: Screen) {
        // egui 0.35 unified SidePanel and TopBottomPanel into `Panel`.
        if screen == Screen::Home {
            egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, screen));
        if screen.shows_tape() {
            egui::Panel::right("tape")
                .resizable(true)
                .default_size(210.0)
                .show(ui, |ui| self.tape_strip(ui));
        }
    }

    /// The Home screen: the live typing document.
    pub fn home(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| self.document(ui));
    }

    /// The Dictionary screen: the file list on the left, the table in the
    /// middle, the editor along the bottom.
    pub fn dictionary(&mut self, ui: &mut egui::Ui) {
        let mut add_clicked = false;
        let mut state_changed = false;
        let mut entries_changed = false;

        egui::Panel::left("dictionary-list")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                if ui
                    .button("Add dictionary...")
                    .on_hover_text(
                        "Copy a Plover .json or .py dictionary into Pluvialis. The \
                         original is not moved or changed.",
                    )
                    .clicked()
                {
                    add_clicked = true;
                }
                state_changed = self.dictionary_pane.list(ui, &mut self.dictionaries);
            });

        egui::Panel::bottom("entry-editor")
            .resizable(true)
            .default_size(132.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                entries_changed = self.dictionary_pane.editor(ui, &mut self.dictionaries);
                ui.add_space(4.0);
            });

        let action = egui::CentralPanel::default()
            .show(ui, |ui| {
                self.dictionary_screen.ui(ui, &mut self.entry_index)
            })
            .inner;

        entries_changed |= self.dictionary_action(action);

        if add_clicked {
            self.add_dictionaries();
            entries_changed = true;
        }
        // A toggled checkbox is remembered so it does not have to be set again
        // next launch.
        if state_changed {
            self.save_dictionary_state();
        }
        if entries_changed {
            self.rebuild_entry_index();
        }
    }

    /// The Settings screen. Anything changed here is written to the config
    /// file at once, and the two settings that cannot apply until the next
    /// start say so on the screen.
    pub fn settings(&mut self, ui: &mut egui::Ui) {
        let documents = self.storage.documents_dir().to_path_buf();
        let changed = crate::settings_screen::ui(
            ui,
            &mut self.config.settings,
            &documents,
            &mut self.stats,
            &mut self.confirm_stats_reset,
        );
        if changed {
            // The interval is read from `storage` every frame, so it has to be
            // pushed across rather than waiting for a restart.
            self.storage.autosave_interval = self.config.settings.autosave_interval();
            crate::config::save(&self.config);
        }
    }

    /// The Stats screen. Returns an outline she asked to write an entry for, so
    /// the shell can switch to the Dictionary screen with it loaded.
    pub fn stats(&mut self, ui: &mut egui::Ui) -> Option<String> {
        crate::stats_screen::ui(ui, &self.stats)
    }

    /// Open the dictionary editor on a new entry for this outline.
    pub fn start_new_entry(&mut self, outline: &str) {
        self.dictionary_pane.start_new_entry(outline);
    }

    /// Carry out what the table asked for. Everything is copied out of the
    /// index first, so it is not still borrowed when the dictionaries are
    /// written to.
    ///
    /// Returns whether any entry changed.
    fn dictionary_action(&mut self, action: crate::dictionary_screen::Action) -> bool {
        use crate::dictionary_screen::Action;

        match action {
            Action::None => false,
            Action::Load(id) => {
                if let Some((path, outline, word)) = self.entry_at(id) {
                    self.dictionary_pane.load_entry(&path, &outline, &word);
                }
                false
            }
            Action::Delete(ids) => {
                let entries: Vec<(std::path::PathBuf, String)> = ids
                    .iter()
                    .filter_map(|id| self.entry_at(*id))
                    .map(|(path, outline, _)| (path, outline))
                    .collect();
                self.dictionary_pane
                    .delete_entries(&mut self.dictionaries, &entries)
            }
            Action::Swap(first, second) => {
                let (Some(first), Some(second)) = (self.entry_at(first), self.entry_at(second))
                else {
                    return false;
                };
                self.dictionary_pane.swap_with(
                    &mut self.dictionaries,
                    &first.0,
                    &first.1,
                    &second.1,
                )
            }
        }
    }

    /// One entry from the table, as file, outline and word.
    fn entry_at(&self, id: u32) -> Option<(std::path::PathBuf, String, String)> {
        let entry = self.entry_index.entry(id)?;
        let path = self.entry_index.path(entry.dictionary)?;
        Some((
            path.to_path_buf(),
            entry.outline.clone(),
            entry.word.clone(),
        ))
    }

    /// Reread every entry for the table. Only after the entries themselves
    /// change: enabling, disabling and reordering do not touch them.
    fn rebuild_entry_index(&mut self) {
        self.entry_index.rebuild(&self.dictionaries);
        // The ids the table hands out are positions in the index, so a rebuild
        // invalidates whatever was highlighted.
        self.dictionary_screen.clear_selection();
    }

    /// Windows floating above whichever screen is showing.
    pub fn overlays(&mut self, ui: &mut egui::Ui, screen: Screen) {
        // Unsaved work from a session that crashed is worth offering wherever
        // she happens to be, not only on Home.
        self.recovery_prompt(ui);
        if screen == Screen::Home {
            self.history_window(ui);
        }
    }

    fn document(&mut self, ui: &mut egui::Ui) {
        let job = self.layout_job(raw_color(ui.visuals()), self.config.settings.font_size);
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

        // Fill the panel rather than growing into it. A multiline `TextEdit`
        // defaults to four rows and expands as text arrives, so an empty
        // document is a thin strip with dead space under it, and the click
        // target for putting the caret somewhere is only as tall as the text.
        // Asking for as many rows as fit makes the editor the whole panel from
        // the start; the scroll area still takes over once the text is longer.
        // The document's own font, not a text style: the layouter paints at
        // DOCUMENT_FONT_SIZE, so any other measure gets the row count wrong.
        let font_size = self.config.settings.font_size;
        let row_height = ui.fonts_mut(|f| f.row_height(&FontId::proportional(font_size)));
        let rows = (ui.available_height() / row_height).floor().max(1.0) as usize;

        let output = egui::ScrollArea::vertical()
            .stick_to_bottom(caret_at_end)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::TextEdit::multiline(&mut text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows)
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

        // Which way the caret is copied depends on who moved it last, and
        // getting this backwards is what turned "I can" into "can I".
        //
        // After a stroke the document is right and the widget is stale, so the
        // widget is told. Otherwise the user is driving, by clicking or with
        // the arrow keys, and the widget is right.
        if self.push_caret {
            self.push_caret = false;
            let index = egui::text::CCursor::new(self.document.caret_char());
            let mut state = output.state.clone();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(index)));
            state.store(ui.ctx(), output.response.id);
        } else if let Some(cursor) = output.cursor_range {
            // egui counts characters, the document counts bytes.
            self.document.set_caret_char(cursor.primary.index.0);
        }
    }

    /// Browse and restore earlier versions.
    fn history_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_history {
            return;
        }

        let now = crate::storage::now();
        let mut restore: Option<crate::storage::Snapshot> = None;
        let mut open = true;

        egui::Window::new("History")
            .open(&mut open)
            .default_size([320.0, 400.0])
            .show(ui.ctx(), |ui| {
                if self.history.is_empty() {
                    ui.label("No saved versions yet.");
                    ui.label(
                        egui::RichText::new(
                            "A version is written whenever the text changes and \
                             the document is saved.",
                        )
                        .small()
                        .weak(),
                    );
                    return;
                }

                ui.label(
                    egui::RichText::new(format!("{} versions", self.history.len()))
                        .small()
                        .weak(),
                );
                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for snapshot in &self.history {
                            ui.horizontal(|ui| {
                                ui.label(crate::storage::how_long_ago(snapshot.at, now));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("Restore").clicked() {
                                            restore = Some(snapshot.clone());
                                        }
                                    },
                                );
                            });
                        }
                    });
            });

        if let Some(snapshot) = restore {
            match self.storage.read_snapshot(&snapshot) {
                Ok(text) => {
                    // Saved first, so restoring is itself undoable: the version
                    // being replaced becomes the newest snapshot rather than
                    // being lost.
                    self.save_now();
                    self.document.reconcile(&text);
                    self.document.set_caret(text.len());
                    self.layout = None;
                    self.history = self.storage.snapshots();
                    log::info!("restored {} characters", text.len());
                }
                Err(e) => {
                    log::error!("could not read the snapshot: {e}");
                    self.save_error = Some(e.to_string());
                }
            }
        }
        if !open {
            self.show_history = false;
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

    fn status_bar(&mut self, ui: &mut egui::Ui, screen: Screen) {
        // Counted only when the text has actually changed, but *sampled* every
        // frame regardless: the meter needs to see time passing to know that
        // writing has stopped.
        let revision = self.document.revision();
        if revision != self.words.0 {
            self.words = (revision, crate::meter::count_words(self.document.text()));
        }
        let words = self.words.1;

        let now = ui.input(|i| i.time);
        self.meter.observe(now, words);
        let idle = self.meter.is_idle(now);

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

            // History is about the open document, so it belongs with it.
            if screen == Screen::Home {
                ui.separator();
                if ui
                    .toggle_value(&mut self.show_history, "History")
                    .on_hover_text("Earlier saved versions of this document")
                    .clicked()
                    && self.show_history
                {
                    // Reading the directory once on open, rather than every
                    // frame.
                    self.history = self.storage.snapshots();
                }
            }

            if let Some(error) = &self.save_error {
                let color = raw_color(ui.visuals());
                ui.colored_label(color, format!("Not saving: {error}"));
            }

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

            // Right aligned, so the counters keep their place as the status on
            // the left changes length between "Searching for a writer" and a
            // connected machine's name.
            if screen != Screen::Home {
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.meter.words_per_minute() {
                    // Nothing rather than "0 wpm" before there is enough
                    // writing to divide by. A confident zero is a worse answer
                    // than no answer.
                    None => ui.label(""),
                    Some(rate) => {
                        // Dimmed while not writing, because the figure is held
                        // rather than live: pauses are excluded from it, so
                        // without this it would look like a current reading.
                        let text = egui::RichText::new(format!("{rate} wpm"));
                        ui.label(if idle { text.weak() } else { text })
                            .on_hover_text(
                                "Real words per minute while you are writing, the way \
                                 dictation speeds are quoted.\n\nPauses are excluded, so \
                                 thinking does not count against you. Dimmed when you \
                                 have stopped, since the figure is then the last one \
                                 measured rather than a live reading.",
                            )
                    }
                };
                ui.separator();
                ui.label(format!("{words} words"))
                    .on_hover_text("Words in this document");
            });
        });
        ui.add_space(2.0);
    }
}

/// How many of a correction's backspaces may actually be sent.
///
/// Never more than this program has typed into the window in front. Erasing
/// past that point would delete the user's own text, which is not this
/// program's to take back, and is the one mistake in the output path that
/// cannot be undone.
fn erasable(requested: usize, ours: usize) -> usize {
    requested.min(ours)
}

/// Where a dictionary sorts, given the saved priority order.
///
/// A file the order does not mention sorts last rather than first. A dictionary
/// added since the last save is the one the user has not ranked yet, and putting
/// it at the top would silently outrank everything she has.
fn priority_rank(order: &[String], path: &std::path::Path) -> usize {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| order.iter().position(|saved| saved == name))
        .unwrap_or(usize::MAX)
}

/// Where documents and their history live.
///
/// Beside the executable rather than in AppData, so the user can find, back up
/// and edit them with ordinary tools; see `paths`.
fn documents_dir() -> std::path::PathBuf {
    crate::paths::base_dir().join("documents")
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
fn highlight(
    text: &str,
    raw_ranges: &[(usize, usize)],
    raw_color: Color32,
    font_size: f32,
) -> LayoutJob {
    let font = FontId::proportional(font_size);

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

    fn tape_of(count: usize) -> Vec<TapeEntry> {
        (0..count)
            .map(|n| TapeEntry {
                outline: format!("KAT{n}"),
                result: n.to_string(),
            })
            .collect()
    }

    /// Undoing a word written into another window has to erase it there.
    ///
    /// This is the case that was broken: the old rule dropped every backspace
    /// whenever the destination differed from the previous batch's, so the
    /// first undo after looking at Pluvialis and going back did nothing.
    /// Nine characters were written into that window, so nine may come back.
    #[test]
    fn a_word_this_program_typed_can_be_erased_again() {
        assert_eq!(erasable(9, 9), 9);
    }

    /// A window this program has never typed in. Its text is the user's, and
    /// erasing it is the one mistake in the output path with no undo.
    #[test]
    fn nothing_is_erased_in_a_window_we_have_not_written_in() {
        assert_eq!(erasable(9, 0), 0);
    }

    /// A correction longer than this program's own text stops at the boundary
    /// rather than eating into what was already there.
    #[test]
    fn erasing_stops_where_our_own_text_stops() {
        assert_eq!(erasable(20, 6), 6);
    }

    #[test]
    fn an_insertion_with_no_backspaces_is_unaffected() {
        assert_eq!(erasable(0, 40), 0);
    }

    /// Priority decides which dictionary wins when two define the same
    /// outline, so the order has to come back exactly as it was left.
    #[test]
    fn the_saved_order_ranks_the_dictionaries_it_names() {
        let order = vec!["second.json".to_owned(), "first.json".to_owned()];
        assert_eq!(
            priority_rank(&order, std::path::Path::new("C:/x/second.json")),
            0
        );
        assert_eq!(
            priority_rank(&order, std::path::Path::new("C:/x/first.json")),
            1
        );
    }

    /// A dictionary imported since the last save. It has never been ranked, so
    /// it must not outrank the ones that have been.
    #[test]
    fn a_dictionary_the_order_does_not_name_sorts_last() {
        let order = vec!["known.json".to_owned()];
        assert_eq!(
            priority_rank(&order, std::path::Path::new("C:/x/brand-new.json")),
            usize::MAX
        );
    }

    #[test]
    fn the_tape_keeps_the_newest_lines_and_drops_the_oldest() {
        let limit = crate::config::DEFAULT_TAPE_LIMIT;
        let mut tape = tape_of(limit + 50);
        trim_tape(&mut tape, limit);

        assert_eq!(tape.len(), limit);
        // The strip sticks to the bottom, so the newest line is the one that
        // must survive. Trimming the wrong end would leave it showing the
        // opening of the session and never updating again.
        assert_eq!(tape.last().unwrap().result, (limit + 49).to_string());
        assert_eq!(tape.first().unwrap().result, "50");
    }

    #[test]
    fn a_tape_within_the_limit_is_left_alone() {
        let limit = crate::config::DEFAULT_TAPE_LIMIT;
        let mut tape = tape_of(limit);
        trim_tape(&mut tape, limit);
        assert_eq!(tape.len(), limit);
        assert_eq!(tape.first().unwrap().result, "0");
    }

    #[test]
    fn trimming_an_empty_tape_does_nothing() {
        let mut tape = tape_of(0);
        trim_tape(&mut tape, crate::config::DEFAULT_TAPE_LIMIT);
        assert!(tape.is_empty());
    }

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
        let job = highlight(&f.0, &f.1, RED, crate::config::DEFAULT_FONT_SIZE);
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
        let job = highlight(&f.0, &f.1, RED, crate::config::DEFAULT_FONT_SIZE);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, Color32::PLACEHOLDER);
    }

    #[test]
    fn empty_document_produces_no_sections() {
        let empty = formatted("", Vec::new());
        let job = highlight(&empty.0, &empty.1, RED, crate::config::DEFAULT_FONT_SIZE);
        assert!(job.sections.is_empty());
    }

    #[test]
    fn a_document_that_is_entirely_raw_steno_is_one_red_section() {
        let f = formatted("KAT", vec![(0, 3)]);
        let job = highlight(&f.0, &f.1, RED, crate::config::DEFAULT_FONT_SIZE);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, RED);
    }

    #[test]
    fn adjacent_raw_ranges_do_not_produce_an_empty_section_between_them() {
        let f = formatted("KATTKOG", vec![(0, 3), (3, 7)]);
        let job = highlight(&f.0, &f.1, RED, crate::config::DEFAULT_FONT_SIZE);
        assert_covers(&job, &f.0);
        assert_eq!(job.sections.len(), 2);
        assert!(job.sections.iter().all(|s| s.format.color == RED));
    }

    #[test]
    fn stale_ranges_are_dropped_rather_than_panicking() {
        // Past the end, backwards, and overlapping the previous range: all
        // reachable if formatting rewrote earlier text.
        let f = formatted("short", vec![(0, 2), (1, 3), (4, 99), (3, 3)]);
        let job = highlight(&f.0, &f.1, RED, crate::config::DEFAULT_FONT_SIZE);
        assert_covers(&job, &f.0);
    }

    #[test]
    fn a_range_splitting_a_character_is_ignored() {
        // The pound sign is two bytes, so 1 is not a character boundary.
        let f = formatted("\u{00A3}5", vec![(1, 2)]);
        let job = highlight(&f.0, &f.1, RED, crate::config::DEFAULT_FONT_SIZE);
        assert_covers(&job, &f.0);
        assert!(job.sections.iter().all(|s| s.format.color != RED));
    }
}
