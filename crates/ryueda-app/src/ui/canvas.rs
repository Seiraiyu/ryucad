//! The canvas tile (design 519, The window): the scene drawn as egui
//! shapes, culled to the view; picking, hover, links; and marks.

use crate::core::geom::{Cut, band, fit};
use crate::core::paint::marks;
use crate::core::pick::pick;
use crate::core::scene::{Anchor, P, Shape};
use crate::core::window::Window;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};
use ryu_app::canvas::{Camera, navigate, visible};
use std::time::Instant;

/// How near a click must come, in screen points (design 519).
pub const REACH: f64 = 6.0;

/// The PDF's layers in `ryusch::paint::Layer::ALL`'s order.
pub const LAYERS: [&str; 4] = ["frames", "notes", "intent", "pin_numbers"];

/// Cut outlines, kept per scene and zoom band.
#[derive(Default)]
pub struct Kept {
    key: Option<(String, i32)>,
    cut: Option<Cut>,
}

fn layer_on(w: &Window, layer: Option<&str>) -> bool {
    layer.is_none_or(|l| {
        LAYERS
            .iter()
            .position(|x| *x == l)
            .is_none_or(|i| w.layers[i])
    })
}

pub fn show(ui: &mut egui::Ui, w: &mut Window, kept: &mut Kept, now: Instant) {
    let size = ui.available_size();
    let (rect, r) = ui.allocate_exact_size(size, Sense::click_and_drag());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Schematic sheet"));
    let px: P = [f64::from(rect.width()), f64::from(rect.height())];
    let Some(sc) = w.scene.as_ref() else {
        ui.painter_at(rect)
            .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        ui.put(
            rect,
            egui::Label::new(if w.lost {
                "Lost the daemon: reconnecting"
            } else {
                "Drawing the sheet…"
            }),
        );
        return;
    };
    let mut cam = w.camera.unwrap_or_else(|| fit(sc.paper.rect, px));
    if let Some(b) = w.frame.take() {
        let g = 4.0 * 1.27;
        cam = fit([[b[0][0] - g, b[0][1] - g], [b[1][0] + g, b[1][1] + g]], px);
    }
    navigate(ui, &r, rect, &mut cam);
    let to_sheet = |cam: &Camera, p: Pos2| {
        cam.to_sheet(
            [f64::from(p.x - rect.left()), f64::from(p.y - rect.top())],
            px,
        )
    };
    let reach = REACH / cam.zoom;
    let alt = ui.input(|i| i.modifiers.alt);
    // Hover and its tooltip.
    let hovered = r
        .hover_pos()
        .and_then(|p| pick(sc, to_sheet(&cam, p), reach, alt));
    let mut clicked: Option<(Option<String>, bool)> = None;
    let mut link: Option<String> = None;
    if r.double_clicked() {
        let at = r.interact_pointer_pos().map(|p| to_sheet(&cam, p));
        let under = at.and_then(|p| pick(sc, p, reach, true));
        let kind = under.as_ref().map(|u| w.kind_of(u));
        match (under, kind.as_deref()) {
            (Some(net), Some("net")) => link = Some(net),
            _ => cam = fit(sc.paper.rect, px),
        }
    } else if r.clicked() {
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let p = r.interact_pointer_pos().map(|p| to_sheet(&cam, p));
        clicked = Some((p.and_then(|p| pick(sc, p, reach, alt)), add));
    }
    if let Some(h) = &hovered {
        let pin = sc.pins.iter().find(|p| &p.reference == h);
        let tip = match pin {
            Some(p) => format!(
                "{}\n{} {} ({})\nnet {}",
                p.reference,
                p.number,
                p.name,
                p.kind,
                p.net.as_deref().unwrap_or("none")
            ),
            None => h.clone(),
        };
        r.clone().on_hover_text(tip);
    }
    // The cut outlines for this scene and band.
    let key = (
        format!(
            "{}\u{0}{}\u{0}{:?}\u{0}{}",
            sc.revision, sc.sheet, sc.layer, sc.theme
        ),
        band(cam.zoom),
    );
    if kept.key.as_ref() != Some(&key) {
        kept.cut = Some(Cut::of(sc, cam.zoom));
        kept.key = Some(key);
    }
    let cut = kept.cut.as_ref().expect("just cut");
    let m = marks(
        sc,
        w.me(),
        &w.selection,
        hovered.as_deref(),
        &w.people,
        &w.flash,
        now,
    );
    let painter = ui.painter_at(rect);
    let screen = |p: P| {
        let q = cam.to_screen(p, px);
        Pos2::new(rect.left() + q[0] as f32, rect.top() + q[1] as f32)
    };
    let zoom = cam.zoom as f32;
    painter.rect_filled(rect, 0.0, sc.paper.canvas);
    painter.rect_filled(
        Rect::from_two_pos(screen(sc.paper.rect[0]), screen(sc.paper.rect[1])),
        0.0,
        sc.paper.colour,
    );
    let view = visible(&cam, px);
    for i in cut.visible(view) {
        let it = &sc.items[i];
        if !layer_on(w, it.layer.as_deref()) {
            continue;
        }
        let over = it
            .owner
            .and_then(|o| m.owners.get(o).copied().flatten())
            .map(|x| x.fill);
        let pts: Vec<Pos2> = cut.lines[i].iter().map(|p| screen(*p)).collect();
        let pen = |c: Color32, width: f64| {
            Stroke::new(
                ((width as f32) * zoom).max(1.0) + if over.is_some() { 1.5 } else { 0.0 },
                over.unwrap_or(c),
            )
        };
        match &it.shape {
            Shape::Polyline {
                stroke,
                fill,
                width,
                dash,
                ..
            } => {
                if let Some(f) = fill
                    && pts.len() >= 3
                {
                    painter.add(egui::Shape::convex_polygon(
                        pts.clone(),
                        over.unwrap_or(*f),
                        Stroke::NONE,
                    ));
                }
                if *dash {
                    painter.extend(egui::Shape::dashed_line(
                        &pts,
                        pen(*stroke, *width),
                        1.0f32.max(zoom),
                        0.5f32.max(zoom * 0.5),
                    ));
                } else {
                    painter.add(egui::Shape::line(pts, pen(*stroke, *width)));
                }
            }
            Shape::Bus { stroke, width, .. }
            | Shape::Arc { stroke, width, .. }
            | Shape::Bezier { stroke, width, .. } => {
                painter.add(egui::Shape::line(pts, pen(*stroke, *width)));
            }
            Shape::Circle {
                stroke,
                fill,
                width,
                ..
            } => {
                if let Some(f) = fill {
                    painter.add(egui::Shape::convex_polygon(
                        pts.clone(),
                        over.unwrap_or(*f),
                        Stroke::NONE,
                    ));
                }
                if let Some(s) = stroke {
                    painter.add(egui::Shape::line(pts, pen(*s, *width)));
                }
            }
            Shape::Frame {
                title_tab,
                stroke,
                paper,
                width,
                frame,
                ..
            } => {
                if *frame {
                    painter.extend(egui::Shape::dashed_line(
                        &pts,
                        pen(*stroke, *width),
                        2.0f32.max(zoom * 2.0),
                        1.0f32.max(zoom),
                    ));
                }
                if let Some(t) = title_tab {
                    let tab = Rect::from_two_pos(screen(t[0]), screen(t[1]));
                    painter.rect_filled(tab, 0.0, *paper);
                    painter.rect_stroke(tab, 0.0, pen(*stroke, *width), egui::StrokeKind::Inside);
                }
            }
            Shape::Text {
                text,
                at,
                size,
                anchor,
                vertical,
                colour,
                width,
                ..
            } => {
                text_at(
                    &painter,
                    text,
                    screen(*at),
                    (*size as f32) * zoom,
                    (*width as f32) * zoom,
                    *anchor,
                    *vertical,
                    over.unwrap_or(*colour),
                );
            }
        }
    }
    // A highlight's participant edges its owner's box.
    for (o, mk) in sc.owners.iter().zip(&m.owners) {
        if let (Some(b), Some(Some(edge))) = (o.boxed, mk.map(|x| x.edge)) {
            painter.rect_stroke(
                Rect::from_two_pos(screen(b[0]), screen(b[1])),
                2.0,
                Stroke::new(1.5, edge),
                egui::StrokeKind::Outside,
            );
        }
    }
    for (k, c) in &m.pins {
        let at = screen(sc.pins[*k].at);
        painter.circle_stroke(at, (0.6 * zoom).max(4.0), Stroke::new(2.0, *c));
    }
    // A scene that couldn't be drawn again: greyed, the error over it.
    if let Some(e) = &w.scene_error {
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(96));
        painter.text(
            rect.center_top() + egui::vec2(0.0, 24.0),
            egui::Align2::CENTER_TOP,
            format!("Not current: {} ({})", e.message, code_word(e.code)),
            egui::FontId::proportional(14.0),
            Color32::WHITE,
        );
    }
    let hover = hovered.clone();
    if w.hover != hover {
        w.hover = hover;
    }
    w.moved(cam);
    if let Some((pickd, add)) = clicked {
        w.pick(pickd, add);
    }
    if let Some(net) = link {
        w.follow_link(&net);
    }
}

