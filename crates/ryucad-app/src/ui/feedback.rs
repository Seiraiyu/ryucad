//! Feedback (design 199), drawn as `design/app/feedback.html` and
//! `progress.html` have it: the window's long request in the status line
//! with its bar and Cancel, toasts at the bottom right, the error card
//! opened from the status line or a toast, and the bar that says the
//! daemon is lost. What they say is `core::progress`, `core::toast` and
//! `core::feedback`'s.

use crate::core::feedback::Card;
pub use crate::core::feedback::title;
use crate::core::toast::{Action, Kind, Toast};
use crate::core::workspace::Workspace;
use crate::ui::app::Opened;
use crate::ui::icon::{self, Icon};
use crate::ui::style::{self, Look, Palette};
use eframe::egui::{self, Color32, RichText};
use std::time::Instant;

/// The palette of `look` in `ui`'s theme.
pub fn palette(ui: &egui::Ui, look: Look) -> Palette {
    look.palette(ui.visuals().dark_mode)
}

/// A monospace chip: a reference, a code.
pub fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    egui::Frame::new()
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).monospace().size(12.0))
        })
        .inner
}

/// A link in the accent colour, named for screen readers.
fn link(ui: &mut egui::Ui, text: &str, p: &Palette) -> egui::Response {
    ui.add(
        egui::Label::new(RichText::new(text).color(style::rgb(p.accent)))
            .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The window's long request at the status line's left: `Rebuilding
/// bracket`, its bar, `12 / 40 features · fillet1` and Cancel. Nothing
/// until it has run [`crate::core::progress::SHOW_AFTER`].
pub fn progress(ui: &mut egui::Ui, ws: &mut Workspace, p: &Palette) -> bool {
    let now = Instant::now();
    let Some(m) = ws.progress.shown(now).cloned() else {
        return false;
    };
    icon::show(ui, Icon::SPINNER_GAP, 14.0, style::rgb(p.accent));
    ui.label(RichText::new(&m.label).color(style::rgb(p.text)));
    let bar = egui::ProgressBar::new(m.running.fraction().unwrap_or(0.0))
        .desired_width(160.0)
        .desired_height(6.0)
        .fill(style::rgb(p.accent));
    ui.add(bar);
    let mut count = m.running.count();
    if !m.running.item.is_empty() {
        let item = m.running.item.rsplit('/').next().unwrap_or_default();
        count = format!("{count} · {item}");
    }
    ui.weak(count);
    let label = if m.cancelling {
        "Cancelling…"
    } else {
        "Cancel"
    };
    if ui
        .add_enabled(!m.cancelling, egui::Button::new(label).small())
        .clicked()
    {
        ws.cancel();
    }
    true
}

/// At the status line's right: what the window's requests wait for while
/// another participant's long request runs.
pub fn waiting(ui: &mut egui::Ui, ws: &Workspace) {
    if ws.idle() {
        return;
    }
    let now = Instant::now();
    let Some((who, r)) = ws.progress.blocking(now) else {
        return;
    };
    let name = ws.people.get(who).map_or(who, |p| p.name.as_str());
    ui.horizontal(|ui| {
        icon::show(ui, Icon::CLOCK, 12.0, ui.visuals().weak_text_color());
        ui.weak(format!("waiting for {name}'s {}", r.method));
    });
}

/// Toasts at the window's bottom right, above the status line, newest
/// last; their links act on `o`.
pub fn toasts(ctx: &egui::Context, o: &mut Opened) {
    let shown: Vec<Toast> = o.ws.toasts.shown().to_vec();
    if shown.is_empty() {
        return;
    }
    let p = o.view.look.palette(ctx.theme() == egui::Theme::Dark);
    let now = Instant::now();
    let mut acted = Vec::new();
    egui::Area::new(egui::Id::unique("toasts"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -34.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_width(340.0);
            for t in &shown {
                let stripe = match &t.kind {
                    Kind::Error => style::rgb(p.danger),
                    Kind::Change { participant, .. } => {
                        style::color32(style::participant(participant))
                    }
                    Kind::Note => style::rgb(p.accent),
                };
                let r = egui::Frame::new()
                    .fill(style::rgb(p.raised))
                    .stroke(egui::Stroke::new(1.0, style::rgb(p.border_strong)))
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 8,
                        bottom: 8,
                    })
                    .show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.horizontal_top(|ui| {
                            let (glyph, colour) = match &t.kind {
                                Kind::Error => (Icon::X_CIRCLE, style::rgb(p.danger)),
                                Kind::Change { agent: true, .. } => {
                                    (Icon::ROBOT, style::rgb(p.accent))
                                }
                                Kind::Change { agent: false, .. } => {
                                    (Icon::USER, ui.visuals().text_color())
                                }
                                Kind::Note => (Icon::PLUGS_CONNECTED, ui.visuals().text_color()),
                            };
                            icon::show(ui, glyph, 18.0, colour);
                            ui.vertical(|ui| {
                                ui.set_width(270.0);
                                ui.horizontal_wrapped(|ui| match &t.who {
                                    Some(who) => {
                                        ui.label(RichText::new(who).strong());
                                        ui.label(&t.text);
                                    }
                                    None => {
                                        ui.label(RichText::new(&t.text).strong());
                                    }
                                });
                                if let Some(d) = &t.detail {
                                    ui.weak(d);
                                }
                                ui.horizontal(|ui| {
                                    for a in &t.actions {
                                        if link(ui, a.title(), &p).clicked() {
                                            acted.push((t.id, Some(a.clone())));
                                        }
                                    }
                                });
                            });
                            let close =
                                icon::button(ui, Icon::X, 12.0, "Close", "Close", false, Ok(()));
                            if close.clicked() {
                                acted.push((t.id, None));
                            }
                        });
                    });
                let rect = r.response.rect;
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                    0.0,
                    stripe,
                );
                if r.response.contains_pointer() {
                    o.ws.toasts.hover(t.id, now);
                }
                ui.add_space(8.0);
            }
        });
    for (id, a) in acted {
        o.ws.toasts.close(id);
        match a {
            None => {}
            Some(Action::Show(refs)) => show(o, &refs),
            Some(Action::Undo(seqs)) => {
                for s in seqs {
                    o.ws.undo(Some(s));
                }
            }
            Some(Action::Details(i)) => o.ws.card = Some(i),
        }
    }
}

