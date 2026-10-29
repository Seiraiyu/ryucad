//! SVG of a block's sheets: symbols, pins, wires and junctions, labels
//! and power ports, and text in Inter at its measured width. One fixed
//! look (the theme is stage 2's). Coordinates are written as exact
//! decimal millimetres, so the same layout gives the same bytes
//! everywhere.

use crate::font::{self, MIN_SIZE, SIZE};
use crate::geom::{GRID, Nm, Point, Rect, Side, mm};
use crate::input::{Block, Fill, Graphic, PartKind};
use crate::label;
use crate::layout::{LabelKind, Layout};
use crate::route;
use crate::shape::Shape;
use std::collections::BTreeMap;
use std::fmt::Write;

const INK: &str = "#7a1f1f";
const BODY: &str = "#fff6dc";
const WIRE: &str = "#1b6e3a";
const TEXT: &str = "#202020";
const POWER: &str = "#a01818";
const LABEL: &str = "#1d3f8f";
const CANVAS: &str = "#e6e8eb";
const PAPER: &str = "#ffffff";
const LINE: Nm = 150_000;

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

fn graphic(out: &mut String, f: &Frame, shape: &Shape, g: &Graphic) {
    let pl = |p: Point| shape.place.apply(p);
    let fill = |x: Fill| match x {
        Fill::None => "none",
        Fill::Body => BODY,
        Fill::Outline => INK,
    };
    match g {
        Graphic::Rect {
            start,
            end,
            fill: x,
        } => {
            let pts = [
                pl(*start),
                pl([end[0], start[1]]),
                pl(*end),
                pl([start[0], end[1]]),
                pl(*start),
            ];
            polyline(out, f, &pts, INK, fill(*x));
        }
        Graphic::Polyline { points, fill: x } => {
            let pts: Vec<Point> = points.iter().map(|p| pl(*p)).collect();
            polyline(out, f, &pts, INK, fill(*x));
        }
        Graphic::Circle {
            center,
            radius,
            fill: x,
        } => {
            let c = pl(*center);
            let _ = writeln!(
                out,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{INK}\" stroke-width=\"{}\"/>",
                f.x(c[0]),
                f.y(c[1]),
                mm(*radius),
                fill(*x),
                mm(LINE)
            );
        }
        Graphic::Arc { start, mid, end } => {
            let (a, b, c) = (pl(*start), pl(*mid), pl(*end));
            let Some((center, r)) = circle(a, b, c) else {
                polyline(out, f, &[a, b, c], INK, "none");
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
                "<path d=\"M{} A{rr},{rr} 0 {large} {sweep} {}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"{}\"/>",
                f.p(a),
                f.p(c),
                mm(LINE)
            );
        }
        Graphic::Bezier { points } => {
            let p: Vec<String> = points.iter().map(|q| f.p(pl(*q))).collect();
            let _ = writeln!(
                out,
                "<path d=\"M{} C{} {} {}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"{}\"/>",
                p[0],
                p[1],
                p[2],
                p[3],
                mm(LINE)
            );
        }
        Graphic::Text { text, at, size } => {
            let a = pl(*at);
            text_el(out, f, text, a, (*size).max(MIN_SIZE), "start", false, TEXT);
        }
    }
}

fn label_el(out: &mut String, f: &Frame, kind: LabelKind, name: &str, at: Point, side: Side) {
    let d = label::drawn(kind, name, at, side);
    let colour = if kind == LabelKind::Net { LABEL } else { POWER };
    for l in &d.lines {
        polyline(out, f, l, colour, "none");
    }
    if let Some((p, anchor, vertical)) = d.text {
        let anchor = match anchor {
            font::Anchor::Start => "start",
            font::Anchor::Middle => "middle",
            font::Anchor::End => "end",
        };
        text_el(out, f, name, p, SIZE, anchor, vertical, colour);
    }
}

