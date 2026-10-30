//! The Activity tile: the live activity feed (design 199), drawn as
//! `design/app/layout.html`'s bottom tab: runs of one participant's
//! changes, newest first, each headed by who (in their colour), what they
//! say they are doing and their long request's progress; each change its
//! time, what it did, the features it touched as chips with Show, and
//! Undo or Redo where the log allows.

use crate::core::feed::{self, Who};
use crate::ui::app::Opened;
use crate::ui::icon::{self, Icon};
use crate::ui::panels::ago;
use crate::ui::style;
use eframe::egui::{self, RichText};

pub fn activity(ui: &mut egui::Ui, o: &mut Opened) {
    let (mut undo, mut redo, mut show) = (None, None, None);
    let now = std::time::Instant::now();
    let runs = feed::runs(&o.ws.activity.entries, o.ws.feed_only.as_deref());
    // The filter: everyone, or one participant (or outside changes).
    let mut only = o.ws.feed_only.clone();
    ui.horizontal(|ui| {
        ui.weak("Show");
        egui::ComboBox::from_id_salt("feed filter")
            .selected_text(match &only {
                None => "everyone".to_owned(),
                Some(id) if id == "external" => "outside ryucad".to_owned(),
                Some(id) => o.ws.people.get(id).map_or(id.clone(), |p| p.name.clone()),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut only, None, "everyone");
                for p in o.ws.people.all.values() {
                    ui.selectable_value(&mut only, Some(p.id.clone()), &p.name);
                }
                ui.selectable_value(&mut only, Some("external".to_owned()), "outside ryucad");
            });
    });
    o.ws.feed_only = only;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            if runs.is_empty() {
                ui.weak("Nothing has changed since the daemon started");
            }
            for run in &runs {
                ui.horizontal(|ui| {
                    let (glyph, colour) = match &run.who {
                        Who::Agent { id, .. } => {
                            (Icon::ROBOT, style::color32(style::participant(id)))
                        }
                        Who::Human { id, .. } => {
                            (Icon::USER, style::color32(style::participant(id)))
                        }
                        Who::External => (Icon::TERMINAL_WINDOW, ui.visuals().weak_text_color()),
                    };
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, colour);
                    icon::show(ui, glyph, 14.0, ui.visuals().text_color());
                    ui.label(RichText::new(run.who.name()).strong());
                    if let Some(id) = run.who.id() {
                        let said =
                            o.ws.people
                                .get(id)
                                .and_then(|p| p.status.as_ref().map(|(s, _)| s.clone()));
                        let running =
                            o.ws.progress
                                .of(id, now)
                                .map(|r| format!("running {}", r.short()));
                        let words: Vec<String> = [said, running].into_iter().flatten().collect();
                        if !words.is_empty() {
                            ui.weak(words.join(" · "));
                        }
                    }
                });
                for row in &run.rows {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [58.0, 18.0],
                            egui::Label::new(RichText::new(ago(row.time_ms)).weak().size(12.0)),
                        );
                        ui.label(&row.what);
                        for c in &row.chips {
                            let r = crate::ui::feedback::chip(ui, c);
                            if row.features {
                                let r = r
                                    .interact(egui::Sense::click())
                                    .on_hover_text(format!("Show {c}"));
                                if r.clicked() {
                                    show = Some(c.clone());
                                }
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if row.undo && ui.small_button("Undo").clicked() {
                                undo = Some(row.seq);
                            } else if row.redo && ui.small_button("Redo").clicked() {
                                redo = Some(row.seq);
                            }
                        });
                    });
                }
                ui.separator();
            }
        });
    if let Some(s) = undo {
        o.ws.undo(Some(s));
    }
    if let Some(s) = redo {
        o.ws.redo(Some(s));
    }
    if let Some(c) = show {
        crate::ui::feedback::show(o, &[c]);
    }
}
