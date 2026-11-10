//! The Annotations tile (design 374, In the app): the project's
//! annotations, open ones first, each with its author's colour, number,
//! comment, pins (Show selects and frames them), status, outdated and
//! stale marks, replies and resolution; filtered by status, mine and
//! part. Choosing one restores its view and compares against its change.
//! Reply in Annotate or Direct edit; Edit and Delete for its author while
//! it is open and unclaimed; Apply in Direct edit.

use crate::core::annotations::{Entry, Show};
use crate::core::modes::Mode;
use crate::core::workspace::Workspace;
use crate::ui::icon::Icon;
use crate::ui::modes;
use crate::ui::style;
use crate::ui::view::View;
use eframe::egui::{self, RichText};

pub fn annotations(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    filters(ui, ws);
    ui.separator();
    let me = ws.my_name().to_owned();
    let numbers = ws.annotations.numbers();
    let shown: Vec<Entry> = ws
        .annotations
        .shown(&ws.annotation_filter, &me)
        .into_iter()
        .cloned()
        .collect();
    if shown.is_empty() {
        super::empty(
            ui,
            Icon::CHAT_CIRCLE_TEXT,
            "No annotations",
            "In Annotate, right-click the model and choose Annotate… to ask the agent for a change.",
            Some("ryucad annotations list --open"),
            |_| {},
        );
    }
    for e in &shown {
        one(ui, ws, view, e, numbers.get(&e.id).copied(), &me);
        ui.separator();
    }
}

fn filters(ui: &mut egui::Ui, ws: &mut Workspace) {
    ui.horizontal(|ui| {
        let f = &mut ws.annotation_filter;
        egui::ComboBox::from_label("Show")
            .selected_text(match f.show {
                Show::Open => "Open",
                Show::All => "All",
                Show::Done => "Done",
                Show::Rejected => "Rejected",
            })
            .show_ui(ui, |ui| {
                for (s, t) in [
                    (Show::Open, "Open"),
                    (Show::All, "All"),
                    (Show::Done, "Done"),
                    (Show::Rejected, "Rejected"),
                ] {
                    ui.selectable_value(&mut f.show, s, t);
                }
            });
        ui.checkbox(&mut f.mine, "Mine");
        let parts: Vec<String> = ws.model.listed.keys().cloned().collect();
        egui::ComboBox::from_label("Part")
            .selected_text(f.part.clone().unwrap_or_else(|| "Any".into()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut f.part, None, "Any");
                for p in parts {
                    ui.selectable_value(&mut f.part, Some(p.clone()), p);
                }
            });
    });
}

