//! What the app needs from the machine: a graphics adapter (a hardware
//! one if there is one, else a software one such as WARP or lavapipe,
//! with a warning), native dialogs for what stops it starting, and crash
//! reports that leave the session and files alone.

use eframe::{egui_wgpu, wgpu};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The window draws with a software adapter: it works, slowly.
static SOFTWARE: AtomicBool = AtomicBool::new(false);

pub fn software() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

/// The adapter the window draws with (`name (backend)`), for About.
static ADAPTER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub fn adapter() -> String {
    ADAPTER.lock().map(|a| a.clone()).unwrap_or_default()
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

/// The message when the window's event loop can't reach a display (#740):
/// no graphics adapter is at fault.
pub const NO_DISPLAY: &str = "No display could be opened for the window. On Linux, run it in a \
desktop session: WAYLAND_DISPLAY (Wayland) or DISPLAY (X11, with XAUTHORITY naming its cookie) \
must name a running display. Or see the model headless with `ryucad render`.";

/// The dialog's title and words when the window can't run, from eframe's
/// error: an event loop that can't start found no display (winit's
/// `EventLoopError`); anything else is the adapter's, unless it says so.
pub fn cant_run(error: &str) -> (&'static str, String) {
    let (title, advice) = if error.starts_with("winit EventLoopError") {
        ("ryucad can't open its window", NO_DISPLAY)
    } else {
        ("ryucad can't draw its window", NO_ADAPTER)
    };
    let message = if error.contains(advice) {
        error.to_owned()
    } else {
        format!("{error}\n\n{advice}")
    };
    (title, message)
}

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

/// Whether `adapter` really draws, tried before the window's device is
/// made: a surface it gives a configuration for, and a device of its own
/// that makes and writes a buffer without an error. Where nothing can draw
/// (Windows' session 0) an adapter may say it supports the surface and
/// still give a device whose buffers are invalid, and wgpu panics when
/// the window's first frame uses them (#85).
pub fn draws(adapter: &wgpu::Adapter, surface: Option<&wgpu::Surface<'_>>) -> bool {
    if surface.is_some_and(|s| s.get_default_config(adapter, 1, 1).is_none()) {
        return false;
    }
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ryucad probe"),
        ..Default::default()
    })) else {
        return false;
    };
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ryucad probe"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[0; 16]);
    queue.submit([]);
    // Popped innermost first, both whatever the first says.
    let internal = pollster::block_on(internal.pop());
    let validation = pollster::block_on(validation.pop());
    internal.is_none() && validation.is_none()
}

/// Picks an adapter for the window or a headless scene, noting a software
/// one. For the window, one that doesn't really draw ([`draws`]) is passed
/// over.
pub fn choose(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, String> {
    let mut candidates: Vec<Candidate> =
        adapters.iter().map(|a| Candidate::of(a, surface)).collect();
    let i = loop {
        let Some(i) = pick(&candidates) else {
            return Err(NO_ADAPTER.to_owned());
        };
        if surface.is_none() || draws(&adapters[i], surface) {
            break i;
        }
        let info = adapters[i].get_info();
        eprintln!(
            "ryucad-app: passing over {} ({:?}): it can't draw the window",
            info.name, info.backend
        );
        candidates[i].usable = false;
    };
    let a = &adapters[i];
    if let Ok(mut name) = ADAPTER.lock() {
        let info = a.get_info();
        *name = format!("{} ({:?})", info.name, info.backend);
    }
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

/// Shows an error that stops the app, on stderr, and natively where
/// someone can see it ([`interactive`]).
pub fn fatal(title: &str, message: &str) {
    eprintln!("ryucad-app: {message}");
    if !interactive() {
        return;
    }
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .show();
}

/// Whether someone can answer a dialog. Not in a Windows window station
/// that isn't visible (session 0, where services and SSH shells run),
/// where a dialog would wait for ever for a click nobody can make.
pub fn interactive() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::StationsAndDesktops::{
            GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::WSF_VISIBLE;
        let mut flags = USEROBJECTFLAGS {
            fInherit: 0,
            fReserved: 0,
            dwFlags: 0,
        };
        let mut needed = 0;
        // SAFETY: the station's handle needs no closing, and `flags` is
        // the size given.
        let ok = unsafe {
            GetUserObjectInformationW(
                GetProcessWindowStation(),
                UOI_FLAGS,
                (&raw mut flags).cast(),
                std::mem::size_of::<USEROBJECTFLAGS>() as u32,
                &mut needed,
            )
        };
        // When it can't be told, as before: a dialog.
        ok == 0 || flags.dwFlags & WSF_VISIBLE as u32 != 0
    }
    #[cfg(not(windows))]
    true
}

/// Frames the window has drawn ([`note_frame`]).
static FRAMES: AtomicU32 = AtomicU32::new(0);

/// Notes a frame of the window, before it is drawn.
pub fn note_frame() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// Whether a panic is wgpu refusing the window before it has drawn its
/// first frame (`frames` counts frames begun): a surface or a device that
/// can't draw it, such as Windows' session 0 gives, not a fault of ours.
/// The last resort, for what [`draws`] didn't catch: wgpu panics on such
/// errors, and some backends only report them when the first frame is
/// drawn.
pub fn cant_draw(message: &str, frames: u32) -> bool {
    frames < 2 && message.starts_with("wgpu error")
}

/// On a panic, writes a crash report to the runtime log folder before the
/// app exits. The daemon, the session and the files are not touched.
pub fn crash_reports() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(m) = info.payload_as_str()
            && cant_draw(m, FRAMES.load(Ordering::Relaxed))
        {
            fatal(
                "ryucad can't draw its window",
                &format!("{m}\n\n{NO_ADAPTER}"),
            );
            trace("exited: can't draw the window");
            std::process::exit(1);
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let report = format!(
            "ryucad-app {} crashed\n{info}\n\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        let what = info.payload_as_str().unwrap_or("?");
        if let Ok(dir) = ryucad::endpoint::runtime_dir() {
            let path = dir.join(format!("ryucad-app-crash-{at}.log"));
            if std::fs::write(&path, report).is_ok() {
                eprintln!("ryucad-app: crashed; report in {}", path.display());
                trace(&format!("panicked: {what}; report in {}", path.display()));
            } else {
                trace(&format!("panicked: {what}; the report couldn't be written"));
            }
        } else {
            trace(&format!(
                "panicked: {what}; no runtime folder for the report"
            ));
        }
        default(info);
    }));
}

