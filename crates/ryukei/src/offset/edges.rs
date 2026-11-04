//! What each edge becomes: the curve where its faces' offsets meet, or,
//! where they move apart, two rails and the fill rolled between them.

use super::classify::{Class, Plan};
use crate::geom::{
    Circle, Curve, Curve2, Cylinder, Frame, Line, Pipe, Point3, Sphere, Surface, Torus, Vector3,
};
use crate::intersect::{Failure, SurfaceHit, fit_along, surface_surface_in};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{EdgeId, FinId, Sense};
use crate::{Error, OffsetCase, OffsetError};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Samples at which an exact moved curve must hold.
const SAMPLES: usize = 17;

/// A new curve: where faces' offsets meet, or a rail.
#[derive(Clone, Debug)]
pub(crate) struct NewCurve {
    pub(crate) curve: Curve,
    /// The stretch of its parameter it is found over: the edge's interval
    /// for a curve moved with it, a branch's range for a closing edge's.
    pub(crate) range: [f64; 2],
    /// Its period where it is closed and may be cut anywhere.
    pub(crate) period: Option<f64>,
    pub(crate) tolerance: f64,
    /// Its pcurves on the offsets of the edge's faces, in the order of its
    /// fins (along the edge first), by its own parameter; a rail has one
    /// on its own face only.
    pub(crate) pcurves: [Option<OnFace>; 2],
    /// It runs against the edge.
    pub(crate) reversed: bool,
}

/// A pcurve and the range of it an edge's parameter maps to.
pub(crate) type OnFace = (Curve2, [f64; 2]);

/// What an edge becomes.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum EdgeGeo {
    /// One curve, where the offsets meet (or the edge itself, kept).
    One(NewCurve),
    /// Where the offsets move apart: the rail on each face's offset (in
    /// the order of the edge's fins) and the fill between them, of radius
    /// `|d|`, with its sense.
    Open {
        rails: [NewCurve; 2],
        fill: Surface,
        sense: Sense,
        /// In an envelope, an inverted fill about a circle no larger than
        /// the distance (a spindle torus): `fill` is its part on the rail
        /// `outer`'s side of the circle's axis, `inner` the part on the
        /// other side, both surfaces of revolution meeting at the `pole`
        /// on the axis that every section passes through.
        spindle: Option<Spindle>,
    },
}

/// The two halves of a spindle fill: see [`EdgeGeo::Open`].
#[derive(Clone, Debug)]
pub(crate) struct Spindle {
    pub(crate) inner: Surface,
    pub(crate) inner_sense: Sense,
    pub(crate) pole: Point3,
    /// The side (in the order of the edge's fins) whose rail bounds
    /// `fill`.
    pub(crate) outer: usize,
}

/// Every edge's new geometry, in parallel.
pub(crate) fn curves(
    plan: &Plan,
    meter: &crate::work::Meter,
) -> Result<BTreeMap<EdgeId, EdgeGeo>, Error> {
    meter
        .par_map(&plan.edges, |meter, &e| {
            edge(plan, e, meter).map(|g| (e, g))
        })
        .into_iter()
        .collect()
}

fn edge(plan: &Plan, e: EdgeId, meter: &crate::work::Meter) -> Result<EdgeGeo, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let fins = plan.fins(e);
    let (class, gap) = plan.class[&e];
    match class {
        Class::Kept => {
            let pcurve = |f: FinId| {
                body.fins[f]
                    .pcurve
                    .map(|p| (body.curves2[p.curve].clone(), p.map))
            };
            Ok(EdgeGeo::One(NewCurve {
                curve: body.curves[edge.curve].clone(),
                range: edge.interval,
                period: None,
                tolerance: edge.tolerance.get(),
                pcurves: [pcurve(fins[0]), pcurve(fins[1])],
                reversed: false,
            }))
        }
        Class::Smooth => {
            let mut c = moved(plan, e, fins[0], meter)?;
            let other = moved(plan, e, fins[1], meter)?;
            c.tolerance = c.tolerance.max(other.tolerance) + gap;
            c.pcurves[1] = other.pcurves[0].clone();
            Ok(EdgeGeo::One(c))
        }
        Class::Opening => {
            let rails = [
                moved(plan, e, fins[0], meter)?,
                moved(plan, e, fins[1], meter)?,
            ];
            let d = plan.d(plan.face_of(fins[0]));
            // An inverted fill (an envelope's closing edge) faces the
            // other way.
            let sense = if (d > 0.0) != plan.inverted.contains(&e) {
                Sense::Forward
            } else {
                Sense::Reversed
            };
            if let Some(f) = fill(plan, e, fins, d) {
                return Ok(EdgeGeo::Open {
                    rails,
                    fill: f,
                    sense,
                    spindle: None,
                });
            }
            let unsupported = OffsetError::Unsupported {
                at,
                case: OffsetCase::Fill,
            };
            if !plan.inverted.contains(&e) {
                return Err(unsupported.into());
            }
            let (outer, outer_sense, spindle) =
                spindle(plan, e, fins, d, sense).ok_or(unsupported)?;
            Ok(EdgeGeo::Open {
                rails,
                fill: outer,
                sense: outer_sense,
                spindle: Some(spindle),
            })
        }
        Class::Closing => closing(plan, e, fins, meter).map(EdgeGeo::One),
    }
}

