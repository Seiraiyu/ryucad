//! Test helpers, behind the `testing` feature: a seeded generator, golden
//! hashes, seeded symbols and features, and checks on shapes. Used by this
//! crate's tests and benchmarks, ryudfm's tests and the fuzz targets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::geom::{Arc, Dir, Edge, Mdeg, Place, Point, Rect, Region};
use crate::model::{Feature, FeatureKind, Features, Polarity, Symbol};
use crate::shape::{Piece, Shape};
use crate::symbol::{CornerSet, Corners, Plating, StandardSymbol, Thermal, ThermalForm};

/// A 64-bit linear congruential generator (Knuth's MMIX constants): the
/// same numbers on every platform.
#[derive(Clone, Debug)]
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Lcg {
        Lcg(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    /// A whole number in `[lo, hi]`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % ((hi - lo) as u64 + 1)) as i64
    }

    pub fn coin(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Hashes outputs per named section (FNV-1a, 64 bits) and checks them
/// against a committed file of `name hash` lines; `UPDATE_GOLDEN=1`
/// rewrites the file instead.
pub struct Golden {
    path: PathBuf,
    sections: BTreeMap<String, u64>,
}

/// One section's running hash.
pub struct Section<'a> {
    hash: &'a mut u64,
}

impl Section<'_> {
    pub fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            *self.hash ^= u64::from(b);
            *self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }

    pub fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    fn point(&mut self, p: Point) {
        self.i64(p.x);
        self.i64(p.y);
    }

    fn edge(&mut self, e: &Edge) {
        match e {
            Edge::Segment(s) => {
                self.str("L");
                self.point(s.a);
                self.point(s.b);
            }
            Edge::Arc(a) => {
                self.str(if a.dir == Dir::Ccw { "A" } else { "C" });
                self.point(a.start);
                self.point(a.end);
                self.point(a.centre);
            }
        }
    }

    fn region(&mut self, r: &Region) {
        self.str("R");
        for c in std::iter::once(&r.outer).chain(&r.holes) {
            self.str("(");
            c.edges().iter().for_each(|e| self.edge(e));
        }
    }

    /// Every bit of a shape, or its error.
    pub fn shape(&mut self, s: &Result<Shape, crate::shape::ShapeError>) {
        let Ok(s) = s else {
            self.str(&format!("{s:?}"));
            return;
        };
        for p in &s.parts {
            self.str(if p.polarity == Polarity::Positive {
                "+"
            } else {
                "-"
            });
            match &p.piece {
                Piece::Region(r) => self.region(r),
                Piece::Stroke(st) => {
                    self.str(&format!("S{:?}", st.pen));
                    st.path.iter().for_each(|e| self.edge(e));
                }
                Piece::Swept { path, pen } => {
                    self.str("W");
                    self.edge(path);
                    pen.iter().for_each(|r| self.region(r));
                }
            }
        }
        self.str(&format!("{:?}{:?}{}", s.approximate, s.missing, s.skipped));
    }
}

impl Golden {
    pub fn new(path: impl AsRef<Path>) -> Golden {
        Golden {
            path: path.as_ref().to_path_buf(),
            sections: BTreeMap::new(),
        }
    }

    pub fn section(&mut self, name: &str) -> Section<'_> {
        Section {
            hash: self.sections.entry(name.to_string()).or_insert(FNV_OFFSET),
        }
    }

    /// The sections as they stand, for comparing two runs in a test.
    pub fn sections_for_test(&self) -> &BTreeMap<String, u64> {
        &self.sections
    }

    fn render(&self) -> String {
        self.sections
            .iter()
            .map(|(n, h)| format!("{n} {h:016x}\n"))
            .collect()
    }

    /// Checks every section against the file, or rewrites it when
    /// `UPDATE_GOLDEN=1`. Panics naming the sections that differ.
    pub fn finish(self) {
        let actual = self.render();
        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&self.path, &actual).expect("write golden file");
            return;
        }
        let expected = std::fs::read_to_string(&self.path).unwrap_or_else(|_| {
            panic!("missing {}; run with UPDATE_GOLDEN=1", self.path.display())
        });
        if expected != actual {
            let exp: BTreeMap<_, _> = expected.lines().filter_map(|l| l.split_once(' ')).collect();
            let act: BTreeMap<_, _> = actual.lines().filter_map(|l| l.split_once(' ')).collect();
            let bad: Vec<_> = exp
                .keys()
                .chain(act.keys())
                .filter(|k| exp.get(*k) != act.get(*k))
                .collect();
            panic!("golden mismatch in {bad:?}\nexpected:\n{expected}actual:\n{actual}");
        }
    }
}

