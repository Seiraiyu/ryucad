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
