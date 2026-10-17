//! `ryucad-app [DIR]`: the desktop app on the project in `DIR` (this
//! folder by default).

use ryucad::doc::project::find_root;
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
    let dir = args
        .first()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let opened = find_root(&dir).and_then(|root| Ok((root, find_ryucad()?)));
    let (root, daemon) = match opened {
        Ok(o) => o,
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
        Box::new(move |cc| Ok(Box::new(App::open(&cc.egui_ctx, &root, &daemon)?))),
    );
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ryucad-app: {e}");
            ExitCode::FAILURE
        }
    }
}