/// A size in `[lo, hi]` micrometres, in nanometres.
fn um(g: &mut Lcg, lo: i64, hi: i64) -> i64 {
    g.range(lo, hi) * 1000
}

fn corners(g: &mut Lcg, w: i64, h: i64, chamfers: bool) -> Corners {
    let which = CornerSet::of(&(1..=4).filter(|_| g.coin()).collect::<Vec<u8>>()).expect("1 to 4");
    let size = g.range(0, w.min(h) / 2);
    match g.range(0, if chamfers { 2 } else { 1 }) {
        0 => Corners::SHARP,
        1 => Corners::rounded(size, which),
        _ => Corners::chamfered(size, which),
    }
}

/// A standard symbol with parameters that make a shape: every kind, sizes
/// from 10 µm to 5 mm.
pub fn seeded_symbol(g: &mut Lcg) -> StandardSymbol {
    use StandardSymbol as S;
    let w = um(g, 10, 5000);
    let h = um(g, 10, 5000);
    let (big, small) = (w.max(h), w.min(h));
    let thermal = |g: &mut Lcg, form: ThermalForm, at45: bool| {
        let spokes = if at45 {
            [0u32, 1, 2, 4, 8][g.range(0, 4) as usize]
        } else {
            [0u32, 1, 2, 3, 4, 8][g.range(0, 5) as usize]
        };
        let angle = if at45 {
            g.range(0, 7) * 45_000
        } else {
            g.range(0, 359_999)
        };
        S::Thermal(Thermal {
            form,
            angle: Mdeg::new(angle),
            spokes,
            gap: small / 20,
        })
    };
    match g.range(0, 23) {
        0 => S::Round { d: w },
        1 => S::Square { s: w },
        2 => S::Rect {
            w,
            h,
            corners: corners(g, w, h, true),
        },
        3 => S::Oval { w, h },
        4 => S::Diamond { w, h },
        5 => S::Octagon {
            w,
            h,
            r: g.range(0, small / 2),
        },
        6 => S::DonutRound {
            od: big,
            id: g.range(small / 4, big - big / 4),
        },
        7 => S::DonutSquare {
            od: big,
            id: big / 2,
            corners: corners(g, big, big, false),
        },
        8 => S::DonutSquareRound {
            od: big,
            id: big / 2,
        },
        9 => S::DonutRect {
            w,
            h,
            lw: small / 4,
            corners: corners(g, w, h, false),
        },
        10 => S::DonutOval {
            w,
            h,
            lw: small / 4,
        },
        11 => S::HexagonH {
            w,
            h,
            r: g.range(0, w / 2),
        },
        12 => S::HexagonV {
            w,
            h,
            r: g.range(0, h / 2),
        },
        13 => S::ButterflyRound { d: w },
        14 => S::ButterflySquare { s: w },
        15 => S::Triangle { base: w, h },
        16 => S::HalfOval { w: big, h: small },
        17 => S::Ellipse { w, h },
        18 => S::Moire {
            ring_width: small / 10,
            ring_gap: small / 10,
            rings: g.range(1, 5) as u32,
            line_width: small / 20,
            line_length: big,
            line_angle: Mdeg::new(g.range(0, 359_999)),
        },
        19 => S::Hole {
            d: w,
            plating: Plating::Plated,
            tol_plus: 0,
            tol_minus: 0,
        },
        20 => S::Null {
            ext: g.range(0, 9) as u32,
        },
        21 => S::Trapezoid {
            w,
            h,
            dx: g.range(-(h - 1), h - 1) / 2,
            dy: 0,
        },
        _ => match g.range(0, 8) {
            0 => thermal(
                g,
                ThermalForm::RoundRounded {
                    od: big,
                    id: big / 2,
                },
                false,
            ),
            1 => thermal(
                g,
                ThermalForm::RoundSquared {
                    od: big,
                    id: big / 2,
                },
                false,
            ),
            2 => thermal(
                g,
                ThermalForm::Square {
                    os: big,
                    is: big / 2,
                    corners: Corners::SHARP,
                    open: false,
                },
                false,
            ),
            3 => thermal(
                g,
                ThermalForm::Square {
                    os: big,
                    is: big / 2,
                    corners: Corners::SHARP,
                    open: true,
                },
                true,
            ),
            4 => thermal(
                g,
                ThermalForm::SquareRound {
                    os: big,
                    id: big / 2,
                },
                false,
            ),
            5 => S::Thermal(Thermal {
                form: ThermalForm::Line {
                    os: big,
                    is: big / 2,
                },
                angle: Mdeg::new(45_000),
                spokes: 4,
                gap: small / 20,
            }),
            6 => {
                let open = g.coin();
                thermal(
                    g,
                    ThermalForm::Rect {
                        w,
                        h,
                        ring: small / 4,
                        corners: Corners::SHARP,
                        open,
                    },
                    true,
                )
            }
            7 => thermal(
                g,
                ThermalForm::Oval {
                    w,
                    h,
                    ring: small / 4,
                },
                true,
            ),
            _ => {
                let rounded = g.coin();
                thermal(
                    g,
                    ThermalForm::Oblong {
                        w,
                        h,
                        ring: small / 4,
                        rounded,
                    },
                    true,
                )
            }
        },
    }
}

