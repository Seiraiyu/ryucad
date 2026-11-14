//! The Motion tile (`motion.edit`, design 529): the shown assembly's
//! motions, one chosen to show, a slider from start to end with Play and
//! Pause (back and forth) and the frame it is at, each drive's value
//! there, the last sweep's marks on the slider (red at colliding steps,
//! grey at blocked ones, a line at the refined contact), Sweep and its
//! collisions each with Show, and each drive's range and the steps
//! edited through `motion.save`. Creating drives is an agent's work.

use crate::core::motion::{Mark, Range};
use crate::core::workspace::Workspace;
use crate::ui::view::View;
use eframe::egui;
use std::time::Instant;

/// Where fraction `f` sits across the slider's rect.
fn mark_x(r: egui::Rect, f: f64) -> f32 {
    r.min.x + r.width() * f.clamp(0.0, 1.0) as f32
}

/// Draws the tile.
pub fn motion(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let save_gate = ws.control("motion.save");
    let Some(part) = ws
        .assembly
        .motions
        .part
        .clone()
        .or_else(|| ws.shown_assembly())
    else {
        ui.weak("Show an assembly to play its motions (View > Show).");
        return;
    };
    if ws.assembly.motions.part.as_deref() != Some(part.as_str()) {
        ws.assembly.motions.part = Some(part.clone());
        ws.read_motions();
    }
    ui.label(format!("Motions of {part}"));
    let shown = ws.assembly.motions.current(&part).map(|e| e.id.clone());
    let mut choose = None;
    for e in &ws.assembly.motions.list {
        let on = shown.as_deref() == Some(e.id.as_str());
        let text = match &e.error {
            Some(_) => format!("{} (doesn't resolve)", e.id),
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
    if ws.assembly.motions.list.is_empty() {
        ui.weak("None yet: an agent saves motions (ryucad motion save).");
    }
    if let Some(id) = choose {
        ws.show_motion(Some(&id), Some(0.0), Some(false), false);
    }
    let Some(id) = shown else {
        return;
    };
    let now = Instant::now();
    let mut f = ws.assembly.motions.fraction(now).unwrap_or(0.0);
    let frames = ws
        .assembly
        .motions
        .frames
        .clone()
        .filter(|_| ws.assembly.motions.frames_current());
    // The slider, with the sweep's marks painted over it.
    let r = ui
        .horizontal_wrapped(|ui| {
            ui.label("Start");
            let r = ui.add(egui::Slider::new(&mut f, 0.0..=1.0).show_value(false));
            ui.label("End");
            r
        })
        .inner;
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "motion fraction"));
    if let Some(w) = ws
        .assembly
        .motions
        .sweep
        .as_ref()
        .filter(|w| w.motion == id && !w.stale)
    {
        let p = ui.painter();
        let (y0, y1) = (r.rect.top(), r.rect.bottom());
        for m in w.marks() {
            let (t, colour, width) = match m {
                Mark::Contact(t) => (t, ui.visuals().strong_text_color(), 2.0),
                Mark::Collision(t) => (t, ui.visuals().error_fg_color, 1.5),
                Mark::Blocked(t) => (t, ui.visuals().weak_text_color(), 1.5),
            };
            let x = mark_x(r.rect, t);
            p.line_segment([egui::pos2(x, y0), egui::pos2(x, y1)], (width, colour));
        }
    }
    if r.changed() {
        ws.slide_motion(f);
    }
    // Written when let go, or at once when typed.
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        ws.show_motion(None, Some(f), Some(false), false);
    }
    ui.horizontal_wrapped(|ui| {
        let playing = ws.assembly.motions.playing();
        if ui
            .button(if playing { "Pause" } else { "Play" })
            .on_hover_text("Play it back and forth, or stop where it is")
            .clicked()
        {
            ws.play_motion(!playing);
        }
        if let Some(fr) = &frames {
            let k = fr.index_at(f);
            ui.label(format!("Frame {} of {}", k + 1, fr.frames.len()));
            if fr.frames.get(k).is_some_and(|x| !x.holds) {
                ui.colored_label(ui.visuals().warn_fg_color, "blocked");
            }
        } else {
            ui.spinner();
        }
        if ui.button("Stop showing").clicked() {
            ws.show_motion(None, None, None, true);
        }
    });
    if let Some(fr) = &frames {
        for line in fr.values_text(f) {
            ui.label(line);
        }
    }
    ui.separator();
    // Sweep: what collides along it, each with Show.
    let mut jump = None;
    ui.horizontal(|ui| {
        if ui
            .button("Sweep")
            .on_hover_text("Check interference at every step of it")
            .clicked()
        {
            ws.sweep_motion();
        }
        if ws
            .assembly
            .motions
            .sweep
            .as_ref()
            .is_some_and(|w| w.motion == id && w.stale)
        {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "The assembly changed since this sweep.",
            );
        }
    });
    if let Some(w) = ws.assembly.motions.sweep.clone().filter(|w| w.motion == id) {
        if !w.loaded {
            ui.spinner();
        } else if let Some(e) = &w.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        } else if w.pairs.is_empty() {
            ui.label("Nothing collides along it.");
        }
        for p in &w.pairs {
            ui.horizontal(|ui| {
                ui.label(format!("{} at {:.1}%", p.label(&part), p.t * 100.0));
                if !w.stale && ui.small_button("Show").clicked() {
                    jump = Some((p.t, p.faces.clone()));
                }
            });
        }
    }
    if let Some((t, faces)) = jump {
        ws.jump_motion(t);
        view.fit_entities(ws, &faces);
    }
    ui.separator();
    // Each drive's range, and the steps, edited.
    let Some(entry) = ws
        .assembly
        .motions
        .list
        .iter()
        .find(|e| e.id == id)
        .cloned()
    else {
        return;
    };
    if ws
        .assembly
        .motions
        .editing
        .as_ref()
        .is_none_or(|(e, ..)| *e != id)
    {
        ws.assembly.motions.editing = Some((
            id.clone(),
            crate::core::motion::ranges(&entry.drives),
            entry.steps.to_string(),
        ));
    }
    let mut save = false;
    if let Some((_, ranges, steps)) = ws.assembly.motions.editing.as_mut() {
        egui::Grid::new("motion-ranges")
            .num_columns(3)
            .show(ui, |ui| {
                for Range {
                    drive,
                    key,
                    from,
                    to,
                } in ranges.iter_mut()
                {
                    ui.label(format!("{}. {key}", *drive + 1));
                    let a = ui.add(egui::TextEdit::singleline(from).desired_width(70.0));
                    a.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            format!("drive {} {key} from", *drive + 1),
                        )
                    });
                    let b = ui.add(egui::TextEdit::singleline(to).desired_width(70.0));
                    b.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            format!("drive {} {key} to", *drive + 1),
                        )
                    });
                    ui.end_row();
                }
                ui.label("steps");
                let s = ui.add(egui::TextEdit::singleline(steps).desired_width(70.0));
                s.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "motion steps")
                });
                ui.end_row();
            });
        save = crate::ui::modes::gated(ui, &save_gate, egui::Button::new("Save motion"))
            .on_hover_text("Write the ranges and steps (motion.save)")
            .clicked();
    }
    for w in &entry.warnings {
        ui.colored_label(ui.visuals().warn_fg_color, w);
    }
    if save {
        ws.save_motion();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_sit_on_the_slider_where_their_fractions_are() {
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(100.0, 20.0));
        assert_eq!(mark_x(r, 0.0), 10.0);
        assert_eq!(mark_x(r, 0.5), 60.0);
        assert_eq!(mark_x(r, 2.0), 110.0);
    }
}
