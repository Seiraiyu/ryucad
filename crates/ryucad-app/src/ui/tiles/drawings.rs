//! The Drawings tile (design 417, The app): the shown part's drawings, a
//! chosen sheet drawn with egui's painter from `drawing.get --items`
//! (curves cut at the screen's chord, text as Inter), pan, zoom and Fit;
//! a click picks a curve's entity (Copy reference, Show in model) or an
//! annotation (its form, for a dimension); a drag moves a view or an
//! annotation in Direct edit, written once when let go; the findings are
//! listed under the sheet, each with Show. Nothing here adds a view or a
//! dimension: those are an agent's.

use crate::core::drawings::{Anchor, Camera, Item, Pick, outline};
use crate::core::workspace::Workspace;
use crate::ui::form;
use crate::ui::icon::Icon;
use crate::ui::view::View;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};

/// How far a click reaches, in screen points.
const REACH: f64 = 6.0;

fn colour([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The drag under way: what it moves and where it started, in sheet mm.
#[derive(Clone, Debug)]
struct Dragging {
    pick: Pick,
    from: [f64; 2],
}

pub fn drawings(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    ws.show_drawings(None);
    let Some(part) = ws.drawings.part.clone() else {
        crate::ui::tiles::empty(
            ui,
            Icon::Glyph(crate::ui::icon::glyphs::Glyph::Drawing),
            "No part shown",
            "Show a part (View > Show) to see its drawings.",
            None,
            |_| {},
        );
        return;
    };
    let list = ws.drawings.list.clone();
    let Some(list) = list else {
        ui.spinner();
        return;
    };
    if list.entries.is_empty() {
        let line = format!("ryucad drawing create {part} main --views standard");
        crate::ui::tiles::empty(
            ui,
            Icon::Glyph(crate::ui::icon::glyphs::Glyph::Drawing),
            &format!("{part} has no drawings"),
            "An agent makes them; they are drawn here.",
            Some(&line),
            |_| {},
        );
        return;
    }
    header(ui, ws, &list);
    let Some(sheet) = ws.drawings.sheet.clone() else {
        if let Some(e) = &ws.drawings.error {
            form::error(ui, e);
        } else {
            ui.spinner();
        }
        return;
    };
    // The sheet takes what the findings and the chosen thing's lines
    // leave.
    let below = 260.0_f32.min(ui.available_height() * 0.45);
    let size = egui::vec2(
        ui.available_width(),
        (ui.available_height() - below).max(120.0),
    );
    let (rect, r) = ui.allocate_exact_size(size, Sense::click_and_drag());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Drawing sheet"));
    let px = [f64::from(rect.width()), f64::from(rect.height())];
    let mut cam = ws
        .drawings
        .camera
        .unwrap_or_else(|| Camera::fit(sheet.paper, px));
    let to_sheet = |cam: &Camera, p: Pos2| {
        cam.to_sheet(
            [f64::from(p.x - rect.left()), f64::from(p.y - rect.top())],
            px,
        )
    };
    // Zoom at the pointer, pan with the middle or right button.
    if r.hovered() {
        let (scroll, at) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if scroll != 0.0
            && let Some(at) = at
        {
            let f = ryu_math::exp(f64::from(scroll) / 200.0);
            cam.zoom_at(
                f,
                [f64::from(at.x - rect.left()), f64::from(at.y - rect.top())],
                px,
            );
        }
    }
    if r.dragged_by(egui::PointerButton::Middle) || r.dragged_by(egui::PointerButton::Secondary)
    {
        let d = r.drag_delta();
        cam.pan([f64::from(d.x), f64::from(d.y)]);
    }
    // A left drag moves what it starts on, in Direct edit; on nothing it
    // pans.
    let id = ui.scope_id().with("drawing drag");
    let mut dragging: Option<Dragging> = ui.data(|d| d.get_temp(id));
    let reach = REACH / cam.zoom;
    // From where the button went down: egui starts a drag only once the
    // pointer has moved past its threshold.
    if r.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = ui
            .input(|i| i.pointer.press_origin())
            .or_else(|| r.interact_pointer_pos())
    {
        let at = to_sheet(&cam, p);
        dragging = crate::core::drawings::pick(&sheet, at, reach).map(|pick| Dragging { pick, from: at });
    }
    let mut moved = [0.0, 0.0];
    if let Some(d) = &dragging
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
    {
        let at = to_sheet(&cam, p);
        moved = [at[0] - d.from[0], at[1] - d.from[1]];
    } else if r.dragged_by(egui::PointerButton::Primary) {
        let d = r.drag_delta();
        cam.pan([f64::from(d.x), f64::from(d.y)]);
    }
    let gate = ws.control("drawing.edit");
    if r.drag_stopped()
        && let Some(d) = dragging.take()
    {
        match gate {
            crate::core::modes::Gate::Refuse(why) => ws.drawings.note = Some(why.to_owned()),
            _ => ws.drag_drawing(&d.pick, moved),
        }
        moved = [0.0, 0.0];
    }
    ui.data_mut(|m| {
        if let Some(d) = &dragging {
            m.insert_temp(id, d.clone());
        } else {
            m.remove::<Dragging>(id);
        }
    });
    if r.clicked()
        && let Some(p) = r.interact_pointer_pos()
    {
        let pick = crate::core::drawings::pick(&sheet, to_sheet(&cam, p), reach);
        ws.drawings.choose(pick);
    }
    if r.double_clicked() {
        cam = Camera::fit(sheet.paper, px);
    }
    ws.drawings.camera = Some(cam);
    // What moves with the drag, and what is chosen.
    let held = dragging.as_ref().and_then(|d| items_of(&sheet, &d.pick));
    let chosen = ws.drawings.selected.as_ref().and_then(|p| match p {
        Pick::Entity { entity, view } => Some(
            sheet
                .view(view)
                .map(|v| v.items.clone())
                .into_iter()
                .flatten()
                .filter(|i| sheet.entities.get(i) == Some(entity))
                .collect::<Vec<usize>>(),
        ),
        p => items_of(&sheet, p).map(|r| r.collect()),
    });
    let accent = ui.visuals().selection.stroke.color;
    let painter = ui.painter_at(rect);
    let screen = |p: [f64; 2], off: [f64; 2]| {
        let q = cam.to_screen([p[0] + off[0], p[1] + off[1]], px);
        Pos2::new(rect.left() + q[0] as f32, rect.top() + q[1] as f32)
    };
    // The paper, on the viewport's grey.
    painter.rect_filled(rect, 0.0, Color32::from_gray(128));
    painter.rect_filled(
        Rect::from_two_pos(screen([0.0, 0.0], [0.0; 2]), screen(sheet.paper, [0.0; 2])),
        0.0,
        Color32::WHITE,
    );
    let chord = cam.chord();
    for (i, it) in sheet.items.iter().enumerate() {
        let off = if held.as_ref().is_some_and(|h| h.contains(&i)) {
            moved
        } else {
            [0.0, 0.0]
        };
        let picked = chosen.as_ref().is_some_and(|c| c.contains(&i));
        paint(&painter, it, chord, cam.zoom, &|p| screen(p, off), picked.then_some(accent));
    }
    if let Some(n) = &ws.drawings.note {
        ui.colored_label(ui.visuals().warn_fg_color, n);
    }
    if let Some(e) = &ws.drawings.error {
        form::error(ui, e);
    }
    egui::ScrollArea::vertical()
        .id_salt("drawing below")
        .auto_shrink(false)
        .show(ui, |ui| {
            chosen_ui(ui, ws, view);
            findings_ui(ui, ws, px);
        });
}

/// The drawing and sheet choosers, Fit, and the sheet's score.
fn header(ui: &mut egui::Ui, ws: &mut Workspace, list: &crate::core::drawings::List) {
    let chosen = ws.drawings.first_choice();
    let mut choose = None;
    ui.horizontal(|ui| {
        let label = ui.label("Drawing");
        let now = chosen.as_ref().map_or(String::new(), |(d, _)| d.clone());
        egui::ComboBox::from_id_salt("drawing choice")
            .selected_text(&now)
            .show_ui(ui, |ui| {
                for e in &list.entries {
                    let text = match &e.error {
                        Some(_) => format!("{} (doesn't read)", e.drawing),
                        None => format!("{} ({})", e.drawing, e.name),
                    };
                    if ui.selectable_label(e.drawing == now, text).clicked()
                        && let Some((k, _)) = e.sheets.first()
                    {
                        choose = Some((e.drawing.clone(), k.clone()));
                    }
                }
            })
            .response
            .labelled_by(label.id);
        if let Some((d, k)) = &chosen
            && let Some(e) = list.entries.iter().find(|e| &e.drawing == d)
        {
            let label = ui.label("Sheet");
            egui::ComboBox::from_id_salt("drawing sheet")
                .selected_text(k)
                .show_ui(ui, |ui| {
                    for (s, name) in &e.sheets {
                        if ui.selectable_label(s == k, format!("{s} ({name})")).clicked() {
                            choose = Some((d.clone(), s.clone()));
                        }
                    }
                })
                .response
                .labelled_by(label.id);
        }
        if ui.button("Fit sheet").clicked() {
            ws.drawings.camera = None;
        }
        if let Some(s) = &ws.drawings.sheet
            && let Some(score) = s.score
        {
            ui.weak(format!("score {score}"));
        }
        if ws.drawings.loading || ws.drawings.saving {
            ui.spinner();
        }
    });
    if let Some((d, k)) = choose {
        ws.choose_drawing(&d, &k);
    }
}

/// The items a pick moves or paints: a view's or an annotation's.
fn items_of(sheet: &crate::core::drawings::Sheet, pick: &Pick) -> Option<std::ops::Range<usize>> {
    match pick {
        Pick::View(v) | Pick::Entity { view: v, .. } => sheet.view(v).map(|v| v.items.clone()),
        Pick::Annotation(a) => sheet.note(a).map(|n| n.items.clone()),
    }
}

/// Paints one item: a path's outline at `chord`, its stroke at least a
/// point wide; text as Inter at its size. `over` paints it chosen.
fn paint(
    painter: &egui::Painter,
    it: &Item,
    chord: f64,
    zoom: f64,
    to: &dyn Fn([f64; 2]) -> Pos2,
    over: Option<Color32>,
) {
    match it {
        Item::Path { stroke, fill, .. } => {
            let pts: Vec<Pos2> = outline(it, chord).into_iter().map(to).collect();
            if let Some(f) = fill
                && pts.len() >= 3
            {
                painter.add(egui::Shape::convex_polygon(
                    pts.clone(),
                    over.unwrap_or(colour(*f)),
                    Stroke::NONE,
                ));
            }
            if let Some((c, w, dash)) = stroke {
                let s = Stroke::new(
                    ((w * zoom) as f32).max(1.0) + if over.is_some() { 1.5 } else { 0.0 },
                    over.unwrap_or(colour(*c)),
                );
                if dash.len() >= 2 && pts.len() >= 2 {
                    let d = ((dash[0] * zoom) as f32).max(1.0);
                    let g = ((dash[1] * zoom) as f32).max(1.0);
                    painter.extend(egui::Shape::dashed_line(&pts, s, d, g));
                } else {
                    painter.add(egui::Shape::line(pts, s));
                }
            }
        }
        Item::Text {
            text,
            at,
            size,
            angle,
            anchor,
            colour: c,
        } => {
            let pt = (size * zoom) as f32;
            if pt < 2.0 {
                return;
            }
            let colour = over.unwrap_or(colour(*c));
            let galley =
                painter.layout_no_wrap(text.clone(), egui::FontId::proportional(pt), colour);
            // The baseline at `at`: up by the ascent, along by the anchor,
            // turned about it.
            let w = galley.size().x;
            let dx = match anchor {
                Anchor::Start => 0.0,
                Anchor::Middle => -w / 2.0,
                Anchor::End => -w,
            };
            let rows = galley.rows.first().map_or(pt * 0.8, |r| r.row.size.y * 0.78);
            let a = -*angle as f32;
            let (s, co) = (a.sin(), a.cos());
            let o = to(*at);
            let local = egui::vec2(dx, -rows);
            let pos = o + egui::vec2(co * local.x - s * local.y, s * local.x + co * local.y);
            painter.add(egui::epaint::TextShape::new(pos, galley, colour).with_angle(a));
        }
    }
}

/// What was clicked: an entity with Copy reference and Show in model, an
/// annotation with its text, or a dimension's form.
fn chosen_ui(ui: &mut egui::Ui, ws: &mut Workspace, view: &mut View) {
    let Some(pick) = ws.drawings.selected.clone() else {
        return;
    };
    match &pick {
        Pick::Entity { entity, view: v } => {
            ui.horizontal(|ui| {
                ui.label(format!("{entity} in {v}"));
                if ui.button("Copy reference").clicked() {
                    ui.ctx().copy_text(entity.clone());
                }
                if ui.button("Show in model").clicked() {
                    ws.select(vec![entity.clone()], ryucad::live::SelectMode::Replace);
                    view.fit_entities(ws, std::slice::from_ref(entity));
                }
            });
        }
        Pick::View(v) => {
            ui.label(format!("view {v}"));
        }
        Pick::Annotation(a) => {
            let note = ws.drawings.sheet.as_ref().and_then(|s| s.note(a)).cloned();
            if let Some(n) = &note {
                ui.label(format!("{} {}: {}", n.kind, n.annotation, n.text));
                if let Some(e) = &n.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            }
            let gate = ws.control("drawing.edit");
            let saving = ws.drawings.saving;
            let mut keys = form::Keys::default();
            let mut dirty = false;
            if let Some(f) = &mut ws.drawings.form {
                keys = form::grid(ui, ("dimension form", &f.annotation), |rows| {
                    rows.text("Text", &mut f.text);
                    rows.choice("Tolerance", &mut f.tolerance);
                    if !["none", "basic", "reference"].contains(&f.tolerance.chosen.as_str()) {
                        rows.text("Tolerance value", &mut f.value);
                    }
                    rows.text("Precision", &mut f.precision);
                });
                dirty = f.dirty();
            }
            if ws.drawings.form.is_some() {
                let foot = form::foot(ui, dirty, saving, ("Apply to dimension", "Revert dimension"), &gate);
                if foot.revert || keys.revert {
                    if let Some(f) = &mut ws.drawings.form {
                        f.revert();
                    }
                } else if (foot.apply || (keys.apply && dirty))
                    && !matches!(gate, crate::core::modes::Gate::Refuse(_))
                {
                    ws.save_drawing_dimension();
                }
            }
        }
    }
}

/// The sheet's findings, each with Show: the view or annotation it names
/// chosen and framed.
fn findings_ui(ui: &mut egui::Ui, ws: &mut Workspace, px: [f64; 2]) {
    let Some(sheet) = ws.drawings.sheet.clone() else {
        return;
    };
    if sheet.findings.is_empty() {
        ui.weak("No findings");
        return;
    }
    ui.label(format!("Findings ({})", sheet.findings.len()));
    for f in &sheet.findings {
        ui.horizontal(|ui| {
            let c = match f.severity.as_str() {
                "error" => ui.visuals().error_fg_color,
                _ => ui.visuals().warn_fg_color,
            };
            ui.colored_label(c, &f.code);
            ui.label(&f.message);
            let pick = f
                .annotation
                .clone()
                .map(Pick::Annotation)
                .or_else(|| f.view.clone().map(Pick::View));
            if let Some(p) = pick
                && ui.button("Show").clicked()
            {
                if let Some(b) = items_of(&sheet, &p).and_then(|r| sheet.bounds_of(r)) {
                    let size = [b[1][0] - b[0][0] + 20.0, b[1][1] - b[0][1] + 20.0];
                    let mut cam = Camera::fit(size, px);
                    cam.centre = [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0];
                    ws.drawings.camera = Some(cam);
                }
                ws.drawings.choose(Some(p));
            }
        });
    }
}
