//! Dimensions' values, measured in their view's plane from the entities'
//! exact geometry, and their places in the view: what a linear,
//! ordinate, angular, radius or diameter dimension measures, and the
//! points on the sheet it stands on. A dimension that isn't true length
//! in its view is `invalid_param` with `reason: foreshortened`.

use super::feat::{Feat, V3, cross, dot, norm, sub};
use crate::doc::drawing_annotation::Along;
use crate::error::{Code, Error, Result};
use ryu_draw::P;

/// How a view takes the model to its sheet.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Map {
    pub towards: V3,
    pub x: V3,
    pub y: V3,
    /// The anchor's image, and where it lies on the sheet.
    pub centre: P,
    pub at: P,
    /// Sheet millimetres per model metre.
    pub k: f64,
}

impl Map {
    /// `p`'s image in the view's plane, model metres.
    pub(crate) fn image(&self, p: V3) -> P {
        [dot(self.x, p), dot(self.y, p)]
    }

    /// A direction's image.
    pub(crate) fn dir(&self, d: V3) -> P {
        [dot(self.x, d), dot(self.y, d)]
    }

    /// An image point on the sheet.
    pub(crate) fn sheet_of(&self, i: P) -> P {
        [
            self.at[0] + self.k * (i[0] - self.centre[0]),
            self.at[1] + self.k * (i[1] - self.centre[1]),
        ]
    }

    pub(crate) fn sheet(&self, p: V3) -> P {
        self.sheet_of(self.image(p))
    }

    /// Whether a direction lies in the view's plane (seen at true length).
    pub(crate) fn in_plane(&self, d: V3) -> bool {
        dot(d, self.towards).abs() <= 1e-9 * norm(d).max(1e-300)
    }

    /// Whether a direction runs along the line of sight.
    pub(crate) fn end_on(&self, d: V3) -> bool {
        norm(cross(d, self.towards)) <= 1e-9 * norm(d).max(1e-300)
    }
}

fn len2(a: P) -> f64 {
    a[0].hypot(a[1])
}

/// What an entity is in a view's plane: a point, or a line (a segment
/// between two points; `bounded` false for a datum's, which runs on).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    Point {
        img: P,
        model: V3,
    },
    Line {
        a: P,
        b: P,
        model: V3,
        dir: V3,
        bounded: bool,
    },
}

fn refused(reason: &str, message: String) -> Error {
    Error::new(Code::InvalidParam, message).with("reason", reason)
}

/// `f` as a dimension's end sees it in `m`'s view: a vertex, an edge seen
/// end on, a circle's or sphere's centre, a round face's axis seen end
/// on, as points; a straight edge, a planar face seen edge on, a round
/// face's axis seen side on, a datum's line or plane edge on, as lines.
pub(crate) fn shape(m: &Map, f: &Feat, what: &str) -> Result<Shape> {
    let pt = |p: V3| Shape::Point {
        img: m.image(p),
        model: p,
    };
    // A line through `model` along `dir`, over the image span of `pts`.
    let line = |model: V3, dir: V3, pts: &[V3], bounded: bool| -> Shape {
        let d2 = m.dir(dir);
        let l = len2(d2);
        let u = [d2[0] / l, d2[1] / l];
        let o = m.image(model);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in pts {
            let i = m.image(*p);
            let t = (i[0] - o[0]) * u[0] + (i[1] - o[1]) * u[1];
            lo = lo.min(t);
            hi = hi.max(t);
        }
        if !lo.is_finite() {
            (lo, hi) = (0.0, 0.0);
        }
        Shape::Line {
            a: [o[0] + lo * u[0], o[1] + lo * u[1]],
            b: [o[0] + hi * u[0], o[1] + hi * u[1]],
            model,
            dir,
            bounded,
        }
    };
    Ok(match f {
        Feat::Point(p) => pt(*p),
        Feat::Segment(a, b) => {
            let d = sub(*b, *a);
            if m.end_on(d) {
                pt(*a)
            } else {
                line(*a, d, &[*a, *b], true)
            }
        }
        Feat::Circle { c, n, r } => {
            if m.in_plane(*n) {
                // Edge on: a line across it, its diameter long.
                let along = cross(*n, m.towards);
                let l = norm(along);
                let u = [along[0] / l, along[1] / l, along[2] / l];
                let ends = [
                    [c[0] - r * u[0], c[1] - r * u[1], c[2] - r * u[2]],
                    [c[0] + r * u[0], c[1] + r * u[1], c[2] + r * u[2]],
                ];
                line(*c, u, &ends, true)
            } else {
                pt(*c)
            }
        }
        Feat::Sphere { c, .. } => pt(*c),
        Feat::Plane { p, n, pts } => {
            if !m.in_plane(*n) {
                return Err(refused(
                    "not_edge_on",
                    format!("{what} is a planar face the view doesn't see edge on"),
                ));
            }
            line(*p, cross(*n, m.towards), pts, true)
        }
        Feat::Round {
            o, d, pts, circles, ..
        } => {
            if m.end_on(*d) {
                pt(*o)
            } else if m.in_plane(*d) {
                let mut ends: Vec<V3> = pts.clone();
                ends.extend(circles.iter().map(|(c, _)| *c));
                line(*o, *d, &ends, true)
            } else {
                return Err(refused(
                    "foreshortened",
                    format!("{what}'s axis runs neither across nor along the view"),
                ));
            }
        }
        Feat::Line { o, d } => {
            if m.end_on(*d) {
                pt(*o)
            } else {
                line(*o, *d, &[*o], false)
            }
        }
        Feat::DatumPlane { p, n } => {
            if !m.in_plane(*n) {
                return Err(refused(
                    "not_edge_on",
                    format!("{what} is a datum plane the view doesn't see edge on"),
                ));
            }
            line(*p, cross(*n, m.towards), &[*p], false)
        }
        Feat::Other(k) => {
            return Err(refused(
                "not_measurable",
                format!(
                    "{what} is a {k}: a dimension measures points, lines, planes and round faces"
                ),
            ));
        }
    })
}

