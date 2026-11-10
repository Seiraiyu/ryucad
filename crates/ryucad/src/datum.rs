//! Datum features to geometry: each construction of a plane, an axis or
//! a point from what its references resolve to ([`crate::geometry`]), in
//! the part's frame, f64 meters through `ryukei::math` and ryukei's
//! `Frame`, so a datum is the same bits everywhere.
//!
//! A plane's frame is what a sketch on it sees: `offset` and `parallel`
//! carry `from`'s origin and `x` across, `angle` turns them about its
//! axis, so sketches on derived planes keep their base's orientation;
//! every other construction takes a face sketch's rule (the part origin
//! projected, the part's `x` projected, its `y` where `x` is along the
//! normal). A written `origin` (a point) and `x` (an axis), projected
//! onto the plane, replace either. `flip` turns the normal over, keeping
//! `x`.

use crate::doc::datum::{Along, AxisBy, CurveAt, Datum, End, PlaneBy, PointBy};
use crate::doc::geometry::GeomRef;
use crate::error::{Code, Error, Result};
use crate::geometry::{
    At, Bounded, Chain, Geom, Line, Plane, Want, default_frame, one, plane_of, resolve,
};
use ryukei::geom::{Curve, Frame, Point3, Rigid, Surface, Vector3};
use ryukei::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use serde_json::{Value, json};

/// What a datum rebuilds to: its plane, axis or point, and a hash of
/// their bits (equal hashes, equal datums).
#[derive(Clone, Debug, PartialEq)]
pub struct Made {
    pub geom: Geom,
    pub hash: String,
}

/// How closely a point is found along a curve: a hundredth of the
/// resolution of the length bisected towards.
const ALONG_REL: f64 = 1e-12;

impl Made {
    pub fn new(geom: Geom) -> Made {
        let mut h = blake3::Hasher::new();
        let mut bits = |xs: &[f64]| {
            for x in xs {
                h.update(&x.to_bits().to_le_bytes());
            }
        };
        let p = |p: Point3| [p.x, p.y, p.z];
        let v = |v: Vector3| [v.x, v.y, v.z];
        match &geom {
            Geom::Plane(pl) => {
                bits(&[0.0]);
                bits(&p(pl.point));
                bits(&v(pl.normal));
                bits(&p(pl.frame.origin()));
                bits(&v(pl.frame.x().get()));
                bits(&v(pl.frame.z().get()));
            }
            Geom::Axis(a) => {
                bits(&[1.0]);
                bits(&p(a.origin));
                bits(&v(a.dir.get()));
            }
            Geom::Point(q) => {
                bits(&[2.0]);
                bits(&p(*q));
            }
            Geom::Curve(_) => unreachable!("a datum is a plane, an axis or a point"),
        }
        Made {
            geom,
            hash: h.finalize().to_hex()[..32].to_owned(),
        }
    }

    /// `plane`, `axis` or `point`.
    pub fn kind(&self) -> &'static str {
        match self.geom {
            Geom::Plane(_) => "plane",
            Geom::Axis(_) => "axis",
            Geom::Point(_) => "point",
            Geom::Curve(_) => "curve",
        }
    }

    /// Its geometry as rebuild reports and `inspect` give it, in meters:
    /// `{kind, origin_m, normal | direction, x}`.
    pub fn report(&self) -> Value {
        let p = |p: Point3| json!([p.x, p.y, p.z]);
        let v = |v: Vector3| json!([v.x, v.y, v.z]);
        match &self.geom {
            Geom::Plane(pl) => json!({
                "kind": "plane",
                "origin_m": p(pl.frame.origin()),
                "normal": v(pl.frame.z().get()),
                "x": v(pl.frame.x().get()),
            }),
            Geom::Axis(a) => json!({
                "kind": "axis",
                "origin_m": p(a.origin),
                "direction": v(a.dir.get()),
            }),
            Geom::Point(q) => json!({"kind": "point", "origin_m": p(*q)}),
            Geom::Curve(_) => unreachable!("a datum is a plane, an axis or a point"),
        }
    }
}

/// Builds datum `at.owner` from what its references resolve to.
pub fn build(at: &At, d: &Datum) -> Result<Made> {
    let geom = match d {
        Datum::Plane(p) => {
            let mut plane = plane_by(at, &p.by)?;
            if p.origin.is_some() || p.x.is_some() {
                plane = framed(at, plane, p.origin.as_ref(), p.x.as_ref())?;
            }
            if p.flip {
                plane = Plane {
                    point: plane.point,
                    normal: -plane.normal,
                    frame: plane.frame.flipped(),
                };
            }
            Geom::Plane(plane)
        }
        Datum::Axis(a) => {
            let axis = axis_by(at, &a.by)?;
            let axis = if a.flip {
                Line::along(axis.origin, -axis.dir)
            } else {
                axis
            };
            Geom::Axis(axis)
        }
        Datum::Point(p) => Geom::Point(point_by(at, &p.by)?),
    };
    Ok(Made::new(geom))
}

/// `datum_degenerate` for the owner, naming the references involved.
fn degenerate(at: &At, msg: &str, refs: &[&GeomRef]) -> Error {
    Error::new(Code::DatumDegenerate, format!("{}: {msg}", at.owner))
        .with("feature", at.owner)
        .with(
            "references",
            refs.iter().map(|g| g.text(&at.part.id)).collect::<Vec<_>>(),
        )
}

fn plane(at: &At, role: &str, g: &GeomRef) -> Result<Plane> {
    Ok(one(at, role, g, Want::Plane)?.plane())
}

