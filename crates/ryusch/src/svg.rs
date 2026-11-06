//! SVG of a block's sheets: what [`crate::paint`] says each draws, in
//! Inter at each text's measured width, on the block's canvas. One fixed
//! look (the theme is stage 2's). Coordinates are written as exact
//! decimal millimetres, so the same layout gives the same bytes
//! everywhere.

use crate::font::{self, SIZE};
use crate::geom::{GRID, Nm, Point, Rect, mm};
use crate::input::Block;
use crate::label;
use crate::layout::Layout;
use crate::paint::{Item, LINE, NAME};
use crate::shape::Shape;
use std::fmt::Write;

const CANVAS: &str = "#e6e8eb";
const PAPER: &str = "#ffffff";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Points on a sheet to the SVG's frame: y down from the canvas's top.
struct Frame {
    origin: Point,
    top: Nm,
}

impl Frame {
    fn x(&self, x: Nm) -> String {
        mm(self.origin[0] + x)
    }

    fn y(&self, y: Nm) -> String {
        mm(self.top - (self.origin[1] + y))
    }

    fn p(&self, p: Point) -> String {
        format!("{},{}", self.x(p[0]), self.y(p[1]))
    }
}

/// A text element: upright or turned a quarter (reading upwards).
#[allow(clippy::too_many_arguments)]
fn text_el(
    out: &mut String,
    f: &Frame,
    s: &str,
    at: Point,
    size: Nm,
    anchor: &str,
    vertical: bool,
    colour: &str,
) {
    let w = font::width(s, size).max(1);
    let (x, y) = (f.x(at[0]), f.y(at[1]));
    let turn = if vertical {
        format!(" transform=\"rotate(-90 {x} {y})\"")
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "<text x=\"{x}\" y=\"{y}\" font-size=\"{}\" text-anchor=\"{anchor}\" textLength=\"{}\" lengthAdjust=\"spacingAndGlyphs\" fill=\"{colour}\"{turn}>{}</text>",
        mm(size),
        mm(w),
        esc(s)
    );
}

fn polyline(out: &mut String, f: &Frame, pts: &[Point], stroke: &str, fill: &str) {
    let list: Vec<String> = pts.iter().map(|p| f.p(*p)).collect();
    let _ = writeln!(
        out,
        "<polyline points=\"{}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>",
        list.join(" "),
        mm(LINE)
    );
}

/// The circle through three points, if they aren't on a line.
fn circle(a: Point, b: Point, c: Point) -> Option<([f64; 2], f64)> {
    let (ax, ay, bx, by, cx, cy) = (
        a[0] as f64,
        a[1] as f64,
        b[0] as f64,
        b[1] as f64,
        c[0] as f64,
        c[1] as f64,
    );
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d == 0.0 {
        return None;
    }
    let sq = |x: f64, y: f64| x * x + y * y;
    let ux = (sq(ax, ay) * (by - cy) + sq(bx, by) * (cy - ay) + sq(cx, cy) * (ay - by)) / d;
    let uy = (sq(ax, ay) * (cx - bx) + sq(bx, by) * (ax - cx) + sq(cx, cy) * (bx - ax)) / d;
    let r = ((ax - ux) * (ax - ux) + (ay - uy) * (ay - uy)).sqrt();
    Some(([ux, uy], r))
}

/// One item, in the SVG's frame.
fn item(out: &mut String, f: &Frame, it: &Item) {
    match it {
        Item::Polyline {
            points,
            stroke,
            fill,
        } => polyline(out, f, points, stroke, fill.unwrap_or("none")),
        Item::Circle {
            center,
            radius,
            stroke: Some(stroke),
            fill,
        } => {
            let _ = writeln!(
                out,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{stroke}\" stroke-width=\"{}\"/>",
                f.x(center[0]),
                f.y(center[1]),
                mm(*radius),
                fill.unwrap_or("none"),
                mm(LINE)
            );
        }
        Item::Circle {
            center,
            radius,
            stroke: None,
            fill,
        } => {
            let _ = writeln!(
                out,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
                f.x(center[0]),
                f.y(center[1]),
                mm(*radius),
                fill.unwrap_or("none"),
            );
        }
        Item::Arc {
            start: a,
            mid: b,
            end: c,
            stroke,
        } => {
            let (a, b, c) = (*a, *b, *c);
            let Some((center, r)) = circle(a, b, c) else {
                polyline(out, f, &[a, b, c], stroke, "none");
                return;
            };
            // Counterclockwise here (y up) is clockwise on screen.
            let cross = |p: [f64; 2]| {
                (c[0] - a[0]) as f64 * (p[1] - a[1] as f64)
                    - (c[1] - a[1]) as f64 * (p[0] - a[0] as f64)
            };
            let mid = cross([b[0] as f64, b[1] as f64]);
            let sweep = i32::from(mid < 0.0);
            // More than half a turn when the middle point and the centre
            // are on the same side of the chord.
            let large = i32::from(mid * cross(center) > 0.0);
            let rr = mm(r.round() as Nm);
            let _ = writeln!(
                out,
                "<path d=\"M{} A{rr},{rr} 0 {large} {sweep} {}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{}\"/>",
                f.p(a),
                f.p(c),
                mm(LINE)
            );
        }
        Item::Bezier { points, stroke } => {
            let p: Vec<String> = points.iter().map(|q| f.p(*q)).collect();
            let _ = writeln!(
                out,
                "<path d=\"M{} C{} {} {}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{}\"/>",
                p[0],
                p[1],
                p[2],
                p[3],
                mm(LINE)
            );
        }
        Item::Frame { rect, title_tab } => {
            let corners = |r: &Rect| {
                vec![
                    r.min,
                    [r.max[0], r.min[1]],
                    r.max,
                    [r.min[0], r.max[1]],
                    r.min,
                ]
            };
            let list: Vec<String> = corners(rect).iter().map(|p| f.p(*p)).collect();
            let _ = writeln!(
                out,
                "<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-dasharray=\"{} {}\"/>",
                list.join(" "),
                crate::paint::FRAME,
                mm(LINE),
                mm(crate::paint::DASH[0]),
                mm(crate::paint::DASH[1]),
            );
            polyline(
                out,
                f,
                &corners(title_tab),
                crate::paint::FRAME,
                crate::paint::PAPER,
            );
        }
        Item::Text {
            text,
            at,
            size,
            anchor,
            vertical,
            colour,
        } => {
            let anchor = match anchor {
                font::Anchor::Start => "start",
                font::Anchor::Middle => "middle",
                font::Anchor::End => "end",
            };
            text_el(out, f, text, *at, *size, anchor, *vertical, colour);
        }
    }
}

