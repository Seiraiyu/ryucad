//! The Conflicts tile: a merge's conflicts, each with its sides and
//! repairs.

use crate::core::workspace::Workspace;
use crate::ui::view::View;
use eframe::egui;

/// A value of a conflict's side, short enough for the panel.
pub(crate) fn side(v: &serde_json::Value) -> String {
    let text = match v {
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    match text.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text,
    }
}

/// The merge Git is in the middle of: each conflict with what it
/// concerns, its sides and who caused it, and a button per choice and
/// planned repair (`merge.resolve`); `merge_broke` conflicts are fixed in
/// the model. "Show" chooses the feature and marks the entities it
/// concerns.
pub fn merge(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let Some(m) = &ws.merge else { return };
    ui.heading(format!("{} in progress", m.step()));
    ui.label(m.summary());
    ui.weak(
        "ryucad never stages or commits: once everything is settled, git add the files and commit.",
    );
    if let Some(e) = m.error.as_ref().or(m.failed.as_ref()) {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
    }
    let mut act: Option<(String, String, Option<String>)> = None;
    let mut show: Option<serde_json::Value> = None;
    for c in m.conflicts() {
        let id = c["id"].as_str().unwrap_or_default();
        let concerns = &c["concerns"];
        egui::CollapsingHeader::new(crate::core::merge::title(c))
            .id_salt(("conflict", id))
            .show(ui, |ui| {
                ui.label(c["message"].as_str().unwrap_or_default());
                ui.weak(format!(
                    "caused by {}",
                    c["cause"].as_str().unwrap_or("both")
                ));
                for k in ["base", "ours", "theirs"] {
                    if let Some(v) = c.get(k) {
                        ui.label(format!("{k}: {}", side(v)));
                    }
                }
                if let Some(h) = crate::core::merge::hint(c) {
                    ui.weak(h);
                }
                ui.horizontal_wrapped(|ui| {
                    let choices = c["choices"].as_array().into_iter().flatten();
                    for choice in choices.filter_map(|x| x.as_str()) {
                        let label = match choice {
                            "ours" => "Take ours",
                            "theirs" => "Take theirs",
                            // A value is typed: the CLI's `--value`.
                            _ => continue,
                        };
                        if ui
                            .add_enabled(!m.resolving, egui::Button::new(label))
                            .clicked()
                        {
                            act = Some((id.to_owned(), choice.to_owned(), None));
                        }
                    }
                    for r in c["repairs"].as_array().into_iter().flatten() {
                        let summary = r["summary"].as_str().unwrap_or("Repair");
                        let button = egui::Button::new(summary);
                        if ui.add_enabled(!m.resolving, button).clicked() {
                            let rid = r["id"].as_str().map(str::to_owned);
                            act = Some((id.to_owned(), "repair".to_owned(), rid));
                        }
                    }
                    if concerns["feature"].is_string() && ui.button("Show").clicked() {
                        show = Some(concerns.clone());
                    }
                });
            });
    }
    let settled = m.settled();
    if !settled.is_empty() {
        ui.label(format!(
            "Settled, waiting for git add: {}",
            settled.join(", ")
        ));
    }
    let untracked = m.untracked();
    if !untracked.is_empty() {
        ui.weak(format!(
            "Not in Git yet (add them, or move them out, before committing): {}",
            untracked.join(", ")
        ));
    }
    for f in m.status["already_failing"].as_array().into_iter().flatten() {
        ui.weak(format!(
            "{}/{} already failed before the merge ({})",
            f["part"].as_str().unwrap_or_default(),
            f["feature"].as_str().unwrap_or_default(),
            f["error"].as_str().unwrap_or_default()
        ));
    }
    if m.resolving {
        ui.spinner();
    }
    ui.separator();
    if let Some((id, choice, repair)) = act {
        ws.resolve(&id, &choice, repair.as_deref());
    }
    if let Some(c) = show {
        let part = c["part"].as_str().unwrap_or_default().to_owned();
        let feature = c["feature"].as_str().unwrap_or_default().to_owned();
        view.marked = c["entities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| Some(format!("{part}/{feature}/{}", e.as_str()?)))
            .collect();
        let key = (part, feature);
        view.chosen = Some(key.clone());
        ws.open_form(Some(key.clone()));
        view.fit(ws, Some(&key));
    }
}
