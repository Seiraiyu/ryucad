//! The menu bar, built from the command registry (`core::command`): its
//! categories in a fixed order, each listing its commands in registry
//! order with their effective keys. egui draws it in the window on every
//! OS, macOS included.

use crate::core::command::{self, Category, Kind};
use crate::ui::app::Opened;
use crate::ui::commands;
use eframe::egui;
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

/// The menu bar.
pub fn bar(ui: &mut egui::Ui, o: &mut Opened) -> Option<Action> {
    let mut action = None;
    let shown = commands::shown(o, ui.ctx());
    egui::MenuBar::new().ui(ui, |ui| {
        for cat in Category::ALL {
            if command::menu(cat).next().is_none() {
                continue;
            }
            ui.menu_button(cat.title(), |ui| {
                if let Some(a) = items(ui, o, &shown, cat) {
                    action = Some(a);
                }
            });
        }
    });
    action
}

/// A category's items. A separator comes before each run of another
/// kind of command: the File menu's import and close groups, the View
/// menu's toggles and compares.
fn items(
    ui: &mut egui::Ui,
    o: &mut Opened,
    shown: &commands::Shown,
    cat: Category,
) -> Option<Action> {
    let mut action = None;
    let mut group = None;
    for c in command::menu(cat) {
        let g = group_of(c.id);
        if group.is_some_and(|was| was != g) {
            ui.separator();
        }
        group = Some(g);
        let enabled = command::enabled(c.id, &shown.state);
        match c.kind {
            Kind::Choices => {
                ui.menu_button(c.title, |ui| {
                    let list = commands::choices(o, c.id);
                    if list.is_empty() {
                        ui.weak("None yet");
                    }
                    for ch in list {
                        let r =
                            ui.add_enabled(ch.enabled, egui::Button::selectable(ch.on, &ch.label));
                        if r.clicked() {
                            action = commands::choose(o, c.id, &ch.key);
                            ui.close();
                        }
                    }
                });
            }
            Kind::Toggle | Kind::Action => {
                let on = command::checked(c.id, &shown.state);
                let mut b = match on {
                    Some(on) => egui::Button::selectable(on, c.title),
                    None => egui::Button::new(c.title),
                };
                if let Some(k) = shown.keys.get(c.id) {
                    b = b.shortcut_text(k.as_str());
                }
                let r = ui.add_enabled(enabled.is_ok(), b);
                let r = match enabled {
                    Err(why) => r.on_disabled_hover_text(why),
                    Ok(()) => r,
                };
                if r.clicked() {
                    action = commands::run(o, ui.ctx(), c.id, None);
                    ui.close();
                }
            }
        }
    }
    action
}

/// Which separated group of its menu a command is in.
fn group_of(id: &str) -> u8 {
    match id {
        "file.import_step" | "file.update_import" | "file.export_step" => 1,
        "file.close" | "file.quit" => 2,
        "edit.preferences" => 1,
        "view.fit" | "view.fit_selection" | "view.quality" | "view.edges" | "view.sketches"
        | "view.nav_cube" | "view.triad" | "view.origin" | "view.show" | "view.show_all" => 1,
        "view.compare_head" | "view.compare_with" | "view.compare_stop" => 2,
        "view.panels" | "view.reset_layout" => 3,
        "measure.clearance" | "tools.interference" => 1,
        "palette.open" => 2,
        _ => 0,
    }
}
