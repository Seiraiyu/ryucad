//! What the window draws outside its tiles (the People strip, the
//! compare bar, the status line) and what its tiles share.

use crate::core::model::Part;
use crate::core::workspace::Workspace;
use crate::ui::style;
use eframe::egui::{self, RichText};
use ryucad::live::Kind;

/// An error as one line for people.
pub fn message(e: &serde_json::Value) -> String {
    e["message"].as_str().unwrap_or("failed").to_owned()
}

/// Everyone attached, what they say they are doing, and "Follow" for
/// anyone with a camera.
pub fn people(ui: &mut egui::Ui, ws: &mut Workspace) {
    ui.horizontal_wrapped(|ui| {
        let me = ws.me().to_owned();
        let mut follow = None;
        for p in ws.people.all.values() {
            let colour = if p.id == me {
                style::SELECTED
            } else {
                style::participant(&p.id)
            };
            let kind = match p.kind {
                Kind::Human => "",
                Kind::Agent => " [agent]",
            };
            let you = if p.id == me { " (you)" } else { "" };
            ui.label(RichText::new(format!("{}{kind}{you}", p.name)).color(style::color32(colour)));
            if let Some((text, _)) = &p.status {
                ui.weak(text);
            }
            // A long request of theirs: `check 3/8 parts`.
            if let Some(r) = ws.progress.of(&p.id, std::time::Instant::now()) {
                ui.weak(format!("· {}", r.short()));
            }
            if p.id != me && p.camera.is_some() {
                let on = ws.following.as_deref() == Some(p.id.as_str());
                if ui.selectable_label(on, "Follow").clicked() {
                    follow = Some(if on { None } else { Some(p.id.clone()) });
                }
            }
            ui.separator();
        }
        if let Some(f) = follow {
            ws.following = f;
        }
    });
}

/// A compare: the revision, what differs and the colours that say so,
/// and Stop; or, while one is typed (`typed`), the revision to compare
/// with.
pub fn compare_bar(ui: &mut egui::Ui, ws: &mut Workspace, typed: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        if let Some(text) = typed {
            ui.label("Compare with");
            let edit = egui::TextEdit::singleline(text)
                .hint_text("HEAD, a branch or a commit")
                .desired_width(180.0);
            let r = ui.add(edit);
            r.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "revision to compare with")
            });
            let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Start comparing").clicked() || go) && !text.trim().is_empty() {
                ws.set_compare(Some(text.trim().to_owned()));
                *typed = None;
            } else if ui.button("Cancel").clicked() {
                *typed = None;
            }
            return;
        }
        let Some(c) = &ws.compare else { return };
        let commit = c
            .commit
            .as_deref()
            .map_or(String::new(), |c| format!(" ({})", &c[..c.len().min(10)]));
        ui.label(format!("Comparing with {}{commit}", c.rev));
        ui.separator();
        match &c.error {
            Some(e) => {
                ui.colored_label(ui.visuals().error_fg_color, &e.message);
            }
            None if !c.loaded => {
                ui.spinner();
            }
            None => {
                let (added, removed, changed) = c.counts();
                for (n, what, colour) in [
                    (added, "added", style::NEW),
                    (removed, "removed", style::REMOVED),
                    (changed, "changed", style::CHANGED),
                ] {
                    let text = RichText::new(format!("{n} {what}")).color(style::color32(colour));
                    ui.label(text);
                }
            }
        }
        ui.separator();
        if ui.button("Stop comparing").clicked() {
            ws.set_compare(None);
        }
    });
}

/// The status line, as `design/app/progress.html` and `feedback.html`
/// draw it: at the left the window's long request with its bar and
/// Cancel, else the latest failure's title (click for its card), else the
/// model's state; at the right what the window waits for, the merge, the
/// connection and the look.
pub fn status(ui: &mut egui::Ui, ws: &mut Workspace, look: crate::ui::style::Look) {
    let p = crate::ui::feedback::palette(ui, look);
    ui.horizontal(|ui| {
        let busy = crate::ui::feedback::progress(ui, ws, &p);
        if !busy {
            match ws.shown_error(std::time::Instant::now()) {
                Some(e) => {
                    let title = crate::ui::feedback::title(e.code);
                    let i = ws.errors.len() - 1;
                    ui.horizontal(|ui| {
                        crate::ui::icon::show(
                            ui,
                            crate::ui::icon::Icon::X_CIRCLE,
                            14.0,
                            crate::ui::style::rgb(p.danger),
                        );
                        let r = ui.add(
                            egui::Label::new(
                                egui::RichText::new(title).color(crate::ui::style::rgb(p.danger)),
                            )
                            .sense(egui::Sense::click()),
                        );
                        ui.weak("· click for details");
                        if r.clicked() {
                            ws.card = Some(i);
                        }
                    });
                }
                None => model_state(ui, ws),
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(format!(
                "{} · {}",
                look.title(),
                if ui.visuals().dark_mode {
                    "dark"
                } else {
                    "light"
                }
            ));
            let (glyph, text) = match ws.lost {
                true => (crate::ui::icon::Icon::PLUGS, "reconnecting".to_owned()),
                false => (
                    crate::ui::icon::Icon::PLUGS_CONNECTED,
                    format!("daemon {}", ryucad::proto::VersionInfo::current().ryucad),
                ),
            };
            ui.weak(text);
            crate::ui::icon::show(ui, glyph, 14.0, ui.visuals().weak_text_color());
            crate::ui::feedback::waiting(ui, ws);
            if crate::ui::platform::software() {
                ui.weak("Software rendering");
            }
            if let Some(m) = &ws.merge {
                ui.colored_label(ui.visuals().warn_fg_color, m.summary());
            }
            if let Some(n) = &ws.drag_note {
                ui.colored_label(ui.visuals().warn_fg_color, n);
            }
        });
    });
}

/// The model's state, when nothing else holds the status line's left:
/// what agents are doing, the parts and features, what failed.
fn model_state(ui: &mut egui::Ui, ws: &Workspace) {
    let busy: Vec<String> = ws
        .people
        .others(ws.me())
        .filter_map(|p| p.status.as_ref().map(|(s, _)| format!("{}: {s}", p.name)))
        .collect();
    if !busy.is_empty() {
        ui.label(busy.join(" · "));
        ui.separator();
    }
    // Every part listed, loaded or not.
    let m = &ws.model;
    let loaded_failed = |p: &Part| p.features.iter().filter(|f| f.error.is_some()).count();
    let mut parts = m.parts.len();
    let mut features: usize = m.parts.values().map(|p| p.features.len()).sum();
    let mut failed: usize = m.parts.values().map(loaded_failed).sum();
    let unloaded = m.listed.iter().filter(|(id, _)| !m.parts.contains_key(*id));
    for (_, l) in unloaded {
        parts += 1;
        features += l.features;
        failed += l.failed.unwrap_or(0) as usize;
    }
    ui.label(format!("{parts} part(s), {features} feature(s)"));
    if failed > 0 {
        ui.colored_label(ui.visuals().error_fg_color, format!("{failed} failed"));
    }
    if !ws.idle() {
        ui.spinner();
    }
}

/// How long ago `ms` (Unix milliseconds) was, for people.
pub fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => format!("{s} s ago"),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}
