//! The Settings dialog (project settings, kept in `ryucad.json` and Git)
//! and the Preferences dialog (this user's, kept on this machine). Both
//! change values only through `config.set`, and show where each comes
//! from.

use crate::core::workspace::Workspace;
use eframe::egui;
use serde_json::{Value, json};

/// Where a setting's value comes from, for people.
fn source(ws: &Workspace, key: &str) -> &'static str {
    match ws.settings.all.iter().find(|s| s.key == key) {
        Some(s) if s.default => "default",
        Some(s) if s.scope == "project" => "this project",
        Some(_) => "your preferences",
        None => "",
    }
}

/// A dialog window, first placed at the top of `view` (the 3D view's
/// rectangle), clear of the menus and panels.
pub fn window(title: &str, view: egui::Rect) -> egui::Window<'_> {
    egui::Window::new(title)
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(view.center_top() + egui::vec2(0.0, 16.0))
}

fn about(ws: &Workspace, key: &str) -> String {
    ws.settings
        .all
        .iter()
        .find(|s| s.key == key)
        .map_or_else(String::new, |s| s.about.clone())
}

/// A setting chosen from a list.
fn choice(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str, options: &[&str]) {
    let current = ws.settings.text(key, "").to_owned();
    let name = ui.label(label).on_hover_text(about(ws, key));
    let mut picked = None;
    egui::ComboBox::from_id_salt(key)
        .selected_text(&current)
        .show_ui(ui, |ui| {
            for o in options {
                if ui.selectable_label(current == *o, *o).clicked() && current != *o {
                    picked = Some(*o);
                }
            }
        })
        .response
        .labelled_by(name.id);
    ui.weak(source(ws, key));
    ui.end_row();
    if let Some(o) = picked {
        ws.set_config(key, json!(o));
    }
}

/// A setting that is on or off.
fn toggle(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str) {
    let mut on = ws
        .settings
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let name = ui.label(label).on_hover_text(about(ws, key));
    if ui.checkbox(&mut on, "").labelled_by(name.id).changed() {
        ws.set_config(key, json!(on));
    }
    ui.weak(source(ws, key));
    ui.end_row();
}

/// A whole number in a range, sent when the drag ends.
fn number(ui: &mut egui::Ui, ws: &mut Workspace, key: &str, label: &str, range: (i64, i64)) {
    let id = ui.make_persistent_id(("setting", key));
    let stored = ws
        .settings
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or(range.0);
    let mut n = ui.data(|d| d.get_temp::<i64>(id)).unwrap_or(stored);
    let name = ui.label(label).on_hover_text(about(ws, key));
    let r = ui
        .add(egui::DragValue::new(&mut n).range(range.0..=range.1))
        .labelled_by(name.id);
    if r.changed() || r.dragged() {
        ui.data_mut(|d| d.insert_temp(id, n));
    }
    let done = (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && n != stored;
    if done {
        ws.set_config(key, json!(n));
    }
    if !r.dragged() && !r.has_focus() && !r.changed() {
        ui.data_mut(|d| d.remove::<i64>(id));
    }
    ui.weak(source(ws, key));
    ui.end_row();
}

/// The project's settings: only what rebuilding or understanding the
/// model needs.
pub fn settings(ctx: &egui::Context, ws: &mut Workspace, open: &mut bool, view: egui::Rect) {
    window("Project settings", view)
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Saved in this project's ryucad.json, so they travel with it in Git.");
            egui::Grid::new("project settings")
                .num_columns(3)
                .show(ui, |ui| {
                    choice(
                        ui,
                        ws,
                        "default_unit",
                        "Default unit",
                        &["mm", "cm", "m", "um", "in", "ft"],
                    );
                });
            ui.weak(
                "Bare numbers in feature files take this unit; values already written keep theirs.",
            );
        });
}

