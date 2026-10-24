//! `ryucad-app [DIR]`: the desktop app, on the project in `DIR` or on its
//! start window. `--help` says so, `--version` prints the build; `--smoke
//! [DIR]` loads the project and draws one frame offscreen, for release
//! checks. Anything else is a usage error, never a folder.

// A release build on Windows opens no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use ryucad_app::core::link::find_ryucad;
use ryucad_app::ui::app::App;
use ryucad_app::ui::platform;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "Usage: ryucad-app [DIR]

Opens the ryucad project in or above DIR, or the start window without one.

Options:
  --version      Print the build, as JSON
  --smoke [DIR]  Load the project and draw one frame offscreen (release checks)
  -h, --help     Print this help";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        let v = serde_json::json!(ryucad::proto::VersionInfo::current());
        println!("{v}");
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "--smoke") {
        let dir = args
            .get(1)
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let (doc, code) = match ryucad_app::ui::smoke::run(&dir) {
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
    // A flag it doesn't know, or more than one folder, is a mistake, not
    // a folder to open.
    if args.len() > 1 || args.first().is_some_and(|a| a.starts_with('-')) {
        eprintln!(
            "ryucad-app: unexpected argument '{}'\n\n{USAGE}",
            args.join(" ")
        );
        return ExitCode::from(2);
    }
    // Before winit opens the display, and before any thread starts.
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(m) = platform::x_modifiers(std::env::var_os("RYUCAD_XIM").is_some()) {
        // SAFETY: nothing else runs yet to read the environment.
        unsafe { std::env::set_var("XMODIFIERS", m) };
    }
    platform::crash_reports();
    // A project to open; the start window without one.
    let dir = args.first().map(PathBuf::from);
    let daemon = match find_ryucad() {
        Ok(d) => d,
        Err(e) => {
            platform::fatal("ryucad can't start", &e.message);
            return ExitCode::FAILURE;
        }
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ryucad")
            .with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: platform::wgpu_options(),
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
            platform::fatal(
                "ryucad can't draw its window",
                &format!("{e}\n\n{}", platform::NO_ADAPTER),
            );
            ExitCode::FAILURE
        }
    }
}
