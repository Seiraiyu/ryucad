//! The main window: the viewport between the feature tree, the
//! participants bar and the status line.

use crate::core::link::Wake;
use crate::core::workspace::Workspace;
use crate::ui::panels;
use crate::ui::view::View;
use eframe::egui;
use ryucad::client::Daemon;
use ryucad::error::Result;
use std::path::Path;
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

/// The app's state.
pub struct App {
    pub ws: Workspace,
    pub view: View,
    titled: bool,
}

impl App {
    pub fn new(ws: Workspace) -> App {
        App {
            ws,
            view: View::new(),
            titled: false,
        }
    }

    /// The app on the project at `root`, repainting when the daemon says
    /// anything.
    pub fn open(ctx: &egui::Context, root: &Path, daemon: &Daemon) -> Result<App> {
        let ctx = ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        Ok(App::new(open_workspace(root, daemon, wake)?))
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
        if !self.titled {
            let name = self.ws.root.file_name().map_or_else(
                || self.ws.root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — ryucad")));
            self.titled = true;
        }
        egui::Panel::top("people").show(ui, |ui| panels::people(ui, &mut self.ws));
        egui::Panel::bottom("status").show(ui, |ui| panels::status(ui, &self.ws));
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
