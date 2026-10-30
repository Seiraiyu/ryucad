//! The Section tile (`section.edit`): the planes this window cuts with,
//! each with its offset from the model's origin and its normal's
//! azimuth and elevation, to type or drag; planes added from the origin's
//! or a selected face's; and the viewport's handles turned on or off.
//! A value dragged moves the plane at once and is written when let go; a
//! value typed is written at once.

use crate::core::workspace::Workspace;
use crate::ui::view::View;
use eframe::egui;
use ryucad::section::{MAX_PLANES, Plane};

/// Draws the tile.
pub fn section(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let unit =
        ryucad::units::Unit::parse(&ws.model.default_unit).unwrap_or(ryucad::units::Unit::Mm);
    let factor = unit.factor();
    let full = ws.sections.planes.len() >= MAX_PLANES;
    ui.horizontal_wrapped(|ui| {
        ui.label("Add a plane:");
        for (name, title) in [("xy", "XY"), ("yz", "YZ"), ("zx", "ZX")] {
            let b = ui.add_enabled(!full, egui::Button::new(title));
            if b.on_hover_text(format!("Cut along the origin's {title} plane"))
                .clicked()
            {
                ws.add_section(name);
            }
        }
        let face = view.selected_face(ws);
        let b = ui.add_enabled(!full && face.is_some(), egui::Button::new("Selected face"));
        let b = match face.is_some() {
            true => b.on_hover_text("Cut along the selected planar face"),
            false => b.on_disabled_hover_text("Select a planar face first."),
        };
        if b.clicked()
            && let Some(f) = face
        {
            ws.add_section(&f);
        }
    });
    if ws.sections.planes.is_empty() {
        ui.weak("No section planes: the model is drawn whole.");
        return;
    }
    let mut planes = ws.sections.planes.clone();
    let (mut write, mut preview) = (false, false);
    let mut remove = None;
    egui::Grid::new("sections").num_columns(6).show(ui, |ui| {
        ui.label("");
        ui.label(format!("Offset ({})", unit.as_str()));
        ui.label("Azimuth (°)");
        ui.label("Elevation (°)");
        ui.label("");
        ui.label("");
        ui.end_row();
        for (k, p) in planes.iter_mut().enumerate() {
            let chosen = ws.sections.active == k;
            if ui
                .selectable_label(chosen, format!("Plane {}", k + 1))
                .on_hover_text("The plane the viewport's handles move")
                .clicked()
            {
                ws.sections.active = k;
            }
            let mut offset = p.offset() / factor;
            let r = ui.add(egui::DragValue::new(&mut offset).speed(0.1).max_decimals(4));
            r.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::SpinButton,
                    true,
                    format!("plane {} offset", k + 1),
                )
            });
            if r.changed() {
                *p = p.with_offset(offset * factor);
            }
            let (mut az, mut el) = p.angles();
            let ra = ui.add(egui::DragValue::new(&mut az).speed(0.5).max_decimals(3));
            ra.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::SpinButton,
                    true,
                    format!("plane {} azimuth", k + 1),
                )
            });
            let re = ui.add(
                egui::DragValue::new(&mut el)
                    .speed(0.5)
                    .range(-90.0..=90.0)
                    .max_decimals(3),
            );
            re.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::SpinButton,
                    true,
                    format!("plane {} elevation", k + 1),
                )
            });
            if ra.changed() || re.changed() {
                *p = p.with_angles(az, el);
            }
            for r in [&r, &ra, &re] {
                // Dragged: moved here at once, written when let go.
                if r.changed() && r.dragged() {
                    preview = true;
                } else if r.changed() || r.drag_stopped() {
                    write = true;
                }
            }
            if ui
                .button("Flip")
                .on_hover_text("Cut away the other side")
                .clicked()
            {
                *p = p.flipped();
                write = true;
            }
            if ui.button("Remove").clicked() {
                remove = Some(k);
            }
            ui.end_row();
        }
    });
    if let Some(k) = remove {
        planes.remove(k);
        write = true;
    }
    if write {
        ws.set_sections(planes);
    } else if preview {
        ws.sections.planes = planes;
        ws.model.generation += 1;
    }
    ui.horizontal(|ui| {
        ui.checkbox(&mut ws.sections.handles, "Handles in the viewport")
            .on_hover_text("Drag the arrow to move the plane, a ring to turn it");
        if ui.button("Remove all").clicked() {
            ws.set_sections(Vec::<Plane>::new());
        }
    });
}
