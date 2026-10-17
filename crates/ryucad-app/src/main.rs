//! `ryucad-app [DIR]`: the desktop app, on the project in `DIR` or on its
//! start window.

use ryucad_app::core::link::find_ryucad;
use ryucad_app::ui::app::App;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        let v = serde_json::json!(ryucad::proto::VersionInfo::current());
        println!("{v}");
        return ExitCode::SUCCESS;
    }
    // A project to open; the start window without one.
    let dir = args.first().map(PathBuf::from);
    let daemon = match find_ryucad() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("ryucad-app: {}", e.message);
            return ExitCode::FAILURE;
        }
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ryucad")
            .with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let run = eframe::run_native(
        "ryucad",
        options,
        Box::new(move |cc| Ok(Box::new(App::open(&cc.egui_ctx, dir.as_deref(), &daemon)))),
    );
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ryucad-app: {e}");
            ExitCode::FAILURE
        }
    }
}
