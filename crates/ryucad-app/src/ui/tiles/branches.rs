//! The Branches tile: the repository's other worktrees, one per agent's
//! branch, each previewed and compared.

use crate::core::workspace::Workspace;
use crate::ui::tiles::conflicts::side;
use eframe::egui;

/// Every worktree of the project's repository, one per agent's branch:
/// who works there, how it stands against this one, its last change, and
/// whether merging it in would be clean, with Open (a window on it),
/// Compare (with its working files) and Show preview (the conflicts it
/// would leave, read only).
pub fn branches(ui: &mut egui::Ui, ws: &mut Workspace) {
    use crate::core::branches::{compare_rev, last_change, name, standing, summary, who};
    // Other agents' work changes no file of this project: while the tab
    // shows, it reads the worktrees again now and then.
    let next = ws.branches_shown(std::time::Instant::now());
    ui.ctx().request_repaint_after(next);
    if ui.small_button("Refresh").clicked() {
        ws.load_branches();
    }
    let b = &ws.branches;
    if let Some(e) = &b.error {
        ui.weak(format!("No worktrees to show: {}", e.message));
        return;
    }
    if b.list.is_none() {
        ui.spinner();
        return;
    }
    let compared = ws.compare.as_ref().map(|c| c.rev.clone());
    let (mut open, mut compare, mut show) = (None, None, None);
    if let Some(me) = b.current() {
        ui.label(format!("This window: {} ({})", name(me), standing(me)));
    }
    if b.others().next().is_none() {
        ui.weak(
            "No other worktrees: `ryucad worktree add <folder> -b <branch>` makes one for an agent",
        );
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("branches")
                .num_columns(6)
                .striped(true)
                .show(ui, |ui| {
                    for w in b.others() {
                        let path = w["path"].as_str().unwrap_or_default().to_owned();
                        ui.label(name(w)).on_hover_text(&path);
                        ui.label(who(w));
                        ui.label(standing(w));
                        ui.weak(last_change(w).unwrap_or_default());
                        let preview = match b.previews.get(&path) {
                            Some(p) if b.key(w).as_ref() == Some(&p.key) => summary(p),
                            Some(p) => format!("{} (checking again)", summary(p)),
                            None => "checking…".to_owned(),
                        };
                        ui.label(preview);
                        ui.horizontal(|ui| {
                            if ui
                                .button("Open")
                                .on_hover_text(format!("Open {path} in a new window"))
                                .clicked()
                            {
                                open = Some(path.clone());
                            }
                            let rev = compare_rev(&path);
                            let on = compared.as_deref() == Some(rev.as_str());
                            if ui
                                .selectable_label(on, "Compare")
                                .on_hover_text("Compare the working files with this worktree's")
                                .clicked()
                            {
                                compare = Some((!on).then_some(rev));
                            }
                            let shown = b.shown.as_deref() == Some(path.as_str());
                            let has = b.previews.get(&path).is_some_and(|p| {
                                p.answer.as_ref().is_ok_and(|v| v["clean"] == false)
                            });
                            if ui
                                .add_enabled(
                                    has || shown,
                                    egui::Button::selectable(shown, "Show preview"),
                                )
                                .on_hover_text("List what merging it in would leave to settle")
                                .clicked()
                            {
                                show = Some((!shown).then_some(path.clone()));
                            }
                        });
                        ui.end_row();
                    }
                });
            if let (Some(path), Some(v)) = (&b.shown, b.shown_preview()) {
                ui.separator();
                let theirs = b
                    .others()
                    .find(|w| w["path"] == path.as_str())
                    .map_or_else(|| path.clone(), name);
                ui.strong(format!("Merging {theirs} in would leave"));
                preview_conflicts(ui, v);
            }
        });
    if let Some(p) = open {
        ws.open_worktree(&p);
    }
    if let Some(rev) = compare {
        ws.set_compare(rev);
    }
    if let Some(s) = show {
        ws.branches.shown = s;
    }
}

/// A merge preview's conflicts as the conflict panel lists them, read
/// only: there is no merge to settle yet.
pub(crate) fn preview_conflicts(ui: &mut egui::Ui, v: &serde_json::Value) {
    for c in v["conflicts"].as_array().into_iter().flatten() {
        let id = c["id"].as_str().unwrap_or_default();
        egui::CollapsingHeader::new(crate::core::merge::title(c))
            .id_salt(("preview", id))
            .show(ui, |ui| {
                ui.label(c["message"].as_str().unwrap_or_default());
                ui.weak(format!(
                    "caused by {}",
                    c["cause"].as_str().unwrap_or("both")
                ));
                for k in ["base", "ours", "theirs"] {
                    if let Some(x) = c.get(k) {
                        ui.label(format!("{k}: {}", side(x)));
                    }
                }
                for r in c["repairs"].as_array().into_iter().flatten() {
                    ui.weak(format!(
                        "a repair after the merge: {}",
                        r["summary"].as_str().unwrap_or_default()
                    ));
                }
            });
    }
    for f in v["already_failing"].as_array().into_iter().flatten() {
        ui.weak(format!(
            "{}/{} already fails on a side ({})",
            f["part"].as_str().unwrap_or_default(),
            f["feature"].as_str().unwrap_or_default(),
            f["error"].as_str().unwrap_or_default()
        ));
    }
    for w in v["warnings"].as_array().into_iter().flatten() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            w["message"].as_str().unwrap_or_default(),
        );
    }
}
