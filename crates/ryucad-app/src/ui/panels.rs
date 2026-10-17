//! The panels around the viewport: the feature tree, the participants bar
//! and the status line.

use crate::core::workspace::Workspace;
use crate::ui::style;
use crate::ui::view::View;
use eframe::egui::{self, RichText};
use ryucad::live::Kind;

/// An error as one line for people.
pub fn message(e: &serde_json::Value) -> String {
    e["message"].as_str().unwrap_or("failed").to_owned()
}

/// Parts and their features in order, with rebuild status and mesh
/// faults. Choosing a feature paints and frames its body.
pub fn tree(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    ui.heading("Parts");
    if ws.model.parts.is_empty() {
        ui.weak("No parts yet");
    }
    let mut choose = None;
    for part in ws.model.parts.values() {
        let title = match &part.error {
            Some(_) => {
                RichText::new(format!("{} (broken)", part.id)).color(ui.visuals().warn_fg_color)
            }
            None => RichText::new(&part.id),
        };
        let header = egui::CollapsingHeader::new(title)
            .id_salt(("part", &part.id))
            .default_open(true)
            .show(ui, |ui| {
                for f in &part.features {
                    let key = (part.id.clone(), f.id.clone());
                    let faults = ws
                        .model
                        .meshes
                        .get(&key)
                        .and_then(|m| m.mesh.as_ref().ok())
                        .map_or(0, |m| m.faults.len());
                    let ty = f.ty.as_deref().unwrap_or("?");
                    let mut text = format!("{} ({ty})", f.id);
                    if f.error.is_some() {
                        text = format!("{text}: failed");
                    } else if faults > 0 {
                        text = format!("{text}: {faults} unmeshed");
                    }
                    let chosen = view.chosen.as_ref() == Some(&key);
                    let r = ui.selectable_label(chosen, text);
                    let r = match &f.error {
                        Some(e) => r.on_hover_text(message(e)),
                        None if faults > 0 => r.on_hover_text(format!(
                            "{faults} face(s) couldn't be meshed: the kernel left them out"
                        )),
                        None => r,
                    };
                    if r.clicked() {
                        choose = Some(if chosen { None } else { Some(key) });
                    }
                }
            });
        if let Some(e) = &part.error {
            header.header_response.on_hover_text(message(e));
        }
    }
    if let Some(c) = choose {
        view.chosen = c;
        ws.open_form(view.chosen.clone());
        if let Some(k) = view.chosen.clone() {
            view.fit(ws, Some(&k));
        }
    }
}

/// Everyone attached, what they say they are doing, and "Follow" for
/// anyone with a camera.
pub fn people(ui: &mut egui::Ui, ws: &mut Workspace) {
    ui.horizontal_wrapped(|ui| {
        let me = ws.me().to_owned();
        let mut follow = None;
        for p in ws.people.all.values() {
            let colour = if p.id == me {
                style::SELECTED
            } else {
                style::participant(&p.id)
            };
            let kind = match p.kind {
                Kind::Human => "",
                Kind::Agent => " [agent]",
            };
            let you = if p.id == me { " (you)" } else { "" };
            ui.label(RichText::new(format!("{}{kind}{you}", p.name)).color(style::color32(colour)));
            if let Some((text, _)) = &p.status {
                ui.weak(text);
            }
            if p.id != me && p.camera.is_some() {
                let on = ws.following.as_deref() == Some(p.id.as_str());
                if ui.selectable_label(on, "Follow").clicked() {
                    follow = Some(if on { None } else { Some(p.id.clone()) });
                }
            }
            ui.separator();
        }
        if let Some(f) = follow {
            ws.following = f;
        }
    });
}

/// The connection, what agents are doing, and the model's state.
pub fn status(ui: &mut egui::Ui, ws: &Workspace) {
    ui.horizontal(|ui| {
        if ws.lost {
            let why = ws
                .lost_why
                .as_ref()
                .map_or(String::new(), |e| format!(": {}", e.message));
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Reconnecting to the daemon{why}"),
            );
        } else {
            ui.label("Connected");
        }
        ui.separator();
        let busy: Vec<String> = ws
            .people
            .others(ws.me())
            .filter_map(|p| p.status.as_ref().map(|(s, _)| format!("{}: {s}", p.name)))
            .collect();
        if !busy.is_empty() {
            ui.label(busy.join(" · "));
            ui.separator();
        }
        let features: usize = ws.model.parts.values().map(|p| p.features.len()).sum();
        let failed: usize = ws
            .model
            .parts
            .values()
            .flat_map(|p| &p.features)
            .filter(|f| f.error.is_some())
            .count();
        ui.label(format!(
            "{} part(s), {features} feature(s)",
            ws.model.parts.len()
        ));
        if failed > 0 {
            ui.colored_label(ui.visuals().error_fg_color, format!("{failed} failed"));
        }
        if !ws.idle() {
            ui.spinner();
        }
        if let Some(e) = ws.errors.last() {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, &e.message);
        }
    });
}