/// The trace's cap: past it the old one becomes `ryucad-app.old.log`.
const TRACE_CAP: u64 = 1 << 20;

/// The app's trace (#799): `ryucad-app.log` in the runtime folder
/// (`%LOCALAPPDATA%\ryucad\run` on Windows), where the crash reports go.
pub fn trace_path() -> Option<std::path::PathBuf> {
    ryucad::endpoint::runtime_dir()
        .ok()
        .map(|d| d.join("ryucad-app.log"))
}

/// Appends a line to the trace: seconds since the epoch, the process,
/// the line. Never fails the app.
pub fn trace(line: &str) {
    use std::io::Write;
    let Some(path) = trace_path() else { return };
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > TRACE_CAP) {
        let _ = std::fs::rename(&path, path.with_file_name("ryucad-app.old.log"));
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{at} {} {line}", std::process::id());
    }
}

/// eframe's, wgpu's and winit's warnings and errors, into the trace:
/// eframe says why it ends the event loop only through `log`.
struct TraceLog;

impl log::Log for TraceLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            trace(&format!("{} {}: {}", r.level(), r.target(), r.args()));
        }
    }
    fn flush(&self) {}
}

/// Starts the trace: the logger, and on Windows without a console (the
/// release build's window, `main.rs`'s `windows_subsystem`) stderr into
/// the trace file, so what Rust's runtime prints as it aborts -- a stack
/// overflow, an allocation that failed, a panic's message -- is kept.
pub fn begin_trace(what: &str) {
    let _ = log::set_logger(&TraceLog).map(|()| log::set_max_level(log::LevelFilter::Warn));
    #[cfg(windows)]
    stderr_to_trace();
    trace(&format!(
        "ryucad-app {} started: {what}",
        env!("CARGO_PKG_VERSION")
    ));
}

#[cfg(windows)]
fn stderr_to_trace() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle};
    // SAFETY: reads the process's standard error handle.
    let h = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    // A console or a redirect: leave it where someone sees it.
    if !h.is_null() && h != INVALID_HANDLE_VALUE {
        return;
    }
    let Some(path) = trace_path() else { return };
    let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    // SAFETY: the handle is ours and stays open for the process's life;
    // std reads the standard handle on each write.
    unsafe { SetStdHandle(STD_ERROR_HANDLE, f.into_raw_handle()) };
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

    #[test]
    fn a_display_that_wont_open_isnt_blamed_on_the_adapter() {
        let x = "winit EventLoopError: os error at …/linux/mod.rs:788: Failed to open connection to X server";
        let (title, m) = cant_run(x);
        assert_eq!(title, "ryucad can't open its window");
        assert!(m.contains("WAYLAND_DISPLAY") && !m.contains("GPU"), "{m}");
        let (title, m) = cant_run("wgpu error: no adapter");
        assert_eq!(title, "ryucad can't draw its window");
        assert!(m.ends_with(NO_ADAPTER));
        assert_eq!(cant_run(NO_ADAPTER).1, NO_ADAPTER, "said once");
    }

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
    fn wgpu_refusing_the_first_frame_is_no_adapter() {
        // #85: Windows' session 0, by backend.
        let vulkan = "wgpu error: Validation Error\n\nCaused by:\n  In Queue::write_buffer\n    \
                      Buffer with 'scene uniforms' label is invalid";
        let dx12 = "wgpu error: Validation Error\n\nCaused by:\n  Invalid surface";
        for frames in [0, 1] {
            assert!(cant_draw(vulkan, frames));
            assert!(cant_draw(dx12, frames));
        }
        // Once a frame has drawn, it's a fault: a crash report.
        assert!(!cant_draw(vulkan, 2));
        assert!(!cant_draw("index out of bounds", 0));
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
