//! The STEP dialogs: File > Import STEP… and Update import… (a file,
//! its plan, then the import). A file is typed or found with Browse…,
//! which opens the OS's dialog once the frame is done; File > Export…
//! (`ui::export`) and the BOM's Export CSV… browse the same way.

use crate::core::step::Plan;
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
    /// Where to write a BOM's CSV.
    SaveCsv,
}

/// A file field named `label` with Browse… beside it; true when Enter
/// was pressed in it.
pub(crate) fn file_row(
    ui: &mut egui::Ui,
    label: &str,
    path: &mut String,
    browse: &mut Option<Browse>,
    how: Browse,
) -> bool {
    let mut entered = false;
    ui.horizontal(|ui| {
        let name = ui.label(egui::RichText::new(label).color(ui.visuals().weak_text_color()));
        let field = ui
            .add(
                egui::TextEdit::singleline(path)
                    .desired_width(320.0)
                    .font(egui::TextStyle::Monospace),
            )
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
                crate::ui::form::error(ui, e);
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

/// The OS's dialog for `how`, in the window `parent`: a STEP file to
/// open, where to save an export (of the format `ws`'s export dialog
/// has), or where to save a BOM.
pub fn ask_os(parent: &eframe::Frame, how: Browse, ws: &Workspace) -> Option<std::path::PathBuf> {
    let d = rfd::FileDialog::new().set_parent(parent);
    match how {
        Browse::Open => d
            .add_filter("STEP", &["step", "stp"])
            .set_title("Import a STEP file")
            .pick_file(),
        Browse::Save => crate::ui::export::ask_os(parent, &ws.export.as_ref()?.choices),
        Browse::SaveCsv => d
            .add_filter("CSV", &["csv"])
            .set_title("Export the BOM as CSV")
            .save_file(),
    }
}
