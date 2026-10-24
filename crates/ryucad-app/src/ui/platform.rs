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

/// The window is a Wayland one: a window can't bring itself forward
/// there, only ask the compositor for attention.
static WAYLAND: AtomicBool = AtomicBool::new(false);

pub fn wayland() -> bool {
    WAYLAND.load(Ordering::Relaxed)
}

/// Notes which display server the window is on.
pub fn note_display(frame: &eframe::Frame) {
    use wgpu::rwh::{HasDisplayHandle, RawDisplayHandle};
    let on = frame
        .display_handle()
        .is_ok_and(|d| matches!(d.as_raw(), RawDisplayHandle::Wayland(_)));
    WAYLAND.store(on, Ordering::Relaxed);
}

/// What brings a window forward when it's asked for (`ryucad open`): focus,
/// and on Wayland, where focus does nothing, a request for attention, which
/// GNOME shows as "ryucad is ready" and KDE by raising the window. Not on
/// X11, whose urgency hint stays on a window that is focused.
pub fn raise(wayland: bool) -> Vec<eframe::egui::ViewportCommand> {
    use eframe::egui::{UserAttentionType, ViewportCommand};
    let mut v = vec![ViewportCommand::Minimized(false), ViewportCommand::Focus];
    if wayland {
        v.push(ViewportCommand::RequestUserAttention(
            UserAttentionType::Informational,
        ));
    }
    v
}

/// Where the window should go when a change of its size (a DPI change
/// on X11, which winit answers by growing the window) left it larger than
/// its monitor or past its right or bottom edge: its inner size and outer
/// position, in points, shrunk and moved onto the monitor. `None` when it
/// fits, or when it wasn't `resized` (in size or scale) since the last
/// frame: then someone moved it there. Only a
/// window starting on the monitor at the origin is moved, since that is
/// the only monitor whose place is known. There it is kept inside the
/// desktop's work `area` too, when known ([`work_area`]): a window manager
/// keeps windows off its panels, so a window as tall as the monitor would
/// be pushed down past its bottom edge.
pub fn keep_on_screen(
    resized: bool,
    inner: eframe::egui::Rect,
    outer: eframe::egui::Rect,
    monitor: eframe::egui::Vec2,
    area: Option<eframe::egui::Rect>,
) -> Option<(eframe::egui::Vec2, eframe::egui::Pos2)> {
    use eframe::egui::{Pos2, Rect};
    const SLACK: f32 = 0.5;
    if !resized {
        return None;
    }
    let on_origin = outer.min.x >= -SLACK
        && outer.min.y >= -SLACK
        && outer.min.x < monitor.x
        && outer.min.y < monitor.y;
    let screen = Rect::from_min_size(Pos2::ZERO, monitor);
    let usable = match area {
        Some(a) if on_origin => Some(a.intersect(screen)).filter(|r| r.is_positive()),
        _ => None,
    }
    .unwrap_or(screen);
    let too_big =
        outer.width() > usable.width() + SLACK || outer.height() > usable.height() + SLACK;
    let past =
        on_origin && (outer.max.x > usable.max.x + SLACK || outer.max.y > usable.max.y + SLACK);
    if !too_big && !past {
        return None;
    }
    let size = outer.size().min(usable.size());
    let frame = outer.size() - inner.size();
    let pos = if on_origin {
        outer.min.min(usable.max - size).max(usable.min)
    } else {
        outer.min
    };
    Some((size - frame, pos))
}

/// The desktop's work area, the screen less panels and docks, in pixels:
/// what an X11 window manager publishes as `_NET_WORKAREA` for the current
/// desktop. `None` on Wayland and other systems, or when the window
/// manager doesn't say. Asked of the X server at most once a second, since
/// a window being resized asks every frame.
pub fn work_area() -> Option<eframe::egui::Rect> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        type Cached = Option<(Instant, Option<eframe::egui::Rect>)>;
        static CACHE: Mutex<Cached> = Mutex::new(None);
        if wayland() {
            return None;
        }
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, area)) = *cache
            && at.elapsed() < Duration::from_secs(1)
        {
            return area;
        }
        let area = x11_work_area().map(|[x, y, w, h]| {
            eframe::egui::Rect::from_min_size(
                eframe::egui::pos2(x as f32, y as f32),
                eframe::egui::vec2(w as f32, h as f32),
            )
        });
        *cache = Some((Instant::now(), area));
        area
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    None
}

