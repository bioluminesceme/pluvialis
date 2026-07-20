// Hide the console window on Windows release builds. Debug builds keep it so
// log output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::process::ExitCode;

mod cli;
mod dictionaries;
mod library;
mod live;
mod storage;

/// Reconnect to the terminal that launched us.
///
/// Release builds are linked as a GUI subsystem binary (see the attribute at
/// the top of this file) so double clicking does not flash up a console. The
/// cost is that a command line run starts with no standard handles at all:
/// every `println!` is silently discarded and the shell does not wait for the
/// process. Output appears only if the caller happens to redirect it, which
/// makes the failure look like the command doing nothing.
#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        STD_HANDLE, SetStdHandle,
    };
    use windows::core::PCSTR;

    /// Whether this standard handle is already usable.
    ///
    /// It is whenever the caller redirected or piped us, and in that case it
    /// must be left alone: pointing it at the console instead would send the
    /// output to the terminal and hand the pipe nothing, which breaks every
    /// `| Select-String` and `> file` the tool is used with.
    fn already_connected(which: STD_HANDLE) -> bool {
        match unsafe { GetStdHandle(which) } {
            Ok(handle) => !handle.is_invalid(),
            Err(_) => false,
        }
    }

    unsafe {
        // Fails when there is no parent console, which is the ordinary case
        // for a double click. Nothing to attach to, so leave things alone.
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }

        let stdout_needed = !already_connected(STD_OUTPUT_HANDLE);
        let stderr_needed = !already_connected(STD_ERROR_HANDLE);
        if !stdout_needed && !stderr_needed {
            return;
        }

        // Attaching does not reliably repoint the standard handles, and a GUI
        // subsystem process starts with none, so set them explicitly. Rust's
        // Windows stdio fetches the handle per write, so this takes effect
        // even though it happens after startup.
        let console = CreateFileA(
            PCSTR(c"CONOUT$".as_ptr().cast()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        );
        if let Ok(console) = console {
            if stdout_needed {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, console);
            }
            if stderr_needed {
                let _ = SetStdHandle(STD_ERROR_HANDLE, console);
            }
        }
    }
}

fn main() -> ExitCode {
    // Any argument means command line mode. With none, open the GUI.
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Before the logger, so its stderr lands on the console too.
    #[cfg(windows)]
    if !args.is_empty() {
        attach_parent_console();
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    if !args.is_empty() {
        return cli::run(&args);
    }

    match run_gui() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("could not start: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_gui() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pluvialis")
            .with_inner_size([1000.0, 700.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Pluvialis",
        options,
        Box::new(|cc| Ok(Box::new(PluvialisApp::new(cc)))),
    )
}

struct PluvialisApp {
    live: live::LiveView,
}

impl PluvialisApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut live = live::LiveView::new();
        // The scanner starts here and never stops while the app runs. There is
        // deliberately no connect button and no machine picker.
        live.start_machines(&cc.egui_ctx);
        PluvialisApp { live }
    }
}

impl eframe::App for PluvialisApp {
    // Non-painting per-frame work belongs in `logic`, painting in `ui`.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.live.pump_machine(ctx);
    }

    // egui 0.35 replaced `update(&Context)` with `ui(&mut Ui)`. Most egui
    // examples online still show the old signature.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.live.ui(ui);
    }

    /// Save and record a clean exit, so the next start does not offer recovery
    /// for a session that ended normally.
    fn on_exit(&mut self) {
        self.live.shutdown();
    }
}