/// Every laminar edge's rail, in parallel: the edge moved along its one
/// face's outward normal by the face's distance (as an opening edge's
/// rail is, see [`moved`]), or at distance zero the edge itself. Its
/// pcurve on the face's offset is the fin's own.
pub(crate) fn rails(
    plan: &Plan,
    meter: &crate::work::Meter,
) -> Result<BTreeMap<EdgeId, NewCurve>, Error> {
    meter
        .par_map(&plan.laminar, |meter, &e| {
            let fin = plan.laminar_fin(e);
            if plan.d(plan.face_of(fin)) == 0.0 {
                let body = plan.body;
                let edge = &body.edges[e];
                let pcurve = body.fins[fin]
                    .pcurve
                    .map(|p| (body.curves2[p.curve].clone(), p.map));
                return Ok((
                    e,
                    NewCurve {
                        curve: body.curves[edge.curve].clone(),
                        range: edge.interval,
                        period: None,
                        tolerance: edge.tolerance.get(),
                        pcurves: [pcurve, None],
                        reversed: false,
                    },
                ));
            }
            moved(plan, e, fin, meter).map(|c| (e, c))
        })
        .into_iter()
        .collect()
}

/// The edge moved along its fin's face's outward normal by the face's
/// distance, on the edge's own parameter: a line or a circle whose
/// normal keeps its place against it moves exactly (to a line, or a
/// circle about the same axis); any other curve is fitted along its
/// pcurve on the face's offset ([`fit_along`]), certified. Its pcurve on
/// the offset is the fin's own, which the offset keeps.
fn moved(
    plan: &Plan,
    e: EdgeId,
    fin: FinId,
    meter: &crate::work::Meter,
) -> Result<NewCurve, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let face = plan.face_of(fin);
    let d = plan.d(face);
    let offset = &plan.surface[&face];
    let pc = body.fins[fin]
        .pcurve
        .ok_or(crate::OffsetError::Uncertain { at })?;
    let pcurve = (body.curves2[pc.curve].clone(), pc.map);
    // On an inverted face the pcurve moves with its parameters.
    let pcurve = match plan.inversions.get(&face).and_then(|i| i.map) {
        Some(m) => {
            let (c, r) = crate::sheet_metal::curve2::mapped(&pcurve.0, &m)
                .map_err(|_| crate::OffsetError::Uncertain { at })?;
            (c, [r[0] + r[1] * pcurve.1[0], r[1] * pcurve.1[1]])
        }
        None => pcurve,
    };
    let curve = &body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let ts: Vec<f64> = (0..SAMPLES)
        .map(|i| t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64)
        .collect();
    let target = |t: f64| curve.point(t) + plan.normal(fin, t) * d;
    let exact = match curve {
        Curve::Line(l) => {
            let n = plan.normal(fin, (t0 + t1) / 2.0);
            Line::from_unit(l.origin() + n * d, l.dir())
                .ok()
                .map(Curve::Line)
        }
        Curve::Circle(c) => {
            let f = c.frame();
            let t = (t0 + t1) / 2.0;
            let n = plan.normal(fin, t);
            let radial = (curve.point(t) - f.origin()) / c.radius();
            let (a, b) = (n.dot(f.z().get()), n.dot(radial));
            let frame = Frame::from_axes(f.origin() + f.z().get() * (a * d), f.z(), f.x()).ok();
            frame
                .and_then(|fr| Circle::new(fr, c.radius() + b * d).ok())
                .map(Curve::Circle)
        }
        _ => None,
    };
    if let Some(c) = exact {
        let worst = ts
            .iter()
            .map(|&t| c.point(t).distance(target(t)))
            .fold(0.0, f64::max);
        if worst <= LINEAR_RESOLUTION / 4.0 {
            return Ok(NewCurve {
                curve: c,
                range: edge.interval,
                period: None,
                tolerance: LINEAR_RESOLUTION.max(2.0 * worst),
                pcurves: [Some(pcurve), None],
                reversed: false,
            });
        }
    }
    let lifted = fit_along(offset, &pcurve.0, pcurve.1, edge.interval, meter)
        .map_err(|_| crate::OffsetError::Uncertain { at })?;
    Ok(NewCurve {
        curve: Curve::Nurbs(Arc::new(lifted.curve)),
        range: edge.interval,
        period: None,
        tolerance: lifted.tolerance,
        pcurves: [Some(pcurve), None],
        reversed: false,
    })
}

