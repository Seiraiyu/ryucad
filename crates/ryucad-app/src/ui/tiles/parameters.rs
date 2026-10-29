//! The Parameters tile: the part's parameters, editable.

use crate::core::workspace::Workspace;
use crate::ui::tiles::properties::{field_width, sized};
use crate::ui::view::View;
use eframe::egui;

/// The parameters of the chosen feature's part (or the first part), each
/// editable, with a row to add one. Apply sends `param.set` against the
/// revision they were read at.
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
    ui.heading(format!("Parameters of {}", p.part));
    if !p.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed these parameters while you edited them. They now read:");
            for c in &p.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                p.changed.clear();
            }
        });
    }
    if p.params.is_empty() {
        ui.weak("No parameters yet");
    }
    let (name_w, value_w) = (
        field_width(ui, 0.3, 70.0, 160.0),
        field_width(ui, 0.45, 110.0, 280.0),
    );
    egui::Grid::new("parameters").num_columns(3).show(ui, |ui| {
        for row in &mut p.params {
            let name = ui.label(&row.field.name);
            let edit = egui::TextEdit::singleline(&mut row.field.text);
            let r = sized(ui, edit, value_w).labelled_by(name.id);
            if row.field.dirty() {
                r.on_hover_text(format!("was {}", row.field.read));
            }
            let shown = match &row.error {
                Some(_) => ui.colored_label(ui.visuals().error_fg_color, "fails"),
                None => ui.weak(&row.value),
            };
            match (&row.error, row.used_by.is_empty()) {
                (Some(e), _) => {
                    shown.on_hover_text(e);
                }
                (None, false) => {
                    shown.on_hover_text(format!("used by {}", row.used_by.join(", ")));
                }
                (None, true) => {}
            }
            ui.end_row();
        }
        let add = egui::TextEdit::singleline(&mut p.new_name).hint_text("new name");
        let named = sized(ui, add, name_w).on_hover_text("A new parameter's name");
        named.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's name")
        });
        let value = egui::TextEdit::singleline(&mut p.new_value).hint_text("value");
        let valued = sized(ui, value, value_w).on_hover_text("A new parameter's expression");
        valued.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new parameter's value")
        });
        ui.end_row();
    });
    if let Some(e) = &p.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let (dirty, saving) = (p.dirty(), p.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply parameters"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert parameters"))
            .clicked()
        {
            for row in &mut p.params {
                row.field.text.clone_from(&row.field.read);
            }
            p.new_name.clear();
            p.new_value.clear();
            p.error = None;
        }
        if saving {
            ui.spinner();
        }
    });
    if save {
        ws.save_params();
    }
}
