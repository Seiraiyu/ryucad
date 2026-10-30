//! The Activity tile: the session's changes, newest first, with Undo and
//! Redo.

use crate::core::workspace::Workspace;
use crate::ui::panels::ago;
use eframe::egui;

pub fn activity(ui: &mut egui::Ui, ws: &mut Workspace) {
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
                        ui.label(ws.activity.describe(e));
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
