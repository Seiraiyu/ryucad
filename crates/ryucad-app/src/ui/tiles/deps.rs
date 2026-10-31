//! The Dependencies tile: each dependency and each project the lock
//! reaches through one, what the lock holds of it and which instances
//! are outdated, with Fetch, Update, Remove and Add dependency, each the
//! request an agent sends (design 233, The app).

use crate::core::workspace::Workspace;
use eframe::egui;

pub fn deps(ui: &mut egui::Ui, ws: &mut Workspace) {
    let (mut fetch, mut update, mut remove, mut add, mut confirm, mut cancel) =
        (None, None, None, false, false, false);
    let d = &mut ws.deps;
    if let Some(e) = &d.error {
        ui.colored_label(ui.visuals().error_fg_color, &e.message);
        return;
    }
    ui.horizontal(|ui| {
        let idle = !d.busy;
        if ui
            .add_enabled(idle && !d.rows.is_empty(), egui::Button::new("Fetch all"))
            .on_hover_text("Fetch every dependency's release branch and tags")
            .clicked()
        {
            fetch = Some(Vec::new());
        }
        if ui
            .add_enabled(idle && !d.rows.is_empty(), egui::Button::new("Update all"))
            .on_hover_text("Move every instance to its part's latest release, after a dry run")
            .clicked()
        {
            update = Some((Vec::new(), None));
        }
        if d.busy {
            ui.spinner();
        }
        if let Some(f) = &d.fetched {
            ui.weak(f);
        }
    });
    if let Some(e) = &d.failed {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("{} ({})", e.message, crate::core::deps::code(e)),
        );
    }
    if let Some(f) = &d.lock_fault {
        ui.colored_label(ui.visuals().warn_fg_color, format!("ryucad.lock: {f}"));
    }
    if let Some(u) = &d.update {
        ui.group(|ui| {
            if u.is_empty() {
                ui.label("Everything is at its part's latest release.");
            } else {
                ui.strong("Update would move");
                for m in &u.moved {
                    ui.label(m);
                }
                for f in &u.failing {
                    ui.colored_label(ui.visuals().warn_fg_color, f);
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!u.is_empty(), egui::Button::new("Move them"))
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    }
    if d.rows.is_empty() {
        ui.weak("No dependencies: add another ryucad project's repository below.");
    }
    egui::Grid::new("deps")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            for r in &d.rows {
                ui.strong(r.title())
                    .on_hover_text(format!("{}\n{}", r.project, r.url));
                ui.label(r.state());
                ui.vertical(|ui| {
                    for l in &r.labels {
                        match &l.outdated {
                            Some(_) => ui.colored_label(ui.visuals().warn_fg_color, l.text()),
                            None => ui.label(l.text()),
                        };
                    }
                    for f in &r.faults {
                        ui.colored_label(ui.visuals().error_fg_color, f);
                    }
                });
                ui.horizontal(|ui| {
                    if let Some(name) = &r.name {
                        if ui
                            .add_enabled(!d.busy, egui::Button::new("Fetch"))
                            .on_hover_text(format!("Fetch {name}"))
                            .clicked()
                        {
                            fetch = Some(vec![name.clone()]);
                        }
                        if ui
                            .add_enabled(!d.busy, egui::Button::new("Update"))
                            .on_hover_text(format!(
                                "Move {name}'s instances to their latest releases"
                            ))
                            .clicked()
                        {
                            update = Some((vec![name.clone()], None));
                        }
                        if ui
                            .add_enabled(!d.busy && r.removable(), egui::Button::new("Remove"))
                            .on_hover_text(format!("Take {name} out of the manifest and the lock"))
                            .clicked()
                        {
                            remove = Some(name.clone());
                        }
                    }
                });
                ui.end_row();
            }
        });
    ui.separator();
    ui.strong("Add dependency");
    egui::Grid::new("deps_add").num_columns(2).show(ui, |ui| {
        let name = ui.label("Git URL");
        ui.add(egui::TextEdit::singleline(&mut d.add_url).hint_text("https://…, git@…, file://…"))
            .labelled_by(name.id);
        ui.end_row();
        let name = ui.label("Folder in the repository");
        ui.add(egui::TextEdit::singleline(&mut d.add_path).hint_text("empty for its root"))
            .labelled_by(name.id);
        ui.end_row();
        let name = ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut d.add_name).hint_text("from its folder"))
            .labelled_by(name.id);
        ui.end_row();
    });
    if ui
        .add_enabled(
            !d.busy && !d.add_url.trim().is_empty(),
            egui::Button::new("Add"),
        )
        .clicked()
    {
        add = true;
    }
    if let Some(n) = fetch {
        ws.fetch_deps(n);
    }
    if let Some((n, p)) = update {
        ws.plan_update(n, p);
    }
    if let Some(n) = remove {
        ws.remove_dep(&n);
    }
    if add {
        ws.add_dep();
    }
    if confirm {
        ws.run_update();
    }
    if cancel {
        ws.deps.update = None;
    }
}
