//! DXF: a sheet's items as AutoCAD R2000 (`AC1015`) ASCII (design 417).
//!
//! Every segment keeps its kind: a line is a `LINE`, an arc an `ARC` (or a
//! `CIRCLE` when whole), an elliptic arc an `ELLIPSE` on its principal
//! axes, a Bézier piece a `SPLINE` of its degree with its weights, never a
//! polyline. Text is `TEXT` in a style naming Inter, its height Inter's
//! cap height at the item's size. Each stroked item goes on its line
//! kind's layer ([`LineKind::layer`]), which carries the kind's linetype
//! and lineweight as the items give them; a stroke with no kind is on
//! `THIN`, text on `TEXT`, and an item drawn in a colour other than black
//! carries ACI 30 (the warning colour's orange) of its own. A filled path
//! of three or four straight sides (an arrowhead) is a `SOLID`.
//!
//! Coordinates are the sheet's in millimetres, or inches, as the shortest
//! text that reads back to the same bits. The tables, blocks and objects
//! are the ones R2000 readers require, with handles in a fixed order (the
//! same skeleton as `ryuformats::dxf`'s, which ezdxf audits): the same
//! items and unit, the same bytes.

use crate::geom::{P, Segment};
use crate::item::{Anchor, Item};
use crate::style::{Colour, LineKind, Stroke};
use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

/// The file's length unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    #[default]
    Mm,
    In,
}

impl Unit {
    pub fn parse(s: &str) -> Option<Unit> {
        match s {
            "mm" => Some(Unit::Mm),
            "in" => Some(Unit::In),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::In => "in",
        }
    }

    /// How many of the unit a sheet millimetre is.
    fn per_mm(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::In => 1.0 / 25.4,
        }
    }

    fn insunits(self) -> i64 {
        match self {
            Unit::Mm => 4,
            Unit::In => 1,
        }
    }

    fn measurement(self) -> i64 {
        match self {
            Unit::Mm => 1,
            Unit::In => 0,
        }
    }
}

/// The lineweights DXF allows, in hundredths of a millimetre.
const WEIGHTS: [i64; 24] = [
    0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158, 200,
    211,
];

/// The allowed lineweight nearest `mm` (the lesser of a tie).
fn weight(mm: f64) -> i64 {
    let w = mm * 100.0;
    *WEIGHTS
        .iter()
        .min_by(|a, b| {
            ((**a as f64) - w)
                .abs()
                .total_cmp(&((**b as f64) - w).abs())
        })
        .expect("listed")
}

/// An angle in degrees in `[0, 360)`.
fn degrees(a: f64) -> f64 {
    let d = (a * 180.0 / PI).rem_euclid(360.0);
    if d >= 360.0 { 0.0 } else { d }
}

/// Inter's cap height over its em.
fn cap() -> f64 {
    let f = ttf_parser::Face::parse(crate::font::INTER, 0).expect("the bundled font reads");
    let c = f.capital_height().unwrap_or(1490);
    f64::from(c) / f64::from(f.units_per_em())
}

/// Groups, and the handles handed out so far.
#[derive(Default)]
struct Out {
    s: String,
    last: u64,
}

type R = Result<(), String>;

impl Out {
    fn pair(&mut self, code: u16, v: &str) {
        let _ = write!(self.s, "{code:>3}\n{v}\n");
    }

    fn int(&mut self, code: u16, v: i64) {
        self.pair(code, &v.to_string());
    }

    fn num(&mut self, code: u16, x: f64) -> R {
        if !x.is_finite() {
            return Err("a number that isn't finite".into());
        }
        if x == 0.0 {
            self.pair(code, "0");
        } else {
            self.pair(code, &x.to_string());
        }
        Ok(())
    }

    fn xy(&mut self, code: u16, p: P) -> R {
        self.num(code, p[0])?;
        self.num(code + 10, p[1])?;
        self.num(code + 20, 0.0)
    }

    fn handle(&mut self) -> String {
        self.last += 1;
        format!("{:X}", self.last)
    }

    fn start(&mut self, kind: &str, owner: &str) -> String {
        self.pair(0, kind);
        let h = self.handle();
        self.pair(5, &h);
        self.pair(330, owner);
        h
    }

    /// An entity's head: handle, owner, layer, its own colour where it has
    /// one, and its subclass.
    fn entity(&mut self, kind: &str, owner: &str, layer: &str, colour: Colour, class: &str) {
        self.start(kind, owner);
        self.pair(100, "AcDbEntity");
        self.pair(8, layer);
        if colour != Colour::BLACK {
            self.int(62, 30);
        }
        self.pair(100, class);
    }
}

/// A layer's linetype and lineweight, from the first stroke of its kind.
struct LayerStyle {
    name: &'static str,
    dash: Vec<f64>,
    weight: i64,
}

