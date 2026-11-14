//! What the window says when it can't run: on stderr, and in a dialog
//! when there is a display to show one.

/// What `--smoke` and the tests say when no adapter draws.
pub const NO_ADAPTER: &str = "No graphics adapter: ryueda-app needs Vulkan, Metal or DirectX 12 (or a software one such as lavapipe); `ryueda render` draws sheets without it";

/// Advice for eframe's error.
pub fn cant_run(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("adapter") || lower.contains("wgpu") {
        format!("{NO_ADAPTER} ({error})")
    } else if lower.contains("display") || lower.contains("wayland") || lower.contains("x11") {
        format!(
            "No display to open a window on; `ryueda render` draws sheets without one ({error})"
        )
    } else {
        error.to_owned()
    }
}

/// Says `message` on stderr, and in a message box: a window started
/// from a desktop entry has no terminal to say it on. `RYUEDA_NO_DIALOG`
/// keeps to stderr (scripts, CI).
pub fn fatal(title: &str, message: &str) {
    eprintln!("ryueda-app: {message}");
    if std::env::var_os("RYUEDA_NO_DIALOG").is_none() {
        let _ = rfd::MessageDialog::new()
            .set_title(title)
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