fn axis(at: &At, role: &str, g: &GeomRef) -> Result<Line> {
    Ok(one(at, role, g, Want::Axis)?.axis())
}

fn point(at: &At, role: &str, g: &GeomRef) -> Result<Point3> {
    Ok(one(at, role, g, Want::Point)?.point())
}

fn curve(at: &At, role: &str, g: &GeomRef) -> Result<Chain> {
    Ok(one(at, role, g, Want::Curve)?.curve())
}

/// A frame `frame` moved to `origin`, keeping its axes bit for bit.
fn moved(frame: &Frame, origin: Point3) -> Result<Frame> {
    Frame::from_axes(origin, frame.z(), frame.x()).map_err(|e| {
        Error::new(
            Code::InvalidPlacement,
            "a datum plane outside the ±500 m size box",
        )
        .with("kernel", format!("{e:?}"))
    })
}

fn plane_by(at: &At, by: &PlaneBy) -> Result<Plane> {
    match by {
        PlaneBy::Offset { from, distance } => {
            let p = plane(at, "from", from)?;
            let d = distance.meters();
            let frame = moved(&p.frame, p.frame.origin() + p.normal * d)?;
            Ok(Plane {
                point: p.point + p.normal * d,
                normal: p.normal,
                frame,
            })
        }
        PlaneBy::Angle { from, about, angle } => {
            let p = plane(at, "from", from)?;
            let a = axis(at, "about", about)?;
            if a.dir.get().dot(p.normal).abs() > ANGULAR_RESOLUTION {
                return Err(degenerate(
                    at,
                    "its axis isn't parallel to the plane it turns",
                    &[from, about],
                ));
            }
            // The plane brought onto the axis, then turned about it.
            let shift = p.normal * (a.origin - p.frame.origin()).dot(p.normal);
            let onto = moved(&p.frame, p.frame.origin() + shift)?;
            let c = a.origin.to_vector();
            let turn = Rigid::translation(c)
                .then_after(&Rigid::rotation(a.dir, angle.radians()))
                .then_after(&Rigid::translation(-c));
            Ok(plane_of(onto.transformed(&turn)))
        }
        PlaneBy::ThreePoints { points } => {
            let [a, b, c] = [
                point(at, "points.0", &points[0])?,
                point(at, "points.1", &points[1])?,
                point(at, "points.2", &points[2])?,
            ];
            let n = (b - a).cross(c - a);
            let scale = (b - a).length().max((c - a).length());
            if n.length() <= LINEAR_RESOLUTION * scale {
                return Err(degenerate(
                    at,
                    "its three points lie on one line",
                    &[&points[0], &points[1], &points[2]],
                ));
            }
            let n = n.normalize().expect("not collinear").get();
            Ok(Plane {
                point: a,
                normal: n,
                frame: default_frame(a, n)?,
            })
        }
        PlaneBy::Parallel { from, through } => {
            let p = plane(at, "from", from)?;
            let q = point(at, "through", through)?;
            let shift = p.normal * (q - p.frame.origin()).dot(p.normal);
            Ok(Plane {
                point: q,
                normal: p.normal,
                frame: moved(&p.frame, p.frame.origin() + shift)?,
            })
        }
        PlaneBy::AxisPoint {
            axis: ax,
            point: pt,
        } => {
            let a = axis(at, "axis", ax)?;
            let q = point(at, "point", pt)?;
            let n = a.dir.get().cross(q - a.origin);
            if n.length() <= LINEAR_RESOLUTION {
                return Err(degenerate(at, "its point lies on its axis", &[ax, pt]));
            }
            let n = n.normalize().expect("off the axis").get();
            Ok(Plane {
                point: a.origin,
                normal: n,
                frame: default_frame(a.origin, n)?,
            })
        }
        PlaneBy::NormalTo {
            curve: cg,
            at: where_,
        } => {
            let chain = curve(at, "curve", cg)?;
            let (first, last) = (&chain.pieces[0], &chain.pieces[chain.pieces.len() - 1]);
            let (c, t, through) = match where_ {
                CurveAt::Start => (first, first.range[0], first.curve.point(first.range[0])),
                CurveAt::End => (last, last.range[1], last.curve.point(last.range[1])),
                CurveAt::Point(g) => {
                    let q = point(at, "at", g)?;
                    let (c, t) = foot_on(at, &chain, q, cg, g)?;
                    (c, t, q)
                }
            };
            let tangent = c.curve.tangent(t).ok_or_else(|| {
                degenerate(at, "its curve has no tangent where it is taken", &[cg])
            })?;
            let n = tangent.get();
            Ok(Plane {
                point: through,
                normal: n,
                frame: default_frame(through, n)?,
            })
        }
        PlaneBy::Midplane { between } => {
            let p = plane(at, "between.0", &between[0])?;
            let q = plane(at, "between.1", &between[1])?;
            let cross = p.normal.cross(q.normal);
            if cross.length() <= ANGULAR_RESOLUTION {
                // Halfway, the first's normal.
                let gap = (q.point - p.point).dot(p.normal);
                let mid = p.point + p.normal * (gap / 2.0);
                return Ok(Plane {
                    point: mid,
                    normal: p.normal,
                    frame: default_frame(mid, p.normal)?,
                });
            }
            let n = (p.normal - q.normal)
                .normalize()
                .expect("not parallel")
                .get();
            let line = meet(&p, &q).expect("not parallel");
            Ok(Plane {
                point: line.origin,
                normal: n,
                frame: default_frame(line.origin, n)?,
            })
        }
    }
}

