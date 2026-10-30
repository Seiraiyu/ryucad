//! The Properties tile: the chosen feature's form.

use crate::core::workspace::Workspace;
use eframe::egui;

/// A row choosing one of a list, by name.
pub(crate) fn choice_row(ui: &mut egui::Ui, c: &mut crate::core::edit::Choice) {
    ui.label(&c.name);
    let r = egui::ComboBox::from_id_salt(("choice", &c.name))
        .selected_text(&c.chosen)
        .show_ui(ui, |ui| {
            for o in &c.options {
                ui.selectable_value(&mut c.chosen, o.clone(), o);
            }
        });
    let r = r.response;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &c.name));
    if c.dirty() {
        r.on_hover_text(format!("was {}", c.read));
    }
    ui.end_row();
}

/// Some of a list, each a checkbox, in the order picked.
pub(crate) fn picks(ui: &mut egui::Ui, p: &mut crate::core::edit::Picks) {
    ui.label(format!("{}: {}", p.name, p.chosen.join(", ")));
    ui.horizontal_wrapped(|ui| {
        for o in p.options.clone() {
            let mut on = p.chosen.contains(&o);
            if ui.checkbox(&mut on, &o).changed() {
                p.toggle(&o);
            }
        }
    });
}

/// The last eight hex digits of a stable name or reference, after what it
/// is: enough to tell a feature's edges apart.
pub(crate) fn short(kind: &str, r: &str) -> String {
    format!("{kind} {}", &r[r.len().saturating_sub(8)..])
}

/// A fillet's or chamfer's groups: each one's form, sizes and face, and
/// its edges, each a checkbox to take it out (adding one is an agent's
/// work); then whether its edges grow along tangent chains.
pub(crate) fn blend(ui: &mut egui::Ui, b: &mut crate::core::edit::BlendForm) {
    let many = b.groups.len() > 1;
    for (i, g) in b.groups.iter_mut().enumerate() {
        if many {
            ui.label(egui::RichText::new(format!("group {}", i + 1)).strong());
        }
        egui::Grid::new(("blend group", i))
            .num_columns(2)
            .show(ui, |ui| {
                if let Some(form) = &g.form {
                    ui.label("form");
                    ui.label(form);
                    ui.end_row();
                }
                for (_, f) in &mut g.sizes {
                    let name = f.name.clone();
                    field_row(ui, f, &name);
                }
                if let Some(face) = &g.face {
                    ui.label("face");
                    ui.monospace(short("face", face)).on_hover_text(face);
                    ui.end_row();
                }
            });
        let e = &mut g.edges;
        ui.label(format!(
            "{}: {} of {}",
            e.name,
            e.chosen.len(),
            e.options.len()
        ));
        ui.horizontal_wrapped(|ui| {
            for o in e.options.clone() {
                let mut on = e.chosen.contains(&o);
                if ui
                    .checkbox(&mut on, short("edge", &o))
                    .on_hover_text(&o)
                    .changed()
                {
                    e.toggle(&o);
                }
            }
        });
    }
    ui.checkbox(&mut b.propagate.chosen, "propagate along tangent chains");
}

/// A hole's form as read, its sizes, its depth (blind with its length,
/// through all, or up to its target, shown), and its points, each a
/// checkbox to take it out (adding one is an agent's work).
pub(crate) fn hole(ui: &mut egui::Ui, h: &mut crate::core::edit::HoleForm) {
    egui::Grid::new("hole").num_columns(2).show(ui, |ui| {
        ui.label("form");
        ui.label(&h.form);
        ui.end_row();
        for (_, f) in &mut h.sizes {
            let name = f.name.clone();
            field_row(ui, f, &name);
        }
        choice_row(ui, &mut h.depth);
        match h.depth.chosen.as_str() {
            "blind" => field_row(ui, &mut h.length, "depth"),
            "up_to" => {
                ui.label("up to");
                let t = h.target.clone().unwrap_or_default();
                ui.monospace(short("face", &t)).on_hover_text(&t);
                ui.end_row();
            }
            _ => {}
        }
    });
    let p = &mut h.points;
    if p.chosen.is_empty() {
        ui.label("points: every point no curve uses");
    } else {
        ui.label(format!("{}: {}", p.name, p.chosen.join(", ")));
    }
    ui.horizontal_wrapped(|ui| {
        for o in p.options.clone() {
            let mut on = p.chosen.contains(&o);
            if ui.checkbox(&mut on, &o).changed() {
                p.toggle(&o);
            }
        }
    });
}