fn field_row(ui: &mut egui::Ui, f: &mut crate::core::edit::Field, label: &str) {
    ui.label(label);
    let edit = egui::TextEdit::singleline(&mut f.text).desired_width(110.0);
    let r = ui.add(edit);
    if f.dirty() {
        r.on_hover_text(format!("was {}", f.read));
    }
    ui.end_row();
}

/// The chosen feature's parameters and placement, edited in place. Apply
/// sends `feature.edit` against the revision the form was read at.
pub fn properties(ui: &mut egui::Ui, ws: &mut Workspace) {
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
    });
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
            for f in form
                .params
                .iter_mut()
                .chain(&mut form.origin)
                .chain(&mut form.z)
                .chain(&mut form.x)
            {
                f.text.clone_from(&f.read);
            }
            form.error = None;
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

/// The stable names of a feature's faces, edges and vertices, to copy.
fn names(ui: &mut egui::Ui, ws: &Workspace, key: &(String, String)) {
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

/// The bottom panel's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Activity,
    History,
}

/// How long ago `ms` (Unix milliseconds) was, for people.
pub fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => format!("{s} s ago"),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// The activity log, newest first, with undo and redo per entry; and the
/// project's Git history, read-only.
pub fn journal(ui: &mut egui::Ui, ws: &mut Workspace, tab: &mut Tab) {
    ui.horizontal(|ui| {
        ui.selectable_value(tab, Tab::Activity, "Activity");
        if ui.selectable_value(tab, Tab::History, "History").clicked() {
            ws.load_history();
        }
    });
    ui.separator();
    match tab {
        Tab::Activity => activity(ui, ws),
        Tab::History => history(ui, ws),
    }
}

fn activity(ui: &mut egui::Ui, ws: &mut Workspace) {
    let mut undo = None;
    let mut redo = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            if ws.activity.entries.is_empty() {
                ui.weak("Nothing has changed since the daemon started");
            }
            egui::Grid::new("activity")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for e in ws.activity.entries.iter().rev() {
                        let seq = e["seq"].as_u64().unwrap_or_default();
                        ui.weak(ago(e["time_ms"].as_u64().unwrap_or_default()));
                        let who = e["participant"]["name"]
                            .as_str()
                            .unwrap_or("outside ryucad");
                        ui.label(who);
                        let what = match e["kind"].as_str() {
                            Some("undo") => format!("undo of {}", e["target"]),
                            Some("redo") => format!("redo of {}", e["target"]),
                            Some("external") => "files changed".to_owned(),
                            _ => e["method"].as_str().unwrap_or_default().to_owned(),
                        };
                        ui.label(what);
                        let files: Vec<String> = e["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|f| {
                                format!(
                                    "{} {}",
                                    f["change"].as_str().unwrap_or(""),
                                    f["path"].as_str().unwrap_or("")
                                )
                            })
                            .collect();
                        ui.label(format!("{} file(s)", files.len()))
                            .on_hover_text(files.join("\n"));
                        if crate::core::activity::can_undo(e) {
                            if ui.small_button("Undo").clicked() {
                                undo = Some(seq);
                            }
                        } else if crate::core::activity::can_redo(e) {
                            if ui.small_button("Redo").clicked() {
                                redo = Some(seq);
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(s) = undo {
        ws.undo(Some(s));
    }
    if let Some(s) = redo {
        ws.redo(Some(s));
    }
}

fn history(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ui.small_button("Refresh").clicked() {
        ws.load_history();
    }
    let Some(h) = &ws.history else {
        ui.spinner();
        return;
    };
    if h["repository"] == false {
        ui.weak("This project isn't in a Git repository, so it has no history");
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let uncommitted = h["uncommitted"].as_array().map_or(0, Vec::len);
            if uncommitted > 0 {
                ui.label(format!("{uncommitted} uncommitted change(s)"));
            }
            egui::Grid::new("history")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for c in h["commits"].as_array().into_iter().flatten() {
                        let id = c["id"].as_str().unwrap_or_default();
                        ui.monospace(&id[..id.len().min(10)]);
                        ui.label(c["author"].as_str().unwrap_or_default());
                        ui.weak(ago(c["time"].as_u64().unwrap_or_default() * 1000));
                        ui.label(c["subject"].as_str().unwrap_or_default());
                        ui.end_row();
                    }
                });
        });
}
