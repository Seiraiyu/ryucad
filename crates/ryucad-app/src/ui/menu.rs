//! The menu bar. egui draws it in the window on every OS, macOS included.

use crate::ui::app::Opened;
use eframe::egui;

/// The dialogs a menu opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dialogs {
    pub settings: bool,
    pub preferences: bool,
    pub doctor: bool,
}

pub fn bar(ui: &mut egui::Ui, o: &mut Opened) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Edit", |ui| {
            if ui.button("Preferences…").clicked() {
                o.dialogs.preferences = true;
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
}