/// Where a ball of radius `|d|` rolled about the edge goes: a cylinder
/// about a line (its seam opposite the middle of the faces' normals), a
/// torus about a circle (`None` unless the circle is larger than `|d|`),
/// otherwise a pipe about the edge's curve over its interval, anchored on
/// the first face's offset (through the fin's pcurve where that isn't
/// analytic).
fn fill(plan: &Plan, e: EdgeId, fins: [FinId; 2], d: f64) -> Option<Surface> {
    let body = plan.body;
    let edge = &body.edges[e];
    let curve = &body.curves[edge.curve];
    let r = d.abs();
    let [t0, t1] = edge.interval;
    match curve {
        Curve::Line(l) => {
            let t = (t0 + t1) / 2.0;
            let middle = plan.normal(fins[0], t) + plan.normal(fins[1], t);
            let x = if middle.length() > 1e-9 {
                -middle
            } else {
                plan.normal(fins[0], t)
            };
            let frame = Frame::new(l.origin(), l.dir().get(), x).ok()?;
            Some(Surface::Cylinder(Cylinder::new(frame, r).ok()?))
        }
        Curve::Circle(c) => {
            if r >= c.radius() {
                return None;
            }
            Some(Surface::Torus(Torus::new(*c.frame(), c.radius(), r).ok()?))
        }
        _ => {
            let spine = match curve {
                Curve::Nurbs(n) => {
                    let [d0, d1] = n.domain();
                    if t0 <= d0 && t1 >= d1 && curve.is_closed() {
                        n.bezier_periodic()
                            .or_else(|| (n.period().is_some()).then(|| (**n).clone()))?
                    } else {
                        n.part([t0.max(d0), t1.min(d1)]).ok()?
                    }
                }
                _ => return None,
            };
            let spine = Curve::Nurbs(Arc::new(spine));
            let face = plan.face_of(fins[0]);
            let anchor = plan.surface[&face].clone();
            let pipe = if anchor.frame().is_some() {
                Pipe::new(spine, r, anchor)
            } else {
                let pc = body.fins[fins[0]].pcurve?;
                Pipe::on_rail(spine, r, anchor, body.curves2[pc.curve].clone(), pc.map)
            };
            Some(Surface::Pipe(Arc::new(pipe.ok()?)))
        }
    }
}

