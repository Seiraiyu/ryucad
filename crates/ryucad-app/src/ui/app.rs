//! The app's window: a project open (the viewport between the feature
//! tree, the participants bar, the property panel, the activity and the
//! status line), or a damaged project being repaired.

use crate::core::doctor::Offline;
use crate::core::link::Wake;
use crate::core::nav::Navigation;
use crate::core::workspace::Workspace;
use crate::ui::start::{self, Start};
use crate::ui::style::{self, Look};
use crate::ui::view::View;
use crate::ui::{dialogs, doctor, menu, panels};
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
    pub tab: panels::Tab,
    pub dialogs: menu::Dialogs,
    pub doctor: doctor::State,
    /// Recent projects, newest first, for the File menu.
    pub recent: Vec<PathBuf>,
    /// A revision being typed to compare with (View > Compare with…).
    pub compare_with: Option<String>,
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
}

impl App {
    /// The app on the project in or above `dir`, or the start window with
    /// none. It repaints whenever anything arrives.
    pub fn open(ctx: &egui::Context, dir: Option<&Path>, daemon: &Daemon) -> App {
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
            ctrl_click: false,
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
                self.screen = Screen::Start(Box::new(Start::new(&self.daemon, &self.wake)));
            }
            Some(menu::Action::Quit) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            // With no project open, Close closes the window.
            None if !matches!(self.screen, Screen::Opened(_))
                && ctx.input_mut(|i| i.consume_shortcut(&menu::CLOSE)) =>
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
            tab: panels::Tab::default(),
            dialogs: menu::Dialogs::default(),
            doctor: doctor::State::default(),
            recent: crate::core::recent::load(),
            compare_with: None,
        };
        o.view.look = user_look();
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
        self.ws.pump();
        // `ryucad open` asked for this window.
        if std::mem::take(&mut self.ws.raised) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
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
        self.apply_preferences(&ctx);
        let recent = self.recent.clone();
        let action = egui::Panel::top("menu")
            .show(ui, |ui| menu::bar(ui, self, &recent))
            .inner;
        egui::Panel::top("people").show(ui, |ui| panels::people(ui, &mut self.ws));
        if self.compare_with.is_some() || self.ws.compare.is_some() {
            egui::Panel::top("compare").show(ui, |ui| {
                panels::compare_bar(ui, &mut self.ws, &mut self.compare_with);
            });
        }
        let action = action.or(self.shortcuts(&ctx));
        egui::Panel::bottom("status").show(ui, |ui| panels::status(ui, &self.ws));
        // Room for about ten entries; drag its top edge for more.
        egui::Panel::bottom("journal")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| panels::journal(ui, &mut self.ws, &mut self.tab));
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(220.0)
            .min_size(180.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| panels::tree(ui, &mut self.ws, &mut self.view));
            });
        egui::Panel::right("properties")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    panels::merge(ui, &mut self.ws, &mut self.view);
                    panels::changes(ui, &self.ws, &self.view);
                    panels::properties(ui, &mut self.ws);
                    ui.separator();
                    panels::parameters(ui, &mut self.ws, &self.view);
                });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.view.show(ui, &mut self.ws, gpu));
        let view = self.view.rect;
        dialogs::settings(&ctx, &mut self.ws, &mut self.dialogs.settings, view);
        dialogs::preferences(&ctx, &mut self.ws, &mut self.dialogs.preferences, view);
        doctor::dialog(
            &ctx,
            &mut self.ws,
            &mut self.dialogs.doctor,
            &mut self.doctor,
            view,
        );
        action
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
        let on = |key| {
            self.ws
                .settings
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        };
        self.view.edges = on("display.edges");
        self.view.sketches = on("display.sketches");
    }

    /// Ctrl/Cmd+Z undoes this window's latest change, Shift+Ctrl/Cmd+Z or
    /// Ctrl+Y redoes, and Escape clears the selection; not while a text
    /// field has the keyboard (it has its own), and Escape not while a menu
    /// or popup is open (it closes those).
    /// The keyboard shortcuts; returns what they ask of the app.
    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<menu::Action> {
        // Closing works from a text field too, as in any other app.
        if ctx.input_mut(|i| i.consume_shortcut(&menu::CLOSE)) {
            return Some(menu::Action::Close);
        }
        if ctx.memory(|m| m.focused().is_some()) {
            return None;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&menu::REDO) || i.consume_shortcut(&menu::REDO_Y)) {
            self.ws.redo(None);
        } else if ctx.input_mut(|i| i.consume_shortcut(&menu::UNDO)) {
            self.ws.undo(None);
        }
        if !egui::Popup::is_any_open(ctx)
            && self.ws.myself().is_some_and(|m| !m.selection.is_empty())
            && ctx.input_mut(|i| i.consume_shortcut(&menu::CLEAR_SELECTION))
        {
            self.ws
                .select(Vec::new(), ryucad::live::SelectMode::Replace);
        }
        None
    }
}

impl eframe::App for App {
    /// On macOS, Ctrl+click is a right-click.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if cfg!(target_os = "macos") {
            crate::ui::platform::ctrl_click_as_secondary(&mut raw.events, &mut self.ctrl_click);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // A new window to the front: on Windows it otherwise stays behind
        // whatever was in front when it was started.
        if !std::mem::replace(&mut self.shown, true) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let gpu = frame.wgpu_render_state().cloned();
        self.frame(ui, gpu.as_ref());
        if std::mem::take(&mut self.pick) {
            if let Some(dir) = start::pick_folder(frame) {
                self.go(&dir);
            }
            // What the folder changed is drawn now, not at the next event:
            // a click must find the buttons where it sees them.
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
