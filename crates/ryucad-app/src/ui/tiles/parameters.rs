//! The Parameters tile: the part's parameters, editable, drawn with the
//! form toolkit (`ui::form`).

use crate::core::workspace::Workspace;
use crate::ui::form::{self, Banner};
use crate::ui::icon::Icon;
use crate::ui::view::View;
use eframe::egui;

/// The parameters of the chosen feature's part (or the first part), each
/// editable, with its value beside it and what reads it on hover, and a
/// row to add one. Apply (or Enter) sends `param.set` against the revision
/// they were read at; Revert (or Esc) puts back what was read.
pub fn parameters(ui: &mut egui::Ui, ws: &mut Workspace, view: &View) {
    let part = view
        .chosen
        .as_ref()
        .map(|k| k.0.clone())
        .or_else(|| ws.model.parts.keys().next().cloned());
    ws.show_params(part.as_deref());
    let Some(p) = &mut ws.params else {
        return;
    };
    form::heading(
        ui,
        Some(Icon::FUNCTION),
        &format!("Parameters of {}", p.part),
        p.dirty(),
    );
    match form::banner(ui, &format!("{}'s parameters", p.part), &p.changed) {
        Some(Banner::Reload) => p.changed.clear(),
        Some(Banner::KeepMine) => {
            for c in std::mem::take(&mut p.changed) {
                if let Some(row) = p.params.iter_mut().find(|r| r.field.name == c.name) {
                    row.field.text = c.yours;
                }
            }
        }
        None => {}
    }
    if let Some(e) = &p.error {
        form::error(ui, e);
    }
    if p.params.is_empty() {
        ui.weak("No parameters yet: add one below, or `ryucad param set`.");
    }
    let keys = form::grid(ui, "parameters", |rows| {
        for row in &mut p.params {
            // The value beside what was written, when it isn't the same.
            row.field.value =
                (!row.value.is_empty() && row.value != row.field.read).then(|| row.value.clone());
            let name = row.field.name.clone();
            let r = rows.quantity(&name, &mut row.field);
            if !row.used_by.is_empty() {
                r.on_hover_text(format!("used by {}", row.used_by.join(", ")));
            }
            if let Some(e) = &row.error {
                rows.note(&format!("fails: {e}"));
            }
        }
        let entered = rows.custom("new", |ui| {
            ui.horizontal(|ui| {
                let width = (ui.available_width() / 2.0 - 6.0).max(60.0);
                let height = ui.spacing().interact_size.y;
                let add = egui::TextEdit::singleline(&mut p.new_name).hint_text("name");
                let named = ui
                    .add_sized([width, height], add)
                    .on_hover_text("A new parameter's name");
                named.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's name")
                });
                let value = egui::TextEdit::singleline(&mut p.new_value)
                    .hint_text("value")
                    .font(egui::TextStyle::Monospace);
                let valued = ui
                    .add_sized([width, height], value)
                    .on_hover_text("A new parameter's expression");
                valued.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's value")
                });
                valued.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            })
            .inner
        });
        // Enter in the new value adds it.
        rows.keys.apply |= entered;
    });
    let (dirty, saving) = (p.dirty(), p.saving);
    let foot = form::foot(ui, dirty, saving, ("Apply parameters", "Revert parameters"));
    if (foot.revert || keys.revert) && dirty {
        for row in &mut p.params {
            row.field.text.clone_from(&row.field.read);
        }
        p.new_name.clear();
        p.new_value.clear();
        p.error = None;
    }
    if (foot.apply || keys.apply) && dirty && !saving {
        ws.save_params();
    }
}
