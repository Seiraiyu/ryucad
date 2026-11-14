//! `ryueda-app [DIR]`: ryueda's window on the project in or above DIR.
//! `--version` prints the build in the CLI's envelope; `--smoke [DIR]`
//! draws one frame offscreen, for release checks.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use ryueda_app::core::link::find_ryueda;
use ryueda_app::ui::app::App;
use ryueda_app::ui::platform;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "Usage: ryueda-app [DIR]

Opens the ryueda project in or above DIR (this folder by default).

Options:
  --version      Print the build, as JSON
  --smoke [DIR]  Load the project and draw one frame offscreen (release checks)
  -h, --help     Print this help";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        let v = serde_json::json!({"ok": true, "result": ryueda::proto::VersionInfo::current(), "warnings": []});
        println!("{v}");
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "--smoke") {
        let dir = args
            .get(1)
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let (doc, code) = match ryueda_app::ui::smoke::run(&dir) {
            Ok(r) => (
                serde_json::json!({"ok": true, "result": r}),
                ExitCode::SUCCESS,
            ),
            Err(e) => (
                serde_json::json!({"ok": false, "error": e}),
                ExitCode::FAILURE,
            ),
        };
        println!("{doc}");
        return code;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.len() > 1 || args.first().is_some_and(|a| a.starts_with('-')) {
        eprintln!(
            "ryueda-app: unexpected argument '{}'\n\n{USAGE}",
            args.join(" ")
        );
        return ExitCode::from(2);
    }
    let dir = args
        .first()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let daemon = match find_ryueda() {
        Ok(d) => d,
        Err(e) => {
            platform::fatal("ryueda can't start", &e.message);
            return ExitCode::FAILURE;
        }
    };
    let saved = ryueda_app::core::prefs::get("app.window");
    let size = saved["size"]
        .as_array()
        .and_then(|s| Some([s.first()?.as_f64()? as f32, s.get(1)?.as_f64()? as f32]))
        .unwrap_or([1280.0, 800.0]);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ryueda")
            .with_inner_size(size),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let run = eframe::run_native(
        "ryueda",
        options,
        Box::new(move |cc| Ok(Box::new(App::open(&cc.egui_ctx, &dir, &daemon)))),
    );
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            platform::fatal(
                "ryueda can't open its window",
                &platform::cant_run(&e.to_string()),
            );
            ExitCode::FAILURE
        }
    }
}
