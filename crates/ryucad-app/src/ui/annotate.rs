//! The comment box (design 374, Making one): Annotate… on an entity or a
//! row, Annotate here on the background. A comment pinned to what was
//! right-clicked, with the camera kept, written at once.

use crate::core::workspace::Workspace;
use crate::ui::modes;
use crate::ui::view::View;
use eframe::egui;

pub fn compose(ctx: &egui::Context, view: &mut View, ws: &mut Workspace, at: egui::Rect) {
    let Some(c) = &mut view.composing else { return };
    let mut open = true;
    let mut send = false;
    let mut cancel = false;
    egui::Window::new("Annotate")
        .id(egui::Id::new("annotate-compose"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(at.center())
        .show(ctx, |ui| {
            for p in &c.pins {
                let t = p.reference.clone().unwrap_or_else(|| "a point".into());
                ui.label(egui::RichText::new(t).monospace().small());
            }
            ui.add(
                egui::TextEdit::multiline(&mut c.text)
                    .hint_text("What should change here?")
                    .desired_rows(3),
            );
            ui.horizontal(|ui| {
                let ok = !c.text.trim().is_empty();
                let gate = ws.control("annotation.create");
                let b = ui
                    .add_enabled_ui(ok, |ui| {
                        modes::gated(ui, &gate, egui::Button::new("Annotate"))
                    })
                    .inner;
                send = b.clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    if send {
        let c = view.composing.take().expect("open");
        ws.create_annotation(c.pins, c.text.trim().to_owned(), c.camera);
    } else if cancel || !open {
        view.composing = None;
    }
}
