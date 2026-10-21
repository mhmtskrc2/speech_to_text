// Keep the console window from appearing behind the GUI on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use whisper_gui::{App, cli};

fn main() -> eframe::Result<()> {
    // Route whisper.cpp's own chatter through the `log` crate instead of stderr.
    whisper_rs::install_logging_hooks();

    // Arguments mean headless mode; a bare launch opens the window.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if !argv.is_empty() {
        std::process::exit(cli::main(argv));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sesten Metne")
            .with_inner_size([920.0, 720.0])
            .with_min_inner_size([680.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Sesten Metne",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