/// The line two planes that aren't parallel meet in, along `n₁ × n₂`,
/// through its point nearest the part origin.
fn meet(p: &Plane, q: &Plane) -> Option<Line> {
    let (n1, n2) = (p.normal, q.normal);
    let d = n1.cross(n2);
    if d.length() <= ANGULAR_RESOLUTION {
        return None;
    }
    // The point of the line nearest the origin: on both planes and on the
    // plane through the origin normal to the line.
    let (h1, h2) = (n1.dot(p.point.to_vector()), n2.dot(q.point.to_vector()));
    let dd = d.dot(d);
    let o = (n2.cross(d) * h1 + d.cross(n1) * h2) * (1.0 / dd);
    Some(Line::along(Point3::ORIGIN + o, d.normalize()?))
}

/// The piece of `chain` holding `q`'s nearest foot on it, and the foot's
/// parameter there: of the pieces whose foot lies inside their range, the
/// nearest, the first on a tie (a foot at a joint is the earlier piece's
/// end). Off every piece is `datum_no_intersection`.
fn foot_on<'c>(
    at: &At,
    chain: &'c Chain,
    q: Point3,
    cg: &GeomRef,
    g: &GeomRef,
) -> Result<(&'c Bounded, f64)> {
    let mut best: Option<(&Bounded, f64, f64)> = None;
    let mut last = None;
    for c in &chain.pieces {
        match foot(at, c, q, cg, g) {
            Ok(t) => {
                let d = c.curve.point(t).distance(q);
                if best.is_none_or(|(_, _, b)| d < b) {
                    best = Some((c, t, d));
                }
            }
            Err(e) => last = Some(e),
        }
    }
    match best {
        Some((c, t, _)) => Ok((c, t)),
        None => Err(last.expect("a chain has a piece")),
    }
}

/// The parameter of `q`'s foot on `c`, inside its range: a circle's moved
/// by whole turns to land there. Off the range is `datum_no_intersection`.
fn foot(at: &At, c: &Bounded, q: Point3, cg: &GeomRef, g: &GeomRef) -> Result<f64> {
    let mut t = c.curve.project(q).param;
    if let Some(period) = c.curve.period() {
        while t < c.range[0] - ANGULAR_RESOLUTION {
            t += period;
        }
        while t > c.range[1] + ANGULAR_RESOLUTION {
            t -= period;
        }
    }
    let slack = LINEAR_RESOLUTION;
    let inside = |t: f64| {
        (c.range[0]..=c.range[1]).contains(&t)
            || c.curve.point(c.range[0]).distance(c.curve.point(t)) <= slack
            || c.curve.point(c.range[1]).distance(c.curve.point(t)) <= slack
    };
    if !inside(t) {
        return Err(Error::new(
            Code::DatumNoIntersection,
            format!("{}: its point's foot lies off its curve", at.owner),
        )
        .with("feature", at.owner)
        .with("references", [cg.text(&at.part.id), g.text(&at.part.id)]));
    }
    Ok(t.clamp(c.range[0], c.range[1]))
}

/// `plane` with its frame's origin at `origin` (projected onto it) and
/// its `x` along `x` (projected), each where written.
fn framed(at: &At, p: Plane, origin: Option<&GeomRef>, x: Option<&GeomRef>) -> Result<Plane> {
    let n = p.normal;
    let o = match origin {
        Some(g) => {
            let q = point(at, "origin", g)?;
            q - n * (q - p.point).dot(n)
        }
        None => p.frame.origin(),
    };
    let hint = match x {
        Some(g) => {
            let d = axis(at, "x", g)?.dir.get();
            let d = d - n * d.dot(n);
            if d.length() <= ANGULAR_RESOLUTION {
                return Err(degenerate(at, "its x runs along its normal", &[g]));
            }
            d
        }
        None => p.frame.x().get(),
    };
    let frame = Frame::new(o, n, hint).map_err(|e| {
        Error::new(Code::InvalidPlacement, "a datum plane that gives no frame")
            .with("kernel", format!("{e:?}"))
    })?;
    Ok(Plane {
        point: p.point,
        normal: p.normal,
        frame,
    })
}

fn axis_by(at: &At, by: &AxisBy) -> Result<Line> {
    match by {
        AxisBy::Of { axis: a } => axis(at, "axis", a),
        AxisBy::TwoPoints { points } => {
            let a = point(at, "points.0", &points[0])?;
            let b = point(at, "points.1", &points[1])?;
            if (b - a).length() <= LINEAR_RESOLUTION {
                return Err(degenerate(
                    at,
                    "its two points are one: an axis needs a length",
                    &[&points[0], &points[1]],
                ));
            }
            Ok(Line::between(a, b).expect("a length"))
        }
        AxisBy::Planes { between } => {
            let p = plane(at, "between.0", &between[0])?;
            let q = plane(at, "between.1", &between[1])?;
            meet(&p, &q).ok_or_else(|| {
                degenerate(
                    at,
                    "its planes are parallel and never meet",
                    &[&between[0], &between[1]],
                )
            })
        }
        AxisBy::Normal {
            plane: pl,
            point: pt,
        } => {
            let p = plane(at, "plane", pl)?;
            let q = point(at, "point", pt)?;
            Ok(Line::along(q, p.normal.normalize().expect("a unit normal")))
        }
    }
}