/// Draws the sheets `which` (all when empty) of a block's layout, side by
/// side as the canvas places them.
pub fn draw(block: &Block, layout: &Layout, which: &[&str]) -> String {
    let sheets: Vec<(&String, &crate::layout::Sheet)> = layout
        .sheets
        .iter()
        .filter(|(id, _)| which.is_empty() || which.contains(&id.as_str()))
        .collect();
    let pad = 6 * GRID;
    let bounds = sheets
        .iter()
        .map(|(_, s)| Rect::new(s.at, [s.at[0] + block.sheet[0], s.at[1] + block.sheet[1]]))
        .reduce(Rect::union)
        .unwrap_or(Rect::new([0, 0], block.sheet))
        .grow(pad);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{}mm\" height=\"{}mm\" font-family=\"Inter\">",
        mm(bounds.width()),
        mm(bounds.height()),
        mm(bounds.width()),
        mm(bounds.height())
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
        let _ = writeln!(out, "<g id=\"sheet-{}\">", esc(sid));
        let _ = writeln!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{PAPER}\"/>",
            f.x(0),
            f.y(block.sheet[1]),
            mm(block.sheet[0]),
            mm(block.sheet[1])
        );
        text_el(
            &mut out,
            &f,
            &sheet.name,
            [0, block.sheet[1] + GRID],
            SIZE,
            "start",
            false,
            "#606060",
        );
        // Parts, in key order.
        let mut arms: BTreeMap<(usize, route::Node), u8> = BTreeMap::new();
        for part in &block.parts {
            let Some(x) = layout.placed.get(&part.key).filter(|x| &x.sheet == sid) else {
                continue;
            };
            let shape = Shape::of(part, crate::geom::Place::new(x.at, x.rotation, x.mirror));
            for g in &part.graphics {
                graphic(&mut out, &f, &shape, g);
            }
            let names = part.pins.len() > 2 && !matches!(part.kind, PartKind::Port { .. });
            for (k, pin) in part.pins.iter().enumerate() {
                let at = shape.pins[k];
                if at.base != at.end {
                    polyline(&mut out, &f, &[at.base, at.end], INK, "none");
                }
                if let Some(n) = pin.net {
                    *arms.entry((n, route::node_of(at.end))).or_default() += 1;
                }
                let dir = at.side.step();
                let inward = [-dir[0] / GRID, -dir[1] / GRID];
                if names && !pin.name.is_empty() {
                    let off = GRID / 2;
                    let p = [at.base[0] + inward[0] * off, at.base[1] + inward[1] * off];
                    let (anchor, p) = match at.side {
                        Side::Left => ("start", [p[0], p[1] - SIZE / 3]),
                        Side::Right => ("end", [p[0], p[1] - SIZE / 3]),
                        Side::Up => ("end", [p[0] + SIZE / 3, p[1]]),
                        Side::Down => ("start", [p[0] + SIZE / 3, p[1]]),
                    };
                    text_el(
                        &mut out,
                        &f,
                        &pin.name,
                        p,
                        SIZE,
                        anchor,
                        !at.side.horizontal(),
                        TEXT,
                    );
                }
                if pin.show_number {
                    let mid = [(at.base[0] + at.end[0]) / 2, (at.base[1] + at.end[1]) / 2];
                    let (p, vertical) = if at.side.horizontal() {
                        ([mid[0], mid[1] + GRID / 4], false)
                    } else {
                        ([mid[0] - GRID / 4, mid[1]], true)
                    };
                    text_el(
                        &mut out,
                        &f,
                        &pin.number,
                        p,
                        MIN_SIZE,
                        "middle",
                        vertical,
                        INK,
                    );
                }
                if pin.nc {
                    let d = GRID / 3;
                    let e = at.end;
                    polyline(
                        &mut out,
                        &f,
                        &[[e[0] - d, e[1] - d], [e[0] + d, e[1] + d]],
                        LABEL,
                        "none",
                    );
                    polyline(
                        &mut out,
                        &f,
                        &[[e[0] - d, e[1] + d], [e[0] + d, e[1] - d]],
                        LABEL,
                        "none",
                    );
                }
            }
            if let Some(fields) = layout.text.get(&part.key) {
                for (field, value) in [("refdes", &part.refdes), ("value", &part.value)] {
                    if let Some(at) = fields.get(field) {
                        text_el(&mut out, &f, value, *at, SIZE, "start", false, TEXT);
                    }
                }
            }
        }
        // Wires and junctions.
        for (key, by_sheet) in &layout.routes {
            let Some(runs) = by_sheet.get(sid) else {
                continue;
            };
            let n = block.net(key).unwrap_or(usize::MAX);
            for run in runs {
                polyline(&mut out, &f, run, WIRE, "none");
                let nodes = route::expand(run);
                for (i, node) in nodes.iter().enumerate() {
                    let ends = u8::from(i > 0) + u8::from(i + 1 < nodes.len());
                    *arms.entry((n, *node)).or_default() += ends;
                }
            }
        }
        for ((_, node), count) in &arms {
            if *count >= 3 {
                let p = route::point_of(*node);
                let _ = writeln!(
                    out,
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{WIRE}\"/>",
                    f.x(p[0]),
                    f.y(p[1]),
                    mm(GRID / 4)
                );
            }
        }
        for (key, list) in &layout.labels {
            let name = block
                .net(key)
                .map_or(key.as_str(), |n| block.nets[n].name.as_str());
            for l in list.iter().filter(|l| &l.sheet == sid) {
                label_el(&mut out, &f, l.kind, name, l.at, l.side);
            }
        }
        let _ = writeln!(out, "</g>");
    }
    let _ = writeln!(out, "</svg>");
    out
}