/// A text at `at` (screen), `size` points high, fitted to `width` points
/// as SVG stretches it, from its anchor; turned upwards when vertical.
#[allow(clippy::too_many_arguments)]
fn text_at(
    p: &egui::Painter,
    text: &str,
    at: Pos2,
    size: f32,
    width: f32,
    anchor: Anchor,
    vertical: bool,
    colour: Color32,
) {
    if size < 2.0 {
        return;
    }
    let galley = p.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), colour);
    let natural = galley.size().x.max(1.0);
    // Fit the width the scene measured (SVG's textLength): a size that
    // lays it out that wide, never more than a quarter off the scene's.
    let fitted = (size * width / natural).clamp(size * 0.75, size * 1.25);
    let galley = if (fitted - size).abs() > 0.25 {
        p.layout_no_wrap(text.to_owned(), egui::FontId::proportional(fitted), colour)
    } else {
        galley
    };
    let w = galley.size().x;
    let dx = match anchor {
        Anchor::Start => 0.0,
        Anchor::Middle => -w / 2.0,
        Anchor::End => -w,
    };
    let rise = galley
        .rows
        .first()
        .map_or(size * 0.8, |r| r.row.size.y * 0.78);
    let angle = if vertical {
        -std::f32::consts::FRAC_PI_2
    } else {
        0.0
    };
    let (s, c) = (angle.sin(), angle.cos());
    let local = egui::vec2(dx, -rise);
    let pos = at + egui::vec2(c * local.x - s * local.y, s * local.x + c * local.y);
    p.add(egui::epaint::TextShape::new(pos, galley, colour).with_angle(angle));
}

/// An error code as the CLI's envelope writes it (`not_found`).
fn code_word(code: ryu_doc::error::Code) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