/// Show: an entity is selected and framed; a feature is chosen and
/// framed, as the tree chooses it.
pub fn show(o: &mut Opened, refs: &[String]) {
    let (entities, features): (Vec<&String>, Vec<&String>) =
        refs.iter().partition(|r| r.split('/').count() >= 3);
    if !entities.is_empty() {
        let entities: Vec<String> = entities.into_iter().cloned().collect();
        o.ws.select(entities.clone(), ryucad::live::SelectMode::Replace);
        o.view.fit_entities(&mut o.ws, &entities);
        return;
    }
    let keys: Vec<(String, String)> = features
        .iter()
        .filter_map(|r| r.split_once('/'))
        .map(|(p, f)| (p.to_owned(), f.to_owned()))
        .collect();
    let Some(first) = keys.first().cloned() else {
        return;
    };
    o.view.chosen = Some(first.clone());
    o.ws.open_form(Some(first.clone()));
    o.ws.choose(Some(first));
    let bodies: Vec<_> = keys.iter().flat_map(|k| o.ws.model.framed(k)).collect();
    o.view.fit_all(&mut o.ws, &bodies);
}

/// The open error card, at the bottom left above the status line: its
/// title and code, message, facts, entity chips with Show, what happened
/// to a gone name, Copy details and Close.
pub fn card(ctx: &egui::Context, o: &mut Opened) {
    let Some(e) = o.ws.card.and_then(|i| o.ws.errors.get(i)) else {
        o.ws.card = None;
        return;
    };
    let c = Card::of(e);
    let p = o.view.look.palette(ctx.theme() == egui::Theme::Dark);
    let (mut close, mut go) = (false, None);
    egui::Area::new(egui::Id::unique("error card"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -30.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(style::rgb(p.raised))
                .stroke(egui::Stroke::new(1.0, style::rgb(p.border_strong)))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(516.0);
                    ui.horizontal(|ui| {
                        icon::show(ui, Icon::X_CIRCLE, 18.0, style::rgb(p.danger));
                        ui.label(RichText::new(&c.title).strong().color(style::rgb(p.danger)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon::button(ui, Icon::X, 14.0, "Close", "Close", false, Ok(()))
                                .clicked()
                            {
                                close = true;
                            }
                            chip(ui, &c.code);
                        });
                    });
                    ui.label(&c.message);
                    if !c.facts.is_empty() {
                        ui.weak(c.facts.join(" · "));
                    }
                    for (i, chipped) in c.chips.iter().enumerate() {
                        ui.horizontal(|ui| {
                            chip(ui, &chipped.reference);
                            if !chipped.what.is_empty() {
                                ui.weak(&chipped.what);
                            }
                            if chipped.shows
                                && link(ui, "Show", &p)
                                    .on_hover_text(format!("Show {}", chipped.reference))
                                    .clicked()
                            {
                                go = Some(i);
                            }
                        });
                    }
                    if let Some(h) = &c.history {
                        ui.label(h);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                        if ui.button("Copy details").clicked() {
                            ui.ctx().copy_text(c.json.clone());
                        }
                    });
                });
        });
    if let Some(i) = go {
        show(o, &[c.chips[i].reference.clone()]);
    }
    if close {
        o.ws.card = None;
    }
}

/// The bar across the window while the daemon is lost: why, when the next
/// try is, and Retry now. Every tile but the viewport is disabled
/// meanwhile (`ui::tiles::show`).
pub fn disconnected(ui: &mut egui::Ui, ws: &mut Workspace, p: &Palette) {
    let why = ws
        .lost_why
        .as_ref()
        .map_or("the daemon stopped answering".to_owned(), |e| {
            e.message.clone()
        });
    let next = ws.retry_at().map(|at| {
        let s = at.saturating_duration_since(Instant::now()).as_secs_f32();
        format!("trying again in {s:.0} s")
    });
    ui.horizontal(|ui| {
        icon::show(ui, Icon::PLUGS, 14.0, style::rgb(p.danger));
        ui.label(RichText::new("Lost the daemon").strong());
        ui.label(why);
        if let Some(n) = next {
            ui.weak(n);
        }
        if ui.button("Retry now").clicked() {
            ws.retry_now();
        }
    });
}

/// The disconnected bar's background: the danger colour over the raised
/// surface at 14%, as `app.css`'s `.disconnected` mixes it.
pub fn disconnected_fill(p: &Palette) -> Color32 {
    let (d, r) = (style::rgb(p.danger), style::rgb(p.raised));
    let mix = |a: u8, b: u8| ((f32::from(a) * 0.14) + (f32::from(b) * 0.86)).round() as u8;
    Color32::from_rgb(mix(d.r(), r.r()), mix(d.g(), r.g()), mix(d.b(), r.b()))
}