/// What a linear or ordinate dimension measured: its value (metres), the
/// unit direction it measures along on the sheet, and the point on each
/// end its extension line leaves from (on the sheet).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Linear {
    pub value: f64,
    pub dir: P,
    pub feet: [P; 2],
    /// The sheet's side the dimension line stands to by default: the
    /// unit normal its positive offset goes along.
    pub normal: P,
}

/// Where along unit `u` an end lies, in image metres, and the image point
/// its extension line leaves from: a line must run square to `u`.
fn coordinate(s: &Shape, u: P, what: &str) -> Result<(f64, P, P)> {
    match *s {
        Shape::Point { img, .. } => Ok((img[0] * u[0] + img[1] * u[1], img, img)),
        Shape::Line { a, b, .. } => {
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = len2(d);
            if l > 0.0 && (d[0] * u[0] + d[1] * u[1]).abs() > 1e-9 * l {
                return Err(refused(
                    "not_square",
                    format!("{what} doesn't run square to the direction the dimension measures"),
                ));
            }
            Ok((a[0] * u[0] + a[1] * u[1], a, b))
        }
    }
}

/// Of a line's two ends, the one farther along `n` (where the dimension
/// line stands).
fn foot(a: P, b: P, n: P) -> P {
    if (b[0] - a[0]) * n[0] + (b[1] - a[1]) * n[1] > 0.0 {
        b
    } else {
        a
    }
}

/// The 3D distance from `p` to the line through `o` along `d`.
fn point_line(p: V3, o: V3, d: V3) -> f64 {
    norm(cross(sub(p, o), d)) / norm(d)
}

