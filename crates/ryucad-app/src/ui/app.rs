//! The app's window: a project open (the viewport between the feature
//! tree, the participants bar, the property panel, the activity and the
//! status line), or a damaged project being repaired.

use crate::core::doctor::Offline;
use crate::core::geometry::{self, Geometry, Tracker};
use crate::core::link::Wake;
use crate::core::nav::Navigation;
use crate::core::workspace::Workspace;
use crate::ui::start::{self, Start};
use crate::ui::style::{self, Look};
use crate::ui::view::View;
use crate::ui::{commands, dialogs, doctor, menu, panels};
use eframe::egui;
use ryucad::client::Daemon;
use ryucad::doc::project::find_project;
use ryucad::error::{Code, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// The name a window says hello with: the OS user and the first window
/// number no other window of theirs has, `dana (window 2)`.
pub fn window_name(user: &str, participants: &serde_json::Value) -> String {
    let taken: Vec<&str> = participants
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["kind"] == "human")
        .filter_map(|p| p["name"].as_str())
        .collect();
    (1..)
        .map(|n| format!("{user} (window {n})"))
        .find(|name| !taken.contains(&name.as_str()))
        .expect("a free number")
}

/// How often the tiles are saved while they change.
pub const LAYOUT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a closing window waits for its tiles' last save: long enough
/// for a daemon to answer, short enough that a hung one can't hold the
/// window open.
pub const LAYOUT_CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Close, on the start window and the repair window, which have no
/// keymap of their own: `file.close`'s default.
const CLOSE: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::W);

/// The OS user's name.
pub fn os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "someone".to_owned())
}

/// Opens the project at `root` as a new window of the OS user's.
pub fn open_workspace(root: &Path, daemon: &Daemon, wake: Wake) -> Result<Workspace> {
    let mut conn = daemon.connect(root)?;
    let list = conn
        .call(&ryucad::proto::Request::ParticipantList {})
        .map_err(|e| ryucad::error::Error::new(ryucad::error::Code::Daemon, e.to_string()))??;
    let name = window_name(&os_user(), &list["participants"]);
    drop(conn);
    Workspace::open(root, daemon, &name, wake)
}

/// The window geometry the `app.window` preference holds, if sensible.
pub fn saved_geometry() -> Option<Geometry> {
    Geometry::from_value(&ryucad::config::user("app.window"))
}

/// The `ui.style` preference as the preferences file has it.
fn user_look() -> Look {
    Look::parse(
        ryucad::config::user("ui.style")
            .as_str()
            .unwrap_or_default(),
    )
}

/// An open project's window.
pub struct Opened {
    pub ws: Workspace,
    pub view: View,
    /// The tiles round the viewport (`ui::shell`, the `ui.layout`
    /// preference).
    pub tiles: egui_tiles::Tree<crate::ui::shell::Pane>,
    /// The tiles changed: saved at most once a second (`LAYOUT_EVERY`).
    pub layout_dirty: bool,
    layout_saved: Option<Instant>,
    /// Why the saved layout wasn't used, said once in the status line.
    pub layout_note: Option<String>,
    pub dialogs: menu::Dialogs,
    pub doctor: doctor::State,
    /// Recent projects, newest first, for the File menu.
    pub recent: Vec<PathBuf>,
    /// A revision being typed to compare with (View > Compare with…).
    pub compare_with: Option<String>,
    /// A STEP dialog's Browse… was pressed: the OS's file dialog opens
    /// once the frame is done.
    pub browse: Option<crate::ui::step::Browse>,
    /// The command palette, while it is open.
    pub palette: Option<crate::ui::palette::Palette>,
    /// Commands the palette ran, newest first.
    pub recent_commands: Vec<&'static str>,
    /// The Shortcuts tile's search and the chord it waits for.
    pub shortcuts: crate::ui::shortcuts::Shortcuts,
    /// A merge was in progress at the last frame: its start opens the
    /// Conflicts tile, its end closes it.
    merging: bool,
    /// The effective keys: the registry's, the navigation preset's, then
    /// `ui.keys` ([`crate::core::keys::Keymap`]).
    pub keymap: crate::core::keys::Keymap,
    /// What the keymap was made from, so it is made again only when that
    /// changes.
    keymap_from: (Navigation, serde_json::Value),
}

