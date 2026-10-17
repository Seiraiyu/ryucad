//! The app's window: a project open (the viewport between the feature
//! tree, the participants bar, the property panel, the activity and the
//! status line), or a damaged project being repaired.

use crate::core::doctor::Offline;
use crate::core::link::Wake;
use crate::core::nav::Navigation;
use crate::core::workspace::Workspace;
use crate::ui::view::View;
use crate::ui::{dialogs, doctor, menu, panels};
use eframe::egui;
use ryucad::client::Daemon;
use ryucad::doc::project::find_project;
use ryucad::error::Result;
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

/// An open project's window.
pub struct Opened {
    pub ws: Workspace,
    pub view: View,
    pub tab: panels::Tab,
    pub dialogs: menu::Dialogs,
    pub doctor: doctor::State,
}

/// What the window shows.
pub enum Screen {
    Opened(Box<Opened>),
    /// A folder with parts but no manifest: repaired before it opens.
    Damaged(Box<(Offline, doctor::State)>),
}

/// The app's state.
pub struct App {
    pub screen: Screen,
    pub daemon: Daemon,
    wake: Wake,
    /// The folder the title was last set for.
    titled: Option<PathBuf>,
}

impl App {
    /// The app on the project in or above `dir`, repainting when anything
    /// arrives. A damaged project opens to its repairs.
    pub fn open(ctx: &egui::Context, dir: &Path, daemon: &Daemon) -> Result<App> {
        let ctx = ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let (root, damaged) = find_project(dir)?;
        let screen = if damaged {
            Screen::Damaged(Box::new((
                Offline::new(&root, daemon, wake.clone()),
                doctor::State::default(),
            )))
        } else {
            Screen::Opened(Box::new(Opened::new(open_workspace(
                &root,
                daemon,
                wake.clone(),
            )?)))
        };
        Ok(App {
            screen,
            daemon: daemon.clone(),
            wake,
            titled: None,
        })
    }

    /// The open project. Panics if there is none: for tests.
    pub fn opened(&self) -> &Opened {
        match &self.screen {
            Screen::Opened(o) => o,
            _ => panic!("no project is open"),
        }
    }

    /// The folder on screen.
    pub fn root(&self) -> &Path {
        match &self.screen {
            Screen::Opened(o) => &o.ws.root,
            Screen::Damaged(d) => &d.0.root,
        }
    }

    /// One frame.
    pub fn frame(&mut self, ui: &mut egui::Ui, gpu: Option<&eframe::egui_wgpu::RenderState>) {
        let ctx = ui.ctx().clone();
        if self.titled.as_deref() != Some(self.root()) {
            let root = self.root().to_path_buf();
            let name = root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — ryucad")));
            self.titled = Some(root);
        }
        let mut next = None;
        match &mut self.screen {
            Screen::Opened(o) => o.frame(ui, gpu),
            Screen::Damaged(d) => {
                let (offline, state) = &mut **d;
                offline.poll();
                if doctor::damaged(ui, offline, state) {
                    match open_workspace(&offline.root, &self.daemon, self.wake.clone()) {
                        Ok(ws) => next = Some(Screen::Opened(Box::new(Opened::new(ws)))),
                        Err(e) => offline.error = Some(e),
                    }
                }
            }
        }
        if let Some(s) = next {
            self.screen = s;
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
        };
        // Damage that doesn't stop the project opening shows as badges.
        o.ws.diagnose();
        o
    }

    /// One frame: take what arrived, then draw.
    pub fn frame(&mut self, ui: &mut egui::Ui, gpu: Option<&eframe::egui_wgpu::RenderState>) {
        let ctx = ui.ctx().clone();
        self.ws.pump();
        self.ws.tick(Instant::now());
        // Keep frames coming while something is due.
        if self.ws.camera_due() {
            ctx.request_repaint_after(crate::core::workspace::CAMERA_EVERY);
        }
        if let Some(at) = self.ws.retry_at() {
            ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
        }
        self.apply_preferences(&ctx);
        egui::Panel::top("menu").show(ui, |ui| menu::bar(ui, self));
        egui::Panel::top("people").show(ui, |ui| panels::people(ui, &mut self.ws));
        self.shortcuts(&ctx);
        egui::Panel::bottom("status").show(ui, |ui| panels::status(ui, &self.ws));
        egui::Panel::bottom("journal")
            .resizable(true)
            .default_size(160.0)
            .show(ui, |ui| panels::journal(ui, &mut self.ws, &mut self.tab));
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| panels::tree(ui, &mut self.ws, &mut self.view));
            });
        egui::Panel::right("properties")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| panels::properties(ui, &mut self.ws));
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.view.show(ui, &mut self.ws, gpu));
        dialogs::settings(&ctx, &mut self.ws, &mut self.dialogs.settings);
        dialogs::preferences(&ctx, &mut self.ws, &mut self.dialogs.preferences);
        doctor::dialog(
            &ctx,
            &mut self.ws,
            &mut self.dialogs.doctor,
            &mut self.doctor,
        );
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
        self.view.nav.style = Navigation::parse(self.ws.settings.text("ui.navigation", "ryucad"));
        self.view.edges = self
            .ws
            .settings
            .get("display.edges")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
    }

    /// Ctrl/Cmd+Z undoes this window's latest change, Shift+Ctrl/Cmd+Z
    /// redoes; not while a text field has the keyboard (it has its own).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        if ctx.input_mut(|i| i.consume_shortcut(&redo)) {
            self.ws.redo(None);
        } else if ctx.input_mut(|i| i.consume_shortcut(&undo)) {
            self.ws.undo(None);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let gpu = frame.wgpu_render_state().cloned();
        self.frame(ui, gpu.as_ref());
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