fn layer_of(s: &Stroke) -> &'static str {
    s.kind.unwrap_or(LineKind::Thin).layer()
}

/// An elliptic arc `centre + cos t a + sin t b`, `t` from `from` by
/// `sweep`, on its principal axes: the major axis's end from the centre,
/// the minor over the major, and DXF's counterclockwise parameters.
fn principal(a: P, b: P, from: f64, sweep: f64) -> (P, f64, f64, f64) {
    let dot = |u: P, v: P| u[0] * v[0] + u[1] * v[1];
    let mut t0 = 0.5 * ryu_math::atan2(2.0 * dot(a, b), dot(a, a) - dot(b, b));
    let along = |t: f64| {
        let (s, c) = ryu_math::sin_cos(t);
        [c * a[0] + s * b[0], c * a[1] + s * b[1]]
    };
    let (mut major, mut minor) = (along(t0), along(t0 + PI / 2.0));
    if dot(minor, minor) > dot(major, major) {
        t0 += PI / 2.0;
        (major, minor) = (minor, [-major[0], -major[1]]);
    }
    let ratio = (dot(minor, minor) / dot(major, major)).sqrt().min(1.0);
    // Counterclockwise as DXF reads it when the minor axis is the major
    // turned a quarter counterclockwise.
    let ccw = major[0] * minor[1] - major[1] * minor[0] > 0.0;
    let to = |t: f64| if ccw { t - t0 } else { t0 - t };
    let (s0, s1) = (to(from), to(from + sweep));
    let (lo, hi) = if s0 <= s1 { (s0, s1) } else { (s1, s0) };
    let start = lo.rem_euclid(TAU);
    let end = if sweep.abs() >= TAU - 1e-12 {
        start + TAU
    } else {
        start + (hi - lo)
    };
    (major, ratio, start, end)
}

