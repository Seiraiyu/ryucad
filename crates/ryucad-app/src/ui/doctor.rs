//! Doctor in the window: the issues `ryucad doctor` finds, each with the
//! repairs it offers and the exact changes they would make, applied on a
//! click. The same list serves the Doctor dialog of an open project and
//! the screen of a damaged one.

use crate::core::doctor::{Offline, proposed_order};
use crate::core::workspace::Workspace;
use eframe::egui::{self, RichText};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// What the person chose for repairs' parameters, by repair ID.
#[derive(Debug, Default)]
pub struct State {
    /// `missing_manifest`: the default unit.
    pub unit: BTreeMap<String, String>,
    /// `missing_part_index`: the feature order, as arranged.
    pub order: BTreeMap<String, Vec<String>>,
}

/// The parameters a repair takes, from what was chosen.
fn params(repair: &Value, state: &State) -> Map<String, Value> {
    let id = repair["id"].as_str().unwrap_or_default();
    let mut out = Map::new();
    for p in repair["params"].as_array().into_iter().flatten() {
        match p["name"].as_str() {
            Some("unit") => {
                if let Some(u) = state.unit.get(id) {
                    out.insert("unit".into(), json!(u));
                }
            }
            Some("order") => {
                if let Some(o) = state.order.get(id) {
                    out.insert("order".into(), json!(o));
                }
            }
            _ => {}
        }
    }
    out
}

/// The parameter widgets of one repair.
fn param_ui(ui: &mut egui::Ui, repair: &Value, state: &mut State) {
    let id = repair["id"].as_str().unwrap_or_default().to_owned();
    for p in repair["params"].as_array().into_iter().flatten() {
        match p["name"].as_str() {
            Some("unit") => {
                let unit = state.unit.entry(id.clone()).or_insert_with(|| "mm".into());
                ui.horizontal(|ui| {
                    let name = ui.label("Default unit");
                    egui::ComboBox::from_id_salt(("unit", &id))
                        .selected_text(unit.as_str())
                        .show_ui(ui, |ui| {
                            for u in ["mm", "cm", "m", "um", "in", "ft"] {
                                ui.selectable_value(unit, u.to_owned(), u);
                            }
                        })
                        .response
                        .labelled_by(name.id);
                });
            }
            Some("order") => {
                let order = state
                    .order
                    .entry(id.clone())
                    .or_insert_with(|| proposed_order(repair));
                ui.label("Feature order (first is built first):");
                let mut swap = None;
                for (i, f) in order.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(i > 0, egui::Button::new("Up").small())
                            .clicked()
                        {
                            swap = Some((i - 1, i));
                        }
                        if ui
                            .add_enabled(i + 1 < order.len(), egui::Button::new("Down").small())
                            .clicked()
                        {
                            swap = Some((i, i + 1));
                        }
                        ui.label(f);
                    });
                }
                if let Some((a, b)) = swap {
                    order.swap(a, b);
                }
            }
            _ => {}
        }
    }
}

/// The issues of a report, each with its repairs. Returns the repair (and
/// its parameters) the person chose to apply.
pub fn issues(
    ui: &mut egui::Ui,
    report: &Value,
    busy: bool,
    state: &mut State,
) -> Option<(Vec<String>, Map<String, Value>)> {
    if report["healthy"] == true {
        ui.label("No problems found.");
        return None;
    }
    let mut chosen = None;
    for (n, issue) in report["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let error = issue["severity"] == "error";
        let colour = if error {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().warn_fg_color
        };
        ui.group(|ui| {
            ui.label(
                RichText::new(issue["code"].as_str().unwrap_or_default())
                    .color(colour)
                    .strong(),
            );
            ui.label(issue["message"].as_str().unwrap_or_default());
            for f in issue["files"].as_array().into_iter().flatten() {
                ui.monospace(f.as_str().unwrap_or_default());
            }
            let repairs = issue["repairs"].as_array().cloned().unwrap_or_default();
            if repairs.is_empty() {
                ui.weak("No repair: this needs a person or an agent.");
            }
            // Alternatives (keep the original, or take the copy) side by side.
            ui.horizontal_top(|ui| {
                for r in &repairs {
                    ui.vertical(|ui| {
                        ui.label(r["summary"].as_str().unwrap_or_default());
                        param_ui(ui, r, state);
                        egui::CollapsingHeader::new("Planned changes")
                            .id_salt(("changes", n, r["id"].as_str()))
                            .show(ui, |ui| {
                                for c in r["changes"].as_array().into_iter().flatten() {
                                    ui.monospace(format!(
                                        "{} {}",
                                        c["action"].as_str().unwrap_or_default(),
                                        c["path"].as_str().unwrap_or_default()
                                    ));
                                    if let Some(text) = c["content"].as_str() {
                                        let shown: String = text.chars().take(2000).collect();
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(shown).monospace().small(),
                                            )
                                            .wrap(),
                                        );
                                    }
                                }
                            });
                        if ui.add_enabled(!busy, egui::Button::new("Repair")).clicked() {
                            let id = r["id"].as_str().unwrap_or_default().to_owned();
                            chosen = Some((vec![id], params(r, state)));
                        }
                    });
                }
            });
        });
    }
    chosen
}

/// The Doctor dialog of an open project.
pub fn dialog(ctx: &egui::Context, ws: &mut Workspace, open: &mut bool, state: &mut State) {
    let mut chosen = None;
    let mut again = false;
    egui::Window::new("Doctor")
        .open(open)
        .default_width(520.0)
        .show(ctx, |ui| {
            again = ui.button("Check again").clicked();
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(480.0)
                .show(ui, |ui| match &ws.doctor {
                    Some(report) => chosen = issues(ui, report, !ws.idle(), state),
                    None => {
                        ui.spinner();
                    }
                });
        });
    if again {
        ws.diagnose();
    }
    if let Some((ids, params)) = chosen {
        ws.repair(ids, params);
    }
}

/// The screen of a damaged project: what is wrong and how to repair it.
/// Returns true when the person asks to open the repaired project.
pub fn damaged(ui: &mut egui::Ui, offline: &mut Offline, state: &mut State) -> bool {
    let mut open = false;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("This project needs repair");
        ui.label(format!(
            "{} has parts but can't be opened as it is.",
            offline.root.display()
        ));
        // No undo to promise: with no daemon there is no activity log yet.
        ui.label("Nothing is changed until you choose a repair, and a repair changes only the files in its planned changes. Git has the files as they were committed.");
        if let Some(e) = &offline.error {
            ui.colored_label(ui.visuals().error_fg_color, &e.message);
        }
        ui.horizontal(|ui| {
            open = ui.add_enabled(offline.openable() && !offline.busy(), egui::Button::new("Open project")).clicked();
            if offline.busy() {
                ui.spinner();
            }
        });
        ui.separator();
        let mut chosen = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(report) = offline.report.clone() {
                chosen = issues(ui, &report, offline.busy(), state);
            }
        });
        if let Some((ids, params)) = chosen {
            offline.repair(&ids, &params);
        }
    });
    open
}
