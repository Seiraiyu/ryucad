//! The Branches tile: the repository's branches, checked out, compared
//! or opened where another worktree has them, and its other worktrees,
//! one per agent's branch, each previewed and compared.

use crate::core::workspace::Workspace;
use crate::ui::tiles::conflicts::side;
use eframe::egui;

/// The repository's branches (design 447): Check out (a dialog first),
/// Compare (with the branch) and Open worktree where another worktree has
/// it. Below, every other worktree, one per agent's branch: who works
/// there, how it stands against this one, its last change, and whether
/// merging it in would be clean, with Open (a window on it), Compare
/// (with its working files) and Show preview (the conflicts it would
/// leave, read only).
pub fn branches(ui: &mut egui::Ui, ws: &mut Workspace) {
    use crate::core::branches::{
        branch_label, branch_rev, compare_rev, last_change, name, standing, summary, who,
    };
    // Other agents' work changes no file of this project: while the tab
    // shows, it reads the branches and worktrees again now and then.
    let next = ws.branches_shown(std::time::Instant::now());
    ui.ctx().request_repaint_after(next);
    if ui.small_button("Refresh").clicked() {
        ws.load_branches();
    }
    let compared = ws.comparing.compare.as_ref().map(|c| c.named().to_owned());
    let (mut open, mut compare, mut show, mut checkout) = (None, None, None, None);
    let b = &ws.branches;
    ui.strong("Branches");
    match (&b.branch_error, b.branch_list.is_some()) {
        (Some(e), _) => {
            ui.weak(format!("No branches to show: {}", e.message));
        }
        (None, false) => {
            ui.spinner();
        }
        (None, true) => {
            egui::Grid::new("branch rows")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for row in b.branches() {
                        let label = branch_label(row);
                        let here = row["current"] == true;
                        if here {
                            ui.strong(format!("{label} (checked out)"));
                        } else {
                            ui.label(&label);
                        }
                        ui.label(standing(row));
                        ui.weak(
                            row["upstream"]
                                .as_str()
                                .map(|u| format!("tracks {u}"))
                                .unwrap_or_default(),
                        );
                        ui.horizontal(|ui| {
                            if here {
                                return;
                            }
                            let branch = row["name"].as_str().unwrap_or_default().to_owned();
                            match row["worktree"].as_str() {
                                // Its worktree's row, below, compares with
                                // its working files; checking it out here
                                // would be refused.
                                Some(w) => {
                                    if ui
                                        .button("Open worktree")
                                        .on_hover_text(format!("Open {w} in a new window"))
                                        .clicked()
                                    {
                                        open = Some(w.to_owned());
                                    }
                                }
                                None => {
                                    if ui
                                        .button("Check out")
                                        .on_hover_text(format!(
                                            "Switch this folder to {branch} (git switch)"
                                        ))
                                        .clicked()
                                    {
                                        checkout = Some(branch.clone());
                                    }
                                    let rev = branch_rev(row);
                                    let on = compared.as_deref() == Some(rev.as_str());
                                    if ui
                                        .selectable_label(on, "Compare")
                                        .on_hover_text(format!(
                                            "Compare the working files with {rev}"
                                        ))
                                        .clicked()
                                    {
                                        compare = Some((!on).then_some(rev));
                                    }
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
        }
    }
    ui.separator();
    ui.strong("Worktrees");
    if let Some(e) = &b.error {
        ui.weak(format!("No worktrees to show: {}", e.message));
    } else if b.list.is_none() {
        ui.spinner();
    } else {
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
    }
    if let Some(p) = open {
        ws.open_worktree(&p);
    }
    if let Some(rev) = compare {
        ws.set_compare(rev);
    }
    if let Some(s) = show {
        ws.branches.shown = s;
    }
    if let Some(branch) = checkout {
        ws.ask_checkout(&branch);
    }
}

/// The Check out dialog, placed at the top of `view`: the branch and how
/// it stands, each other participant attached (question 436), why a
/// switch was refused, and Switch to <branch> (needs Direct edit, and the
/// working files in front) or Cancel.
pub fn branch_dialogs(ctx: &egui::Context, ws: &mut Workspace, view: egui::Rect) {
    use crate::core::branches::{attached_lines, refusal};
    use crate::core::modes::Gate;
    let Some(c) = ws.branches.checkout.clone() else {
        return;
    };
    let gate = ws.control("branch.switch");
    let (mut go, mut cancel, mut open) = (false, false, true);
    crate::ui::dialogs::window(&format!("Check out {}", c.branch), view)
        .open(&mut open)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(format!(
                "Switch this folder to {} ({}). Git rewrites the project's files; nothing is committed or stashed.",
                c.branch, c.standing
            ));
            for line in attached_lines(&c) {
                ui.colored_label(ui.visuals().warn_fg_color, line);
            }
            if let Some(e) = &c.refused {
                for line in refusal(e) {
                    ui.colored_label(ui.visuals().error_fg_color, line);
                }
            }
            ui.horizontal(|ui| {
                let can = !c.busy && gate == Gate::Send;
                let r = ui.add_enabled(can, egui::Button::new(format!("Switch to {}", c.branch)));
                let r = match &gate {
                    Gate::Refuse(why) => r.on_disabled_hover_text(*why),
                    _ => r,
                };
                if r.clicked() {
                    go = true;
                }
                if c.busy {
                    ui.spinner();
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if go {
        ws.confirm_checkout();
    }
    if cancel || !open {
        ws.branches.checkout = None;
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