fn one(
    ui: &mut egui::Ui,
    ws: &mut Workspace,
    view: &mut View,
    e: &Entry,
    number: Option<usize>,
    me: &str,
) {
    let chosen = ws.chosen_annotation.as_deref() == Some(e.id.as_str());
    let colour = style::color32(style::participant(&e.by));
    ui.horizontal_wrapped(|ui| {
        let tag = number.map_or_else(|| "·".to_owned(), |n| format!("#{n}"));
        if ui
            .selectable_label(chosen, RichText::new(tag).strong().color(colour))
            .clicked()
        {
            ws.choose_annotation((!chosen).then_some(e.id.as_str()));
        }
        ui.label(RichText::new(&e.by).color(colour));
        ui.weak(e.status.replace('_', " "));
        if let Some(c) = &e.claimed_by {
            ui.weak(format!("claimed by {c}"));
        }
        if !e.outdated.is_empty() || !e.outdated_marks.is_empty() {
            let why: Vec<String> = e
                .outdated
                .iter()
                .chain(&e.outdated_marks)
                .cloned()
                .collect();
            ui.label(
                RichText::new("outdated").color(style::color32(ryucad::render::COMPARE_CHANGED)),
            )
            .on_hover_text(why.join("\n"));
        }
        // Redline marks (design 423): a badge and how many.
        let marks = e.mark_count();
        if marks > 0 {
            let text = if marks == 1 {
                "1 mark".to_owned()
            } else {
                format!("{marks} marks")
            };
            crate::ui::icon::show(ui, Icon::PENCIL_LINE, 14.0, colour)
                .on_hover_text("Redline marks");
            ui.label(RichText::new(text).color(colour))
                .on_hover_text("Choosing it restores its view and draws its marks.");
        }
        if e.stale {
            ui.label("stale")
                .on_hover_text("A file its change touches has moved since it was suggested.");
        }
        if let Some(m) = &e.change {
            let what = if e.applied {
                format!("suggests {m} (applied)")
            } else {
                format!("suggests {m}")
            };
            ui.weak(what);
        }
    });
    let editing = ws
        .annotation_ui
        .editing
        .as_ref()
        .filter(|(id, _)| *id == e.id)
        .cloned();
    match editing {
        Some((id, mut text)) => {
            ui.text_edit_multiline(&mut text);
            ui.horizontal(|ui| {
                if modes::button(ui, ws, "annotation.edit", "Save").clicked() {
                    ws.edit_annotation(&id, text.clone());
                    ws.annotation_ui.editing = None;
                } else if ui.button("Cancel").clicked() {
                    ws.annotation_ui.editing = None;
                } else {
                    ws.annotation_ui.editing = Some((id, text));
                }
            });
        }
        None => {
            ui.label(&e.comment);
        }
    }
    if !e.pins.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for p in &e.pins {
                let text = p
                    .reference
                    .clone()
                    .unwrap_or_else(|| format!("a point on {}", p.part));
                ui.label(RichText::new(&text).monospace().small());
                if let Some(r) = &p.reference
                    && ui.small_button("Show").clicked()
                {
                    ws.select(vec![r.clone()], ryucad::live::SelectMode::Replace);
                    view.fit_entities(ws, std::slice::from_ref(r));
                }
            }
        });
    }
    // What its marks touch, each a chip with Show, as pins are.
    let touches: Vec<String> = e
        .marks
        .iter()
        .flat_map(|m| m.list.iter())
        .flat_map(|m| m.touches.iter().flatten())
        .map(|t| t.reference.clone())
        .fold(Vec::new(), |mut all, r| {
            if !all.contains(&r) {
                all.push(r);
            }
            all
        });
    if !touches.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.weak("marks touch");
            for r in &touches {
                ui.label(RichText::new(r).monospace().small());
                if ui.small_button("Show").clicked() {
                    ws.select(vec![r.clone()], ryucad::live::SelectMode::Replace);
                    view.fit_entities(ws, std::slice::from_ref(r));
                }
            }
        });
    }
    for r in &e.replies {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&r.by).color(style::color32(style::participant(&r.by))));
            ui.label(&r.text);
        });
    }
    if let Some(r) = &e.resolution {
        ui.weak(format!(
            "{} by {}{}",
            if e.status == "done" {
                "Resolved"
            } else {
                "Rejected"
            },
            r.by,
            r.note
                .as_ref()
                .map(|n| format!(": {n}"))
                .unwrap_or_default()
        ));
    }
    let open_mine = e.is_open() && e.by == me && e.claimed_by.is_none();
    ui.horizontal(|ui| {
        if e.is_open() {
            let typed = ws.annotation_ui.replies.entry(e.id.clone()).or_default();
            ui.add(egui::TextEdit::singleline(typed).hint_text("Reply"));
            let text = typed.clone();
            if modes::button(ui, ws, "annotation.reply", "Reply").clicked()
                && !text.trim().is_empty()
            {
                ws.reply_annotation(&e.id, text);
                ws.annotation_ui.replies.remove(&e.id);
            }
        }
        if open_mine
            && ws.annotation_ui.editing.is_none()
            && modes::button(ui, ws, "annotation.edit", "Edit").clicked()
        {
            // With marks, the Markup bar reopens at its view with them.
            if e.mark_count() > 0 {
                if let Err(why) = ws.edit_markup(&e.id) {
                    ws.drag_note = Some(why);
                }
            } else {
                ws.annotation_ui.editing = Some((e.id.clone(), e.comment.clone()));
            }
        }
        if (open_mine || !e.is_open())
            && modes::button(ui, ws, "annotation.delete", "Delete").clicked()
        {
            ws.delete_annotation(&e.id);
        }
        if ws.mode == Mode::Edit
            && e.change.is_some()
            && !e.applied
            && e.is_open()
            && ui.button("Apply").clicked()
        {
            ws.apply_annotation(&e.id);
        }
    });
}