fn point_by(at: &At, by: &PointBy) -> Result<Point3> {
    match by {
        PointBy::Of { point: g } => point(at, "point", g),
        PointBy::Coordinates {
            at: xyz,
            plane: in_,
        } => {
            let [x, y, z] = xyz.each_ref().map(|q| q.meters());
            match in_ {
                None => Ok(Point3::new(x, y, z)),
                Some(g) => Ok(plane(at, "in", g)?.frame.to_world(x, y, z)),
            }
        }
        PointBy::Along {
            curve: g,
            from,
            how,
        } => along(at, g, *from, how),
        PointBy::Intersection { of } => intersection(at, of),
        PointBy::Projection { point: pg, onto } => {
            let q = point(at, "point", pg)?;
            match resolve(at, "onto", onto, &[Want::Plane, Want::Axis])? {
                Geom::Plane(p) => Ok(q - p.normal * (q - p.point).dot(p.normal)),
                Geom::Axis(a) => {
                    let d = a.dir.get();
                    Ok(a.origin + d * (q - a.origin).dot(d))
                }
                _ => unreachable!("asked for a plane or an axis"),
            }
        }
        PointBy::Midpoint { points } => {
            let a = point(at, "points.0", &points[0])?;
            let b = point(at, "points.1", &points[1])?;
            Ok(a + (b - a) * 0.5)
        }
    }
}

/// The certified length of `c` from `t0` to `t1`.
fn length(c: &Bounded, t0: f64, t1: f64) -> Result<f64> {
    if t1 <= t0 {
        return Ok(0.0);
    }
    ryukei::measure::curve_length(&c.curve, [t0, t1], ALONG_REL)
        .map(|v| v.value())
        .ok_or_else(|| {
            Error::new(
                Code::RebuildFailed,
                "a curve whose length can't be certified",
            )
        })
}

/// The point `how` far along curve `g` from its end `from`: on a path,
/// along the piece that far takes it to; exact on a line or a circle,
/// else bisected on the curve's parameter until the length is found to
/// resolution.
fn along(at: &At, g: &GeomRef, from: End, how: &Along) -> Result<Point3> {
    let chain = curve(at, "curve", g)?;
    let lengths = chain
        .pieces
        .iter()
        .map(|c| length(c, c.range[0], c.range[1]))
        .collect::<Result<Vec<f64>>>()?;
    let total: f64 = lengths.iter().sum();
    let s = match how {
        Along::Distance(q) => q.meters(),
        Along::Fraction(q) => q.number() * total,
    };
    if s > total + LINEAR_RESOLUTION {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{}: {s} m along its curve is past its end: it is {total} m long",
                at.owner
            ),
        )
        .with("feature", at.owner)
        .with("pointer", "/distance")
        .with("length_m", total));
    }
    let s = s.min(total);
    // The piece that far along, from the end measured from, and how far
    // into it.
    let mut order: Vec<usize> = (0..chain.pieces.len()).collect();
    if from == End::End {
        order.reverse();
    }
    let (mut k, mut s) = (order[order.len() - 1], s);
    for &i in &order {
        if s <= lengths[i] {
            k = i;
            break;
        }
        s -= lengths[i];
    }
    let c = &chain.pieces[k];
    let s = s.min(lengths[k]);
    let [t0, t1] = c.range;
    let t = match &c.curve {
        // Lines run at unit speed, circles at their radius.
        Curve::Line(_) => match from {
            End::Start => t0 + s,
            End::End => t1 - s,
        },
        Curve::Circle(k) => match from {
            End::Start => t0 + s / k.radius(),
            End::End => t1 - s / k.radius(),
        },
        _ => {
            let (mut lo, mut hi) = (t0, t1);
            for _ in 0..200 {
                let mid = 0.5 * (lo + hi);
                let l = match from {
                    End::Start => length(c, t0, mid)?,
                    End::End => length(c, mid, t1)?,
                };
                let before = match from {
                    End::Start => l < s,
                    End::End => l > s,
                };
                if before {
                    lo = mid;
                } else {
                    hi = mid;
                }
                if c.curve.point(lo).distance(c.curve.point(hi)) <= LINEAR_RESOLUTION / 2.0 {
                    break;
                }
            }
            0.5 * (lo + hi)
        }
    };
    Ok(c.curve.point(t))
}

/// Where the two references of an intersection meet: a plane and an axis
/// or a curve, or two axes.
fn intersection(at: &At, of: &[GeomRef; 2]) -> Result<Point3> {
    let none = |what: &str| {
        Error::new(Code::DatumNoIntersection, format!("{}: {what}", at.owner))
            .with("feature", at.owner)
            .with(
                "references",
                [of[0].text(&at.part.id), of[1].text(&at.part.id)],
            )
    };
    // The plane is whichever of the two is one.
    let firsts = resolve(at, "of.0", &of[0], &[Want::Plane, Want::Axis, Want::Curve])?;
    let (plane_side, other, other_role) = match firsts {
        Geom::Plane(p) => (Some(p), &of[1], "of.1"),
        g => match resolve(at, "of.1", &of[1], &[Want::Plane, Want::Axis])? {
            Geom::Plane(p) => {
                return cut(at, p, g, &none);
            }
            Geom::Axis(b) => {
                let Geom::Axis(a) = g else {
                    return Err(none("a curve meets only a plane"));
                };
                return axes_meet(a, b).ok_or_else(|| none("its two axes don't meet"));
            }
            _ => unreachable!("asked for a plane or an axis"),
        },
    };
    let p = plane_side.expect("a plane");
    let g = resolve(at, other_role, other, &[Want::Axis, Want::Curve])?;
    cut(at, p, g, &none)
}