/// An inverted fill about a circle edge of radius `R <= |d|`: the ball
/// rolled about it sweeps a spindle torus, whose sections all pass
/// through one point of the circle's axis (`P*`, at `h = ±√(d² - R²)`
/// from its plane). Each half is the section's arc from a rail to `P*`
/// (a rational quadratic part of the circle) turned about the axis, a
/// [`crate::geom::Revolution`] with its pole at `P*`; the half past the
/// axis is turned from the arc mirrored across it. Each face's sense is
/// the one whose normal points as the torus fill's would, `sense` taken
/// about the edge (away from it for `Forward`). `None` where the section
/// meets the axis other than once.
fn spindle(
    plan: &Plan,
    e: EdgeId,
    fins: [FinId; 2],
    d: f64,
    sense: Sense,
) -> Option<(Surface, Sense, Spindle)> {
    use crate::geom::Revolution;
    let body = plan.body;
    let edge = &body.edges[e];
    let Curve::Circle(c) = &body.curves[edge.curve] else {
        return None;
    };
    let (f, big_r, r) = (c.frame(), c.radius(), d.abs());
    let slack = 10.0 * LINEAR_RESOLUTION;
    if r * r - big_r * big_r <= slack * slack {
        return None;
    }
    let [t0, t1] = edge.interval;
    let tm = (t0 + t1) / 2.0;
    let curve = &body.curves[edge.curve];
    let rad = (curve.point(tm) - f.origin()) / big_r;
    let z = f.z().get();
    // A rail point in the section's (ρ, h) plane: ρ along `rad`, h along z.
    let local = |q: Point3| {
        let v = q - f.origin();
        (v.dot(rad), v.dot(z))
    };
    let e_pt = curve.point(tm);
    let pa = e_pt + plan.normal(fins[0], tm) * d;
    let pb = e_pt + plan.normal(fins[1], tm) * d;
    let (a, b) = (local(pa), local(pb));
    let angle = |(x, y): (f64, f64)| crate::math::atan2(y, x - big_r);
    let (fa, fb) = (angle(a), angle(b));
    // The shorter arc between them, and where it meets ρ = 0.
    let mut span = fb - fa;
    if span > core::f64::consts::PI {
        span -= core::f64::consts::TAU;
    } else if span < -core::f64::consts::PI {
        span += core::f64::consts::TAU;
    }
    let star = crate::math::acos(-big_r / r);
    let within = |phi: f64| {
        let x = (phi - fa).rem_euclid(core::f64::consts::TAU);
        let x = if span < 0.0 {
            core::f64::consts::TAU - x
        } else {
            x
        };
        x > 0.0 && x < span.abs()
    };
    let hits: Vec<f64> = [star, -star].into_iter().filter(|&p| within(p)).collect();
    let [phi] = hits[..] else {
        return None;
    };
    let h = r * crate::math::sin(phi);
    let pole = f.origin() + z * h;
    // Which rail is outside the axis.
    let outer = if a.0 > 0.0 && b.0 < 0.0 {
        0
    } else if b.0 > 0.0 && a.0 < 0.0 {
        1
    } else {
        return None;
    };
    let outer_angle = if outer == 0 { fa } else { fb };
    let inner_angle = if outer == 0 { fb } else { fa };
    // The arc from angle `from` to `phi` of the section circle centred at
    // (ρ0, 0), as a NURBS in the frame's xz half-plane (y = 0).
    let half = |rho0: f64, from: f64, to: f64| -> Option<Surface> {
        // The shorter way from `from` to `to`.
        let to = from + (to - from + core::f64::consts::PI).rem_euclid(core::f64::consts::TAU)
            - core::f64::consts::PI;
        let mid = 0.5 * (from + to);
        // The circle's seam opposite the arc's middle.
        let x = f.x().get();
        let dir = -(x * crate::math::cos(mid) + z * crate::math::sin(mid));
        let frame = crate::geom::Frame::new(f.origin() + x * rho0, -f.y().get(), dir).ok()?;
        let circle = Circle::new(frame, r).ok()?;
        let (n, _) = circle.to_nurbs().ok()?;
        let p0 =
            f.origin() + x * (rho0 + r * crate::math::cos(from)) + z * (r * crate::math::sin(from));
        let p1 =
            f.origin() + x * (rho0 + r * crate::math::cos(to)) + z * (r * crate::math::sin(to));
        let (s0, s1) = (n.project(p0).param, n.project(p1).param);
        let part = n.part([s0.min(s1), s0.max(s1)]).ok()?;
        Revolution::new(*f, Curve::Nurbs(Arc::new(part)))
            .ok()
            .map(|s| Surface::Revolution(Arc::new(s)))
    };
    let outer_surface = half(big_r, outer_angle, phi)?;
    // Past the axis, mirrored across it: the points (R + r cos φ, r sin φ)
    // with ρ < 0 are, turned half a turn about the axis, (-R - r cos φ,
    // r sin φ), the circle about (-R, 0) at the angle π - φ.
    let mirror = |phi: f64| core::f64::consts::PI - phi;
    let inner_surface = half(-big_r, mirror(phi), mirror(inner_angle))?;
    // Senses: each face's normal where it is clear of the pole must point
    // as the torus fill's would there: along ±(P - e) by `sense`.
    let want = |p: Point3| -> Vector3 {
        let towards = p - e_pt;
        if sense == Sense::Forward {
            towards
        } else {
            -towards
        }
    };
    let sense_on = |s: &Surface, p: Point3| -> Option<Sense> {
        let q = s.project(p);
        let n = s.normal(q.param.0, q.param.1)?.get();
        let w = want(p);
        Some(if n.dot(w) > 0.0 {
            Sense::Forward
        } else {
            Sense::Reversed
        })
    };
    // A point of the section between a rail and the pole, the shorter way.
    let sample = |from: f64| {
        let to = from + (phi - from + core::f64::consts::PI).rem_euclid(core::f64::consts::TAU)
            - core::f64::consts::PI;
        let ph = 0.5 * (from + to);
        e_pt + (rad * crate::math::cos(ph) + z * crate::math::sin(ph)) * r
    };
    let outer_sense = sense_on(&outer_surface, sample(outer_angle))?;
    let inner_sense = sense_on(&inner_surface, sample(inner_angle))?;
    Some((
        outer_surface,
        outer_sense,
        Spindle {
            inner: inner_surface,
            inner_sense,
            pole,
            outer,
        },
    ))
}