/// A pattern's form, directions, axis or plane as read, its spacings,
/// counts and angle, whether it runs against its directions, its skips
/// as typed, and its seeds, each a checkbox to take it out.
pub(crate) fn pattern(ui: &mut egui::Ui, p: &mut crate::core::edit::PatternForm) {
    egui::Grid::new("pattern").num_columns(2).show(ui, |ui| {
        ui.label("form");
        ui.label(&p.form);
        ui.end_row();
        for (k, r) in &p.refs {
            ui.label(k);
            if r.contains('/') {
                ui.monospace(short("entity", r)).on_hover_text(r);
            } else {
                ui.label(r);
            }
            ui.end_row();
        }
        for (_, f) in &mut p.sizes {
            let name = f.name.clone();
            field_row(ui, f, &name);
        }
        if p.form != "mirror" {
            field_row(ui, &mut p.skip, "skip");
        }
    });
    for (k, f) in &mut p.reverse {
        let label = match k.as_str() {
            "reverse" => "against the direction",
            _ => "against the second direction",
        };
        ui.checkbox(&mut f.chosen, label);
    }
    picks(ui, &mut p.seeds);
}

/// A connector as one short line: its instance path, then its entity's
/// last name digits or `origin`; the whole text on hover.
pub(crate) fn connector_text(c: &str) -> String {
    use crate::core::model::MateInfo;
    let path = MateInfo::path_of(c);
    match MateInfo::face_of(c) {
        Some((_, name)) => format!("{path} : {}", &name[name.len() - 8..]),
        None if c.split('/').next_back().is_some_and(|n| n.len() == 32) => {
            format!("{path} : {}", &c[c.len() - 8..])
        }
        None => format!("{path} : origin"),
    }
}

/// A mate's form, its connectors as read (changing them is an agent's
/// work), the angle and distance its form may drive, and each
/// connector's offset.
pub(crate) fn mate(ui: &mut egui::Ui, m: &mut crate::core::edit::MateForm) {
    egui::Grid::new("mate").num_columns(2).show(ui, |ui| {
        choice_row(ui, &mut m.form);
        for (side, c) in &m.connectors {
            ui.label(side);
            ui.monospace(connector_text(c)).on_hover_text(c);
            ui.end_row();
        }
        let (angle, distance) = m.takes();
        if angle {
            field_row(ui, &mut m.angle, "angle (empty: free)");
        }
        if distance {
            field_row(ui, &mut m.distance, "distance (empty: free)");
        }
    });
    for (side, o) in [("a", &mut m.a), ("b", &mut m.b)] {
        ui.collapsing(format!("Offset of {side}"), |ui| {
            egui::Grid::new(("offset", side))
                .num_columns(2)
                .show(ui, |ui| {
                    for (k, f) in ["x", "y", "z"].iter().zip(o.origin.iter_mut()) {
                        field_row(ui, f, &format!("along {k}"));
                    }
                    field_row(ui, &mut o.rotate, "turn about z");
                });
            ui.checkbox(&mut o.flip.chosen, "turned over");
        });
    }
}

/// The width of a text field in a grid's column: a grid gives a column
/// other than its last only the width it had last frame, at least 40
/// points, so a field there asking for more never gets it (`add_sized`
/// takes it). A share of the panel, within limits.
pub(crate) fn field_width(ui: &egui::Ui, share: f32, min: f32, max: f32) -> f32 {
    (ui.available_width() * share).clamp(min, max)
}

/// Adds a single line text field `width` wide, in a grid cell or not.
pub(crate) fn sized(ui: &mut egui::Ui, edit: egui::TextEdit<'_>, width: f32) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    ui.add_sized([width, height], edit)
}

