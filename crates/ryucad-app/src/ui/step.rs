//! The STEP dialogs: File > Import STEP… and Update import… (a file,
//! its plan, then the import) and File > Export STEP… (a schema, a unit
//! and where to write). A file is typed or found with Browse…, which
//! opens the OS's dialog once the frame is done.

use crate::core::step::{Plan, SCHEMAS, UNITS};
use crate::core::workspace::Workspace;
use crate::ui::dialogs::window;
use eframe::egui;

/// Which OS file dialog Browse… asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Browse {
    /// A STEP file to import.
    Open,
    /// Where to write an export.
    Save,
}

/// A file field named `label` with Browse… beside it; true when Enter
/// was pressed in it.
fn file_row(
    ui: &mut egui::Ui,
    label: &str,
    path: &mut String,
    browse: &mut Option<Browse>,
    how: Browse,
) -> bool {
    let mut entered = false;
    ui.horizontal(|ui| {
        let name = ui.label(label);
        let field = ui
            .add(egui::TextEdit::singleline(path).desired_width(320.0))
            .labelled_by(name.id);
        entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("Browse…").clicked() {
            *browse = Some(how);
        }
    });
    entered
}

/// A plan's lines: each part, then what it would make or change.
fn plan(ui: &mut egui::Ui, p: &Plan) {
    if p.reused {
        ui.label(format!(
            "The project has these bytes already, as {}.",
            p.stored
        ));
    } else {
        ui.label(format!("Kept in the project as {}.", p.stored));
    }
    for (head, lines) in &p.parts {
        ui.strong(head);
        for l in lines {
            ui.label(format!("    {l}"));
        }
    }
    if !p.failing.is_empty() {
        ui.colored_label(ui.visuals().warn_fg_color, "These would no longer build:");
        for f in &p.failing {
            ui.label(format!("    {f}"));
        }
    }
    if !p.vanished.is_empty() {
        ui.label("Gone from the file (their parts stay, and fail):");
        for v in &p.vanished {
            ui.label(format!("    {v}"));
        }
    }
    for w in &p.warnings {
        ui.colored_label(ui.visuals().warn_fg_color, w);
    }
}

/// File > Import STEP… or Update import…, while `ws.import` is open.
pub fn import(
    ctx: &egui::Context,
    ws: &mut Workspace,
    browse: &mut Option<Browse>,
    view: egui::Rect,
) {
    let Some(i) = &mut ws.import else {
        return;
    };
    let title = if i.update.is_some() {
        "Update import"
    } else {
        "Import STEP"
    };
    let (mut open, mut ask, mut go, mut close) = (true, false, false, false);
    window(title, view)
        .open(&mut open)
        .default_width(460.0)
        .show(ctx, |ui| {
            if let Some(u) = &i.update {
                ui.label(format!(
                    "A new version of {u}: unchanged faces, edges and vertices keep their names."
                ));
            }
            if let Some(done) = &i.done {
                ui.label("Done: one change, which Undo takes back.");
                plan(ui, done);
                close = ui.button("Close").clicked();
                return;
            }
            ask = file_row(ui, "STEP file", &mut i.path, browse, Browse::Open);
            let typed = !i.path.trim().is_empty();
            ask |= ui
                .add_enabled(typed && !i.waiting, egui::Button::new("Plan"))
                .on_hover_text("What it would make, without writing anything")
                .clicked();
            if i.waiting {
                ui.spinner();
            }
            if let Some(e) = &i.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
            }
            if let Some(p) = i.current() {
                ui.separator();
                plan(ui, p);
                ui.separator();
                let verb = if i.update.is_some() {
                    "Update"
                } else {
                    "Import"
                };
                go = ui
                    .add_enabled(!i.waiting, egui::Button::new(verb))
                    .clicked();
            }
        });
    if ask {
        ws.plan_import();
    }
    if go {
        ws.run_import();
    }
    if !open || close {
        ws.import = None;
    }
}

/// A choice among `options` in a labelled combo box.
fn combo(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[&str]) {
    let name = ui.label(label);
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
}

/// File > Export STEP…, while `ws.export` is open.
pub fn export(
    ctx: &egui::Context,
    ws: &mut Workspace,
    browse: &mut Option<Browse>,
    view: egui::Rect,
) {
    let Some(e) = &mut ws.export else {
        return;
    };
    let (mut open, mut go) = (true, false);
    window("Export STEP", view)
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(format!("{} as the working files have it", e.part));
            egui::Grid::new("export").num_columns(2).show(ui, |ui| {
                combo(ui, "Schema", &mut e.schema, &SCHEMAS);
                combo(ui, "Unit", &mut e.unit, &UNITS);
            });
            let entered = file_row(ui, "Save as", &mut e.path, browse, Browse::Save);
            let typed = !e.path.trim().is_empty();
            go = ui
                .add_enabled(typed && !e.waiting, egui::Button::new("Export"))
                .clicked()
                || (entered && typed && !e.waiting);
            if e.waiting {
                ui.spinner();
            }
            if let Some(err) = &e.error {
                ui.colored_label(ui.visuals().error_fg_color, &err.message);
            }
            if let Some((path, n)) = &e.done {
                ui.label(format!("Wrote {n} bytes to {path}"));
            }
        });
    if go {
        ws.run_export();
    }
    if !open {
        ws.export = None;
    }
}

/// The OS's dialog for `how`, in the window `parent`: a STEP file to open
/// or where to save one.
pub fn ask_os(parent: &eframe::Frame, how: Browse) -> Option<std::path::PathBuf> {
    let d = rfd::FileDialog::new()
        .set_parent(parent)
        .add_filter("STEP", &["step", "stp"]);
    match how {
        Browse::Open => d.set_title("Import a STEP file").pick_file(),
        Browse::Save => d.set_title("Export as STEP").save_file(),
    }
}
