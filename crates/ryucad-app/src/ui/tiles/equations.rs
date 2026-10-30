//! The Equations tile: every parameter of the project and of each part,
//! and every expression of each part's features, in one table, filtered
//! by project, part or feature, each edited in place and written where it
//! lives when Enter is pressed (or its Apply clicked); a row adds a
//! parameter to the project or a part.

use crate::core::equations::{Filter, Place};
use crate::core::workspace::Workspace;
use crate::ui::tiles::properties::{field_width, sized};
use eframe::egui;

pub fn equations(ui: &mut egui::Ui, ws: &mut Workspace) {
    ws.show_equations();
    let parts: Vec<String> = ws.model.parts.keys().cloned().collect();
    let Some(e) = &mut ws.equations else {
        ui.spinner();
        return;
    };
    ui.horizontal(|ui| {
        let show = ui.label("Show");
        egui::ComboBox::from_id_salt("equations filter")
            .selected_text(e.filter.label())
            .show_ui(ui, |ui| {
                for f in e.filters() {
                    let label = f.label();
                    ui.selectable_value(&mut e.filter, f, label);
                }
            })
            .response
            .labelled_by(show.id);
    });
    if !e.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed these while you edited them. They now read:");
            for c in &e.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                e.changed.clear();
            }
        });
    }
    let shown = e.shown();
    if shown.is_empty() {
        ui.weak(match e.filter {
            Filter::Project => "The project has no parameters yet",
            _ => "No parameters or expressions here",
        });
    }
    let value_w = field_width(ui, 0.35, 110.0, 280.0);
    let mut write = None;
    let saving = e.saving.is_some();
    egui::Grid::new("equations")
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            for i in shown {
                let row = &mut e.rows[i];
                let place = match &row.place {
                    Place::Parameter { part: None, .. } => "project".to_owned(),
                    Place::Parameter { part: Some(p), .. } => p.clone(),
                    Place::Field { part, feature, .. } => format!("{part}/{feature}"),
                    Place::Dimension { part, sketch, .. } => format!("{part}/{sketch}"),
                };
                ui.weak(place);
                let name = ui.label(row.place.name());
                let edit = egui::TextEdit::singleline(&mut row.field.text);
                let r = sized(ui, edit, value_w).labelled_by(name.id);
                let dirty = row.field.dirty();
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && dirty {
                    write = Some(i);
                }
                if dirty {
                    r.on_hover_text(format!("was {}", row.field.read));
                }
                let shown = match &row.error {
                    Some(_) => ui.colored_label(ui.visuals().error_fg_color, "fails"),
                    None => ui.weak(&row.value),
                };
                match (&row.error, row.reads.is_empty()) {
                    (Some(err), _) => {
                        shown.on_hover_text(err);
                    }
                    (None, false) => {
                        shown.on_hover_text(format!("reads {}", row.reads.join(", ")));
                    }
                    (None, true) => {}
                }
                let apply = ui
                    .add_enabled(dirty && !saving, egui::Button::new("Apply"))
                    .on_hover_text("Write this one where it lives");
                apply.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        dirty && !saving,
                        format!("Apply {}", row.place.label()),
                    )
                });
                if apply.clicked() {
                    write = Some(i);
                }
                ui.end_row();
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        let to = ui.label("Add to");
        let current = e
            .new_part
            .clone()
            .map_or("the project".to_owned(), |p| format!("part {p}"));
        egui::ComboBox::from_id_salt("equations new scope")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut e.new_part, None, "the project");
                for p in &parts {
                    ui.selectable_value(&mut e.new_part, Some(p.clone()), format!("part {p}"));
                }
            })
            .response
            .labelled_by(to.id);
        let name_w = field_width(ui, 0.2, 70.0, 140.0);
        let add = egui::TextEdit::singleline(&mut e.new_name).hint_text("new name");
        sized(ui, add, name_w).widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new equation's name")
        });
        let value = egui::TextEdit::singleline(&mut e.new_value).hint_text("value");
        sized(ui, value, value_w).widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new equation's value")
        });
    });
    let can_add = e.add_request().is_some() && !saving;
    let add = ui
        .add_enabled(can_add, egui::Button::new("Add parameter"))
        .clicked();
    if let Some(err) = &e.error {
        ui.colored_label(ui.visuals().error_fg_color, &err.message);
    }
    if saving {
        ui.spinner();
    }
    if let Some(i) = write {
        ws.save_equation(i);
    } else if add {
        ws.add_equation();
    }
}