/// `_NET_WORKAREA` of the current desktop (`_NET_CURRENT_DESKTOP`, else
/// the first) on the display in `DISPLAY`: x, y, width and height.
#[cfg(all(unix, not(target_os = "macos")))]
fn x11_work_area() -> Option<[u32; 4]> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &[u8]| {
        let a = conn.intern_atom(true, name).ok()?.reply().ok()?.atom;
        (a != u32::from(AtomEnum::NONE)).then_some(a)
    };
    let cardinals = |property: u32, offset: u32, count: u32| {
        let reply = conn
            .get_property(false, root, property, AtomEnum::CARDINAL, offset, count)
            .ok()?
            .reply()
            .ok()?;
        Some(reply.value32()?.collect::<Vec<u32>>())
    };
    let desktop = atom(b"_NET_CURRENT_DESKTOP")
        .and_then(|a| cardinals(a, 0, 1))
        .and_then(|v| v.first().copied())
        .unwrap_or(0);
    let v = cardinals(atom(b"_NET_WORKAREA")?, desktop.checked_mul(4)?, 4)?;
    Some([*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?])
}

/// The message when no adapter at all will draw.
pub const NO_ADAPTER: &str = "No graphics adapter can draw this window. Install your GPU's drivers \
(or Mesa's lavapipe on Linux), or see the model headless with `ryucad render`.";

/// What the scene needs of an adapter beyond the downlevel defaults: a
/// plain view of its sRGB texture, for egui to sample. OpenGL often lacks
/// it, so an adapter without it is passed over, never chosen to crash.
pub const NEEDS: wgpu::DownlevelFlags = wgpu::DownlevelFlags::VIEW_FORMATS;

/// What [`pick`] knows of an adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub software: bool,
    pub gl: bool,
    /// It has what the scene needs ([`NEEDS`]) and, for a window, can
    /// present to it.
    pub usable: bool,
}

impl Candidate {
    pub fn of(adapter: &wgpu::Adapter, surface: Option<&wgpu::Surface<'_>>) -> Candidate {
        let info = adapter.get_info();
        Candidate {
            software: info.device_type == wgpu::DeviceType::Cpu,
            gl: info.backend == wgpu::Backend::Gl,
            usable: adapter.get_downlevel_capabilities().flags.contains(NEEDS)
                && surface.is_none_or(|s| adapter.is_surface_supported(s)),
        }
    }
}

/// The adapter to draw with, by index: a usable one, hardware before
/// software and, within each, Vulkan, Metal or DX12 before OpenGL (whose
/// drivers lack more). `None` when none is usable.
pub fn pick(candidates: &[Candidate]) -> Option<usize> {
    let rank = |c: &Candidate| (c.software, c.gl);
    (0..candidates.len())
        .filter(|&i| candidates[i].usable)
        .min_by_key(|&i| rank(&candidates[i]))
}

