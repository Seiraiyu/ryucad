//! The Markup bar (design 423, Making marks in the app): over the top of
//! the viewport while the tool is open, its tools (Arrow, Line, Ellipse,
//! Rectangle, Pen, Pen on faces, Text), Undo mark, Clear, the comment,
//! Post (Suggest… is the daemon's: an annotation is written in Annotate
//! and Direct edit alike) and Cancel, and what it says: the camera is
//! locked, and why Post was refused. Every button is a registry command
//! the window runs, so the keys and the palette do the same.

use crate::core::command;
use crate::core::markup::Tool;
use crate::core::workspace::Workspace;
use crate::ui::icon::{self, Icon};
use crate::ui::modes;
use crate::ui::view::View;
use eframe::egui;

/// The bar's icon size.
const ICON: f32 = 18.0;

/// Draws the bar while the tool is open, and gives the commands its
/// buttons pressed, for the window to run.
pub fn bar(
    ctx: &egui::Context,
    view: &mut View,
    ws: &mut Workspace,
    at: egui::Rect,
) -> Vec<&'static str> {
    let mut ran = Vec::new();
    let Some(m) = ws.markup.as_ref() else {
        return ran;
    };
    let shown = view.commands.clone();
    let (tool, posting, note, editing) = (m.tool, m.posting, m.note.clone(), m.editing.is_some());
    let mut comment = m.comment.clone();
    let gate = ws.control(if editing {
        "annotation.edit"
    } else {
        "annotation.create"
    });
    egui::Area::new(egui::Id::unique("markup-bar"))
        .fixed_pos(at.center_top() + egui::vec2(0.0, 8.0))
        .pivot(egui::Align2::CENTER_TOP)
        .constrain_to(at)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for t in Tool::ALL {
                        let c = command::get(t.command()).expect("a markup command");
                        let tip = match shown.keys.get(c.id) {
                            Some(k) => format!("{} ({k})", c.title),
                            None => c.title.to_owned(),
                        };
                        let icon = c
                            .icon
                            .and_then(crate::ui::toolbar::icon_of)
                            .unwrap_or(Icon::PENCIL_LINE);
                        if icon::button(ui, icon, ICON, c.title, &tip, tool == t, Ok(())).clicked()
                        {
                            ran.push(c.id);
                        }
                    }
                    ui.separator();
                    if icon::button(
                        ui,
                        Icon::ARROW_COUNTER_CLOCKWISE,
                        ICON,
                        "Undo mark",
                        "Undo mark (Mod+Z)",
                        false,
                        Ok(()),
                    )
                    .clicked()
                    {
                        ran.push("markup.undo");
                    }
                    if ui.button("Clear").clicked()
                        && let Some(m) = ws.markup.as_mut()
                    {
                        m.clear();
                    }
                    ui.separator();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut comment)
                            .hint_text("What should change?")
                            .desired_width(260.0),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Comment")
                    });
                    let post = modes::gated_if(
                        ui,
                        &gate,
                        !posting && !comment.trim().is_empty(),
                        egui::Button::new(if editing { "Save" } else { "Post" }),
                    );
                    if post.clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        ran.push("markup.post");
                    }
                    if ui.button("Cancel").clicked() {
                        ran.push("annotation.markup");
                    }
                });
                ui.horizontal(|ui| {
                    ui.weak("The camera is locked to this view while you mark up.");
                    if let Some(why) = &note {
                        ui.colored_label(ui.visuals().warn_fg_color, why);
                    }
                });
            });
        });
    if let Some(m) = ws.markup.as_mut() {
        m.comment = comment;
    }
    ran
}