/// What the window shows.
pub enum Screen {
    /// No project: recent projects and Open.
    Start(Box<Start>),
    Opened(Box<Opened>),
    /// A folder with parts but no manifest: repaired before it opens.
    Damaged(Box<(Offline, doctor::State)>),
}

/// The app's state.
pub struct App {
    pub screen: Screen,
    pub daemon: Daemon,
    wake: Wake,
    /// The window title last set.
    titled: Option<String>,
    /// The window has asked for the foreground once, when it first showed.
    shown: bool,
    /// Open… was chosen: the folder dialog runs once the frame is done,
    /// not in the middle of drawing it.
    pick: bool,
    /// A Ctrl+click (macOS) is down, so its release is a right one too.
    ctrl_click: bool,
    /// The window's inner size, in points, and its scale at the last frame.
    size: Option<(egui::Vec2, f32)>,
    /// The window's size, saved in `app.window` once it settles.
    geometry: Tracker,
}

impl App {
    /// The app on the project in or above `dir`, or the start window with
    /// none. It repaints whenever anything arrives.
    pub fn open(ctx: &egui::Context, dir: Option<&Path>, daemon: &Daemon) -> App {
        crate::ui::fonts::install(ctx);
        // The start window wears the user's style too; an open project
        // follows the preference as the daemon announces it.
        style::apply(ctx, user_look());
        let ctx = ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let mut app = App {
            screen: Screen::Start(Box::new(Start::new(daemon, &wake))),
            daemon: daemon.clone(),
            wake,
            titled: None,
            shown: false,
            pick: false,
            size: None,
            ctrl_click: false,
            geometry: Tracker::new(saved_geometry()),
        };
        if let Some(dir) = dir {
            app.go(dir);
        }
        app
    }

    /// Opens the project in or above `dir`: a damaged one to its repairs,
    /// a folder that isn't one to the offer of a new project.
    pub fn go(&mut self, dir: &Path) {
        let failed = |app: &mut App, not_a_project: Option<PathBuf>, error: Option<String>| {
            let mut start = Start::new(&app.daemon, &app.wake);
            start.not_a_project = not_a_project;
            start.error = error;
            app.screen = Screen::Start(Box::new(start));
        };
        match find_project(dir) {
            Ok((root, true)) => {
                let offline = Offline::new(&root, &self.daemon, self.wake.clone());
                self.screen = Screen::Damaged(Box::new((offline, doctor::State::default())));
            }
            Ok((root, false)) => match open_workspace(&root, &self.daemon, self.wake.clone()) {
                Ok(ws) => self.screen = Screen::Opened(Box::new(Opened::new(ws))),
                Err(e) => failed(self, None, Some(e.message)),
            },
            Err(e) if e.code == Code::ProjectNotFound => {
                let dir = dir
                    .canonicalize()
                    .map_or_else(|_| dir.to_path_buf(), ryucad::doc::project::plain);
                failed(self, Some(dir), None);
            }
            Err(e) => failed(self, None, Some(e.message)),
        }
    }

    /// Makes `dir` a project (`ryucad init`), then opens it.
    fn create(&mut self, dir: &Path) {
        match crate::core::doctor::cli(&self.daemon, dir, &["init".to_owned()]) {
            Ok(_) => self.go(dir),
            Err(e) => {
                if let Screen::Start(s) = &mut self.screen {
                    s.error = Some(e.message);
                }
            }
        }
    }

    /// The open project. Panics if there is none: for tests.
    pub fn opened(&self) -> &Opened {
        match &self.screen {
            Screen::Opened(o) => o,
            _ => panic!("no project is open"),
        }
    }

    /// The open project, to change. Panics if there is none: for tests.
    pub fn opened_mut(&mut self) -> &mut Opened {
        match &mut self.screen {
            Screen::Opened(o) => o,
            _ => panic!("no project is open"),
        }
    }