pub(crate) fn field_row(ui: &mut egui::Ui, f: &mut crate::core::edit::Field, label: &str) {
    ui.label(label);
    let edit = egui::TextEdit::singleline(&mut f.text).desired_width(110.0);
    let r = ui.add(edit);
    if f.dirty() {
        r.on_hover_text(format!("was {}", f.read));
    }
    ui.end_row();
}

/// A sketch dimension's value, edited in place. Apply sends `sketch.edit`
/// against the revision it was read at.
pub(crate) fn dimension(ui: &mut egui::Ui, ws: &mut Workspace) {
    let Some(form) = &mut ws.dimension else {
        return;
    };
    ui.heading(&form.constraint);
    ui.label(format!(
        "{} in sketch {} of part {}",
        form.ty, form.sketch, form.part
    ));
    if let Some(c) = &form.changed {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        let mut ok = false;
        frame.show(ui, |ui| {
            ui.label("Someone changed this dimension while you edited it. It now reads:");
            ui.label(format!("{} (you typed {})", c.theirs, c.yours));
            ok = ui.button("OK").clicked();
        });
        if ok {
            form.changed = None;
        }
    }
    ui.separator();
    egui::Grid::new("dimension").num_columns(2).show(ui, |ui| {
        field_row(ui, &mut form.value, "value");
    });
    if let Some(e) = &form.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let (dirty, saving) = (form.dirty(), form.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert"))
            .clicked()
        {
            form.value.text.clone_from(&form.value.read);
            form.error = None;
        }
        if saving {
            ui.spinner();
        }
    });
    if save {
        ws.save_dimension();
    }
}

/// The chosen feature's parameters and placement, edited in place. Apply
/// sends `feature.edit` against the revision the form was read at. A
/// dimension chosen by its label shows instead.
pub fn properties(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ws.dimension.is_some() {
        dimension(ui, ws);
        return;
    }
    let Some(form) = &mut ws.form else {
        ui.weak("Choose a feature in the tree to see and edit it");
        return;
    };
    ui.heading(&form.feature);
    ui.label(format!("{} in part {}", form.ty, form.part));
    if !form.changed.is_empty() {
        let frame =
            egui::Frame::group(ui.style()).fill(ui.visuals().warn_fg_color.gamma_multiply(0.15));
        frame.show(ui, |ui| {
            ui.label("Someone changed this feature while you edited it. It now reads:");
            for c in &form.changed {
                ui.label(format!("{}: {} (you typed {})", c.name, c.theirs, c.yours));
            }
            if ui.button("OK").clicked() {
                form.changed.clear();
            }
        });
    }
    ui.separator();
    egui::Grid::new("params").num_columns(2).show(ui, |ui| {
        for f in &mut form.params {
            let name = f.name.clone();
            field_row(ui, f, &name);
        }
        if let Some(e) = &mut form.extent {
            choice_row(ui, &mut e.ty);
            if matches!(e.ty.chosen.as_str(), "blind" | "symmetric") {
                field_row(ui, &mut e.length, "length");
            }
            if e.ty.chosen == "through_all" {
                ui.label("");
                ui.checkbox(&mut e.both.chosen, "both ways");
                ui.end_row();
            }
        }
        for c in [
            &mut form.operation,
            &mut form.op,
            &mut form.target,
            &mut form.rev,
        ]
        .into_iter()
        .flatten()
        {
            choice_row(ui, c);
        }
        if let Some(f) = &mut form.fixed {
            ui.label("");
            ui.checkbox(&mut f.chosen, "fixed where it is");
            ui.end_row();
        }
    });
    if let Some(b) = &mut form.blend {
        blend(ui, b);
    }
    if let Some(h) = &mut form.hole {
        hole(ui, h);
    }
    if let Some(p) = &mut form.pattern {
        pattern(ui, p);
    }
    if let Some(m) = &mut form.mate {
        mate(ui, m);
    }
    if let Some(i) = &form.import {
        let built = ws
            .model
            .parts
            .get(&form.part)
            .and_then(|p| p.feature(&form.feature))
            .and_then(|f| f.import.clone());
        import(ui, i, built.as_ref());
        let key = (form.part.clone(), form.feature.clone());
        names(ui, ws, &key);
        return;
    }
    // What it acts on: none for a new solid.
    let new = form.operation.as_ref().is_some_and(|o| o.chosen == "new");
    for p in [&mut form.targets, &mut form.tools].into_iter().flatten() {
        if !new {
            picks(ui, p);
        }
    }
    if form.placed() {
        ui.collapsing("Placement", |ui| {
            egui::Grid::new("placement").num_columns(2).show(ui, |ui| {
                for (axis, fields) in [
                    ("origin", &mut form.origin),
                    ("z axis", &mut form.z),
                    ("x axis", &mut form.x),
                ] {
                    for (k, f) in ["x", "y", "z"].iter().zip(fields.iter_mut()) {
                        field_row(ui, f, &format!("{axis} {k}"));
                    }
                }
            });
        });
    }
    if let Some(e) = &form.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let (dirty, saving) = (form.dirty(), form.saving);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(dirty && !saving, egui::Button::new("Apply"))
            .clicked();
        if ui
            .add_enabled(dirty && !saving, egui::Button::new("Revert"))
            .clicked()
        {
            form.revert();
        }
        if saving {
            ui.spinner();
        }
    });
    let key = (form.part.clone(), form.feature.clone());
    if save {
        ws.save_form();
    }
    names(ui, ws, &key);
}