fn point(g: &mut Lcg, reach: i64) -> Point {
    Point::new(g.range(-reach, reach), g.range(-reach, reach))
}

/// A table of `symbols` seeded symbols and `count` features drawn with
/// them, within ±`reach` nm: lines, arcs, pads at any angle and mirror,
/// and surfaces with a hole.
pub fn seeded_features(g: &mut Lcg, symbols: usize, count: usize, reach: i64) -> Features {
    let table: Vec<Symbol> = (0..symbols.max(1))
        .map(|i| match i {
            0 => Symbol::Standard(StandardSymbol::Round { d: um(g, 10, 500) }),
            1 => Symbol::Standard(StandardSymbol::Square { s: um(g, 10, 500) }),
            _ => Symbol::Standard(seeded_symbol(g)),
        })
        .collect();
    let mut features = Vec::with_capacity(count);
    for _ in 0..count {
        let symbol = g.range(0, table.len() as i64 - 1) as u32;
        let kind = match g.range(0, 3) {
            0 => FeatureKind::Line {
                a: point(g, reach),
                b: point(g, reach),
                symbol,
            },
            1 => {
                let centre = point(g, reach);
                let r = g.range(1_000, 2_000_000);
                let a0 = Mdeg::new(g.range(0, 359_999));
                let a1 = Mdeg::new(g.range(0, 359_999));
                let on = |m: Mdeg| {
                    Place {
                        at: centre,
                        rot: m,
                        ..Place::IDENTITY
                    }
                    .point(Point::new(r, 0))
                };
                let dir = if g.coin() { Dir::Ccw } else { Dir::Cw };
                FeatureKind::Arc {
                    arc: Arc {
                        start: on(a0),
                        end: on(a1),
                        centre,
                        dir,
                    },
                    symbol,
                }
            }
            2 => FeatureKind::Pad {
                symbol,
                place: Place {
                    at: point(g, reach),
                    rot: Mdeg::new(if g.coin() {
                        g.range(0, 3) * 90_000
                    } else {
                        g.range(0, 359_999)
                    }),
                    mirror: g.coin(),
                    scale: None,
                },
            },
            _ => {
                let c = point(g, reach);
                let (w, h) = (um(g, 100, 5000), um(g, 100, 5000));
                let outer = crate::shape::rect(c.x, c.y, c.x + w, c.y + h, &Corners::SHARP)
                    .expect("a size");
                let hole = crate::shape::rect(
                    c.x + w / 4,
                    c.y + h / 4,
                    c.x + w / 2,
                    c.y + h / 2,
                    &Corners::SHARP,
                )
                .expect("a size");
                FeatureKind::Surface {
                    regions: vec![Region::ring(outer, hole)],
                }
            }
        };
        let polarity = if g.range(0, 4) == 0 {
            Polarity::Negative
        } else {
            Polarity::Positive
        };
        features.push(Feature::new(kind, polarity));
    }
    Features::new(table, features)
}

