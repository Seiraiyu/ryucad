//! The menu bar. egui draws it in the window on every OS, macOS included.

use crate::ui::app::Opened;
use eframe::egui;
use ryucad::live::SelectMode;
use ryucad::view::StandardView;
use serde_json::json;
use std::path::PathBuf;

/// The dialogs a menu opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dialogs {
    pub settings: bool,
    pub preferences: bool,
    pub doctor: bool,
}

/// What a menu asks of the app rather than of the open project.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open a project: this folder, or one the person picks.
    Open(Option<PathBuf>),
    Close,
    Quit,
}

const VIEWS: [(&str, StandardView); 7] = [
    ("Front", StandardView::Front),
    ("Back", StandardView::Back),
    ("Left", StandardView::Left),
    ("Right", StandardView::Right),
    ("Top", StandardView::Top),
    ("Bottom", StandardView::Bottom),
    ("Isometric", StandardView::Iso),
];

pub fn bar(ui: &mut egui::Ui, o: &mut Opened, recent: &[PathBuf]) -> Option<Action> {
    let mut action = None;
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.button("Open…").clicked() {
                action = Some(Action::Open(None));
            }
            ui.menu_button("Open recent", |ui| {
                for p in recent.iter().filter(|p| **p != o.ws.root) {
                    if ui.button(p.display().to_string()).clicked() {
                        action = Some(Action::Open(Some(p.clone())));
                    }
                }
            });
            if ui.button("Close").clicked() {
                action = Some(Action::Close);
            }
            if ui.button("Quit").clicked() {
                action = Some(Action::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.button("Undo").clicked() {
                o.ws.undo(None);
            }
            if ui.button("Redo").clicked() {
                o.ws.redo(None);
            }
            let selection =
                o.ws.myself()
                    .map(|m| m.selection.clone())
                    .unwrap_or_default();
            if ui
                .add_enabled(!selection.is_empty(), egui::Button::new("Copy reference"))
                .clicked()
            {
                ui.ctx().copy_text(selection.join("\n"));
            }
            if ui
                .add_enabled(!selection.is_empty(), egui::Button::new("Clear selection"))
                .clicked()
            {
                o.ws.select(Vec::new(), SelectMode::Replace);
            }
            ui.separator();
            if ui.button("Preferences…").clicked() {
                o.dialogs.preferences = true;
            }
        });
        ui.menu_button("View", |ui| {
            for (name, v) in VIEWS {
                if ui.button(name).clicked() {
                    o.view.standard(&mut o.ws, v);
                }
            }
            ui.separator();
            if ui.button("Zoom to fit").clicked() {
                o.view.fit(&mut o.ws, None);
            }
            let selection =
                o.ws.myself()
                    .map(|m| m.selection.clone())
                    .unwrap_or_default();
            if ui
                .add_enabled(
                    !selection.is_empty(),
                    egui::Button::new("Zoom to selection"),
                )
                .clicked()
            {
                o.view.fit_entities(&mut o.ws, &selection);
            }
            ui.menu_button("Display quality", |ui| {
                let now = o.ws.settings.text("display.quality", "normal").to_owned();
                for q in ["coarse", "normal", "fine"] {
                    if ui.selectable_label(now == q, q).clicked() && now != q {
                        o.ws.set_config("display.quality", json!(q));
                    }
                }
            });
            let edges = o.view.edges;
            if ui.selectable_label(edges, "Edges").clicked() {
                o.ws.set_config("display.edges", json!(!edges));
            }
        });
        ui.menu_button("Project", |ui| {
            if ui.button("Settings…").clicked() {
                o.dialogs.settings = true;
            }
            if ui.button("Doctor…").clicked() {
                o.dialogs.doctor = true;
                o.ws.diagnose();
            }
        });
    });
    action
}