/// Where an axis or a curve `g` meets plane `p`.
fn cut(at: &At, p: Plane, g: Geom, none: &dyn Fn(&str) -> Error) -> Result<Point3> {
    match g {
        Geom::Axis(a) => {
            let d = a.dir.get();
            let along = d.dot(p.normal);
            if along.abs() <= ANGULAR_RESOLUTION {
                return Err(none("its axis runs parallel to its plane"));
            }
            let t = (p.point - a.origin).dot(p.normal) / along;
            Ok(a.origin + d * t)
        }
        Geom::Curve(chain) => {
            let surface = Surface::Plane(ryukei::geom::Plane::new(
                Frame::new(p.point, p.normal, p.frame.x().get()).map_err(|e| {
                    Error::new(Code::Internal, "a plane that isn't one")
                        .with("kernel", format!("{e:?}"))
                })?,
            ));
            // Each piece's crossings in order along the chain, one where
            // pieces meet at a crossing.
            let (mut points, mut on) = (Vec::<Point3>::new(), 0usize);
            for c in &chain.pieces {
                let hits = ryukei::intersect::curve_surface(
                    &c.curve,
                    c.range,
                    &surface,
                    LINEAR_RESOLUTION,
                )
                .map_err(|e| {
                    Error::new(
                        Code::RebuildFailed,
                        format!("{}: where its curve meets its plane is uncertain", at.owner),
                    )
                    .with("feature", at.owner)
                    .with("kernel", format!("{e:?}"))
                })?;
                for h in &hits {
                    match h {
                        ryukei::intersect::CurveHit::Point { point, .. } => {
                            let again = points
                                .last()
                                .is_some_and(|q| q.distance(*point) <= LINEAR_RESOLUTION);
                            if !again {
                                points.push(*point);
                            }
                        }
                        ryukei::intersect::CurveHit::On { .. } => on += 1,
                    }
                }
            }
            // A closed path's last piece ends where its first starts: a
            // crossing at that joint is the first piece's start hit and
            // the last piece's end hit, one point.
            let (start, end) = chain.ends();
            if start.distance(end) <= LINEAR_RESOLUTION
                && points.len() > 1
                && points[0].distance(points[points.len() - 1]) <= LINEAR_RESOLUTION
            {
                points.pop();
            }
            let count = points.len() + on;
            match (points.as_slice(), on) {
                ([one], 0) => Ok(*one),
                ([], 0) => Err(none("its curve doesn't meet its plane")),
                _ => Err(Error::new(
                    Code::DatumAmbiguous,
                    format!(
                        "{}: its curve meets its plane {} times; a point is where it meets once",
                        at.owner, count
                    ),
                )
                .with("feature", at.owner)
                .with("count", count)),
            }
        }
        _ => unreachable!("an axis or a curve"),
    }
}

/// Where two axes meet within resolution: halfway between their nearest
/// points. Parallel axes, and skew ones, don't.
fn axes_meet(a: Line, b: Line) -> Option<Point3> {
    let (d1, d2) = (a.dir.get(), b.dir.get());
    let n = d1.cross(d2);
    if n.length() <= ANGULAR_RESOLUTION {
        return None;
    }
    let w = a.origin - b.origin;
    let (aa, bb, cc) = (d1.dot(d1), d1.dot(d2), d2.dot(d2));
    let (dd, ee) = (d1.dot(w), d2.dot(w));
    let den = aa * cc - bb * bb;
    let s = (bb * ee - cc * dd) / den;
    let t = (aa * ee - bb * dd) / den;
    let (p, q) = (a.origin + d1 * s, b.origin + d2 * t);
    (p.distance(q) <= LINEAR_RESOLUTION).then(|| p + (q - p) * 0.5)
}

/// The request fields a datum type takes, by their request names.
pub fn request_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "plane" => &[
            "by", "from", "distance", "about", "angle", "points", "point", "through", "axis",
            "curve", "at", "between", "origin", "x", "flip",
        ],
        "axis" => &["by", "axis", "points", "point", "between", "plane", "flip"],
        _ => &[
            "by",
            "points",
            "point",
            "coordinates",
            "in",
            "curve",
            "from",
            "distance",
            "fraction",
            "between",
            "onto",
        ],
    }
}