/// Draws the sheets `which` (all when empty) of a block's layout, side by
/// side as the canvas places them.
pub fn draw(block: &Block, layout: &Layout, which: &[&str]) -> String {
    draw_in(block, layout, which, None)
}

/// Where the sheets `which` (all when empty) draw anything, on the
/// canvas: parts, text, labels and wires, a few steps round them.
pub fn drawn(block: &Block, layout: &Layout, which: &[&str]) -> Option<Rect> {
    let on = |s: &str| which.is_empty() || which.contains(&s);
    let at = |s: &str| layout.sheets.get(s).map_or([0, 0], |x| x.at);
    let mut boxes: Vec<Rect> = Vec::new();
    for part in &block.parts {
        let Some(x) = layout.placed.get(&part.key).filter(|x| on(&x.sheet)) else {
            continue;
        };
        let o = at(&x.sheet);
        boxes.push(
            Shape::of(part, crate::geom::Place::new(x.at, x.rotation, x.mirror))
                .extent
                .shift(o),
        );
        if let Some(fields) = layout.text.get(&part.key) {
            for (field, v) in [("refdes", &part.refdes), ("value", &part.value)] {
                if let Some(p) = fields.get(field) {
                    boxes.push(font::text_box(v, SIZE, *p, font::Anchor::Start).shift(o));
                }
            }
        }
    }
    for list in layout.labels.values() {
        for l in list.iter().filter(|l| on(&l.sheet)) {
            boxes.push(label::label_box(l.kind, &l.text, l.at, l.side).shift(at(&l.sheet)));
        }
    }
    for by_sheet in layout.routes.values() {
        for (s, runs) in by_sheet.iter().filter(|(s, _)| on(s)) {
            for run in runs {
                boxes.extend(Rect::of(run.iter().copied()).map(|r| r.shift(at(s))));
            }
        }
    }
    boxes
        .into_iter()
        .reduce(Rect::union)
        .map(|r| r.grow(4 * GRID))
}

/// [`draw`], its view cropped to what it draws ([`drawn`]).
pub fn draw_fit(block: &Block, layout: &Layout, which: &[&str]) -> String {
    draw_in(block, layout, which, drawn(block, layout, which))
}

fn draw_in(block: &Block, layout: &Layout, which: &[&str], view: Option<Rect>) -> String {
    let sheets: Vec<(&String, &crate::layout::Sheet)> = layout
        .sheets
        .iter()
        .filter(|(id, _)| which.is_empty() || which.contains(&id.as_str()))
        .collect();
    let pad = 6 * GRID;
    let bounds = sheets
        .iter()
        .map(|(_, s)| {
            let a = block.area_of(s);
            Rect::new(s.at, [s.at[0] + a[0], s.at[1] + a[1]])
        })
        .reduce(Rect::union)
        .unwrap_or(Rect::new([0, 0], block.sheet))
        .grow(pad);
    let mut out = String::new();
    // The view: the whole canvas, or a box on it (y down from the top).
    let (vx, vy, vw, vh) = match view {
        Some(v) => (
            v.min[0] - bounds.min[0],
            bounds.max[1] - v.max[1],
            v.width(),
            v.height(),
        ),
        None => (0, 0, bounds.width(), bounds.height()),
    };
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" width=\"{}mm\" height=\"{}mm\" font-family=\"Inter\">",
        mm(vx),
        mm(vy),
        mm(vw),
        mm(vh),
        mm(vw),
        mm(vh)
    );
    let _ = writeln!(
        out,
        "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"{CANVAS}\"/>",
        mm(bounds.width()),
        mm(bounds.height())
    );
    for (sid, sheet) in sheets {
        let f = Frame {
            origin: [sheet.at[0] - bounds.min[0], sheet.at[1] - bounds.min[1]],
            top: bounds.height(),
        };
        let size = block.area_of(sheet);
        let _ = writeln!(out, "<g id=\"sheet-{}\">", esc(sid));
        let _ = writeln!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{PAPER}\"/>",
            f.x(0),
            f.y(size[1]),
            mm(size[0]),
            mm(size[1])
        );
        text_el(
            &mut out,
            &f,
            &sheet.name,
            [0, size[1] + GRID],
            SIZE,
            "start",
            false,
            NAME,
        );
        for it in &crate::paint::sheet(block, layout, sid).items {
            item(&mut out, &f, it);
        }
        let _ = writeln!(out, "</g>");
    }
    let _ = writeln!(out, "</svg>");
    out
}
