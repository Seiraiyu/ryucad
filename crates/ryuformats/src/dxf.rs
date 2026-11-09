//! DXF: a sheet's flat pattern ([`FlatPattern`], design 346) as AutoCAD
//! R2000 (`AC1015`) ASCII, for a laser cutter's CAM. Outlines on layer
//! `OUTLINE`, holes on `INNER`, each bend line on `BEND_UP` or
//! `BEND_DOWN` (dashed) and, with notes, its note on `BEND_NOTES`
//! centred above the line's middle along it. Lines, arcs, circles,
//! ellipses and splines keep each segment exact: an arc is written
//! counterclockwise as DXF draws arcs, from its end to its start where
//! the pattern walks it clockwise. Coordinates are the pattern's in its
//! frame, in the unit asked for, as the shortest text that reads back to
//! the same bits. Built in memory: the same pattern and options, the same
//! bytes. The handles, tables, blocks and objects are what R2000 readers
//! require, numbered in a fixed order.

use crate::text::push_f64;
use ryukei::geom::{Curve2, Point2};
use ryukei::sheet_metal::{Allowance, BendDirection, BendLine, FlatPattern, PatternSegment};
use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;
use std::io;

/// The file's length unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DxfUnit {
    #[default]
    Mm,
    In,
}

impl DxfUnit {
    pub fn parse(s: &str) -> Option<DxfUnit> {
        match s {
            "mm" => Some(DxfUnit::Mm),
            "in" => Some(DxfUnit::In),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DxfUnit::Mm => "mm",
            DxfUnit::In => "in",
        }
    }

    /// How many of the unit make a metre.
    pub fn per_meter(self) -> f64 {
        match self {
            DxfUnit::Mm => 1000.0,
            DxfUnit::In => 1.0 / 0.0254,
        }
    }

    /// `$INSUNITS`.
    fn insunits(self) -> i64 {
        match self {
            DxfUnit::Mm => 4,
            DxfUnit::In => 1,
        }
    }

    /// `$MEASUREMENT`: 1 metric, 0 imperial.
    fn measurement(self) -> i64 {
        match self {
            DxfUnit::Mm => 1,
            DxfUnit::In => 0,
        }
    }

    /// A note's text height: 2.5 mm, or 0.1 in.
    pub fn text_height(self) -> f64 {
        match self {
            DxfUnit::Mm => 2.5,
            DxfUnit::In => 0.1,
        }
    }

    /// `DASHED`'s dash and gap.
    fn dash(self) -> [f64; 2] {
        match self {
            DxfUnit::Mm => [6.0, -3.0],
            DxfUnit::In => [0.25, -0.125],
        }
    }
}

/// How to write a DXF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DxfOptions {
    pub unit: DxfUnit,
    /// Each bend line's note on `BEND_NOTES`.
    pub notes: bool,
}

impl Default for DxfOptions {
    fn default() -> Self {
        DxfOptions {
            unit: DxfUnit::Mm,
            notes: true,
        }
    }
}

/// The layers, in the order written, with their ACI colours and whether
/// they are dashed. Fixed names: a shop maps layers to operations by name.
pub const LAYERS: [(&str, i64, bool); 5] = [
    ("OUTLINE", 7, false),
    ("INNER", 1, false),
    ("BEND_UP", 2, true),
    ("BEND_DOWN", 4, true),
    ("BEND_NOTES", 7, false),
];

