//! The Explode tile (`explode.edit`): the shown assembly's exploded
//! views, one chosen to show, a slider playing it from assembled to
//! exploded, its steps (each can be taken out), and a new step made by
//! dragging the selected occurrences along an axis in the viewport.

use crate::core::workspace::Workspace;
use eframe::egui;

/// Draws the tile.
pub fn explode(ui: &mut egui::Ui, ws: &mut Workspace) {
    let Some(part) = ws.explodes.part.clone().or_else(|| ws.shown_assembly()) else {
        ui.weak("Show an assembly to explode it (View > Show).");
        return;
    };
    if ws.explodes.part.as_deref() != Some(part.as_str()) {
        ws.explodes.part = Some(part.clone());
        ws.read_explodes();
    }
    ui.label(format!("Exploded views of {part}"));
    let shown = ws.explodes.current(&part).map(|(e, f)| (e.id.clone(), f));
    let mut choose = None;
    for e in &ws.explodes.list {
        let on = shown.as_ref().is_some_and(|(id, _)| *id == e.id);
        let text = match &e.error {
            Some(_) => format!("{} (doesn't read)", e.id),
            None => e.id.clone(),
        };
        let r = ui.selectable_label(on, text);
        let r = match &e.error {
            Some(why) => r.on_hover_text(why),
            None => r.on_hover_text("Show it"),
        };
        if r.clicked() && e.error.is_none() {
            choose = Some(e.id.clone());
        }
    }
    if ws.explodes.list.is_empty() {
        ui.weak("None yet: select occurrences and add a step.");
    }
    if let Some(id) = choose {
        ws.show_explode(Some(&id), Some(1.0), false);
    }
    if let Some((id, mut f)) = shown {
        let r = ui
            .horizontal(|ui| {
                ui.label("Assembled");
                let r = ui.add(egui::Slider::new(&mut f, 0.0..=1.0).show_value(false));
                ui.label(format!("Exploded ({:.0}%)", f * 100.0));
                r
            })
            .inner;
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "explode fraction"));
        if r.changed() {
            ws.slide_explode(f);
        }
        // Written when let go, or at once when typed.
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            ws.show_explode(None, Some(f), false);
        }
        if ui
            .button("Collapse")
            .on_hover_text("Show it assembled, no explode")
            .clicked()
        {
            ws.show_explode(None, None, true);
        }
        let entry = ws.explodes.list.iter().find(|e| e.id == id).cloned();
        if let Some(e) = entry {
            let mut remove = None;
            for (k, step) in e.steps.as_array().into_iter().flatten().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}. {}", k + 1, describe(step)));
                    if ui
                        .small_button("Remove")
                        .on_hover_text("Take this step out")
                        .clicked()
                    {
                        remove = Some(k);
                    }
                });
            }
            for w in &e.warnings {
                ui.colored_label(ui.visuals().warn_fg_color, w);
            }
            if let Some(k) = remove {
                ws.remove_step(k);
            }
        }
    }
    ui.separator();
    match &ws.explodes.adding {
        Some(a) => {
            ui.label(format!(
                "Moving {}: drag an arrow in the viewport.",
                a.instances.join(", ")
            ));
            if ui.button("Cancel step").clicked() {
                ws.explodes.adding = None;
                ws.sync_explode();
            }
        }
        None => {
            let new = ws.explodes.current(&part).is_none();
            if new {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut ws.explodes.name)
                        .hint_text("new explode's name"),
                );
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "new explode's name")
                });
            }
            let named = !new || !ws.explodes.name.is_empty();
            let selected = ws.myself().is_some_and(|m| {
                m.selection
                    .iter()
                    .any(|r| ws.model.occurrence_path(r).is_some_and(|(a, _)| a == part))
            });
            let b = ui.add_enabled(
                named && selected,
                egui::Button::new("Add step from selection"),
            );
            let b = if !selected {
                b.on_disabled_hover_text("Select occurrences of the assembly first.")
            } else if !named {
                b.on_disabled_hover_text("Name the new explode first.")
            } else {
                b.on_hover_text("Drag the selection along an axis; let go to add the step")
            };
            if b.clicked() {
                ws.start_step();
            }
        }
    }
}

/// A step as the tile lists it: what it moves, then how.
fn describe(step: &serde_json::Value) -> String {
    let list = |v: &serde_json::Value| {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut how = Vec::new();
    if let Some(t) = step.get("translate") {
        how.push(format!("by ({})", list(t)));
    }
    if let Some(r) = step.get("rotate") {
        how.push(format!(
            "turned {} about {}",
            r["angle"].as_str().unwrap_or_default(),
            r["axis"]
        ));
    }
    format!("{}: {}", list(&step["instances"]), how.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn steps_read_as_what_moves_and_how() {
        assert_eq!(
            describe(&json!({"instances": ["p1", "lid"], "translate": ["0 mm", "0 mm", "40 mm"]})),
            "p1, lid: by (0 mm, 0 mm, 40 mm)"
        );
        assert_eq!(
            describe(&json!({"instances": ["base"],
                             "rotate": {"axis": [0, 0, 1], "angle": "90 deg"}})),
            "base: turned 90 deg about [0,0,1]"
        );
    }
}
