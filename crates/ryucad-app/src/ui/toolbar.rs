//! The toolbar (design 199, Command registry): one row under the menu
//! bar of icon buttons for the commands marked `toolbar`, in registry
//! order, with the standard views behind one button, and the People strip
//! at its right end. Every button has its command's title as its name and
//! tooltip, with its keys, and says why when it is disabled.

use crate::core::command::{self, STANDARD_VIEWS};
use crate::ui::app::Opened;
use crate::ui::commands;
use crate::ui::icon::{self, Icon};
use crate::ui::menu::Action;
use eframe::egui;

/// The toolbar's icon size, at `ui.scale` 1.0.
pub const ICON: f32 = 20.0;

/// The icon a command names.
pub fn icon_of(name: &str) -> Option<Icon> {
    Icon::all().find(|i| i.name() == name)
}

/// The tooltip of a command: its title and its keys.
fn tip(shown: &commands::Shown, id: &str, title: &str) -> String {
    match shown.keys.get(id) {
        Some(k) => format!("{title} ({k})"),
        None => title.to_owned(),
    }
}

/// Where a separator goes: before the first command of each group.
fn starts_group(id: &str) -> bool {
    matches!(
        id,
        "view.fit" | "select.faces" | "view.compare_head" | "palette.open"
    )
}

/// The toolbar.
pub fn bar(ui: &mut egui::Ui, o: &mut Opened) -> Option<Action> {
    let mut action = None;
    let shown = o.view.commands.clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for c in command::toolbar() {
            if starts_group(c.id) {
                ui.separator();
            }
            // The standard views after redo, behind one button.
            if c.id == "view.fit" {
                views(ui, o, &shown);
            }
            let Some(icon) = c.icon.and_then(icon_of) else {
                continue;
            };
            let on = command::checked(c.id, &shown.state).unwrap_or(false);
            let r = icon::button(
                ui,
                icon,
                ICON,
                c.title,
                &tip(&shown, c.id, c.title),
                on,
                command::enabled(c.id, &shown.state),
            );
            if r.clicked() {
                action = commands::run(o, ui.ctx(), c.id, None);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::ui::panels::people(ui, &mut o.ws);
        });
    });
    action
}

/// The standard views button: a menu of the nine views, each with its
/// keys.
fn views(ui: &mut egui::Ui, o: &mut Opened, shown: &commands::Shown) {
    let r = icon::button(
        ui,
        Icon::CUBE,
        ICON,
        "Standard views",
        "Standard views",
        false,
        Ok(()),
    );
    egui::Popup::menu(&r).show(|ui| {
        for id in STANDARD_VIEWS {
            let Some(c) = command::get(id) else { continue };
            let mut b = egui::Button::new(c.title);
            if let Some(k) = shown.keys.get(id) {
                b = b.shortcut_text(k.as_str());
            }
            if ui.add(b).clicked() {
                commands::run(o, ui.ctx(), id, None);
                ui.close();
            }
        }
    });
}