/// `x` to three decimals, trailing zeros and a bare point dropped, `-0`
/// as `0`.
fn three(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// A bend line's note: `UP 90%%d R1 K0.44` (direction, angle in degrees,
/// inner radius in `unit`, K-factor), or `BA<length>` for a bend given
/// its developed length. `degree` is the degree sign: `%%d` in a DXF,
/// `°` elsewhere.
pub fn note(b: &BendLine, unit: DxfUnit, degree: &str) -> String {
    let dir = match b.direction {
        BendDirection::Up => "UP",
        BendDirection::Down => "DOWN",
    };
    let k = unit.per_meter();
    let allowance = match b.allowance {
        Allowance::KFactor(f) => format!("K{}", three(f)),
        Allowance::Length(l) => format!("BA{}", three(l * k)),
    };
    format!(
        "{dir} {}{degree} R{} {allowance}",
        three(b.angle * 180.0 / PI),
        three(b.radius * k)
    )
}

/// A segment as DXF draws it, in the unit's coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Line {
        from: [f64; 2],
        to: [f64; 2],
    },
    Circle {
        centre: [f64; 2],
        radius: f64,
    },
    /// Counterclockwise from `start_deg` to `end_deg`, each in `[0, 360)`.
    Arc {
        centre: [f64; 2],
        radius: f64,
        start_deg: f64,
        end_deg: f64,
    },
    /// `major` the major axis's end from the centre; parameters
    /// counterclockwise from `start` to `end`, `end > start`.
    Ellipse {
        centre: [f64; 2],
        major: [f64; 2],
        ratio: f64,
        start: f64,
        end: f64,
    },
    /// Clamped, over the segment's run.
    Spline {
        degree: usize,
        knots: Vec<f64>,
        weights: Option<Vec<f64>>,
        points: Vec<[f64; 2]>,
    },
}

fn at(p: Point2, k: f64) -> [f64; 2] {
    [p.x * k, p.y * k]
}

/// An angle in degrees in `[0, 360)`.
fn degrees(a: f64) -> f64 {
    let d = a * 180.0 / PI;
    let d = d.rem_euclid(360.0);
    if d >= 360.0 { 0.0 } else { d }
}

/// Segment `s` as DXF draws it, `k` units a metre. `InvalidData` for a
/// curve it doesn't write or numbers that aren't finite.
pub fn shape(s: &PatternSegment, k: f64) -> io::Result<Shape> {
    let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_owned());
    let whole = (s.t1 - s.t0).abs() >= TAU - 1e-9;
    // Counterclockwise in the plane as the pattern walks it.
    let ccw_walk = |frame_ccw: bool| (s.t1 > s.t0) == frame_ccw;
    let out = match &s.curve {
        Curve2::Line2(_) => Shape::Line {
            from: at(s.curve.point(s.t0), k),
            to: at(s.curve.point(s.t1), k),
        },
        Curve2::Circle2(c) => {
            let f = c.frame();
            let centre = at(f.origin(), k);
            let radius = c.radius() * k;
            if whole {
                Shape::Circle { centre, radius }
            } else {
                let angle = |t: f64| {
                    let d = s.curve.point(t) - f.origin();
                    degrees(ryukei::math::atan2(d.y, d.x))
                };
                let (a, b) = if ccw_walk(f.is_ccw()) {
                    (s.t0, s.t1)
                } else {
                    (s.t1, s.t0)
                };
                Shape::Arc {
                    centre,
                    radius,
                    start_deg: angle(a),
                    end_deg: angle(b),
                }
            }
        }
        Curve2::Ellipse2(e) => {
            let f = e.frame();
            // DXF's parameter runs counterclockwise from the major axis:
            // the curve's own on a counterclockwise frame, negated on a
            // clockwise one.
            let p = |t: f64| if f.is_ccw() { t } else { -t };
            let (a, b) = if ccw_walk(f.is_ccw()) {
                (s.t0, s.t1)
            } else {
                (s.t1, s.t0)
            };
            let start = p(a).rem_euclid(TAU);
            let sweep = if whole { TAU } else { (p(b) - p(a)).abs() };
            let x = f.x();
            Shape::Ellipse {
                centre: at(f.origin(), k),
                major: [x.x * e.major() * k, x.y * e.major() * k],
                ratio: e.minor() / e.major(),
                start,
                end: start + sweep,
            }
        }
        Curve2::Nurbs2(n) => {
            let range = [s.t0.min(s.t1), s.t0.max(s.t1)];
            let part = if range == n.domain() {
                (**n).clone()
            } else {
                n.part(range)
                    .map_err(|e| bad(&format!("a spline's run: {e:?}")))?
            };
            let part = part.to_clamped();
            let w = part.weights();
            let rational = w.iter().any(|x| *x != 1.0);
            Shape::Spline {
                degree: part.degree(),
                knots: part.knots().flat().to_vec(),
                weights: rational.then(|| w.to_vec()),
                points: part.control_points().map(|p| at(p, k)).collect(),
            }
        }
        _ => return Err(bad("a plane curve DXF doesn't write")),
    };
    Ok(out)
}