/// An import, read only: its file, the hash its bytes must have, its
/// product and body, and when it built what healing did to it.
pub(crate) fn import(
    ui: &mut egui::Ui,
    i: &crate::core::model::ImportInfo,
    built: Option<&crate::core::model::ImportInfo>,
) {
    egui::Grid::new("import").num_columns(2).show(ui, |ui| {
        ui.label("file");
        ui.label(&i.file);
        ui.end_row();
        ui.label("BLAKE3");
        ui.label(format!("{}…", &i.blake3[..i.blake3.len().min(16)]))
            .on_hover_text(&i.blake3);
        ui.end_row();
        ui.label("product");
        ui.label(match &i.product {
            Some((id, name, 0)) => format!("{id} ({name})"),
            Some((id, name, n)) => format!("{id} ({name}), ordinal {n}"),
            None => "none: a body no product reaches".to_owned(),
        });
        ui.end_row();
        ui.label("body");
        ui.label(i.body.to_string());
        ui.end_row();
        if let Some(b) = built {
            for (k, n) in b.heal.iter().filter(|(_, n)| *n > 0) {
                ui.label(k);
                ui.label(n.to_string());
                ui.end_row();
            }
            if let Some(t) = b.tolerance_text() {
                ui.label("largest tolerance");
                ui.label(t);
                ui.end_row();
            }
        }
    });
    ui.weak("Imports are written by ryucad import; File > Update import… takes a new version of the file.");
}

/// The stable names of the faces, edges and vertices of a feature's
/// solid as the model shows it, to copy.
pub(crate) fn names(ui: &mut egui::Ui, ws: &Workspace, key: &(String, String)) {
    let Some(key) = ws.model.solid_of(key) else {
        return;
    };
    let key = &key;
    let Some(mesh) = ws.model.meshes.get(key).and_then(|m| m.mesh.as_ref().ok()) else {
        return;
    };
    ui.collapsing("Stable names", |ui| {
        let reference = |n: &[u8; 16]| format!("{}/{}/{}", key.0, key.1, ryucad::mesh::name_hex(n));
        let mut rows: Vec<(&str, String)> = Vec::new();
        rows.extend(mesh.faces.iter().map(|f| ("face", reference(&f.name))));
        rows.extend(mesh.edges.iter().map(|(n, _)| ("edge", reference(n))));
        rows.extend(mesh.vertices.iter().map(|(n, _)| ("vertex", reference(n))));
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for (kind, r) in rows {
                    ui.horizontal(|ui| {
                        ui.label(kind);
                        ui.monospace(&r[r.len() - 32..]);
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(r.clone());
                        }
                    });
                }
            });
    });
}
