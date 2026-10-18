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
