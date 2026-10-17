//! The menu bar. egui draws it in the window on every OS, macOS included.

use crate::ui::app::App;
use eframe::egui;

/// The dialogs a menu opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dialogs {
    pub settings: bool,
    pub preferences: bool,
}

pub fn bar(ui: &mut egui::Ui, app: &mut App) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Edit", |ui| {
            if ui.button("Preferences…").clicked() {
                app.dialogs.preferences = true;
            }
        });
        ui.menu_button("Project", |ui| {
            if ui.button("Settings…").clicked() {
                app.dialogs.settings = true;
            }
        });
    });
}
