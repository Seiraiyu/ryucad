//! The History tile: the project's Git commits.

use crate::core::workspace::Workspace;
use crate::ui::panels::ago;
use eframe::egui;

pub fn history(ui: &mut egui::Ui, ws: &mut Workspace) {
    if ui.small_button("Refresh").clicked() {
        ws.load_history();
    }
    // Commits made in a terminal change no project file: while the tab
    // shows, it reads the history again now and then.
    let next = ws.history_shown(std::time::Instant::now());
    ui.ctx().request_repaint_after(next);
    let Some(h) = &ws.history else {
        ui.spinner();
        return;
    };
    if let Some(e) = h["error"].as_str() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Can't read the history: {e}"),
        );
        return;
    }
    if h["repository"] == false {
        ui.weak("This project isn't in a Git repository, so it has no history");
        return;
    }
    let compared = ws.compare.as_ref().map(|c| c.rev.clone());
    let mut pick = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let uncommitted = h["uncommitted"].as_array().map_or(0, Vec::len);
            if uncommitted > 0 {
                ui.label(format!("{uncommitted} uncommitted change(s)"));
            }
            egui::Grid::new("history")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for c in h["commits"].as_array().into_iter().flatten() {
                        let id = c["id"].as_str().unwrap_or_default();
                        ui.monospace(&id[..id.len().min(10)]);
                        ui.label(c["author"].as_str().unwrap_or_default());
                        ui.weak(ago(c["time"].as_u64().unwrap_or_default() * 1000));
                        ui.label(c["subject"].as_str().unwrap_or_default());
                        let on = compared.as_deref() == Some(id);
                        if ui
                            .selectable_label(on, "Compare")
                            .on_hover_text("Compare the working files with this commit")
                            .clicked()
                        {
                            pick = Some((!on).then(|| id.to_owned()));
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(rev) = pick {
        ws.set_compare(rev);
    }
}
