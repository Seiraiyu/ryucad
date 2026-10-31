//! Place from dependency…: a dependency, then one of the parts it has
//! released, then a label; Place adds an instance of it at the
//! assembly's origin (`feature.add`, as an agent places one).

use crate::core::workspace::Workspace;
use eframe::egui;

pub fn picker(ctx: &egui::Context, ws: &mut Workspace, view: egui::Rect) {
    let names: Vec<String> = ws.deps.rows.iter().filter_map(|r| r.name.clone()).collect();
    let Some(p) = ws.picker.as_mut() else {
        return;
    };
    let (mut open, mut dependency, mut place) = (true, None, false);
    crate::ui::dialogs::window(&format!("Place in {}", p.assembly), view)
        .open(&mut open)
        .default_width(460.0)
        .show(ctx, |ui| {
            if names.is_empty() {
                ui.weak("The project has no dependencies: Project > Dependencies adds one.");
                return;
            }
            ui.horizontal(|ui| {
                let name = ui.label("Dependency");
                egui::ComboBox::from_id_salt("picker_dependency")
                    .selected_text(p.dependency.clone().unwrap_or_else(|| "choose…".into()))
                    .show_ui(ui, |ui| {
                        for n in &names {
                            if ui
                                .selectable_label(p.dependency.as_deref() == Some(n), n)
                                .clicked()
                            {
                                dependency = Some(n.clone());
                            }
                        }
                    })
                    .response
                    .labelled_by(name.id);
            });
            if p.reading {
                ui.spinner();
            }
            if p.dependency.is_some() && !p.reading && p.parts.is_empty() && p.error.is_none() {
                ui.weak("It has released nothing on its release branch, as fetched.");
            }
            let mut chose = None;
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for (i, r) in p.parts.iter().enumerate() {
                        if ui.selectable_label(p.chosen == Some(i), r.text()).clicked() {
                            chose = Some(i);
                        }
                    }
                });
            if let Some(i) = chose {
                p.choose(i);
            }
            if let Some(r) = p.part().cloned() {
                ui.horizontal(|ui| {
                    let name = ui.label("Label");
                    egui::ComboBox::from_id_salt("picker_label")
                        .selected_text(p.label.clone().unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for (l, s) in &r.labels {
                                ui.selectable_value(
                                    &mut p.label,
                                    Some(l.clone()),
                                    format!("{l} ({s})"),
                                );
                            }
                        })
                        .response
                        .labelled_by(name.id);
                });
            }
            if let Some(e) = &p.error {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
            }
            let ready = p.label.is_some() && !p.placing;
            if ui.add_enabled(ready, egui::Button::new("Place")).clicked() {
                place = true;
            }
        });
    if !open {
        ws.picker = None;
        return;
    }
    if let Some(n) = dependency {
        ws.pick_dependency(&n);
    }
    if place {
        ws.place_picked();
    }
}