/// A linear dimension between `a` and `b` (or along `a` alone) measured
/// `along`, in `m`'s view.
pub(crate) fn linear(m: &Map, a: &Shape, b: Option<&Shape>, along: Along) -> Result<Linear> {
    let true_len = |img: f64, model: f64| -> Result<()> {
        if (img - model).abs() <= 1e-9 * model.abs().max(1e-6) {
            Ok(())
        } else {
            Err(refused(
                "foreshortened",
                format!(
                    "the distance measured isn't true length in this view ({:.6} mm seen of {:.6} mm)",
                    img * 1e3,
                    model * 1e3
                ),
            ))
        }
    };
    // One straight edge: its own two ends.
    let (a, b) = match (a, b) {
        (a, Some(b)) => (*a, *b),
        (
            Shape::Line {
                a: p,
                b: q,
                model,
                dir,
                bounded: true,
            },
            None,
        ) => {
            let other = [model[0] + dir[0], model[1] + dir[1], model[2] + dir[2]];
            let _ = other;
            (
                Shape::Point {
                    img: *p,
                    model: *model,
                },
                Shape::Point {
                    img: *q,
                    model: [model[0] + dir[0], model[1] + dir[1], model[2] + dir[2]],
                },
            )
        }
        _ => {
            return Err(refused(
                "one_end",
                "a linear dimension of one entity measures a straight edge seen side on".into(),
            ));
        }
    };
    let u: P = match along {
        Along::Horizontal => [1.0, 0.0],
        Along::Vertical => [0.0, 1.0],
        Along::Aligned => {
            let (pa, pb) = (anchor(&a), anchor(&b));
            let d = match (&a, &b) {
                (Shape::Point { .. }, Shape::Point { .. }) => [pb[0] - pa[0], pb[1] - pa[1]],
                (Shape::Line { a: p, b: q, .. }, other)
                | (other, Shape::Line { a: p, b: q, .. }) => {
                    // Square to the line, towards the other end.
                    let t = [q[0] - p[0], q[1] - p[1]];
                    let l = len2(t);
                    let nn = [-t[1] / l, t[0] / l];
                    let o = anchor(other);
                    let s = (o[0] - p[0]) * nn[0] + (o[1] - p[1]) * nn[1];
                    let s = if matches!(a, Shape::Line { .. }) {
                        s
                    } else {
                        -s
                    };
                    [nn[0] * s, nn[1] * s]
                }
            };
            let l = len2(d);
            if l <= 0.0 {
                return Err(refused(
                    "zero",
                    "the dimension's ends meet: it measures nothing".into(),
                ));
            }
            [d[0] / l, d[1] / l]
        }
    };
    let (ca, a0, a1) = coordinate(&a, u, "a")?;
    let (cb, b0, b1) = coordinate(&b, u, "b")?;
    let value = (cb - ca).abs();
    if along == Along::Aligned {
        // True length: the model's own distance is the one seen.
        let model = match (&a, &b) {
            (Shape::Point { model: p, .. }, Shape::Point { model: q, .. }) => norm(sub(*q, *p)),
            (Shape::Point { model: p, .. }, Shape::Line { model: o, dir, .. })
            | (Shape::Line { model: o, dir, .. }, Shape::Point { model: p, .. }) => {
                point_line(*p, *o, *dir)
            }
            (Shape::Line { model: o, dir, .. }, Shape::Line { model: p, .. }) => {
                point_line(*p, *o, *dir)
            }
        };
        true_len(value, model)?;
    }
    let normal = match along {
        Along::Horizontal => [0.0, 1.0],
        Along::Vertical => [1.0, 0.0],
        Along::Aligned => [-u[1], u[0]],
    };
    let sheet_n = normal;
    let fa = foot(m.sheet_of(a0), m.sheet_of(a1), sheet_n);
    let fb = foot(m.sheet_of(b0), m.sheet_of(b1), sheet_n);
    Ok(Linear {
        value,
        dir: u,
        feet: [fa, fb],
        normal,
    })
}

/// A shape's point an aligned dimension measures from.
fn anchor(s: &Shape) -> P {
    match *s {
        Shape::Point { img, .. } => img,
        Shape::Line { a, b, .. } => [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
    }
}

/// An angular dimension's vertex, its two rays' unit directions on the
/// sheet, its value (radians) and how far each edge reaches from the
/// vertex on the sheet.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Angular {
    pub value: f64,
    pub vertex: P,
    pub rays: [P; 2],
    pub reach: [f64; 2],
}

/// The angle between two straight edges seen at true angle, at their
/// lines' crossing (or `vertex`).
pub(crate) fn angular(m: &Map, a: &Feat, b: &Feat, vertex: Option<&Feat>) -> Result<Angular> {
    let seg = |f: &Feat, what: &str| -> Result<(V3, V3)> {
        match f {
            Feat::Segment(p, q) => Ok((*p, *q)),
            other => Err(refused(
                "not_straight",
                format!(
                    "{what} is a {}: an angle is between straight edges",
                    other.kind()
                ),
            )),
        }
    };
    let (a0, a1) = seg(a, "a")?;
    let (b0, b1) = seg(b, "b")?;
    for (d, w) in [(sub(a1, a0), "a"), (sub(b1, b0), "b")] {
        if !m.in_plane(d) {
            return Err(refused(
                "foreshortened",
                format!("edge {w} runs out of the view's plane: its angle isn't true"),
            ));
        }
    }
    let (p0, p1, q0, q1) = (m.sheet(a0), m.sheet(a1), m.sheet(b0), m.sheet(b1));
    let da = [p1[0] - p0[0], p1[1] - p0[1]];
    let db = [q1[0] - q0[0], q1[1] - q0[1]];
    let det = da[0] * db[1] - da[1] * db[0];
    let v = match vertex {
        Some(Feat::Point(p)) => m.sheet(*p),
        Some(other) => {
            return Err(refused(
                "not_a_vertex",
                format!("an angle's vertex is a vertex, not a {}", other.kind()),
            ));
        }
        None => {
            if det.abs() <= 1e-12 * len2(da) * len2(db) {
                return Err(refused(
                    "not_crossing",
                    "the edges are parallel: they make no angle".into(),
                ));
            }
            let t = ((q0[0] - p0[0]) * db[1] - (q0[1] - p0[1]) * db[0]) / det;
            [p0[0] + t * da[0], p0[1] + t * da[1]]
        }
    };
    // Each ray from the vertex towards the edge's farther end.
    let ray = |s: P, e: P| -> (P, f64) {
        let ds = len2([s[0] - v[0], s[1] - v[1]]);
        let de = len2([e[0] - v[0], e[1] - v[1]]);
        let far = if de >= ds { e } else { s };
        let d = [far[0] - v[0], far[1] - v[1]];
        let l = len2(d).max(1e-12);
        ([d[0] / l, d[1] / l], l)
    };
    let (ra, la) = ray(p0, p1);
    let (rb, lb) = ray(q0, q1);
    let value = ryu_math::atan2(ra[0] * rb[1] - ra[1] * rb[0], ra[0] * rb[0] + ra[1] * rb[1]).abs();
    Ok(Angular {
        value,
        vertex: v,
        rays: [ra, rb],
        reach: [la, lb],
    })
}