/// Groups, and the handles handed out so far.
#[derive(Default)]
struct Out {
    s: String,
    last: u64,
}

impl Out {
    fn pair(&mut self, code: u16, v: &str) {
        let _ = write!(self.s, "{code:>3}\n{v}\n");
    }

    fn int(&mut self, code: u16, v: i64) {
        self.pair(code, &v.to_string());
    }

    fn num(&mut self, code: u16, x: f64) -> io::Result<()> {
        if !x.is_finite() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a number that isn't finite",
            ));
        }
        let mut t = String::new();
        push_f64(&mut t, x);
        self.pair(code, &t);
        Ok(())
    }

    fn xy(&mut self, code: u16, p: [f64; 2]) -> io::Result<()> {
        self.num(code, p[0])?;
        self.num(code + 10, p[1])?;
        self.num(code + 20, 0.0)
    }

    /// The next handle.
    fn handle(&mut self) -> String {
        self.last += 1;
        format!("{:X}", self.last)
    }

    /// `0 kind`, a new handle and its owner.
    fn start(&mut self, kind: &str, owner: &str) -> String {
        self.pair(0, kind);
        let h = self.handle();
        self.pair(5, &h);
        self.pair(330, owner);
        h
    }

    /// An entity's head: its handle, owner, layer and subclass.
    fn entity(&mut self, kind: &str, owner: &str, layer: &str, class: &str) {
        self.start(kind, owner);
        self.pair(100, "AcDbEntity");
        self.pair(8, layer);
        self.pair(100, class);
    }
}