/// Picks an adapter for the window or a headless scene, noting a software
/// one.
pub fn choose(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, String> {
    let candidates: Vec<Candidate> = adapters.iter().map(|a| Candidate::of(a, surface)).collect();
    let Some(i) = pick(&candidates) else {
        return Err(NO_ADAPTER.to_owned());
    };
    let a = &adapters[i];
    if candidates[i].software {
        SOFTWARE.store(true, Ordering::Relaxed);
        eprintln!(
            "ryucad-app: drawing with a software adapter ({})",
            a.get_info().name
        );
    }
    Ok(a.clone())
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

/// The X input method to use on X11: none of the server's, unless
/// `RYUCAD_XIM` asks to keep them. Through XIM (IBus, Ubuntu's default)
/// fast typing loses keys and puts spaces in the wrong place, which
/// silently changes values; without it, keys go straight to the window
/// and dead keys and Compose still work. Wayland is unaffected.
pub fn x_modifiers(keep_xim: bool) -> Option<&'static str> {
    (!keep_xim).then_some("@im=none")
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
mod screen_tests {
    use super::keep_on_screen;
    use eframe::egui::{Rect, pos2, vec2};

    #[test]
    fn a_window_grown_past_its_monitor_is_brought_back() {
        let monitor = vec2(1600.0, 1200.0);
        // #768: 1280x800 grown to 1600x1000 at (2, 215) under a 20-point
        // title bar: past the bottom.
        let inner = Rect::from_min_size(pos2(2.0, 235.0), vec2(1600.0, 1000.0));
        let outer = Rect::from_min_size(pos2(2.0, 215.0), vec2(1600.0, 1020.0));
        assert_eq!(
            keep_on_screen(true, inner, outer, monitor, None),
            Some((vec2(1600.0, 1000.0), pos2(0.0, 180.0)))
        );
        // 2000x1400: shrunk to the monitor too.
        let inner = Rect::from_min_size(pos2(0.0, 20.0), vec2(2000.0, 1400.0));
        let outer = Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 1420.0));
        assert_eq!(
            keep_on_screen(true, inner, outer, monitor, None),
            Some((vec2(1600.0, 1180.0), pos2(0.0, 0.0)))
        );
    }

    #[test]
    fn a_grown_window_is_kept_off_the_panels() {
        // #448's re-run: Xfce with a 27-point top panel on a 1533x1138
        // screen; 1400x1000 at DPI 96 grown by 120 to 1750x1250 under
        // xfwm4's frame (5, 5, 29, 5). Shrunk to the whole screen, it was
        // pushed below the panel and its status line past the bottom.
        let monitor = vec2(1533.0, 1138.0);
        let area = Some(Rect::from_min_size(pos2(0.0, 27.0), vec2(1533.0, 1111.0)));
        let inner = Rect::from_min_size(pos2(10.0, 85.0), vec2(1750.0, 1250.0));
        let outer = Rect::from_min_size(pos2(5.0, 56.0), vec2(1760.0, 1284.0));
        assert_eq!(
            keep_on_screen(true, inner, outer, monitor, area),
            Some((vec2(1523.0, 1077.0), pos2(0.0, 27.0)))
        );
        // Fits the monitor but runs onto a bottom panel too: moved up.
        let both = Some(Rect::from_min_max(pos2(0.0, 27.0), pos2(1533.0, 1077.0)));
        let inner = Rect::from_min_size(pos2(105.0, 129.0), vec2(1000.0, 1000.0));
        let outer = Rect::from_min_size(pos2(100.0, 100.0), vec2(1010.0, 1034.0));
        assert_eq!(keep_on_screen(true, inner, outer, monitor, None), None);
        assert_eq!(keep_on_screen(true, inner, outer, monitor, area), None);
        assert_eq!(
            keep_on_screen(true, inner, outer, monitor, both),
            Some((vec2(1000.0, 1000.0), pos2(100.0, 43.0)))
        );
        // A work area that misses the monitor is no help: the monitor's used.
        let away = Some(Rect::from_min_size(pos2(2000.0, 0.0), vec2(800.0, 600.0)));
        assert_eq!(keep_on_screen(true, inner, outer, monitor, away), None);
    }

    #[test]
    fn windows_that_fit_or_were_moved_are_left_alone() {
        let monitor = vec2(1600.0, 1200.0);
        let inner = Rect::from_min_size(pos2(900.0, 820.0), vec2(1280.0, 800.0));
        let outer = Rect::from_min_size(pos2(900.0, 800.0), vec2(1280.0, 820.0));
        // Moved past the edge, not resized: someone put it there.
        assert_eq!(keep_on_screen(false, inner, outer, monitor, None), None);
        // Resized, and fits.
        let inner = Rect::from_min_size(pos2(10.0, 30.0), vec2(1000.0, 700.0));
        let outer = Rect::from_min_size(pos2(10.0, 10.0), vec2(1000.0, 720.0));
        assert_eq!(keep_on_screen(true, inner, outer, monitor, None), None);
        // On another monitor (to the right): shrunk, not moved.
        let inner = Rect::from_min_size(pos2(1700.0, 20.0), vec2(2000.0, 1000.0));
        let outer = Rect::from_min_size(pos2(1700.0, 0.0), vec2(2000.0, 1020.0));
        assert_eq!(
            keep_on_screen(true, inner, outer, monitor, None),
            Some((vec2(1600.0, 1000.0), pos2(1700.0, 0.0)))
        );
    }
}

#[cfg(test)]
mod xim_tests {
    #[test]
    fn xim_is_off_unless_kept() {
        assert_eq!(super::x_modifiers(false), Some("@im=none"));
        assert_eq!(super::x_modifiers(true), None);
    }
}

#[cfg(test)]
mod raise_tests {
    use super::*;
    use eframe::egui::{UserAttentionType, ViewportCommand};

    #[test]
    fn wayland_asks_for_attention_as_well() {
        let attention = ViewportCommand::RequestUserAttention(UserAttentionType::Informational);
        assert!(raise(true).contains(&attention));
        assert!(raise(true).contains(&ViewportCommand::Focus));
        assert!(!raise(false).contains(&attention));
        assert!(raise(false).contains(&ViewportCommand::Focus));
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn c(software: bool, gl: bool, usable: bool) -> Candidate {
        Candidate {
            software,
            gl,
            usable,
        }
    }

    #[test]
    fn adapters_the_scene_cant_use_are_passed_over() {
        // #764: Vulkan hidden, GL still offers the hardware without
        // VIEW_FORMATS; lavapipe draws.
        assert_eq!(
            pick(&[c(false, true, false), c(true, false, true)]),
            Some(1)
        );
        assert_eq!(pick(&[c(false, true, false)]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn hardware_first_then_not_gl() {
        let gl_hw = c(false, true, true);
        let vk_hw = c(false, false, true);
        let vk_sw = c(true, false, true);
        let gl_sw = c(true, true, true);
        assert_eq!(pick(&[gl_hw, vk_sw, vk_hw]), Some(2));
        assert_eq!(pick(&[vk_sw, gl_hw]), Some(1));
        assert_eq!(pick(&[gl_sw, vk_sw]), Some(1));
        // Ties keep the order wgpu gave.
        assert_eq!(pick(&[vk_hw, vk_hw]), Some(0));
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
