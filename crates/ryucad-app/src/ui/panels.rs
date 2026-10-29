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

/// The connection, what agents are doing, and the model's state.
pub fn status(ui: &mut egui::Ui, ws: &Workspace) {
    ui.horizontal(|ui| {
        if ws.lost {
            let why = ws
                .lost_why
                .as_ref()
                .map_or(String::new(), |e| format!(": {}", e.message));
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Reconnecting to the daemon{why}"),
            );
        } else {
            ui.label("Connected");
        }
        ui.separator();
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
        if crate::ui::platform::software() {
            ui.separator();
            ui.weak("Software rendering");
        }
        if let Some(m) = &ws.merge {
            ui.separator();
            ui.colored_label(ui.visuals().warn_fg_color, m.summary());
        }
        if let Some(n) = &ws.drag_note {
            ui.separator();
            ui.colored_label(ui.visuals().warn_fg_color, n);
        }
        if let Some(e) = ws.shown_error(std::time::Instant::now()) {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, &e.message);
        }
    });
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