/// A datum of type `ty` from `current` (none for a new one) with the
/// request's fields laid over it: references as text in `part`
/// (`<part>/<feature>`, a bare feature ID, `xy`, …), quantities as
/// expressions. A new `by` keeps only the fields it takes, as a chamfer's
/// form does; `""` takes an optional reference away.
pub fn from_given(
    ty: &str,
    current: Option<&Datum>,
    f: &crate::proto::FeatureFields,
    part: &crate::doc::id::Id,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<crate::doc::files::Feature> {
    use crate::doc::datum::constructions;
    let geom = |t: &str, at: &str| crate::server::geom_json(t, part, at);
    let mut v = current.map_or_else(|| json!({}), Datum::fields);
    let by = match (&f.by, current) {
        (Some(b), _) => b.clone(),
        (None, Some(d)) => d.by().to_owned(),
        (None, None) => {
            let names: Vec<&str> = constructions(ty).iter().map(|(n, _)| *n).collect();
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a {ty} says how it is built: by is one of {}",
                    names.join(", ")
                ),
            )
            .with("param", "by")
            .with("known", names));
        }
    };
    let takes: &[&str] = constructions(ty)
        .iter()
        .find(|(n, _)| *n == by)
        .map(|(_, f)| *f)
        .ok_or_else(|| {
            let names: Vec<&str> = constructions(ty).iter().map(|(n, _)| *n).collect();
            Error::new(
                Code::InvalidParam,
                format!("a {ty} is built by one of {}, not {by}", names.join(", ")),
            )
            .with("param", "by")
            .with("known", names)
        })?;
    if current.is_some_and(|d| d.by() != by)
        && let Some(o) = v.as_object_mut()
    {
        o.retain(|k, _| {
            takes.contains(&k.as_str()) || ["origin", "x", "flip"].contains(&k.as_str())
        });
    }
    v["by"] = json!(by);
    let set = |v: &mut Value, k: &str, x: Value| {
        if let Some(o) = v.as_object_mut() {
            o.insert(k.to_owned(), x);
        }
    };
    let unset = |v: &mut Value, k: &str| {
        if let Some(o) = v.as_object_mut() {
            o.remove(k);
        }
    };
    // References, one each.
    for (k, x) in [
        ("about", &f.about),
        ("through", &f.through),
        ("curve", &f.curve),
        ("onto", &f.onto),
        ("axis", &f.axis),
        ("plane", &f.plane),
    ] {
        if let Some(t) = x {
            set(&mut v, k, geom(t, &format!("/{k}"))?);
        }
    }
    for (k, x) in [("origin", &f.origin), ("x", &f.x), ("in", &f.in_plane)] {
        match x.as_deref() {
            None => {}
            Some("") => unset(&mut v, k),
            Some(t) => set(&mut v, k, geom(t, &format!("/{k}"))?),
        }
    }
    // `from`: a plane, or for a point along a curve its end.
    if let Some(t) = &f.from {
        let x = if ty == "point" {
            json!(t)
        } else {
            geom(t, "/from")?
        };
        set(&mut v, "from", x);
    }
    // `at`: a curve's end, or a point.
    if let Some(t) = &f.at {
        let x = match t.as_str() {
            "start" | "end" => json!(t),
            t => geom(t, "/at")?,
        };
        set(&mut v, "at", x);
    }
    // Points: a list where the construction takes several, else one.
    let one_point = f.point.as_ref().map(|p| vec![p.clone()]);
    if let Some(ps) = f.points.as_ref().or(one_point.as_ref()) {
        let list = ps
            .iter()
            .enumerate()
            .map(|(i, t)| geom(t, &format!("/points/{i}")))
            .collect::<Result<Vec<_>>>()?;
        if takes.contains(&"points") {
            set(&mut v, "points", json!(list));
        } else if takes.contains(&"point") {
            let [one] = list.as_slice() else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} by {by} takes one point, not {}", list.len()),
                )
                .with("param", "points"));
            };
            set(&mut v, "point", one.clone());
        } else {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a {ty} by {by} takes no points"),
            )
            .with("param", "points"));
        }
    }
    if let Some(bs) = &f.between {
        let key = if takes.contains(&"of") {
            "of"
        } else {
            "between"
        };
        let list = bs
            .iter()
            .enumerate()
            .map(|(i, t)| geom(t, &format!("/{key}/{i}")))
            .collect::<Result<Vec<_>>>()?;
        set(&mut v, key, json!(list));
    }
    if let Some(c) = &f.coordinates {
        set(&mut v, "at", json!(c));
    }
    for (k, x) in [
        ("distance", &f.distance),
        ("angle", &f.angle),
        ("fraction", &f.fraction),
    ] {
        if let Some(q) = x {
            set(&mut v, k, q.clone());
            // A distance along a curve, or a fraction of it: one or the
            // other.
            if ty == "point" {
                match k {
                    "distance" => unset(&mut v, "fraction"),
                    "fraction" => unset(&mut v, "distance"),
                    _ => {}
                }
            }
        }
    }
    match f.flip.as_ref().map(crate::proto::Flip::on).transpose()? {
        Some(true) => set(&mut v, "flip", json!(true)),
        Some(false) => unset(&mut v, "flip"),
        None => {}
    }
    // What a construction doesn't take is refused, by its request name.
    if let Some(o) = v.as_object() {
        for k in o.keys() {
            let fine = k == "by"
                || takes.contains(&k.as_str())
                || (ty == "plane" && ["origin", "x", "flip"].contains(&k.as_str()))
                || (ty == "axis" && k == "flip");
            if !fine {
                let name = match k.as_str() {
                    "point" => "points",
                    "of" => "between",
                    "at" if by == "coordinates" => "coordinates",
                    k => k,
                };
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} by {by} has no {name}"),
                )
                .with("param", name));
            }
        }
    }
    v["format"] = json!(crate::doc::datum::FORMAT);
    v["kind"] = json!(crate::doc::files::FEATURE_KIND);
    v["type"] = json!(ty);
    crate::doc::files::Feature::from_json(&v, unit, env)
}

/// Refuses datum `id` whose references don't resolve to the kinds their
/// fields want now; what they make of each other (parallel, collinear)
/// only rebuild can tell.
pub fn check(
    part: &crate::doc::project::Part,
    order: &[crate::doc::id::Id],
    id: &crate::doc::id::Id,
    d: &Datum,
    results: &dyn crate::rebuild::Results,
) -> Result<()> {
    let at = At {
        part,
        order,
        owner: id,
        results,
    };
    for (role, g) in d.geometry() {
        resolve(&at, &role, g, wants(d, &role))?;
    }
    Ok(())
}

/// What a datum's field (`role`, as [`Datum::geometry`] names it) wants
/// its reference to give.
pub fn wants(d: &Datum, role: &str) -> &'static [Want] {
    let field = role.split('.').next().unwrap_or(role);
    match (d, field) {
        (_, "from" | "between" | "plane" | "in") => &[Want::Plane],
        (_, "about" | "axis" | "x") => &[Want::Axis],
        (_, "points" | "point" | "through" | "origin" | "at") => &[Want::Point],
        (_, "curve") => &[Want::Curve],
        (_, "onto") => &[Want::Plane, Want::Axis],
        (Datum::Point(_), "of") => match role {
            "of.0" => &[Want::Plane, Want::Axis, Want::Curve],
            _ => &[Want::Plane, Want::Axis, Want::Curve],
        },
        _ => &[Want::Plane, Want::Axis, Want::Point, Want::Curve],
    }
}