/// What a radius or diameter dimension stands on.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Round {
    /// A circle seen as one: its centre and radius on the sheet, and its
    /// radius in the model.
    Circle {
        centre: P,
        r_sheet: f64,
        r: f64,
        sphere: bool,
    },
    /// A cylinder seen side on: its two silhouettes' feet, across it, and
    /// its radius.
    Side { feet: [P; 2], dir: P, r: f64 },
}

/// The circle a radius or diameter of `f` measures, in `m`'s view.
pub(crate) fn round(m: &Map, f: &Feat, diameter: bool) -> Result<Round> {
    let face_on = |n: V3| -> Result<()> {
        if m.end_on(n) {
            Ok(())
        } else {
            Err(refused(
                "foreshortened",
                "the circle is seen as an ellipse in this view: its size isn't true".into(),
            ))
        }
    };
    match f {
        Feat::Circle { c, n, r } => {
            face_on(*n)?;
            Ok(Round::Circle {
                centre: m.sheet(*c),
                r_sheet: m.k * r,
                r: *r,
                sphere: false,
            })
        }
        Feat::Sphere { c, r } => Ok(Round::Circle {
            centre: m.sheet(*c),
            r_sheet: m.k * r,
            r: *r,
            sphere: true,
        }),
        Feat::Round {
            o,
            d,
            r,
            pts,
            circles,
        } => {
            // A cone: its largest circular edge.
            let (c, r) = match r {
                Some(r) => (*o, *r),
                None => match circles.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                    Some((c, r)) => (*c, *r),
                    None => {
                        return Err(refused(
                            "not_round",
                            "the conical face has no circular edge to measure".into(),
                        ));
                    }
                },
            };
            if m.end_on(*d) {
                return Ok(Round::Circle {
                    centre: m.sheet(c),
                    r_sheet: m.k * r,
                    r,
                    sphere: false,
                });
            }
            if !m.in_plane(*d) {
                return Err(refused(
                    "foreshortened",
                    "the face's axis runs neither across nor along the view".into(),
                ));
            }
            if !diameter {
                return Err(refused(
                    "side_on",
                    "a cylinder seen side on is dimensioned by its diameter".into(),
                ));
            }
            let s = shape(m, f, "of")?;
            let Shape::Line { a, b, .. } = s else {
                unreachable!("side on is a line")
            };
            let t = [b[0] - a[0], b[1] - a[1]];
            let l = len2(t).max(1e-12);
            let across = [-t[1] / l, t[0] / l];
            let mid = m.sheet_of([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
            // The silhouettes' feet at the axis's far end from where the
            // dimension stands: the end nearer `+across` on the sheet.
            let _ = pts;
            let rs = m.k * r;
            Ok(Round::Side {
                feet: [
                    [mid[0] - rs * across[0], mid[1] - rs * across[1]],
                    [mid[0] + rs * across[0], mid[1] + rs * across[1]],
                ],
                dir: across,
                r,
            })
        }
        other => Err(refused(
            "not_round",
            format!(
                "a {} has no radius: a radius or diameter is of a circular edge or a cylindrical, conical or spherical face",
                other.kind()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A front view at 1:1: x right, z up, looking along +y.
    fn front() -> Map {
        Map {
            towards: [0.0, -1.0, 0.0],
            x: [1.0, 0.0, 0.0],
            y: [0.0, 0.0, 1.0],
            centre: [0.0, 0.0],
            at: [100.0, 100.0],
            k: 1000.0,
        }
    }

    #[test]
    fn linear_values_are_measured_in_the_views_plane() {
        let m = front();
        let a = shape(&m, &Feat::Point([0.0, 0.0, 0.0]), "a").unwrap();
        let b = shape(&m, &Feat::Point([0.03, 0.5, 0.04]), "b").unwrap();
        assert_eq!(
            linear(&m, &a, Some(&b), Along::Horizontal).unwrap().value,
            0.03
        );
        assert_eq!(
            linear(&m, &a, Some(&b), Along::Vertical).unwrap().value,
            0.04
        );
        // Aligned across a depth of 0.5 m: foreshortened.
        let e = linear(&m, &a, Some(&b), Along::Aligned).unwrap_err();
        assert_eq!(e.details["reason"], "foreshortened");
        let b = shape(&m, &Feat::Point([0.03, 0.0, 0.04]), "b").unwrap();
        let l = linear(&m, &a, Some(&b), Along::Aligned).unwrap();
        assert!((l.value - 0.05).abs() < 1e-15);
        // A vertical edge to a point, horizontally.
        let e = shape(&m, &Feat::Segment([0.01, 0.0, 0.0], [0.01, 0.0, 0.02]), "a").unwrap();
        let l = linear(&m, &e, Some(&b), Along::Horizontal).unwrap();
        assert!((l.value - 0.02).abs() < 1e-15);
        assert_eq!(l.feet[0], [110.0, 120.0]);
        // A slanted edge can't be measured horizontally to.
        let s = shape(&m, &Feat::Segment([0.0; 3], [0.01, 0.0, 0.02]), "a").unwrap();
        let e = linear(&m, &s, Some(&b), Along::Horizontal).unwrap_err();
        assert_eq!(e.details["reason"], "not_square");
        // An edge's own length.
        assert!(
            (linear(&m, &s, None, Along::Aligned).unwrap().value - 0.0005f64.sqrt()).abs() < 1e-15
        );
    }

    #[test]
    fn faces_seen_edge_on_are_lines_and_others_are_refused() {
        let m = front();
        let top = Feat::Plane {
            p: [0.0, 0.0, 0.01],
            n: [0.0, 0.0, 1.0],
            pts: vec![[0.0, 0.0, 0.01], [0.02, 0.0, 0.01], [0.02, 0.01, 0.01]],
        };
        let Shape::Line { a, b, .. } = shape(&m, &top, "a").unwrap() else {
            panic!()
        };
        assert_eq!((a, b), ([0.0, 0.01], [0.02, 0.01]));
        let front_face = Feat::Plane {
            p: [0.0; 3],
            n: [0.0, -1.0, 0.0],
            pts: vec![],
        };
        assert_eq!(
            shape(&m, &front_face, "a").unwrap_err().details["reason"],
            "not_edge_on"
        );
    }

    #[test]
    fn circles_are_true_only_face_on_and_angles_only_in_plane() {
        let m = front();
        let c = Feat::Circle {
            c: [0.0; 3],
            n: [0.0, 1.0, 0.0],
            r: 0.005,
        };
        let Round::Circle { r_sheet, .. } = round(&m, &c, true).unwrap() else {
            panic!()
        };
        assert_eq!(r_sheet, 5.0);
        let tilted = Feat::Circle {
            c: [0.0; 3],
            n: [0.0, 1.0, 1.0],
            r: 0.005,
        };
        assert_eq!(
            round(&m, &tilted, true).unwrap_err().details["reason"],
            "foreshortened"
        );
        let cyl = Feat::Round {
            o: [0.0; 3],
            d: [0.0, 0.0, 1.0],
            r: Some(0.004),
            pts: vec![[0.004, 0.0, 0.0], [0.004, 0.0, 0.01]],
            circles: vec![],
        };
        let Round::Side { feet, .. } = round(&m, &cyl, true).unwrap() else {
            panic!()
        };
        assert!((feet[1][0] - feet[0][0]).abs() - 8.0 < 1e-9);
        assert_eq!(
            round(&m, &cyl, false).unwrap_err().details["reason"],
            "side_on"
        );
        let a = Feat::Segment([0.0; 3], [0.01, 0.0, 0.0]);
        let b = Feat::Segment([0.0; 3], [0.01, 0.0, 0.01]);
        let ang = angular(&m, &a, &b, None).unwrap();
        assert!((ang.value - std::f64::consts::FRAC_PI_4).abs() < 1e-15);
        let c = Feat::Segment([0.0; 3], [0.01, 0.01, 0.0]);
        assert_eq!(
            angular(&m, &a, &c, None).unwrap_err().details["reason"],
            "foreshortened"
        );
    }
}