/// Points along a region's contours: every edge's ends and, along an arc,
/// one every eighth of a radian at most.
fn flatten(c: &crate::geom::Contour) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for e in c.edges() {
        match e {
            Edge::Segment(s) => out.push((s.a.x as f64, s.a.y as f64)),
            Edge::Arc(a) => {
                let sweep = a.sweep();
                let n = ((sweep * 8.0) as usize).max(2);
                let (cx, cy) = (a.centre.x as f64, a.centre.y as f64);
                let r = a.radius();
                let a0 = ryu_math::atan2(a.start.y as f64 - cy, a.start.x as f64 - cx);
                let sign = if a.dir == Dir::Ccw { 1.0 } else { -1.0 };
                for k in 0..n {
                    let t = a0 + sign * sweep * k as f64 / n as f64;
                    let (s, co) = ryu_math::sin_cos(t);
                    out.push((cx + r * co, cy + r * s));
                }
            }
        }
    }
    out
}

fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Whether a contour's flattened polygon has no two non-neighbouring
/// edges crossing properly. Approximate, for tests.
pub fn is_simple(c: &crate::geom::Contour) -> bool {
    let p = flatten(c);
    let n = p.len();
    if n < 3 {
        return false;
    }
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (a, b, c2, d) = (p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]);
            let (d1, d2) = (orient(a, b, c2), orient(a, b, d));
            let (d3, d4) = (orient(c2, d, a), orient(c2, d, b));
            if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                return false;
            }
        }
    }
    true
}

/// What a shape gets wrong: a region's contour that isn't simple or runs
/// the wrong way, or a flattened point outside the shape's box.
pub fn shape_faults(s: &Shape) -> Vec<String> {
    let mut out = Vec::new();
    let Some(b) = s.bounds() else { return out };
    let inside = |(x, y): (f64, f64)| {
        let r = Rect {
            min: b.min.offset(-1, -1),
            max: b.max.offset(1, 1),
        };
        (r.min.x as f64) <= x
            && x <= (r.max.x as f64)
            && (r.min.y as f64) <= y
            && y <= (r.max.y as f64)
    };
    for (i, p) in s.parts.iter().enumerate() {
        if let Piece::Region(r) = &p.piece {
            if r.outer.orientation() != Dir::Ccw {
                out.push(format!("part {i}: outer contour clockwise"));
            }
            for (k, c) in std::iter::once(&r.outer).chain(&r.holes).enumerate() {
                if !is_simple(c) {
                    out.push(format!("part {i}: contour {k} crosses itself"));
                }
                if !flatten(c).into_iter().all(inside) {
                    out.push(format!("part {i}: contour {k} leaves the box"));
                }
            }
            for (k, h) in r.holes.iter().enumerate() {
                if h.orientation() != Dir::Cw {
                    out.push(format!("part {i}: hole {k} counterclockwise"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Contour;

    #[test]
    fn the_generator_is_fixed() {
        let mut g = Lcg::new(7);
        let a: Vec<i64> = (0..4).map(|_| g.range(0, 99)).collect();
        let mut h = Lcg::new(7);
        assert_eq!(a, (0..4).map(|_| h.range(0, 99)).collect::<Vec<_>>());
        assert!(a.iter().all(|v| (0..=99).contains(v)));
    }

    #[test]
    fn fnv_known_values() {
        let mut g = Golden::new("unused");
        g.section("s").str("a");
        assert_eq!(g.render(), "s af63dc4c8601ec8c\n");
    }

    #[test]
    fn bowties_are_not_simple() {
        let p = |x, y| Point::new(x, y);
        let edges = |pts: &[Point]| {
            Contour::new(
                (0..pts.len())
                    .map(|i| Edge::line(pts[i], pts[(i + 1) % pts.len()]))
                    .collect(),
            )
            .unwrap()
        };
        assert!(is_simple(&edges(&[p(0, 0), p(10, 0), p(10, 10), p(0, 10)])));
        assert!(!is_simple(&edges(&[
            p(0, 0),
            p(10, 10),
            p(10, 0),
            p(0, 10)
        ])));
    }
}