/// A datum as output reports it: its file's fields, every reference as
/// text, every length and angle as written and as a value.
pub fn report(part: &crate::doc::id::Id, d: &Datum) -> Value {
    let mut v = d.fields();
    for (role, g) in d.geometry() {
        let at = format!("/{}", role.replace('.', "/"));
        if let Some(x) = v.pointer_mut(&at) {
            *x = json!(g.text(part));
        }
    }
    for (k, q) in d.quantities() {
        let at = format!("/{}", k.replace('.', "/"));
        if let Some(x) = v.pointer_mut(&at) {
            *x = q.report();
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::call;
    use crate::session::Session;
    use crate::sketch::tests::{edge_between, face_along, vertex_at, with_box};
    use serde_json::json;

    /// The datum `d` of part `p` as rebuild reports it.
    fn datum(s: &mut Session, d: &str) -> Value {
        let r = call(s, "rebuild", json!({"part": "p"})).unwrap();
        let f = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == d)
            .unwrap_or_else(|| panic!("{r}"))
            .clone();
        assert_eq!(f["ok"], true, "{f}");
        f["datum"].clone()
    }

    fn add(s: &mut Session, fields: Value) -> Result<Value> {
        let mut f = fields;
        f["part"] = json!("p");
        call(s, "feature.add", f)
    }

    fn near(v: &Value, want: [f64; 3]) {
        for (i, w) in want.iter().enumerate() {
            let got = v[i].as_f64().unwrap();
            assert!((got - w).abs() < 1e-12, "{v} against {want:?}");
        }
    }

    #[test]
    fn planes_by_every_construction() {
        let (_dir, mut s, body) = with_box();
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.005]),
            vertex_at(&body, [0.02, 0.0, 0.005]),
        );
        let front_top = format!("p/b/{}", edge_between(&body, &a, &b));
        // 30 mm above the top, carrying its frame across.
        add(
            &mut s,
            json!({"type": "plane", "id": "d1", "by": "offset", "from": top,
                           "distance": "30 mm"}),
        )
        .unwrap();
        let d1 = datum(&mut s, "d1");
        assert_eq!(d1["kind"], "plane");
        near(&d1["origin_m"], [0.0, 0.0, 0.035]);
        near(&d1["normal"], [0.0, 0.0, 1.0]);
        near(&d1["x"], [1.0, 0.0, 0.0]);
        // Turned 90° about the top's front edge: its normal is the front's.
        add(
            &mut s,
            json!({"type": "plane", "id": "d2", "by": "angle", "from": top,
                           "about": front_top, "angle": "90 deg"}),
        )
        .unwrap();
        let d2 = datum(&mut s, "d2");
        let n = &d2["normal"];
        assert!(n[1].as_f64().unwrap().abs() > 1.0 - 1e-12, "{d2}");
        assert!(d2["origin_m"][1].as_f64().unwrap().abs() < 1e-12, "{d2}");
        // Through three of the box's corners: the top again.
        let c = vertex_at(&body, [0.0, 0.01, 0.005]);
        add(
            &mut s,
            json!({"type": "plane", "id": "d3", "by": "three_points",
                           "points": [format!("p/b/{a}"), format!("p/b/{b}"), format!("p/b/{c}")]}),
        )
        .unwrap();
        let d3 = datum(&mut s, "d3");
        near(&d3["normal"], [0.0, 0.0, 1.0]);
        near(&d3["origin_m"], [0.0, 0.0, 0.005]);
        // xy through a corner of the top, and through an axis and a point.
        add(
            &mut s,
            json!({"type": "plane", "id": "d4", "by": "parallel", "from": "xy",
                           "through": format!("p/b/{c}")}),
        )
        .unwrap();
        near(&datum(&mut s, "d4")["origin_m"], [0.0, 0.0, 0.005]);
        add(
            &mut s,
            json!({"type": "plane", "id": "d5", "by": "axis_point", "axis": "x",
                           "point": format!("p/b/{c}")}),
        )
        .unwrap();
        let d5 = datum(&mut s, "d5");
        assert!(d5["normal"][0].as_f64().unwrap().abs() < 1e-12, "{d5}");
        // Normal to the front top edge at its end, and halfway between xy
        // and the 30 mm plane.
        add(
            &mut s,
            json!({"type": "plane", "id": "d6", "by": "normal_to",
                           "curve": front_top, "at": "end"}),
        )
        .unwrap();
        let d6 = datum(&mut s, "d6");
        assert!(
            d6["normal"][0].as_f64().unwrap().abs() > 1.0 - 1e-12,
            "{d6}"
        );
        add(
            &mut s,
            json!({"type": "plane", "id": "d7", "by": "midplane",
                           "between": ["xy", "p/d1"]}),
        )
        .unwrap();
        near(&datum(&mut s, "d7")["origin_m"], [0.0, 0.0, 0.0175]);
        // Flipped, with its own origin and x.
        add(
            &mut s,
            json!({"type": "plane", "id": "d8", "by": "offset", "from": "xy",
                           "distance": 2, "flip": true, "origin": format!("p/b/{c}"),
                           "x": "y"}),
        )
        .unwrap();
        let d8 = datum(&mut s, "d8");
        near(&d8["normal"], [0.0, 0.0, -1.0]);
        near(&d8["x"], [0.0, 1.0, 0.0]);
        near(&d8["origin_m"], [0.0, 0.01, 0.002]);
    }

    #[test]
    fn axes_and_points() {
        let (_dir, mut s, body) = with_box();
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.01, 0.005]),
        );
        add(
            &mut s,
            json!({"type": "axis", "id": "a1", "by": "two_points",
                           "points": [format!("p/b/{a}"), format!("p/b/{b}")]}),
        )
        .unwrap();
        let a1 = datum(&mut s, "a1");
        near(&a1["origin_m"], [0.0, 0.0, 0.0]);
        let l = (0.02f64 * 0.02 + 0.01 * 0.01 + 0.005 * 0.005).sqrt();
        near(&a1["direction"], [0.02 / l, 0.01 / l, 0.005 / l]);
        add(
            &mut s,
            json!({"type": "axis", "id": "a2", "by": "planes",
                           "between": ["xy", "zx"], "flip": true}),
        )
        .unwrap();
        near(&datum(&mut s, "a2")["direction"], [1.0, 0.0, 0.0]);
        add(
            &mut s,
            json!({"type": "axis", "id": "a3", "by": "normal", "plane": "xy",
                           "point": format!("p/b/{b}")}),
        )
        .unwrap();
        near(&datum(&mut s, "a3")["origin_m"], [0.02, 0.01, 0.005]);
        add(
            &mut s,
            json!({"type": "point", "id": "q1", "by": "midpoint",
                           "points": [format!("p/b/{a}"), format!("p/b/{b}")]}),
        )
        .unwrap();
        near(&datum(&mut s, "q1")["origin_m"], [0.01, 0.005, 0.0025]);
        add(
            &mut s,
            json!({"type": "point", "id": "q2", "by": "along", "curve": "p/a1",
                           "from": "start", "fraction": 0.5}),
        )
        .unwrap_err();
        add(
            &mut s,
            json!({"type": "point", "id": "q3", "by": "intersection",
                           "between": ["p/a2", "yz"]}),
        )
        .unwrap();
        near(&datum(&mut s, "q3")["origin_m"], [0.0, 0.0, 0.0]);
        add(
            &mut s,
            json!({"type": "point", "id": "q4", "by": "projection",
                           "point": format!("p/b/{b}"), "onto": "z"}),
        )
        .unwrap();
        near(&datum(&mut s, "q4")["origin_m"], [0.0, 0.0, 0.005]);
        add(
            &mut s,
            json!({"type": "point", "id": "q5", "by": "coordinates",
                           "coordinates": ["1 mm", "2 mm", "3 mm"]}),
        )
        .unwrap();
        near(&datum(&mut s, "q5")["origin_m"], [0.001, 0.002, 0.003]);
    }

    #[test]
    fn points_along_edges() {
        let (_dir, mut s, body) = with_box();
        let (a, b) = (
            vertex_at(&body, [0.0, 0.0, 0.0]),
            vertex_at(&body, [0.02, 0.0, 0.0]),
        );
        let e = format!("p/b/{}", edge_between(&body, &a, &b));
        add(
            &mut s,
            json!({"type": "point", "id": "q1", "by": "along", "curve": e,
                           "from": "start", "fraction": 0.25}),
        )
        .unwrap();
        let q = datum(&mut s, "q1");
        let x = q["origin_m"][0].as_f64().unwrap();
        assert!(
            (x - 0.005).abs() < 1e-12 || (x - 0.015).abs() < 1e-12,
            "{q}"
        );
        add(
            &mut s,
            json!({"type": "point", "id": "q2", "by": "along", "curve": e,
                           "from": "end", "distance": "30 mm"}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
        let f = &r.details["parts"][0]["features"][2];
        assert_eq!(f["error"]["code"], "invalid_param", "{f}");
        assert_eq!(f["error"]["details"]["length_m"], 0.02);
    }

    #[test]
    fn refusals_name_what_wont_do() {
        let (_dir, mut s, body) = with_box();
        let a = vertex_at(&body, [0.0, 0.0, 0.0]);
        // Three points on one line, two planes that never meet, an axis
        // not parallel to the plane it turns, a point on its axis.
        for (id, f, code) in [
            (
                "d1",
                json!({"type": "plane", "by": "three_points",
                          "points": ["origin", format!("p/b/{a}"), "origin"]}),
                "datum_degenerate",
            ),
            (
                "d2",
                json!({"type": "axis", "by": "planes", "between": ["xy", "xy"]}),
                "datum_degenerate",
            ),
            (
                "d3",
                json!({"type": "plane", "by": "angle", "from": "xy", "about": "z",
                          "angle": 10}),
                "datum_degenerate",
            ),
            (
                "d4",
                json!({"type": "plane", "by": "axis_point", "axis": "x", "point": "origin"}),
                "datum_degenerate",
            ),
            (
                "d5",
                json!({"type": "point", "by": "intersection", "between": ["x", "x"]}),
                "datum_no_intersection",
            ),
            (
                "d6",
                json!({"type": "point", "by": "intersection", "between": ["x", "xy"]}),
                "datum_no_intersection",
            ),
        ] {
            let mut f = f;
            f["id"] = json!(id);
            add(&mut s, f).unwrap();
            let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
            let got = r.details["parts"][0]["features"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["id"] == id)
                .unwrap()
                .clone();
            assert_eq!(got["error"]["code"], code, "{got}");
            call(&mut s, "feature.delete", json!({"part": "p", "id": id})).unwrap();
        }
        // A plane where a point goes: refused when written.
        let e = add(
            &mut s,
            json!({"type": "axis", "by": "two_points",
                                    "points": ["xy", "origin"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind);
        assert_eq!(e.details["got"], "origin plane");
    }
}
