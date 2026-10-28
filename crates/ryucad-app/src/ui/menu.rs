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
    pub interference: bool,
    pub mass: bool,
    pub clearance: bool,
}

/// What a menu asks of the app rather than of the open project.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open a project: this folder, or one the person picks.
    Open(Option<PathBuf>),
    Close,
    Quit,
}

use egui::{Key, KeyboardShortcut, Modifiers};

/// The window's shortcuts, shown in the menus. Command is Ctrl, or Cmd on
/// macOS.
pub const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
/// Redo as Windows has it too.
pub const REDO_Y: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
pub const CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
pub const CLEAR_SELECTION: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Escape);

/// A menu item showing its shortcut.
fn item(ui: &egui::Ui, text: &str, shortcut: &KeyboardShortcut) -> egui::Button<'static> {
    egui::Button::new(text.to_owned()).shortcut_text(ui.ctx().format_shortcut(shortcut))
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
            ui.separator();
            if ui.button("Import STEP…").clicked() {
                o.ws.import = Some(crate::core::step::Import::new());
            }
            let file = o.view.chosen.as_ref().and_then(|(p, f)| {
                o.ws.model
                    .parts
                    .get(p)?
                    .feature(f)?
                    .import
                    .as_ref()
                    .map(|i| i.file.clone())
            });
            if ui
                .add_enabled(file.is_some(), egui::Button::new("Update import…"))
                .on_disabled_hover_text("Choose an imported feature in the tree first")
                .clicked()
                && let Some(f) = &file
            {
                o.ws.import = Some(crate::core::step::Import::update(f));
            }
            let shown = match o.ws.model.shown().as_slice() {
                [one] => Some((*one).to_owned()),
                _ => None,
            };
            if ui
                .add_enabled(shown.is_some(), egui::Button::new("Export STEP…"))
                .on_disabled_hover_text("Show one part or assembly first (View > Show)")
                .clicked()
                && let Some(p) = &shown
            {
                o.ws.export = Some(crate::core::step::Export::new(p));
            }
            ui.separator();
            if ui.add(item(ui, "Close", &CLOSE)).clicked() {
                action = Some(Action::Close);
            }
            if ui.button("Quit").clicked() {
                action = Some(Action::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.add(item(ui, "Undo", &UNDO)).clicked() {
                o.ws.undo(None);
            }
            if ui.add(item(ui, "Redo", &REDO)).clicked() {
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
                .add_enabled(
                    !selection.is_empty() || o.view.chosen.is_some(),
                    item(ui, "Clear selection", &CLEAR_SELECTION),
                )
                .clicked()
            {
                o.ws.select(Vec::new(), SelectMode::Replace);
                o.view.unchoose(&mut o.ws);
            }
            ui.separator();
            if ui.button("Preferences…").clicked() {
                o.dialogs.preferences = true;
                o.ws.read_cache();
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
            let sketches = o.view.sketches;
            if ui.selectable_label(sketches, "Sketches").clicked() {
                o.ws.set_config("display.sketches", json!(!sketches));
            }
            ui.menu_button("Show", |ui| {
                let focus = o.ws.model.focus.clone();
                if ui
                    .selectable_label(focus.is_none(), "The project's model")
                    .clicked()
                {
                    o.ws.set_focus(None);
                }
                let parts: Vec<(String, bool)> =
                    o.ws.model
                        .parts
                        .values()
                        .map(|p| (p.id.clone(), p.assembly))
                        .collect();
                for (id, assembly) in parts {
                    let label = if assembly {
                        format!("{id} (assembly)")
                    } else {
                        id.clone()
                    };
                    if ui
                        .selectable_label(focus.as_deref() == Some(&id), label)
                        .clicked()
                    {
                        o.ws.set_focus(Some(id));
                    }
                }
            });
            ui.separator();
            let rev = o.ws.compare.as_ref().map(|c| c.rev.clone());
            if ui
                .add_enabled(
                    rev.as_deref() != Some("HEAD"),
                    egui::Button::new("Compare with HEAD"),
                )
                .clicked()
            {
                o.ws.set_compare(Some("HEAD".to_owned()));
            }
            if ui.button("Compare with…").clicked() {
                o.compare_with = Some(String::new());
            }
            if ui
                .add_enabled(rev.is_some(), egui::Button::new("Stop comparing"))
                .clicked()
            {
                o.ws.set_compare(None);
            }
        });
        ui.menu_button("Tools", |ui| {
            let mut on = o.ws.measure.on;
            if ui.checkbox(&mut on, "Measure").clicked() {
                o.ws.measure.set(on);
            }
            let target = o.ws.mass_target();
            if ui
                .add_enabled(target.is_some(), egui::Button::new("Mass properties"))
                .on_disabled_hover_text("Show one part or assembly, or select a solid")
                .clicked()
                && let Some(of) = target
            {
                o.ws.weigh(&of);
                o.dialogs.mass = true;
            }
            ui.separator();
            let shown = o.ws.shown_assembly().is_some();
            if ui
                .add_enabled(shown, egui::Button::new("Clearance"))
                .on_disabled_hover_text("Show an assembly first (View > Show)")
                .clicked()
            {
                o.ws.check_clearance();
                o.dialogs.clearance = true;
            }
            if ui
                .add_enabled(shown, egui::Button::new("Check interference"))
                .on_disabled_hover_text("Show an assembly first (View > Show)")
                .clicked()
            {
                o.ws.check_interference();
                o.dialogs.interference = true;
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