/// `p` as R2000 DXF bytes.
pub fn write(p: &FlatPattern, o: &DxfOptions) -> io::Result<Vec<u8>> {
    let k = o.unit.per_meter();
    let mut b = Out::default();
    // Handles in a fixed order: tables, their records, blocks, entities,
    // objects. The header comes last, once `$HANDSEED` is known.
    b.pair(0, "SECTION");
    b.pair(2, "CLASSES");
    b.pair(0, "ENDSEC");
    b.pair(0, "SECTION");
    b.pair(2, "TABLES");
    let table = |b: &mut Out, name: &str, n: i64| -> String {
        let h = b.start("TABLE", "0");
        // A TABLE's 0 and 2 come before its handle in R2000: rewrite.
        b.s.truncate(b.s.len() - format!("  5\n{h}\n330\n0\n").len());
        b.pair(2, name);
        b.pair(5, &h);
        b.pair(330, "0");
        b.pair(100, "AcDbSymbolTable");
        b.int(70, n);
        h
    };
    let record = |b: &mut Out, kind: &str, owner: &str, class: &str| -> String {
        let h = b.start(kind, owner);
        b.pair(100, "AcDbSymbolTableRecord");
        b.pair(100, class);
        h
    };
    // VPORT, empty.
    table(&mut b, "VPORT", 0);
    b.pair(0, "ENDTAB");
    // LTYPE: ByBlock, ByLayer, Continuous, DASHED.
    let t = table(&mut b, "LTYPE", 4);
    for (name, desc, dash) in [
        ("ByBlock", "", None),
        ("ByLayer", "", None),
        ("CONTINUOUS", "Solid line", None),
        ("DASHED", "Dashed __ __ __", Some(o.unit.dash())),
    ] {
        record(&mut b, "LTYPE", &t, "AcDbLinetypeTableRecord");
        b.pair(2, name);
        b.int(70, 0);
        b.pair(3, desc);
        b.int(72, 65);
        match dash {
            None => {
                b.int(73, 0);
                b.num(40, 0.0)?;
            }
            Some([on, off]) => {
                b.int(73, 2);
                b.num(40, on - off)?;
                for x in [on, off] {
                    b.num(49, x)?;
                    b.int(74, 0);
                }
            }
        }
    }
    b.pair(0, "ENDTAB");
    // LAYER: 0 and ours.
    let t = table(&mut b, "LAYER", 1 + LAYERS.len() as i64);
    for (name, colour, dashed) in std::iter::once(("0", 7, false)).chain(LAYERS) {
        record(&mut b, "LAYER", &t, "AcDbLayerTableRecord");
        b.pair(2, name);
        b.int(70, 0);
        b.int(62, colour);
        b.pair(6, if dashed { "DASHED" } else { "CONTINUOUS" });
        b.int(370, -3);
    }
    b.pair(0, "ENDTAB");
    // STYLE: Standard.
    let t = table(&mut b, "STYLE", 1);
    record(&mut b, "STYLE", &t, "AcDbTextStyleTableRecord");
    b.pair(2, "Standard");
    b.int(70, 0);
    b.num(40, 0.0)?;
    b.num(41, 1.0)?;
    b.num(50, 0.0)?;
    b.int(71, 0);
    b.num(42, o.unit.text_height())?;
    b.pair(3, "txt");
    b.pair(4, "");
    b.pair(0, "ENDTAB");
    for name in ["VIEW", "UCS"] {
        table(&mut b, name, 0);
        b.pair(0, "ENDTAB");
    }
    let t = table(&mut b, "APPID", 1);
    record(&mut b, "APPID", &t, "AcDbRegAppTableRecord");
    b.pair(2, "ACAD");
    b.int(70, 0);
    b.pair(0, "ENDTAB");
    // DIMSTYLE: Standard (its handle is a 105).
    let t = table(&mut b, "DIMSTYLE", 1);
    b.pair(100, "AcDbDimStyleTable");
    b.pair(0, "DIMSTYLE");
    let h = b.handle();
    b.pair(105, &h);
    b.pair(330, &t);
    b.pair(100, "AcDbSymbolTableRecord");
    b.pair(100, "AcDbDimStyleTableRecord");
    b.pair(2, "Standard");
    b.int(70, 0);
    b.pair(0, "ENDTAB");
    // BLOCK_RECORD: model and paper space, each pointing at its layout
    // (handed out below, so known now: the next two after the blocks'
    // four and the entities).
    let t = table(&mut b, "BLOCK_RECORD", 2);
    let model = record(&mut b, "BLOCK_RECORD", &t, "AcDbBlockTableRecord");
    let model_layout_at = b.s.len();
    b.pair(2, "*Model_Space");
    let paper = record(&mut b, "BLOCK_RECORD", &t, "AcDbBlockTableRecord");
    let paper_layout_at = b.s.len();
    b.pair(2, "*Paper_Space");
    b.pair(0, "ENDTAB");
    b.pair(0, "ENDSEC");
    // BLOCKS.
    b.pair(0, "SECTION");
    b.pair(2, "BLOCKS");
    for (owner, name) in [(&model, "*Model_Space"), (&paper, "*Paper_Space")] {
        b.start("BLOCK", owner);
        b.pair(100, "AcDbEntity");
        b.pair(8, "0");
        b.pair(100, "AcDbBlockBegin");
        b.pair(2, name);
        b.int(70, 0);
        b.xy(10, [0.0, 0.0])?;
        b.pair(3, name);
        b.pair(1, "");
        b.start("ENDBLK", owner);
        b.pair(100, "AcDbEntity");
        b.pair(8, "0");
        b.pair(100, "AcDbBlockEnd");
    }
    b.pair(0, "ENDSEC");
    // ENTITIES: outlines, holes, bend lines, notes.
    b.pair(0, "SECTION");
    b.pair(2, "ENTITIES");
    for outer in [true, false] {
        let layer = if outer { "OUTLINE" } else { "INNER" };
        for s in p
            .loops
            .iter()
            .filter(|l| l.outer == outer)
            .flat_map(|l| &l.segments)
        {
            match shape(s, k)? {
                Shape::Line { from, to } => {
                    b.entity("LINE", &model, layer, "AcDbLine");
                    b.xy(10, from)?;
                    b.xy(11, to)?;
                }
                Shape::Circle { centre, radius } => {
                    b.entity("CIRCLE", &model, layer, "AcDbCircle");
                    b.xy(10, centre)?;
                    b.num(40, radius)?;
                }
                Shape::Arc {
                    centre,
                    radius,
                    start_deg,
                    end_deg,
                } => {
                    b.entity("ARC", &model, layer, "AcDbCircle");
                    b.xy(10, centre)?;
                    b.num(40, radius)?;
                    b.pair(100, "AcDbArc");
                    b.num(50, start_deg)?;
                    b.num(51, end_deg)?;
                }
                Shape::Ellipse {
                    centre,
                    major,
                    ratio,
                    start,
                    end,
                } => {
                    b.entity("ELLIPSE", &model, layer, "AcDbEllipse");
                    b.xy(10, centre)?;
                    b.xy(11, major)?;
                    b.num(40, ratio)?;
                    b.num(41, start)?;
                    b.num(42, end)?;
                }
                Shape::Spline {
                    degree,
                    knots,
                    weights,
                    points,
                } => {
                    b.entity("SPLINE", &model, layer, "AcDbSpline");
                    b.num(210, 0.0)?;
                    b.num(220, 0.0)?;
                    b.num(230, 1.0)?;
                    b.int(70, 8 | if weights.is_some() { 4 } else { 0 });
                    b.int(71, degree as i64);
                    b.int(72, knots.len() as i64);
                    b.int(73, points.len() as i64);
                    b.int(74, 0);
                    b.num(42, 1e-10)?;
                    b.num(43, 1e-10)?;
                    for x in &knots {
                        b.num(40, *x)?;
                    }
                    for x in weights.iter().flatten() {
                        b.num(41, *x)?;
                    }
                    for q in &points {
                        b.xy(10, *q)?;
                    }
                }
            }
        }
    }
    for l in &p.bend_lines {
        let layer = match l.direction {
            BendDirection::Up => "BEND_UP",
            BendDirection::Down => "BEND_DOWN",
        };
        b.entity("LINE", &model, layer, "AcDbLine");
        b.xy(10, at(l.start, k))?;
        b.xy(11, at(l.end, k))?;
    }
    if o.notes {
        let h = o.unit.text_height();
        for l in &p.bend_lines {
            // Reading left to right, half a text height above the line.
            let (mut dx, mut dy) = ((l.end.x - l.start.x) * k, (l.end.y - l.start.y) * k);
            if dx < 0.0 || (dx == 0.0 && dy < 0.0) {
                (dx, dy) = (-dx, -dy);
            }
            let len = (dx * dx + dy * dy).sqrt();
            let (ux, uy) = if len > 0.0 {
                (dx / len, dy / len)
            } else {
                (1.0, 0.0)
            };
            let mid = [
                (l.start.x + l.end.x) * 0.5 * k,
                (l.start.y + l.end.y) * 0.5 * k,
            ];
            let anchor = [mid[0] - uy * 0.5 * h, mid[1] + ux * 0.5 * h];
            b.entity("TEXT", &model, "BEND_NOTES", "AcDbText");
            b.xy(10, anchor)?;
            b.num(40, h)?;
            b.pair(1, &note(l, o.unit, "%%d"));
            b.num(50, degrees(ryukei::math::atan2(uy, ux)))?;
            b.int(72, 1);
            b.xy(11, anchor)?;
            b.pair(100, "AcDbText");
            b.int(73, 1);
        }
    }
    b.pair(0, "ENDSEC");
    // OBJECTS: the root dictionary, its groups and layouts.
    b.pair(0, "SECTION");
    b.pair(2, "OBJECTS");
    let root = b.start("DICTIONARY", "0");
    b.pair(100, "AcDbDictionary");
    b.int(281, 1);
    let (groups, layouts) = (format!("{:X}", b.last + 1), format!("{:X}", b.last + 2));
    b.pair(3, "ACAD_GROUP");
    b.pair(350, &groups);
    b.pair(3, "ACAD_LAYOUT");
    b.pair(350, &layouts);
    b.start("DICTIONARY", &root);
    b.pair(100, "AcDbDictionary");
    b.int(281, 1);
    b.start("DICTIONARY", &root);
    b.pair(100, "AcDbDictionary");
    b.int(281, 1);
    let (model_layout, paper_layout) = (format!("{:X}", b.last + 1), format!("{:X}", b.last + 2));
    b.pair(3, "Model");
    b.pair(350, &model_layout);
    b.pair(3, "Layout1");
    b.pair(350, &paper_layout);
    for (name, order, record) in [("Model", 0, &model), ("Layout1", 1, &paper)] {
        b.start("LAYOUT", &layouts);
        b.pair(100, "AcDbPlotSettings");
        b.pair(1, "");
        b.pair(2, "none_device");
        b.pair(4, "");
        b.pair(6, "");
        for c in 40..=49 {
            b.num(c, 0.0)?;
        }
        b.num(140, 0.0)?;
        b.num(141, 0.0)?;
        b.num(142, 1.0)?;
        b.num(143, 1.0)?;
        b.int(70, 688);
        b.int(72, 0);
        b.int(73, 0);
        b.int(74, 5);
        b.pair(7, "");
        b.int(75, 16);
        b.num(147, 1.0)?;
        b.num(148, 0.0)?;
        b.num(149, 0.0)?;
        b.pair(100, "AcDbLayout");
        b.pair(1, name);
        b.int(70, 1);
        b.int(71, order);
        b.xy(10, [0.0, 0.0])?;
        b.xy(11, [420.0, 297.0])?;
        b.xy(12, [0.0, 0.0])?;
        b.xy(14, [0.0, 0.0])?;
        b.xy(15, [0.0, 0.0])?;
        b.num(146, 0.0)?;
        b.xy(13, [0.0, 0.0])?;
        b.num(16, 1.0)?;
        b.num(26, 0.0)?;
        b.num(36, 0.0)?;
        b.num(17, 0.0)?;
        b.num(27, 1.0)?;
        b.num(37, 0.0)?;
        b.int(76, 0);
        b.pair(330, record);
    }
    b.pair(0, "ENDSEC");
    b.pair(0, "EOF");
    // Each block record points at its layout (340), written in place.
    let paper_link = format!("340\n{paper_layout}\n");
    b.s.insert_str(paper_layout_at, &paper_link);
    b.s.insert_str(model_layout_at, &format!("340\n{model_layout}\n"));
    // HEADER, now that every handle is known.
    let mut h = Out::default();
    h.pair(0, "SECTION");
    h.pair(2, "HEADER");
    let var = |h: &mut Out, name: &str| h.pair(9, name);
    var(&mut h, "$ACADVER");
    h.pair(1, "AC1015");
    var(&mut h, "$DWGCODEPAGE");
    h.pair(3, "ANSI_1252");
    var(&mut h, "$INSBASE");
    h.xy(10, [0.0, 0.0])?;
    var(&mut h, "$EXTMIN");
    h.xy(10, at(p.bounds.min(), k))?;
    var(&mut h, "$EXTMAX");
    h.xy(10, at(p.bounds.max(), k))?;
    var(&mut h, "$LTSCALE");
    h.num(40, 1.0)?;
    var(&mut h, "$INSUNITS");
    h.int(70, o.unit.insunits());
    var(&mut h, "$MEASUREMENT");
    h.int(70, o.unit.measurement());
    var(&mut h, "$HANDSEED");
    h.pair(5, &format!("{:X}", b.last + 1));
    h.pair(0, "ENDSEC");
    h.s.push_str(&b.s);
    Ok(h.s.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_places_and_no_trailing_zeros() {
        assert_eq!(three(90.0), "90");
        assert_eq!(three(0.44), "0.44");
        assert_eq!(three(1.2345), "1.234");
        assert_eq!(three(-0.0001), "0");
    }

    #[test]
    fn units_parse_and_scale() {
        assert_eq!(DxfUnit::parse("in"), Some(DxfUnit::In));
        assert_eq!(DxfUnit::parse("cm"), None);
        assert_eq!(DxfUnit::In.per_meter() * 0.0254, 1.0);
    }
}