/// `items` on a sheet of `paper` millimetres as R2000 DXF bytes in `unit`.
/// `Err` for a number that isn't finite.
pub fn write(paper: P, items: &[Item], unit: Unit) -> Result<Vec<u8>, String> {
    let k = unit.per_mm();
    let at = |p: P| [p[0] * k, p[1] * k];
    // Each kind's style, from the first of its strokes; then text's.
    let mut styles: Vec<LayerStyle> = Vec::new();
    for kind in LineKind::ALL {
        let first = items.iter().find_map(|it| match it {
            Item::Path {
                stroke: Some(s), ..
            } if layer_of(s) == kind.layer() => Some(s),
            _ => None,
        });
        styles.push(LayerStyle {
            name: kind.layer(),
            dash: first.map(|s| s.dash.clone()).unwrap_or_default(),
            weight: first.map_or(-3, |s| weight(s.width)),
        });
    }
    let mut b = Out::default();
    b.pair(0, "SECTION");
    b.pair(2, "CLASSES");
    b.pair(0, "ENDSEC");
    b.pair(0, "SECTION");
    b.pair(2, "TABLES");
    let table = |b: &mut Out, name: &str, n: i64| -> String {
        let h = b.start("TABLE", "0");
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
    table(&mut b, "VPORT", 0);
    b.pair(0, "ENDTAB");
    // LTYPE: ByBlock, ByLayer, Continuous, then each dashed layer's.
    let dashed: Vec<&LayerStyle> = styles.iter().filter(|s| !s.dash.is_empty()).collect();
    let t = table(&mut b, "LTYPE", 3 + dashed.len() as i64);
    for (name, dash) in [("ByBlock", &[][..]), ("ByLayer", &[]), ("CONTINUOUS", &[])]
        .into_iter()
        .chain(dashed.iter().map(|s| (s.name, &s.dash[..])))
    {
        record(&mut b, "LTYPE", &t, "AcDbLinetypeTableRecord");
        b.pair(2, name);
        b.int(70, 0);
        b.pair(3, "");
        b.int(72, 65);
        b.int(73, dash.len() as i64);
        b.num(40, dash.iter().sum::<f64>() * k)?;
        for (i, x) in dash.iter().enumerate() {
            b.num(49, if i % 2 == 0 { x * k } else { -x * k })?;
            b.int(74, 0);
        }
    }
    b.pair(0, "ENDTAB");
    // LAYER: 0, each kind's, TEXT.
    let t = table(&mut b, "LAYER", 2 + styles.len() as i64);
    let rows = std::iter::once(("0", "CONTINUOUS", -3))
        .chain(styles.iter().map(|s| {
            let lt = if s.dash.is_empty() {
                "CONTINUOUS"
            } else {
                s.name
            };
            (s.name, lt, s.weight)
        }))
        .chain(std::iter::once(("TEXT", "CONTINUOUS", -3)));
    for (name, lt, w) in rows {
        record(&mut b, "LAYER", &t, "AcDbLayerTableRecord");
        b.pair(2, name);
        b.int(70, 0);
        b.int(62, 7);
        b.pair(6, lt);
        b.int(370, w);
    }
    b.pair(0, "ENDTAB");
    // STYLE: Standard, and Inter.
    let t = table(&mut b, "STYLE", 2);
    for (name, file) in [("Standard", "txt"), ("Inter", "Inter-Regular.ttf")] {
        record(&mut b, "STYLE", &t, "AcDbTextStyleTableRecord");
        b.pair(2, name);
        b.int(70, 0);
        b.num(40, 0.0)?;
        b.num(41, 1.0)?;
        b.num(50, 0.0)?;
        b.int(71, 0);
        b.num(42, 2.5 * k)?;
        b.pair(3, file);
        b.pair(4, "");
    }
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
    let t = table(&mut b, "BLOCK_RECORD", 2);
    let model = record(&mut b, "BLOCK_RECORD", &t, "AcDbBlockTableRecord");
    let model_layout_at = b.s.len();
    b.pair(2, "*Model_Space");
    let paper_rec = record(&mut b, "BLOCK_RECORD", &t, "AcDbBlockTableRecord");
    let paper_layout_at = b.s.len();
    b.pair(2, "*Paper_Space");
    b.pair(0, "ENDTAB");
    b.pair(0, "ENDSEC");
    b.pair(0, "SECTION");
    b.pair(2, "BLOCKS");
    for (owner, name) in [(&model, "*Model_Space"), (&paper_rec, "*Paper_Space")] {
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
    b.pair(0, "SECTION");
    b.pair(2, "ENTITIES");
    let cap = cap();
    for it in items {
        match it {
            Item::Path { path, stroke, fill } => {
                let (layer, colour) = match (stroke, fill) {
                    (Some(s), _) => (layer_of(s), s.colour),
                    (None, Some(f)) => (LineKind::Thin.layer(), *f),
                    (None, None) => continue,
                };
                // An arrowhead: a filled path of three or four lines.
                let corners: Option<Vec<P>> = path
                    .segments
                    .iter()
                    .map(|s| match s {
                        Segment::Line(p) => Some(*p),
                        _ => None,
                    })
                    .collect();
                if let (Some(_), Some(c)) = (fill, &corners)
                    && (2..=3).contains(&c.len())
                {
                    let mut q = vec![path.start];
                    q.extend(c);
                    if q.len() == 3 {
                        q.push(q[2]);
                    }
                    b.entity("SOLID", &model, layer, colour, "AcDbTrace");
                    // SOLID's third and fourth corners are crossed.
                    for (code, p) in [(10, q[0]), (11, q[1]), (12, q[3]), (13, q[2])] {
                        b.xy(code, at(p))?;
                    }
                    continue;
                }
                let mut from = path.start;
                let mut segments: Vec<Segment> = path.segments.clone();
                let end = segments.last().map_or(path.start, Segment::end);
                if path.closed && end != path.start {
                    segments.push(Segment::Line(path.start));
                }
                for s in &segments {
                    match s {
                        Segment::Line(p) => {
                            b.entity("LINE", &model, layer, colour, "AcDbLine");
                            b.xy(10, at(from))?;
                            b.xy(11, at(*p))?;
                        }
                        Segment::Arc {
                            centre,
                            radius,
                            from: a0,
                            sweep,
                        } => {
                            if sweep.abs() >= TAU - 1e-12 {
                                b.entity("CIRCLE", &model, layer, colour, "AcDbCircle");
                                b.xy(10, at(*centre))?;
                                b.num(40, radius * k)?;
                            } else {
                                let (s0, s1) = if *sweep >= 0.0 {
                                    (*a0, a0 + sweep)
                                } else {
                                    (a0 + sweep, *a0)
                                };
                                b.entity("ARC", &model, layer, colour, "AcDbCircle");
                                b.xy(10, at(*centre))?;
                                b.num(40, radius * k)?;
                                b.pair(100, "AcDbArc");
                                b.num(50, degrees(s0))?;
                                b.num(51, degrees(s1))?;
                            }
                        }
                        Segment::Ellipse {
                            centre,
                            a,
                            b: bb,
                            from: t0,
                            sweep,
                        } => {
                            let (major, ratio, start, end) = principal(*a, *bb, *t0, *sweep);
                            b.entity("ELLIPSE", &model, layer, colour, "AcDbEllipse");
                            b.xy(10, at(*centre))?;
                            b.xy(11, at(major))?;
                            b.num(210, 0.0)?;
                            b.num(220, 0.0)?;
                            b.num(230, 1.0)?;
                            b.num(40, ratio)?;
                            b.num(41, start)?;
                            b.num(42, end)?;
                        }
                        Segment::Bezier { points, weights } => {
                            let n = points.len();
                            let rational = weights
                                .as_ref()
                                .is_some_and(|w| w.iter().any(|x| *x != w[0]));
                            b.entity("SPLINE", &model, layer, colour, "AcDbSpline");
                            b.num(210, 0.0)?;
                            b.num(220, 0.0)?;
                            b.num(230, 1.0)?;
                            b.int(70, 8 | if rational { 4 } else { 0 });
                            b.int(71, n as i64);
                            b.int(72, 2 * (n as i64 + 1));
                            b.int(73, n as i64 + 1);
                            b.int(74, 0);
                            b.num(42, 1e-10)?;
                            b.num(43, 1e-10)?;
                            for i in 0..2 * (n + 1) {
                                b.num(40, if i <= n { 0.0 } else { 1.0 })?;
                            }
                            if rational {
                                for x in weights.iter().flatten() {
                                    b.num(41, *x)?;
                                }
                            }
                            b.xy(10, at(from))?;
                            for q in points {
                                b.xy(10, at(*q))?;
                            }
                        }
                    }
                    from = s.end();
                }
            }
            Item::Text {
                text,
                at: p,
                size,
                angle,
                anchor,
                colour,
            } => {
                b.entity("TEXT", &model, "TEXT", *colour, "AcDbText");
                b.xy(10, at(*p))?;
                b.num(40, size * cap * k)?;
                b.pair(1, text);
                b.num(50, degrees(*angle))?;
                b.pair(7, "Inter");
                let h = match anchor {
                    Anchor::Start => 0,
                    Anchor::Middle => 1,
                    Anchor::End => 2,
                };
                if h != 0 {
                    b.int(72, h);
                    b.xy(11, at(*p))?;
                }
                b.pair(100, "AcDbText");
                b.int(73, 0);
            }
        }
    }
    b.pair(0, "ENDSEC");
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
    for (name, order, rec) in [("Model", 0, &model), ("Layout1", 1, &paper_rec)] {
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
        b.xy(11, at(paper))?;
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
        b.pair(330, rec);
    }
    b.pair(0, "ENDSEC");
    b.pair(0, "EOF");
    b.s.insert_str(paper_layout_at, &format!("340\n{paper_layout}\n"));
    b.s.insert_str(model_layout_at, &format!("340\n{model_layout}\n"));
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
    h.xy(10, [0.0, 0.0])?;
    var(&mut h, "$EXTMAX");
    h.xy(10, at(paper))?;
    var(&mut h, "$LIMMIN");
    h.num(10, 0.0)?;
    h.num(20, 0.0)?;
    var(&mut h, "$LIMMAX");
    h.num(10, paper[0] * k)?;
    h.num(20, paper[1] * k)?;
    var(&mut h, "$LTSCALE");
    h.num(40, 1.0)?;
    var(&mut h, "$INSUNITS");
    h.int(70, unit.insunits());
    var(&mut h, "$MEASUREMENT");
    h.int(70, unit.measurement());
    var(&mut h, "$HANDSEED");
    h.pair(5, &format!("{:X}", b.last + 1));
    h.pair(0, "ENDSEC");
    h.s.push_str(&b.s);
    Ok(h.s.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn lineweights_snap_to_dxfs_own() {
        assert_eq!(weight(0.6), 60);
        assert_eq!(weight(0.25), 25);
        assert_eq!(weight(0.5), 50);
        assert_eq!(weight(0.3), 30);
    }

    #[test]
    fn conjugate_axes_become_principal_ones() {
        // A circle's quarter from conjugate axes at 45°: a stays its radius.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let (major, ratio, start, end) = principal([3.0, 0.0], [0.0, 1.0], 0.0, PI / 2.0);
        assert!(close(major[0], 3.0) && close(major[1], 0.0) && close(ratio, 1.0 / 3.0));
        assert!(close(start, 0.0) && close(end, PI / 2.0));
        // Run backwards, the same arc counterclockwise.
        let (_, _, s1, e1) = principal([3.0, 0.0], [0.0, 1.0], PI / 2.0, -PI / 2.0);
        assert!(close(s1, 0.0) && close(e1, PI / 2.0));
        // Axes given minor first: the major is found.
        let (m, r, _, _) = principal([0.0, 1.0], [3.0 * s, 3.0 * s], 0.0, TAU);
        assert!(close(
            ryu_math::hypot(m[0], m[1]),
            (m[0] * m[0] + m[1] * m[1]).sqrt()
        ));
        assert!(r < 1.0);
    }
}
