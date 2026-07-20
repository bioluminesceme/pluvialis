// Hide the console window on Windows release builds. Debug builds keep it so
// log output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::process::ExitCode;

mod cli;
mod live;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Any argument means command line mode. With none, open the GUI.
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.live.pump_machine();
    }

    // egui 0.35 replaced `update(&Context)` with `ui(&mut Ui)`. Most egui
    // examples online still show the old signature.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.live.ui(ui);
    }
}
