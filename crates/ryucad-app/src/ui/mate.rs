//! The Mate window (design 374, phase 5): the two picks, the form, its
//! quantities, Flip, the preview's verdict, Add (Suggest in Annotate) and
//! Cancel. Everything it does is a `Workspace` method, so ryucad's tiles
//! can host it unchanged.

use crate::core::mating::{Form, Side};
use crate::core::workspace::Workspace;
use crate::ui::modes;
use eframe::egui;

pub fn window(ctx: &egui::Context, ws: &mut Workspace, at: egui::Rect) {
    let Some(d) = ws.mating.clone() else { return };
    let mut open = true;
    let mut changed = false;
    let mut add = false;
    let mut cancel = false;
    let (mut form, mut angle, mut distance, mut flip) =
        (d.form, d.angle.clone(), d.distance.clone(), d.flip);
    let gate = ws.control("feature.add");
    egui::Window::new("Mate")
        .id(egui::Id::unique("mate-dialog"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(at.right_top() + egui::vec2(-320.0, 40.0))
        .show(ctx, |ui| {
            let side = |ui: &mut egui::Ui, name: &str, pick: &Option<String>, next: bool| {
                ui.horizontal(|ui| {
                    ui.label(name);
                    match pick {
                        Some(r) => ui.monospace(r),
                        None if next => ui.weak("click a face, edge or vertex of an instance"),
                        None => ui.weak("—"),
                    };
                });
            };
            side(ui, "First", &d.a, d.next() == Some(Side::A));
            side(ui, "Second", &d.b, d.next() == Some(Side::B));
            if let Some(why) = &d.note {
                ui.colored_label(ui.visuals().warn_fg_color, why);
            }
            egui::ComboBox::from_label("Form")
                .selected_text(form.name())
                .show_ui(ui, |ui| {
                    for f in Form::ALL {
                        ui.selectable_value(&mut form, f, f.name());
                    }
                });
            let (takes_angle, takes_distance) = form.takes();
            let field = |ui: &mut egui::Ui, name: &str, text: &mut String| {
                ui.horizontal(|ui| {
                    ui.label(name);
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(90.0));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, name)
                    });
                    r.lost_focus()
                })
                .inner
            };
            if takes_angle {
                changed |= field(ui, "Angle", &mut angle);
            }
            if takes_distance {
                changed |= field(ui, "Distance", &mut distance);
            }
            changed |= ui.checkbox(&mut flip, "Flip").changed();
            changed |= form != d.form;
            match (&d.refusal, d.held, d.waiting) {
                (Some(r), _, _) => {
                    ui.colored_label(ui.visuals().warn_fg_color, r);
                }
                (None, _, true) => {
                    ui.weak("solving…");
                }
                (None, Some(true), _) => {
                    ui.label("Holds: shown where it puts them.");
                }
                (None, Some(false), _) => {
                    ui.colored_label(ui.visuals().warn_fg_color, "Doesn't converge from here.");
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                let label = if gate == crate::core::modes::Gate::Propose {
                    "Suggest"
                } else {
                    "Add"
                };
                add = ui
                    .add_enabled_ui(d.addable(), |ui| {
                        modes::gated(ui, &gate, egui::Button::new(label))
                    })
                    .inner
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    if let Some(m) = ws.mating.as_mut() {
        (m.form, m.angle, m.distance, m.flip) = (form, angle, distance, flip);
    }
    if add {
        ws.add_mate();
    } else if cancel || !open {
        ws.close_mate();
    } else if changed {
        ws.preview_mate();
    }
}
