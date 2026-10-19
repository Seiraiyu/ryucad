//! What the app needs from the machine: a graphics adapter (a hardware
//! one if there is one, else a software one such as WARP or lavapipe,
//! with a warning), native dialogs for what stops it starting, and crash
//! reports that leave the session and files alone.

use eframe::{egui_wgpu, wgpu};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The window draws with a software adapter: it works, slowly.
static SOFTWARE: AtomicBool = AtomicBool::new(false);

pub fn software() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

/// The message when no adapter at all will draw.
pub const NO_ADAPTER: &str = "No graphics adapter can draw this window. Install your GPU's drivers \
(or Mesa's lavapipe on Linux), or see the model headless with `ryucad render`.";

/// Picks an adapter that can present to the window: hardware first.
fn choose(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, String> {
    let usable: Vec<&wgpu::Adapter> = adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .collect();
    let cpu = |a: &&wgpu::Adapter| a.get_info().device_type == wgpu::DeviceType::Cpu;
    if let Some(a) = usable.iter().find(|a| !cpu(a)) {
        return Ok((*a).clone());
    }
    match usable.first() {
        Some(a) => {
            SOFTWARE.store(true, Ordering::Relaxed);
            eprintln!(
                "ryucad-app: drawing with a software adapter ({})",
                a.get_info().name
            );
            Ok((*a).clone())
        }
        None => Err(NO_ADAPTER.to_owned()),
    }
}

/// The window's wgpu setup: our adapter choice, otherwise eframe's.
pub fn wgpu_options() -> egui_wgpu::WgpuConfiguration {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.native_adapter_selector = Some(Arc::new(choose));
    egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// Shows an error that stops the app, natively, and on stderr.
pub fn fatal(title: &str, message: &str) {
    eprintln!("ryucad-app: {message}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .show();
}

/// On a panic, writes a crash report to the runtime log folder before the
/// app exits. The daemon, the session and the files are not touched.
pub fn crash_reports() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let report = format!(
            "ryucad-app {} crashed\n{info}\n\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(dir) = ryucad::endpoint::runtime_dir() {
            let path = dir.join(format!("ryucad-app-crash-{at}.log"));
            if std::fs::write(&path, report).is_ok() {
                eprintln!("ryucad-app: crashed; report in {}", path.display());
            }
        }
        default(info);
    }));
}

/// Ctrl+click is a right-click, as on a one-button Mac: a primary press
/// with Ctrl becomes a secondary one, without Ctrl, and so does its
/// release, wherever Ctrl is by then (`held` remembers, across frames).
pub fn ctrl_click_as_secondary(events: &mut [eframe::egui::Event], held: &mut bool) {
    use eframe::egui::{Event, PointerButton};
    for e in events {
        if let Event::PointerButton {
            button,
            pressed,
            modifiers,
            ..
        } = e
            && *button == PointerButton::Primary
        {
            if *pressed {
                *held = modifiers.ctrl && !modifiers.mac_cmd;
            }
            if *held {
                *button = PointerButton::Secondary;
                modifiers.ctrl = false;
            }
            if !*pressed {
                *held = false;
            }
        }
    }
}

#[cfg(test)]
mod ctrl_click_tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, pos2};

    fn button(button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton {
            pos: pos2(1.0, 2.0),
            button,
            pressed,
            modifiers,
        }
    }

    #[test]
    fn ctrl_click_is_a_right_click() {
        let mut held = false;
        // Ctrl released before the button: still one right-click.
        let mut events = vec![
            button(PointerButton::Primary, true, Modifiers::CTRL),
            button(PointerButton::Primary, false, Modifiers::NONE),
            button(PointerButton::Primary, true, Modifiers::NONE),
            button(PointerButton::Primary, false, Modifiers::NONE),
            // Cmd+click adds to the selection, as it did.
            button(
                PointerButton::Primary,
                true,
                Modifiers::MAC_CMD | Modifiers::COMMAND,
            ),
        ];
        ctrl_click_as_secondary(&mut events, &mut held);
        let buttons: Vec<(PointerButton, bool)> = events
            .iter()
            .map(|e| match e {
                Event::PointerButton {
                    button, modifiers, ..
                } => (*button, modifiers.ctrl),
                _ => unreachable!(),
            })
            .collect();
        use PointerButton::{Primary, Secondary};
        assert_eq!(
            buttons,
            [
                (Secondary, false),
                (Secondary, false),
                (Primary, false),
                (Primary, false),
                (Primary, false)
            ]
        );
        assert!(!held);
    }
}
