//! The Releases tile: the part's release records and findings, and the
//! dialogs that propose, obsolete and sync tags.

use crate::core::workspace::Workspace;
use crate::ui::panels::message;
use eframe::egui;

/// `part`'s release records, the checks' findings about it, and Propose…,
/// Obsolete… and Sync tags…, each opening a dialog to confirm.
pub fn releases(ui: &mut egui::Ui, ws: &mut Workspace, part: Option<&str>) {
    use crate::core::releases::{Confirm, record_line};
    let Some(part) = part else {
        ui.weak("No parts yet");
        return;
    };
    let badge = ws.states.badge(part);
    let t = &mut ws.releases;
    if t.list.is_none() {
        ui.spinner();
        return;
    }
    let mut dry = false;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("Releases of {part}"));
                if let Some(b) = &badge {
                    ui.weak(b);
                }
            });
            if !t.in_repository() {
                ui.weak("This project isn't in a Git repository: no record can become official.");
            }
            let records = t.records(part);
            if records.is_empty() {
                ui.weak("No release records yet");
            }
            for r in records {
                ui.label(record_line(r));
            }
            let obsoletable = t.obsoletable(part);
            ui.horizontal(|ui| {
                let idle = !t.busy;
                if ui
                    .add_enabled(idle, egui::Button::new("Propose…"))
                    .on_hover_text("Write the record of this part's next revision")
                    .clicked()
                {
                    dry = true;
                }
                let obsolete = egui::Button::new("Obsolete…");
                if ui
                    .add_enabled(idle && obsoletable.is_some(), obsolete)
                    .on_hover_text("Make this part's latest official revision obsolete")
                    .clicked()
                    && let Some(label) = obsoletable.clone()
                {
                    t.note.clear();
                    t.confirm = Some(Confirm::Obsolete {
                        part: part.to_owned(),
                        label,
                    });
                }
                if ui
                    .add_enabled(idle, egui::Button::new("Sync tags…"))
                    .on_hover_text("Create and move the local ryupdm/ tags to match the records")
                    .clicked()
                {
                    t.confirm = Some(Confirm::Sync);
                }
                if t.busy {
                    ui.spinner();
                }
            });
            if let Some(d) = &t.done {
                ui.label(d);
            }
            if let Some(e) = &t.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
            }
            let findings = t.findings_of(part);
            if !findings.is_empty() {
                ui.strong("Checks");
                for f in findings {
                    ui.label(format!(
                        "{} {}: {}",
                        f["severity"].as_str().unwrap_or_default(),
                        f["code"].as_str().unwrap_or_default(),
                        f["message"].as_str().unwrap_or_default()
                    ));
                }
            }
        });
    if dry {
        ws.releases.note.clear();
        ws.propose_dry(part);
    }
}

/// The Releases tab's dialogs, placed at the top of `view`: Propose
/// (the label it takes, every reason it can't yet, a note), Obsolete
/// and Sync tags (what it will write), each confirmed or cancelled.
pub fn release_dialogs(ctx: &egui::Context, ws: &mut Workspace, view: egui::Rect) {
    use crate::core::releases::{Confirm, note};
    let t = &mut ws.releases;
    // What a dialog's button asks for: a proposal with its note, or a
    // confirmed obsolete record or sync.
    let mut act: Option<(Confirm, Option<String>)> = None;
    let mut propose: Option<(String, Option<String>)> = None;
    if let Some((part, dry)) = t.proposal.clone() {
        let mut open = true;
        crate::ui::dialogs::window(&format!("Propose {part}"), view)
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                let label = dry["label"].as_str().unwrap_or("?");
                ui.label(format!("It will be revision {label}."));
                let problems = dry["problems"].as_array().cloned().unwrap_or_default();
                if !problems.is_empty() {
                    ui.label("It can't be proposed yet:");
                    for p in &problems {
                        ui.colored_label(ui.visuals().error_fg_color, message(p));
                    }
                }
                let name = ui.label("Note");
                ui.add(egui::TextEdit::multiline(&mut t.note).desired_rows(2))
                    .labelled_by(name.id);
                ui.horizontal(|ui| {
                    let ready = dry["ready"] == true && !t.busy;
                    if ui
                        .add_enabled(ready, egui::Button::new("Propose"))
                        .clicked()
                    {
                        propose = Some((part.clone(), note(&t.note)));
                    }
                    if ui.button("Cancel").clicked() {
                        t.proposal = None;
                    }
                });
            });
        if !open {
            t.proposal = None;
        }
    }
    if let Some(c) = t.confirm.clone() {
        let title = match &c {
            Confirm::Obsolete { part, label } => format!("Make {part} {label} obsolete"),
            Confirm::Sync => "Sync tags".to_owned(),
        };
        let to_sync = t.to_sync();
        let mut open = true;
        crate::ui::dialogs::window(&title, view)
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                let go = match &c {
                    Confirm::Obsolete { part, label } => {
                        ui.label(format!(
                            "This writes an obsolete record for {part} {label}; it becomes official when the release branch holds it."
                        ));
                        let name = ui.label("Note");
                        ui.add(egui::TextEdit::multiline(&mut t.note).desired_rows(2))
                            .labelled_by(name.id);
                        "Make obsolete"
                    }
                    Confirm::Sync => {
                        if to_sync.is_empty() {
                            ui.label("Every tag is where its release is: nothing to do.");
                        }
                        for (tag, what) in &to_sync {
                            ui.label(format!("{what} {tag}"));
                        }
                        ui.weak("Only local tags: nothing is pushed or deleted.");
                        "Sync"
                    }
                };
                ui.horizontal(|ui| {
                    let can = !t.busy && !(c == Confirm::Sync && to_sync.is_empty());
                    if ui.add_enabled(can, egui::Button::new(go)).clicked() {
                        act = Some((c.clone(), note(&t.note)));
                    }
                    if ui.button("Cancel").clicked() {
                        t.confirm = None;
                    }
                });
            });
        if !open {
            t.confirm = None;
        }
    }
    if let Some((part, n)) = propose {
        ws.propose(&part, n);
    }
    match act {
        Some((Confirm::Obsolete { part, label }, n)) => ws.obsolete(&part, &label, n),
        Some((Confirm::Sync, _)) => ws.sync_tags(),
        None => {}
    }
}