/// Where a closing edge's faces' offsets meet. Both analytic: the branch
/// of their intersection near the edge, nearest to where the edge's middle
/// goes (the point on both faces' tangent planes there, moved), within
/// the larger distance of it (`EdgeVanishes` where no branch comes that
/// near), exact where the intersection has a closed form. Otherwise
/// [`traced`] from the edge.
fn closing(
    plan: &Plan,
    e: EdgeId,
    fins: [FinId; 2],
    meter: &crate::work::Meter,
) -> Result<NewCurve, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let faces = fins.map(|f| plan.face_of(f));
    let ds = faces.map(|f| plan.d(f));
    let surfaces = faces.map(|f| &plan.surface[&f]);
    if surfaces.iter().any(|s| s.frame().is_none()) {
        return traced(plan, e, fins, meter);
    }
    let curve = &body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let tm = (t0 + t1) / 2.0;
    let reach = ds[0].abs().max(ds[1].abs());
    let estimate = moved_point(
        curve.point(tm),
        [plan.normal(fins[0], tm), plan.normal(fins[1], tm)],
        ds,
    );
    // Within two distances of the edge: the faces' offsets reach three.
    let grow = 2.0 * reach;
    let region = curve.bounds(edge.interval).grow(grow);
    let hit =
        surface_surface_in(surfaces[0], surfaces[1], &region, meter).map_err(|f| match f {
            Failure::Uncertain => Error::from(crate::OffsetError::Uncertain { at }),
            Failure::Unsupported => OffsetError::Unsupported {
                at,
                case: OffsetCase::PartlySmooth,
            }
            .into(),
        })?;
    let SurfaceHit::Meet { branches, .. } = hit else {
        return Err(crate::OffsetError::Uncertain { at }.into());
    };
    let best = branches
        .into_iter()
        .map(|b| {
            let q = project_on(&b.curve, b.range, estimate);
            (q.1.distance(estimate), q.0, b)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((dist, _, b)) = best else {
        return Err(OffsetError::EdgeVanishes { edge: at }.into());
    };
    if dist > reach.max(10.0 * LINEAR_RESOLUTION) {
        return Err(OffsetError::EdgeVanishes { edge: at }.into());
    }
    if b.tangent {
        return Err(crate::OffsetError::Uncertain { at }.into());
    }
    // A closed edge's closed clamped branch starts where its vertex goes,
    // so the edge can run over the whole of it.
    let b = if edge.start == edge.end && b.curve.period().is_none() {
        let p = body.vertices[edge.start].point;
        let at = moved_point(p, [plan.normal(fins[0], t0), plan.normal(fins[1], t0)], ds);
        let (s, _) = project_on(&b.curve, b.range, at);
        crate::intersect::reseamed(b, s)
    } else {
        b
    };
    let t = project_on(&b.curve, b.range, estimate).0;
    let along = curve.eval(tm).d1;
    let reversed = b.curve.eval(t).d1.dot(along) < 0.0;
    let period =
        (b.curve.period() == Some(b.range[1] - b.range[0])).then(|| b.range[1] - b.range[0]);
    let [pa, pb] = b.pcurves;
    Ok(NewCurve {
        curve: b.curve,
        range: b.range,
        period,
        tolerance: b.tolerance,
        pcurves: [Some(pa), Some(pb)],
        reversed,
    })
}

/// The point the faces' tangent planes at `p`, with outward normals `n`,
/// meet at once each is moved by its distance: on both moved planes,
/// along the normals' span. Where the normals are nearly parallel, the
/// mean of the moved points.
pub(crate) fn moved_point(p: Point3, n: [Vector3; 2], d: [f64; 2]) -> Point3 {
    let c = n[0].dot(n[1]);
    let det = 1.0 - c * c;
    if det < 1e-12 {
        return p + (n[0] * d[0] + n[1] * d[1]) * 0.5;
    }
    let alpha = (d[0] - c * d[1]) / det;
    let beta = (d[1] - c * d[0]) / det;
    p + n[0] * alpha + n[1] * beta
}

/// The parameter in `range` of the point of `curve` nearest `p`, and that
/// point: the curve's projection, else the nearer end of the range.
pub(crate) fn project_on(curve: &Curve, range: [f64; 2], p: Point3) -> (f64, Point3) {
    let q = curve.project(p);
    let inside = |t: f64| range[0] <= t && t <= range[1];
    let t = match curve.period() {
        Some(per) if !inside(q.param) => {
            let shifted = range[0] + (q.param - range[0]).rem_euclid(per);
            if inside(shifted) { shifted } else { q.param }
        }
        _ => q.param,
    };
    if inside(t) {
        return (t, curve.point(t));
    }
    let ends = range.map(|t| (t, curve.point(t)));
    if ends[0].1.distance(p) <= ends[1].1.distance(p) {
        ends[0]
    } else {
        ends[1]
    }
}

/// The sphere about `centre` of `radius` through the points `feet`, with
/// its seam opposite them and its poles square to the direction they lie
/// in (as blends make ball corners).
pub(crate) fn sphere(centre: Point3, radius: f64, feet: &[Point3]) -> Option<Surface> {
    let m = feet
        .iter()
        .fold(Vector3::ZERO, |acc, &f| acc + (f - centre))
        .normalize()?
        .get();
    let z = (feet[0] - centre).cross(m);
    let z = if z.length() > 1e-6 * radius {
        z
    } else {
        let a = m.cross(Vector3::new(1.0, 0.0, 0.0));
        if a.length() > 1e-3 {
            a
        } else {
            m.cross(Vector3::new(0.0, 1.0, 0.0))
        }
    };
    let frame = Frame::new(centre, z, -m).ok()?;
    Some(Surface::Sphere(Sphere::new(frame, radius).ok()?))
}

/// How many Newton steps find a closing edge's point in one section.
const NEWTON: usize = 32;

/// How many Newton steps from the fins' own parameters, before a section
/// starts again from the feet of where its edge moves.
const FIRST_STEPS: usize = 8;

/// Where a closing edge's faces' offsets meet, traced from the edge: at
/// each parameter `t`, the point on both offsets in the plane across the
/// edge at `C(t)` (Newton's method on `O_A(u_A, v_A) = O_B(u_B, v_B)` and
/// `(O_A - C(t)) · T(t) = 0`, from the fins' own parameters, which the
/// offsets keep), with its derivative along `n_A × n_B`; fitted on
/// shared knots and certified on both offsets ([`fit_on`]), the same
/// curve on each. Over the edge's interval, carried on past its ends by
/// three distances along a line or a circle, where its ends may move out.
/// A curve this finds is a curve where the offsets meet, certified, but
/// not every such curve: the self-intersection check on the result is
/// what rules out another crossing. `EdgeVanishes` where a section has no
/// such point near where the edge moves; `Uncertain` where the fit can't
/// be certified.
fn traced(
    plan: &Plan,
    e: EdgeId,
    fins: [FinId; 2],
    meter: &crate::work::Meter,
) -> Result<NewCurve, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let faces = fins.map(|f| plan.face_of(f));
    let ds = faces.map(|f| plan.d(f));
    let surfaces = faces.map(|f| &plan.surface[&f]);
    let curve = &body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let reach = ds[0].abs().max(ds[1].abs());
    let range = match curve {
        Curve::Line(_) => [t0 - 3.0 * reach, t1 + 3.0 * reach],
        Curve::Circle(c) if t1 - t0 < core::f64::consts::TAU => {
            let m =
                (3.0 * reach / c.radius()).min((core::f64::consts::TAU - (t1 - t0)) / 2.0 * 0.9);
            [t0 - m, t1 + m]
        }
        _ => [t0, t1],
    };
    let uv = |k: usize, t: f64| body.fin_uv(fins[k], t.clamp(t0, t1)).map(|p| [p.x, p.y]);
    let solve = |t: f64| -> Option<(Point3, Vector3, [[f64; 2]; 2])> {
        let c = curve.eval(t);
        let tan = c.d1.normalize()?.get();
        let tan_d = (c.d2 - tan * c.d2.dot(tan)) / c.d1.length();
        let start = moved_point(
            c.point,
            [
                plan.normal(fins[0], t.clamp(t0, t1)),
                plan.normal(fins[1], t.clamp(t0, t1)),
            ],
            ds,
        );
        // Newton's method from `x`, at most `steps` steps: the parameters
        // and whether a step came within rounding of nothing.
        let newton = |mut x: [[f64; 2]; 2], steps: usize| -> Option<([[f64; 2]; 2], bool)> {
            for _ in 0..steps {
                let a = surfaces[0].eval(x[0][0], x[0][1]);
                let b = surfaces[1].eval(x[1][0], x[1][1]);
                let r = a.point - b.point;
                let plane = (a.point - c.point).dot(tan);
                let m = [
                    [a.du.x, a.dv.x, -b.du.x, -b.dv.x],
                    [a.du.y, a.dv.y, -b.du.y, -b.dv.y],
                    [a.du.z, a.dv.z, -b.du.z, -b.dv.z],
                    [a.du.dot(tan), a.dv.dot(tan), 0.0, 0.0],
                ];
                let step = solve4(m, [-r.x, -r.y, -r.z, -plane])?;
                x = [
                    [x[0][0] + step[0], x[0][1] + step[1]],
                    [x[1][0] + step[2], x[1][1] + step[3]],
                ];
                if step
                    .iter()
                    .all(|s| s.abs() <= 1e-15 * (1.0 + x[0][0].abs().max(x[1][0].abs())))
                {
                    return Some((x, true));
                }
            }
            Some((x, false))
        };
        // Within each offset's domain (a periodic direction anywhere).
        let within = |x: [[f64; 2]; 2]| {
            (0..2).all(|k| {
                let dom = surfaces[k].domain();
                let (pu, pv) = surfaces[k].periods();
                (0..2).all(|i| {
                    let [lo, hi] = dom[i];
                    [pu, pv][i].is_some()
                        || (x[k][i] >= lo - 1e-12 * (1.0 + lo.abs())
                            && x[k][i] <= hi + 1e-12 * (1.0 + hi.abs()))
                })
            })
        };
        // From the fins' own parameters first, which the offsets keep
        // (#408: projecting each section's point onto a spline's offset
        // took seconds a sample); from the feet of where the edge moves
        // only where that leaves a domain or takes more than a few steps.
        let own = [uv(0, t)?, uv(1, t)?];
        let x = match newton(own, FIRST_STEPS) {
            Some((x, true)) if within(x) => x,
            _ => {
                let mut x = own;
                for k in 0..2 {
                    meter.projected();
                    let p = surfaces[k].project(start).param;
                    x[k] = [p.0, p.1];
                }
                newton(x, NEWTON)?.0
            }
        };
        let a = surfaces[0].eval(x[0][0], x[0][1]);
        let b = surfaces[1].eval(x[1][0], x[1][1]);
        if a.point.distance(b.point) > LINEAR_RESOLUTION / 16.0
            || a.point.distance(start) > 2.0 * reach.max(10.0 * LINEAR_RESOLUTION)
        {
            return None;
        }
        let n = |s: &Surface, p: [f64; 2]| s.normal(p[0], p[1]).map(|n| n.get());
        let dir = n(surfaces[0], x[0])?.cross(n(surfaces[1], x[1])?);
        // `(X - C) · T = 0` along the curve: `X' · T = C' · T - (X - C) · T'`.
        let along = dir.dot(tan);
        if along.abs() < 1e-12 * dir.length() {
            return None;
        }
        let rate = (c.d1.dot(tan) - (a.point - c.point).dot(tan_d)) / along;
        Some((a.point, dir * rate, x))
    };
    let sample = |t: f64| solve(t).map(|(p, v, _)| (p, v));
    // Carried on past an end, or even at it, the section's point may leave
    // a bounded offset's domain (a sweep's offset ends with its profile
    // segment), where nothing can be certified: each end of the trace is
    // where the point leaves either domain, found by bisection from the
    // edge's middle, if that comes first.
    let domains = surfaces.map(|s| (s.domain(), s.periods()));
    let inside = |t: f64| {
        solve(t).is_some_and(|(_, _, x)| {
            (0..2).all(|k| {
                let (dom, (pu, pv)) = domains[k];
                (0..2).all(|i| {
                    let [lo, hi] = dom[i];
                    let periodic = if i == 0 { pu } else { pv };
                    periodic.is_some()
                        || (x[k][i] >= lo - 1e-12 * (1.0 + lo.abs())
                            && x[k][i] <= hi + 1e-12 * (1.0 + hi.abs()))
                })
            })
        })
    };
    let middle = (t0 + t1) / 2.0;
    let carried = |end: f64| {
        if inside(end) || !inside(middle) {
            return end;
        }
        let (mut good, mut bad) = (middle, end);
        for _ in 0..40 {
            let m = (good + bad) / 2.0;
            if inside(m) {
                good = m;
            } else {
                bad = m;
            }
        }
        good
    };
    let range = [carried(range[0]), carried(range[1])];
    // Every section must have its point, else the edge has vanished.
    for i in 0..=8 {
        let t = range[0] + (range[1] - range[0]) * f64::from(i) / 8.0;
        if sample(t).is_none() {
            return Err(OffsetError::EdgeVanishes { edge: at }.into());
        }
    }
    // Each sample with its parameters on both offsets, which Newton's
    // method found: the fit needn't project it again (#408).
    let both = |t: f64| {
        solve(t).map(|(p, v, x)| {
            (
                [(p, v), (p, v)],
                x.map(|[u, v]| crate::geom::Point2::new(u, v)),
            )
        })
    };
    let fit = crate::intersect::fit_on_feet([surfaces[0], surfaces[1]], range, &both, meter)
        .map_err(|_| crate::OffsetError::Uncertain { at })?;
    let tolerance = fit.bounds[0].max(fit.bounds[1]).max(LINEAR_RESOLUTION);
    let pcurves = fit
        .pcurves
        .map(|p| p.map(|(c, _)| (Curve2::Nurbs2(Arc::new(c)), [0.0, 1.0])));
    let [c0, _] = fit.curves;
    Ok(NewCurve {
        curve: Curve::Nurbs(Arc::new(c0)),
        range,
        period: None,
        tolerance,
        pcurves,
        reversed: false,
    })
}

/// The solution of a 4 × 4 linear system by Gaussian elimination with
/// partial pivoting; `None` where it is singular.
fn solve4(mut m: [[f64; 4]; 4], mut r: [f64; 4]) -> Option<[f64; 4]> {
    for c in 0..4 {
        let p = (c..4).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, p);
        r.swap(c, p);
        for k in c + 1..4 {
            let f = m[k][c] / m[c][c];
            let pivot = m[c];
            for (x, y) in m[k][c..].iter_mut().zip(&pivot[c..]) {
                *x -= f * y;
            }
            r[k] -= f * r[c];
        }
    }
    let mut x = [0.0; 4];
    for c in (0..4).rev() {
        let s: f64 = (c + 1..4).map(|j| m[c][j] * x[j]).sum();
        x[c] = (r[c] - s) / m[c][c];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}