    /// The window's title: the project, and a merge in progress.
    pub fn title(&self) -> String {
        let name = |root: &Path| {
            root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        match &self.screen {
            Screen::Start(_) => "ryucad".to_owned(),
            Screen::Opened(o) => match &o.ws.merge {
                Some(m) => format!("{} ({} in progress) — ryucad", name(&o.ws.root), m.step()),
                None => format!("{} — ryucad", name(&o.ws.root)),
            },
            Screen::Damaged(d) if d.0.openable() => {
                format!("{} (repaired) — ryucad", name(&d.0.root))
            }
            Screen::Damaged(d) => format!("{} (needs repair) — ryucad", name(&d.0.root)),
        }
    }

    /// One frame.
    pub fn frame(&mut self, ui: &mut egui::Ui, gpu: Option<&eframe::egui_wgpu::RenderState>) {
        let ctx = ui.ctx().clone();
        let title = self.title();
        if self.titled.as_ref() != Some(&title) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.titled = Some(title);
        }
        let mut go = None;
        match &mut self.screen {
            Screen::Start(s) => match start::show(ui, s) {
                Some(start::Action::Open(dir)) => go = Some(menu::Action::Open(Some(dir))),
                Some(start::Action::Pick) => go = Some(menu::Action::Open(None)),
                Some(start::Action::Create(dir)) => {
                    self.create(&dir);
                    return;
                }
                None => {}
            },
            Screen::Opened(o) => go = o.frame(ui, gpu),
            Screen::Damaged(d) => {
                let (offline, state) = &mut **d;
                offline.poll();
                if doctor::damaged(ui, offline, state) {
                    go = Some(menu::Action::Open(Some(offline.root.clone())));
                }
            }
        }
        match go {
            Some(menu::Action::Open(Some(dir))) => self.go(&dir),
            Some(menu::Action::Open(None)) => self.pick = true,
            Some(menu::Action::Close) => {
                if let Screen::Opened(o) = &mut self.screen {
                    o.save_layout_before_closing();
                }
                self.screen = Screen::Start(Box::new(Start::new(&self.daemon, &self.wake)));
            }
            Some(menu::Action::Quit) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            // With no project open, Close closes the window.
            None if !matches!(self.screen, Screen::Opened(_))
                && ctx.input_mut(|i| i.consume_shortcut(&CLOSE)) =>
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }
}

impl Opened {
    pub fn new(ws: Workspace) -> Opened {
        let mut o = Opened {
            ws,
            view: View::new(),
            tiles: egui_tiles::Tree::empty(crate::ui::shell::tree_id()),
            layout_dirty: false,
            layout_saved: None,
            layout_note: None,
            dialogs: menu::Dialogs::default(),
            doctor: doctor::State::default(),
            recent: crate::core::recent::load(),
            compare_with: None,
            browse: None,
            palette: None,
            recent_commands: Vec::new(),
            shortcuts: Default::default(),
            merging: false,
            keymap: crate::core::keys::Keymap::new(Navigation::default(), &serde_json::json!({})),
            keymap_from: (Navigation::default(), serde_json::json!({})),
        };
        o.view.look = user_look();
        let read = crate::core::layout::read(&ryucad::config::user("ui.layout"));
        o.tiles = crate::ui::shell::tree(&read.root);
        o.layout_note = read.note;
        // Damage that doesn't stop the project opening shows as badges.
        o.ws.diagnose();
        o
    }

