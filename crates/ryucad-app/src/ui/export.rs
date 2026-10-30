//! File > Export…: the format (STEP, STL, 3MF or USD), the options it
//! takes, and where to write. The path's extension follows the format;
//! Browse… opens the OS's dialog for the format, once the frame is done.
//! The daemon writes the file, so the window never holds its bytes.

use crate::core::export::{
    Choices, ENCODINGS, Export, Format, SCHEMAS, STEP_UNITS, UNITS, shown_refs,
};
use crate::core::workspace::Workspace;
use crate::ui::dialogs::window;
use crate::ui::step::{Browse, file_row};
use eframe::egui;

/// A choice among `options` in a labelled combo box; true when it
/// changed.
fn combo(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[&str]) -> bool {
    let name = ui.label(label);
    let before = value.clone();
    egui::ComboBox::from_id_salt(label)
        .selected_text(value.as_str())
        .show_ui(ui, |ui| {
            for o in options {
                ui.selectable_value(value, (*o).to_owned(), *o);
            }
        })
        .response
        .labelled_by(name.id);
    ui.end_row();
    *value != before
}

/// A labelled text field.
fn field(ui: &mut egui::Ui, label: &str, value: &mut String, enabled: bool) {
    let name = ui.label(label);
    ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(value).desired_width(120.0),
    )
    .labelled_by(name.id);
    ui.end_row();
}

/// The format's options, in the dialog's grid; true when the extension
/// it writes changed.
fn options(ui: &mut egui::Ui, c: &mut Choices) -> bool {
    let name = ui.label("Format");
    let before = c.format;
    egui::ComboBox::from_id_salt("Format")
        .selected_text(c.format.title())
        .show_ui(ui, |ui| {
            for f in Format::ALL {
                ui.selectable_value(&mut c.format, f, f.title());
            }
        })
        .response
        .labelled_by(name.id);
    ui.end_row();
    let mut changed = c.format != before;
    match c.format {
        Format::Step => {
            combo(ui, "Schema", &mut c.schema, &SCHEMAS);
            combo(ui, "Unit", &mut c.step_unit, &STEP_UNITS);
        }
        Format::Stl | Format::ThreeMf => {
            combo(ui, "Unit", &mut c.unit, &UNITS);
            field(ui, "Chord", &mut c.chord, true);
            field(ui, "Angle", &mut c.angle, true);
            if c.format == Format::Stl {
                ui.label("");
                ui.checkbox(&mut c.ascii, "ASCII");
                ui.end_row();
            }
        }
        Format::Usd => {
            changed |= combo(ui, "Encoding", &mut c.encoding, &ENCODINGS);
            combo(ui, "Unit", &mut c.unit, &UNITS);
            // A USD file holds B-rep, meshes or both: the one left ticked
            // can't be unticked.
            ui.label("");
            ui.add_enabled(c.mesh, egui::Checkbox::new(&mut c.brep, "B-rep"));
            ui.end_row();
            ui.label("");
            ui.add_enabled(c.brep, egui::Checkbox::new(&mut c.mesh, "Meshes"));
            ui.end_row();
            field(ui, "Chord", &mut c.chord, c.mesh);
            field(ui, "Angle", &mut c.angle, c.mesh);
        }
    }
    changed
}

/// What was written, or why not: the refusal, and each face it names
/// with Show. Returns the references to show when Show is pressed.
fn outcome(ui: &mut egui::Ui, e: &Export, ws: &Workspace) -> Option<Vec<String>> {
    let mut show = None;
    if let Some(err) = &e.error {
        let code = serde_json::to_value(err.code).unwrap_or_default();
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("{}: {}", code.as_str().unwrap_or_default(), err.message),
        );
        for f in &e.faults {
            ui.horizontal(|ui| {
                ui.label(format!("{} ({})", f.face, f.why));
                let refs = shown_refs(&ws.model, &f.face);
                if ui
                    .add_enabled(!refs.is_empty(), egui::Button::new("Show").small())
                    .on_disabled_hover_text(format!("Show {} to see this face.", f.part))
                    .clicked()
                {
                    show = Some(refs);
                }
            });
        }
    }
    if let Some(w) = &e.done {
        let triangles = w
            .triangles
            .map(|t| format!(", {t} triangles"))
            .unwrap_or_default();
        ui.label(format!("Wrote {} bytes{triangles} to {}", w.bytes, w.out));
    }
    show
}

/// File > Export…, while `ws.export` is open. Returns the faces to show
/// when a refusal's Show is pressed.
pub fn dialog(
    ctx: &egui::Context,
    ws: &mut Workspace,
    browse: &mut Option<Browse>,
    view: egui::Rect,
) -> Option<Vec<String>> {
    let mut e = ws.export.take()?;
    let (mut open, mut go, mut show) = (true, false, None);
    // Titled by what it exports, so its title and its Export button are
    // two names.
    window(&format!("Export {}", e.part), view)
        .id(egui::Id::new("export"))
        .open(&mut open)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.label("As the working files have it.");
            let changed = egui::Grid::new("export")
                .num_columns(2)
                .show(ui, |ui| options(ui, &mut e.choices))
                .inner;
            if changed {
                e.follow_format();
            }
            let entered = file_row(ui, "Save as", &mut e.path, browse, Browse::Save);
            let typed = !e.path.trim().is_empty();
            go = ui
                .add_enabled(typed && !e.waiting, egui::Button::new("Export"))
                .clicked()
                || (entered && typed && !e.waiting);
            if e.waiting {
                ui.spinner();
            }
            show = outcome(ui, &e, ws);
        });
    if open {
        ws.export = Some(e);
    }
    if go {
        ws.run_export();
    }
    if let Some(refs) = &show {
        ws.select(refs.clone(), ryucad::live::SelectMode::Replace);
    }
    show
}

/// The OS's dialog for where to write an export of `c`'s format: its
/// name and extension as the filter.
pub fn ask_os(parent: &eframe::Frame, c: &Choices) -> Option<std::path::PathBuf> {
    let (name, exts) = match c.format {
        Format::Step => ("STEP", vec!["step", "stp"]),
        Format::Stl => ("STL", vec!["stl"]),
        Format::ThreeMf => ("3MF", vec!["3mf"]),
        Format::Usd => ("USD", vec![c.encoding.as_str()]),
    };
    rfd::FileDialog::new()
        .set_parent(parent)
        .add_filter(name, &exts)
        .set_title(format!("Export as {name}"))
        .save_file()
}