/// This user's preferences, on this machine.
pub fn preferences(ctx: &egui::Context, ws: &mut Workspace, open: &mut bool, view: egui::Rect) {
    window("Preferences", view)
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Saved for you on this machine; projects don't carry them.");
            for w in ws.settings.warnings.clone() {
                ui.colored_label(ui.visuals().warn_fg_color, w);
            }
            egui::Grid::new("preferences")
                .num_columns(3)
                .show(ui, |ui| {
                    choice(
                        ui,
                        ws,
                        "ui.style",
                        "Style",
                        &["drafting", "graphite", "studio"],
                    );
                    choice(ui, ws, "ui.theme", "Theme", &["system", "light", "dark"]);
                    choice(
                        ui,
                        ws,
                        "ui.navigation",
                        "Mouse navigation",
                        &["ryucad", "solidworks", "onshape"],
                    );
                    choice(
                        ui,
                        ws,
                        "display.quality",
                        "Display quality",
                        &["coarse", "normal", "fine"],
                    );
                    toggle(ui, ws, "display.edges", "Draw edges");
                    toggle(ui, ws, "display.sketches", "Draw sketches");
                    number(
                        ui,
                        ws,
                        "display.mesh_cache_mib",
                        "Mesh cache (MiB)",
                        (16, 65536),
                    );
                    number(
                        ui,
                        ws,
                        "daemon.poll_ms",
                        "Check files every (ms)",
                        (50, 5000),
                    );
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Disk cache");
                ui.weak(cache_text(ws.cache.as_ref()));
                let enabled = ws.cache.as_ref().is_some_and(|c| c["enabled"] == true);
                if ui
                    .add_enabled(enabled, egui::Button::new("Clear cache"))
                    .on_hover_text("Results and meshes every project on this machine reuses; anything cleared is rebuilt when next needed")
                    .clicked()
                {
                    ws.clear_cache();
                }
            });
        });
}

/// What the disk cache holds, as the Preferences window says it.
pub fn cache_text(stats: Option<&serde_json::Value>) -> String {
    let Some(s) = stats else {
        return "reading…".to_owned();
    };
    if s["enabled"] != true {
        return "none on this machine".to_owned();
    }
    let mib = |b: &serde_json::Value| b.as_u64().unwrap_or_default() as f64 / f64::from(1 << 20);
    format!(
        "{} entries, {:.1} MiB of {:.0} MiB",
        s["entries"].as_u64().unwrap_or_default(),
        mib(&s["bytes"]),
        mib(&s["max_bytes"])
    )
}

/// Tools > Check interference: the pairs of the shown assembly's solids
/// that share a volume, each with a Show button (the pair's touched
/// faces, to frame), how many only touch, and those that couldn't be
/// checked. Returns the faces to frame when Show is pressed.
pub fn interference(
    ctx: &egui::Context,
    ws: &mut Workspace,
    open: &mut bool,
    view: egui::Rect,
) -> Option<Vec<String>> {
    let mut show = None;
    let mut again = false;
    window("Interference", view)
        .open(open)
        .default_width(420.0)
        .show(ctx, |ui| {
            let Some(i) = &ws.interference else {
                ui.label("Show an assembly, then check it.");
                return;
            };
            ui.label(format!("Assembly {}", i.part));
            again = ui.button("Check again").clicked();
            ui.separator();
            if !i.loaded {
                ui.spinner();
                return;
            }
            if let Some(e) = &i.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
                return;
            }
            if i.pairs.is_empty() {
                ui.label("No instances interfere.");
            }
            for p in &i.pairs {
                ui.horizontal(|ui| {
                    ui.label(p.label(&i.part));
                    if ui.small_button("Show").clicked() {
                        show = Some(p.faces.clone());
                    }
                });
            }
            if i.touching > 0 {
                ui.label(format!("{} pair(s) only touch.", i.touching));
            }
            for (a, b) in &i.unchecked {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{a} and {b} couldn't be checked"),
                );
            }
            ui.weak("Volumes are estimated from meshes.");
        });
    if again {
        ws.check_interference();
    }
    show
}