    /// One frame: take what arrived, then draw. Returns what the menus ask
    /// of the app.
    pub fn frame(
        &mut self,
        ui: &mut egui::Ui,
        gpu: Option<&eframe::egui_wgpu::RenderState>,
    ) -> Option<menu::Action> {
        let ctx = ui.ctx().clone();
        // Before the menus draw: an Escape that closes one is theirs.
        let popup_open = egui::Popup::is_any_open(&ctx);
        self.ws.pump();
        // `ryucad open` asked for this window.
        if std::mem::take(&mut self.ws.raised) {
            for c in crate::ui::platform::raise(crate::ui::platform::wayland()) {
                ctx.send_viewport_cmd(c);
            }
        }
        self.read_recent();
        self.ws.tick(Instant::now());
        // Keep frames coming while something is due.
        if self.ws.camera_due() {
            ctx.request_repaint_after(crate::core::workspace::CAMERA_EVERY);
        }
        // An error leaves the status line on time, mouse or not.
        if let Some(d) = self.ws.error_due(Instant::now()) {
            ctx.request_repaint_after(d);
        }
        if let Some(at) = self.ws.retry_at() {
            ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
        }
        // A bar due to show, a toast due to go; the bar moves while shown.
        if let Some(d) = self.ws.feedback_due(Instant::now()) {
            ctx.request_repaint_after(d);
        }
        if self.ws.progress.shown(Instant::now()).is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.ws.focused = ctx.input(|i| i.focused);
        self.apply_preferences(&ctx);
        self.view.commands = commands::shown(self, &ctx);
        let action = egui::Panel::top("menu")
            .show(ui, |ui| menu::bar(ui, self))
            .inner;
        let tools = egui::Panel::top("toolbar")
            .show(ui, |ui| crate::ui::toolbar::bar(ui, self))
            .inner;
        let action = action.or(tools);
        let action = action.or_else(|| commands::keys(self, &ctx, popup_open));
        let look = self.view.look;
        egui::Panel::bottom("status").show(ui, |ui| panels::status(ui, &mut self.ws, look));
        if self.ws.lost {
            let p = look.palette(ctx.theme() == egui::Theme::Dark);
            egui::Panel::top("disconnected")
                .frame(
                    egui::Frame::new()
                        .fill(crate::ui::feedback::disconnected_fill(&p))
                        .stroke(egui::Stroke::new(1.0, style::rgb(p.danger)))
                        .inner_margin(egui::Margin::symmetric(12, 4)),
                )
                .show(ui, |ui| {
                    crate::ui::feedback::disconnected(ui, &mut self.ws, &p)
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut tiles = std::mem::replace(
                &mut self.tiles,
                egui_tiles::Tree::empty(crate::ui::shell::tree_id()),
            );
            let mut b = crate::ui::shell::Behavior {
                o: self,
                gpu,
                edited: false,
            };
            tiles.ui(&mut b, ui);
            if b.edited {
                self.layout_dirty = true;
            }
            self.tiles = tiles;
        });
        self.save_layout(&ctx, false);
        self.follow_merge();
        crate::ui::feedback::card(&ctx, self);
        crate::ui::feedback::toasts(&ctx, self);
        let view = self.view.rect;
        dialogs::settings(&ctx, &mut self.ws, &mut self.dialogs.settings, view);
        dialogs::preferences(&ctx, &mut self.ws, &mut self.dialogs.preferences, view);
        crate::ui::tiles::releases::release_dialogs(&ctx, &mut self.ws, view);
        doctor::dialog(
            &ctx,
            &mut self.ws,
            &mut self.dialogs.doctor,
            &mut self.doctor,
            view,
        );
        if let Some(faces) =
            dialogs::interference(&ctx, &mut self.ws, &mut self.dialogs.interference, view)
        {
            self.view.fit_entities(&mut self.ws, &faces);
        }
        dialogs::measure(&ctx, &mut self.ws, view);
        dialogs::appearance(&ctx, &mut self.ws, view);
        dialogs::mass(&ctx, &mut self.ws, &mut self.dialogs.mass, view);
        if let Some(points) =
            dialogs::clearance(&ctx, &mut self.ws, &mut self.dialogs.clearance, view)
        {
            self.view.fit_points(&mut self.ws, &points);
        }
        crate::ui::help::about(&ctx, &mut self.dialogs.about, view);
        crate::ui::help::agents(&ctx, &mut self.dialogs.agents, &self.ws.root, view);
        let action = action.or_else(|| crate::ui::palette::show(&ctx, self));
        crate::ui::step::import(&ctx, &mut self.ws, &mut self.browse, view);
        if let Some(faces) = crate::ui::export::dialog(&ctx, &mut self.ws, &mut self.browse, view) {
            self.view.fit_entities(&mut self.ws, &faces);
        }
        crate::ui::bom::export(&ctx, &mut self.ws, &mut self.browse, view);
        // What a context menu chose, with what it was opened on.
        let action = match self.view.ran.take() {
            Some((id, target)) => action.or(commands::run(self, &ctx, id, target.as_deref())),
            None => action,
        };
        // Closing the dialog ends what it paints (#138).
        if !self.dialogs.interference {
            self.ws.interference = None;
        }
        if !self.dialogs.mass {
            self.ws.mass = None;
        }
        if !self.dialogs.clearance {
            self.ws.clearance = None;
        }
        action
    }

    /// The Conflicts tile opens, as the first tab on the right, when a
    /// merge starts, and closes itself when it ends.
    fn follow_merge(&mut self) {
        let merging = self.ws.merge.is_some();
        if merging && !self.merging {
            crate::ui::shell::open_first(&mut self.tiles, "conflicts");
        } else if !merging && self.merging {
            crate::ui::shell::close(&mut self.tiles, "conflicts");
        }
        self.merging = merging;
    }

    /// Saves the tiles to `ui.layout` once they changed, at most once a
    /// second, or at once with `now` (the window closing).
    pub fn save_layout(&mut self, ctx: &egui::Context, now: bool) {
        if !self.layout_dirty {
            return;
        }
        let due = self
            .layout_saved
            .is_none_or(|t| t.elapsed() >= LAYOUT_EVERY);
        if !(due || now) {
            ctx.request_repaint_after(LAYOUT_EVERY);
            return;
        }
        if let Some(n) = crate::ui::shell::node(&self.tiles) {
            self.ws.set_config("ui.layout", n.to_value());
        }
        self.layout_dirty = false;
        self.layout_saved = Some(Instant::now());
    }

    /// The tiles as they were left, saved before the window closes and
    /// waited for (at most `LAYOUT_CLOSE_WAIT`): the process may exit as
    /// soon as this returns, and an unsent request would be lost.
    pub fn save_layout_before_closing(&mut self) {
        if !self.layout_dirty {
            return;
        }
        if let Some(n) = crate::ui::shell::node(&self.tiles) {
            self.ws
                .set_config_waiting("ui.layout", n.to_value(), LAYOUT_CLOSE_WAIT);
        }
        self.layout_dirty = false;
        self.layout_saved = Some(Instant::now());
    }

    /// The recent projects, as the daemon last reported them: it puts
    /// this project first when the window says hello.
    fn read_recent(&mut self) {
        if let Some(v) = self.ws.settings.get("app.recent_projects") {
            self.recent = crate::core::recent::parse(v);
        }
    }

    /// The user's preferences, as they stand: the daemon announces every
    /// change, whoever makes it.
    fn apply_preferences(&mut self, ctx: &egui::Context) {
        let theme = match self.ws.settings.text("ui.theme", "system") {
            "dark" => egui::ThemePreference::Dark,
            "light" => egui::ThemePreference::Light,
            _ => egui::ThemePreference::System,
        };
        if ctx.options(|o| o.theme_preference) != theme {
            ctx.set_theme(theme);
        }
        let look = self
            .ws
            .settings
            .get("ui.style")
            .and_then(serde_json::Value::as_str);
        if let Some(look) = look.map(Look::parse)
            && self.view.look != look
        {
            style::apply(ctx, look);
            self.view.look = look;
        }
        self.view.nav.style = Navigation::parse(self.ws.settings.text("ui.navigation", "ryucad"));
        // Which toasts show.
        let notify = self
            .ws
            .settings
            .get("ui.notify")
            .cloned()
            .unwrap_or_default();
        self.ws.toasts.notify = crate::core::toast::Notify::parse(&notify);
        let keys = self.ws.settings.get("ui.keys").cloned().unwrap_or_default();
        if self.keymap_from.0 != self.view.nav.style || self.keymap_from.1 != keys {
            self.keymap = crate::core::keys::Keymap::new(self.view.nav.style, &keys);
            self.keymap_from = (self.view.nav.style, keys);
        }
        // The viewport's own keys (Tab and its kin), for `View::take_keys`.
        let mac = commands::mac(ctx);
        self.view.keys = self
            .keymap
            .all()
            .filter(|(id, _)| {
                crate::core::command::get(id)
                    .is_some_and(|c| c.scope == crate::core::command::Scope::Viewport)
            })
            .flat_map(|(id, chords)| chords.iter().map(move |c| (c.shortcut(mac), id)))
            .collect();
        let on = |key| {
            self.ws
                .settings
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        };
        self.view.edges = on("display.edges");
        self.view.sketches = on("display.sketches");
        self.view.cube = on("display.nav_cube");
        self.view.triad = on("display.triad");
        self.view.origin = self
            .ws
            .settings
            .get("display.origin")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    }
}

impl App {
    /// Brings the window back onto its monitor when a change of size left
    /// it past it ([`crate::ui::platform::keep_on_screen`]).
    fn keep_on_screen(&mut self, ctx: &egui::Context) {
        let info = ctx.input(|i| i.viewport().clone());
        let (Some(inner), Some(outer), Some(monitor)) =
            (info.inner_rect, info.outer_rect, info.monitor_size)
        else {
            return;
        };
        let full = info.maximized == Some(true) || info.fullscreen == Some(true);
        let now = (inner.size(), info.native_pixels_per_point.unwrap_or(1.0));
        // The first frame has nothing to compare with.
        let resized = self.size.replace(now).is_some_and(|(size, scale)| {
            (size - now.0).length() >= 0.5 || (scale - now.1).abs() > 1e-3
        });
        if full || !resized {
            return;
        }
        // The work area is in pixels; the window's rects are in points.
        let area = crate::ui::platform::work_area().map(|r| r * (1.0 / now.1));
        if let Some((size, pos)) =
            crate::ui::platform::keep_on_screen(resized, inner, outer, monitor, area)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
    }

