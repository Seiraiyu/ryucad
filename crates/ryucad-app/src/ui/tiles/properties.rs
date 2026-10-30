//! The Properties tile: the chosen feature's form, or a sketch dimension's,
//! drawn with the form toolkit (`ui::form`).

use crate::core::edit::Picks;
use crate::core::workspace::Workspace;
use crate::ui::form::{self, Banner, Keys, Rows};
use crate::ui::icon::Icon;
use eframe::egui;

/// Some of a list, each a checkbox, in the order picked.
pub(crate) fn picks(ui: &mut egui::Ui, p: &mut Picks) {
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
pub(crate) fn blend(ui: &mut egui::Ui, b: &mut crate::core::edit::BlendForm) -> Keys {
    let mut keys = Keys::default();
    let many = b.groups.len() > 1;
    for (i, g) in b.groups.iter_mut().enumerate() {
        if many {
            ui.label(egui::RichText::new(format!("group {}", i + 1)).strong());
        }
        let k = form::grid(ui, ("blend group", i), |rows| {
            if let Some(f) = &g.form {
                rows.read_only("form", f, false, None);
            }
            for (_, f) in &mut g.sizes {
                let name = f.name.clone();
                rows.quantity(&name, f);
            }
            if let Some(face) = &g.face {
                rows.read_only("face", &short("face", face), true, Some(face));
            }
        });
        keys.apply |= k.apply;
        keys.revert |= k.revert;
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
    keys
}

/// A hole's form as read, its sizes, its depth (blind with its length,
/// through all, or up to its target, shown), and its points, each a
/// checkbox to take it out (adding one is an agent's work).
pub(crate) fn hole(ui: &mut egui::Ui, h: &mut crate::core::edit::HoleForm) -> Keys {
    let keys = form::grid(ui, "hole", |rows| {
        rows.read_only("form", &h.form, false, None);
        for (_, f) in &mut h.sizes {
            let name = f.name.clone();
            rows.quantity(&name, f);
        }
        let name = h.depth.name.clone();
        rows.choice(&name, &mut h.depth);
        match h.depth.chosen.as_str() {
            "blind" => {
                rows.quantity("depth", &mut h.length);
            }
            "up_to" => {
                let t = h.target.clone().unwrap_or_default();
                rows.read_only("up to", &short("face", &t), true, Some(&t));
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
    keys
}

/// A pattern's form, directions, axis or plane as read, its spacings,
/// counts and angle, whether it runs against its directions, its skips
/// as typed, and its seeds, each a checkbox to take it out.
pub(crate) fn pattern(ui: &mut egui::Ui, p: &mut crate::core::edit::PatternForm) -> Keys {
    let keys = form::grid(ui, "pattern", |rows| {
        rows.read_only("form", &p.form, false, None);
        for (k, r) in &p.refs {
            if r.contains('/') {
                rows.read_only(k, &short("entity", r), true, Some(r));
            } else {
                rows.read_only(k, r, false, None);
            }
        }
        for (_, f) in &mut p.sizes {
            let name = f.name.clone();
            rows.quantity(&name, f);
        }
        if p.form != "mirror" {
            rows.text("skip", &mut p.skip);
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
    keys
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
pub(crate) fn mate(ui: &mut egui::Ui, m: &mut crate::core::edit::MateForm) -> Keys {
    let mut keys = form::grid(ui, "mate", |rows| {
        let name = m.form.name.clone();
        rows.choice(&name, &mut m.form);
        for (side, c) in &m.connectors {
            rows.read_only(side, &connector_text(c), true, Some(c));
        }
        let (angle, distance) = m.takes();
        if angle {
            rows.quantity("angle (empty: free)", &mut m.angle);
        }
        if distance {
            rows.quantity("distance (empty: free)", &mut m.distance);
        }
    });
    for (side, o) in [("a", &mut m.a), ("b", &mut m.b)] {
        ui.collapsing(format!("Offset of {side}"), |ui| {
            let k = form::grid(ui, ("offset", side), |rows| {
                for (k, f) in ["x", "y", "z"].iter().zip(o.origin.iter_mut()) {
                    rows.quantity(&format!("along {k}"), f);
                }
                rows.quantity("turn about z", &mut o.rotate);
            });
            keys.apply |= k.apply;
            keys.revert |= k.revert;
            ui.checkbox(&mut o.flip.chosen, "turned over");
        });
    }
    keys
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

/// A sketch dimension's value, edited in place. Apply sends `sketch.edit`
/// against the revision it was read at.
pub(crate) fn dimension(ui: &mut egui::Ui, ws: &mut Workspace) {
    let Some(f) = &mut ws.dimension else {
        return;
    };
    form::heading(
        ui,
        Some(Icon::RULER),
        &format!("{} · dimension", f.constraint),
        f.dirty(),
    );
    ui.label(format!(
        "{} in sketch {} of part {}",
        f.ty, f.sketch, f.part
    ));
    let changed: Vec<_> = f.changed.iter().cloned().collect();
    match form::banner(ui, &format!("{}'s dimension", f.sketch), &changed) {
        Some(Banner::Reload) => f.changed = None,
        Some(Banner::KeepMine) => {
            if let Some(c) = f.changed.take() {
                f.value.text = c.yours;
            }
        }
        None => {}
    }
    if let Some(e) = &f.error {
        form::error(ui, e);
    }
    let keys = form::grid(ui, "dimension", |rows| {
        rows.quantity("value", &mut f.value);
    });
    let (dirty, saving) = (f.dirty(), f.saving);
    let foot = form::foot(ui, dirty, saving, ("Apply", "Revert"));
    if (foot.revert || keys.revert) && dirty {
        f.value.text.clone_from(&f.value.read);
        f.error = None;
    }
    if (foot.apply || keys.apply) && dirty && !saving {
        ws.save_dimension();
    }
}

/// Every field of the chosen feature's form but the sub-forms', in one
/// grid.
fn fields(rows: &mut Rows<'_>, form: &mut crate::core::edit::Form) {
    for f in &mut form.params {
        let name = f.name.clone();
        rows.quantity(&name, f);
    }
    if let Some(e) = &mut form.extent {
        let name = e.ty.name.clone();
        rows.choice(&name, &mut e.ty);
        if matches!(e.ty.chosen.as_str(), "blind" | "symmetric") {
            rows.quantity("length", &mut e.length);
        }
        if e.ty.chosen == "through_all" {
            rows.flag("both ways", &mut e.both);
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
        let name = c.name.clone();
        rows.choice(&name, c);
    }
    if let Some(f) = &mut form.fixed {
        rows.flag("fixed where it is", f);
    }
}

/// The chosen feature's parameters and placement, edited in place. Apply
/// (or Enter in a field) sends `feature.edit` against the revision the
/// form was read at; Revert (or Esc) puts back what was read. A dimension
/// chosen by its label shows instead.
pub fn properties(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ws.dimension.is_some() {
        dimension(ui, ws);
        return;
    }
    let Some(form) = &mut ws.form else {
        ui.weak("Choose a feature in the tree to see and edit it");
        return;
    };
    let icon = match &form.mate {
        Some(m) => crate::core::tree::mate_icon(&m.form.read),
        None => crate::core::tree::type_icon(
            &form.ty,
            form.operation.as_ref().map(|o| o.read.as_str()),
            None,
        ),
    };
    let icon = crate::ui::toolbar::icon_of(icon);
    form::heading(
        ui,
        icon,
        &format!("{} · {}", form.feature, form.ty),
        form.dirty(),
    );
    ui.label(format!("{} in part {}", form.ty, form.part));
    match form::banner(ui, &form.feature, &form.changed) {
        Some(Banner::Reload) => form.changed.clear(),
        Some(Banner::KeepMine) => form.keep_mine(),
        None => {}
    }
    if let Some(e) = &form.error {
        form::error(ui, e);
    }
    let mut keys = form::grid(ui, "params", |rows| fields(rows, form));
    for k in [
        form.blend.as_mut().map(|b| blend(ui, b)),
        form.hole.as_mut().map(|h| hole(ui, h)),
        form.pattern.as_mut().map(|p| pattern(ui, p)),
        form.mate.as_mut().map(|m| mate(ui, m)),
    ]
    .into_iter()
    .flatten()
    {
        keys.apply |= k.apply;
        keys.revert |= k.revert;
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
            let k = form::grid(ui, "placement", |rows| {
                for (axis, fields) in [
                    ("origin", &mut form.origin),
                    ("z axis", &mut form.z),
                    ("x axis", &mut form.x),
                ] {
                    for (k, f) in ["x", "y", "z"].iter().zip(fields.iter_mut()) {
                        rows.quantity(&format!("{axis} {k}"), f);
                    }
                }
            });
            keys.apply |= k.apply;
            keys.revert |= k.revert;
        });
    }
    let (dirty, saving) = (form.dirty(), form.saving);
    let foot = form::foot(ui, dirty, saving, ("Apply", "Revert"));
    if (foot.revert || keys.revert) && dirty {
        form.revert();
    }
    let key = (form.part.clone(), form.feature.clone());
    if (foot.apply || keys.apply) && dirty && !saving {
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
    form::grid(ui, "import", |rows| {
        rows.read_only("file", &i.file, true, Some(&i.file));
        rows.read_only(
            "BLAKE3",
            &format!("{}…", &i.blake3[..i.blake3.len().min(16)]),
            true,
            Some(&i.blake3),
        );
        let product = match &i.product {
            Some((id, name, 0)) => format!("{id} ({name})"),
            Some((id, name, n)) => format!("{id} ({name}), ordinal {n}"),
            None => "none: a body no product reaches".to_owned(),
        };
        rows.read_only("product", &product, false, None);
        rows.read_only("body", &i.body.to_string(), false, None);
        if let Some(b) = built {
            for (k, n) in b.heal.iter().filter(|(_, n)| *n > 0) {
                rows.read_only(k, &n.to_string(), false, None);
            }
            if let Some(t) = b.tolerance_text() {
                rows.read_only("largest tolerance", &t, false, None);
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