    /// Saves the window's size once it settles, through the open project's
    /// `config.set` (the start window has no daemon to ask).
    fn keep_geometry(&mut self, ctx: &egui::Context) {
        let Screen::Opened(o) = &mut self.screen else {
            return;
        };
        // The viewport rect is the window's inner size in points.
        let (r, maximized) = ctx.input(|i| (i.viewport_rect(), i.viewport().maximized));
        let size = [r.width(), r.height()];
        if let Some(g) = self
            .geometry
            .observe(size, maximized.unwrap_or(false), Instant::now())
        {
            o.ws.set_config(
                "app.window",
                g.into_value(&ryucad::config::user("app.window")),
            );
        }
        if self.geometry.pending() {
            ctx.request_repaint_after(geometry::SETTLE);
        }
    }
}

impl eframe::App for App {
    /// The tiles as they were left.
    fn on_exit(&mut self) {
        if let Screen::Opened(o) = &mut self.screen {
            o.save_layout_before_closing();
        }
    }

    /// On macOS, Ctrl+click is a right-click; Tab over the viewport is the
    /// view's, not egui's focus key.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if cfg!(target_os = "macos") {
            crate::ui::platform::ctrl_click_as_secondary(&mut raw.events, &mut self.ctrl_click);
        }
        if let Screen::Opened(o) = &mut self.screen {
            o.view.take_keys(ctx, &mut raw.events);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        crate::ui::platform::note_frame();
        // A new window to the front: on Windows it otherwise stays behind
        // whatever was in front when it was started.
        if !std::mem::replace(&mut self.shown, true) {
            crate::ui::platform::note_display(frame);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.keep_on_screen(ui.ctx());
        let gpu = frame.wgpu_render_state().cloned();
        self.frame(ui, gpu.as_ref());
        self.keep_geometry(ui.ctx());
        if std::mem::take(&mut self.pick) {
            if let Some(dir) = start::pick_folder(frame) {
                self.go(&dir);
            }
            // What the folder changed is drawn now, not at the next event:
            // a click must find the buttons where it sees them.
            ui.ctx().request_repaint();
        }
        if let Screen::Opened(o) = &mut self.screen
            && let Some(how) = o.browse.take()
        {
            use crate::ui::step::Browse;
            if let Some(path) = crate::ui::step::ask_os(frame, how, &o.ws) {
                let path = path.display().to_string();
                match how {
                    Browse::Open => {
                        if let Some(i) = &mut o.ws.import {
                            i.path = path;
                        }
                        o.ws.plan_import();
                    }
                    Browse::Save => {
                        if let Some(e) = &mut o.ws.export {
                            e.path = path;
                        }
                    }
                    Browse::SaveCsv => {
                        if let Some(e) = &mut o.ws.bom_export {
                            e.path = path;
                        }
                    }
                }
            }
            ui.ctx().request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn windows_are_numbered_per_user() {
        let list = json!([
            {"kind": "human", "name": "dana (window 1)"},
            {"kind": "agent", "name": "dana (window 2)"},
            {"kind": "human", "name": "kai (window 2)"},
        ]);
        assert_eq!(window_name("dana", &list), "dana (window 2)");
        assert_eq!(window_name("kai", &list), "kai (window 1)");
        assert_eq!(window_name("dana", &json!([])), "dana (window 1)");
    }
}
