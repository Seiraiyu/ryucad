//! Ends: where a blend that runs past a vertex is cut off by the third
//! face there.
//!
//! At an end each rail crosses the edge between its face and the end
//! face (its cut), and the blend surface meets the end face in the end
//! curve between those crossings. Crossings come from intersecting the
//! rail with the end face's surface and keeping the hit on the cut; the
//! end curve is the branch of the blend surface's intersection with the
//! end face that passes both crossings, taken the short way round: the
//! arc nearer the vertex the blend takes away.

use super::rail::Rail;
use crate::geom::{Box3, Curve, Curve2, Point3, Surface};
use crate::intersect::{self, Branch, CurveHit, Failure, SurfaceHit};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId};

/// Where a rail crosses its cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Crossing {
    /// The rail's own parameter there.
    pub(crate) tau: f64,
    pub(crate) point: Point3,
    pub(crate) cut: EdgeId,
    /// The cut's parameter there.
    pub(crate) at: f64,
}

/// Why an end could not be found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndFail {
    /// The rail does not cross its cut once: it leaves its face elsewhere.
    Overflow,
    /// The end face's surface can't be intersected.
    Unsupported,
    /// An intersection stayed uncertain, or no branch passes both
    /// crossings.
    Uncertain,
}

impl From<Failure> for EndFail {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Unsupported => Self::Unsupported,
            Failure::Uncertain => Self::Uncertain,
        }
    }
}

/// Where `rail` over its parameters `range` crosses `cut`, an edge of the
/// face on `surface` the blend ends on, near the vertex at `near`: of the
/// rail's hits with the surface that lie on the cut (within the cut's and
/// the rail's tolerances of its curve, inside its interval), the one
/// nearest the vertex. A cut may run between both ends of a blend, which
/// then crosses it twice.
pub(crate) fn crossing(
    body: &Body,
    rail: &Rail,
    range: [f64; 2],
    surface: &Surface,
    cut: EdgeId,
    near: Point3,
) -> Result<Crossing, EndFail> {
    let tol = rail.tolerance;
    let hits = match surface {
        Surface::Pipe(_) => intersect::curve_pipe(&rail.curve, range, surface, tol)?,
        _ => intersect::curve_surface(&rail.curve, range, surface, tol)?,
    };
    let e = &body.edges[cut];
    let curve = &body.curves[e.curve];
    let reach = tol + e.tolerance.get() + 10.0 * LINEAR_RESOLUTION;
    let mut found: Option<Crossing> = None;
    for h in hits {
        let CurveHit::Point { t, point, .. } = h else {
            return Err(EndFail::Uncertain);
        };
        let Some(at) = on_edge(curve, e.interval, point, reach) else {
            continue;
        };
        if found.is_none_or(|f| point.distance(near) < f.point.distance(near)) {
            found = Some(Crossing {
                tau: t,
                point,
                cut,
                at,
            });
        }
    }
    found.ok_or(EndFail::Overflow)
}

/// The parameter on `curve` over `interval` of its point nearest `p`,
/// strictly inside the interval, when `p` is within `reach` of it.
pub(crate) fn on_edge(curve: &Curve, [t0, t1]: [f64; 2], p: Point3, reach: f64) -> Option<f64> {
    let mut t = curve.project(p).param;
    if let Some(period) = curve.period() {
        t += ((t0 - t) / period).ceil() * period;
    }
    if !(t0 < t && t < t1) {
        return None;
    }
    (curve.point(t).distance(p) <= reach).then_some(t)
}

/// A blend's end curve on the end face.
#[derive(Clone, Debug)]
pub(crate) struct EndCurve {
    /// The curve, running from the first crossing to the second over
    /// `interval`.
    pub(crate) curve: Curve,
    pub(crate) interval: [f64; 2],
    /// Its pcurves on the blend surface and the end face's surface, with
    /// the maps from the curve's parameter.
    pub(crate) pcurves: [(Curve2, [f64; 2]); 2],
    pub(crate) tolerance: f64,
}

/// The end curve where `blend` meets `end` between `from` and `to`, the
/// rails' crossings near `vertex`: the arc whose middle `on_face` scores
/// higher, a point's place on the blend face (at least 0 on it, below 0
/// round the back of its surface). `reach` bounds how far the curve may
/// go from them (the ball's diameter).
pub(crate) fn end_curve(
    blend: &Surface,
    end: &Surface,
    from: Point3,
    to: Point3,
    vertex: Point3,
    reach: f64,
    on_face: &dyn Fn(Point3) -> f64,
) -> Result<EndCurve, EndFail> {
    let region = Box3::new(from, to)
        .union(&Box3::from_point(vertex))
        .grow(reach);
    let hit = super::meet(blend, end, &region)?;
    let SurfaceHit::Meet { branches, .. } = hit else {
        return Err(EndFail::Uncertain);
    };
    for mut b in branches {
        let tol = b.tolerance.max(LINEAR_RESOLUTION);
        let [r0, r1] = b.range;
        // A closed branch: periodic, or fitted as a clamped spline that
        // ends where it starts (a pipe cut all round).
        let clamped = b.curve.period().is_none();
        let closed = !clamped || b.curve.point(r0).distance(b.curve.point(r1)) <= tol;
        let period = r1 - r0;
        let wrap = |t: f64| {
            if clamped {
                r0 + (t - r0).rem_euclid(period)
            } else {
                t
            }
        };
        let Some((a, z)) = arc_ends(&b, from, to, 10.0 * tol) else {
            continue;
        };
        // The arc from `a` to `z`, or the other way round a closed branch,
        // whichever lies on the blend face.
        let mut arcs = vec![(a, z)];
        if closed {
            arcs = vec![
                (a, if z > a { z } else { z + period }),
                (a, if z < a { z } else { z - period }),
            ];
        }
        let score = |(s, e): (f64, f64)| on_face(b.curve.point(wrap((s + e) / 2.0)));
        arcs.sort_by(|x, y| score(*y).total_cmp(&score(*x)));
        if arcs.len() == 2 && (score(arcs[0]) - score(arcs[1])).abs() <= 1e-9 {
            return Err(EndFail::Uncertain);
        }
        let (mut s, mut e) = arcs[0];
        if clamped && closed && (s.max(e) > r1 || s.min(e) < r0) {
            // The arc crosses the spline's start: move the start to the
            // middle of the other arc.
            let (os, oe) = arcs[1];
            b = crate::intersect::reseamed(b, wrap((os + oe) / 2.0));
            (s, e) = arc_ends(&b, from, to, 10.0 * tol).ok_or(EndFail::Uncertain)?;
        }
        let mut out = EndCurve {
            curve: b.curve,
            interval: [s, e],
            pcurves: b.pcurves,
            tolerance: tol,
        };
        if e < s {
            out = out.reversed();
        }
        return Ok(out.covered([blend, end])?);
    }
    Err(EndFail::Uncertain)
}

/// The parameters in its range where branch `b` passes within `reach` of
/// `from` and `to`.
fn arc_ends(b: &Branch, from: Point3, to: Point3, reach: f64) -> Option<(f64, f64)> {
    let at = |p: Point3| -> Option<f64> {
        let mut t = b.curve.project(p).param;
        if let Some(period) = b.curve.period() {
            t += ((b.range[0] - t) / period).ceil() * period;
        }
        (b.range[0] <= t && t <= b.range[1] && b.curve.point(t).distance(p) <= reach).then_some(t)
    };
    Some((at(from)?, at(to)?))
}

impl EndCurve {
    /// The same curve, its pcurves on `surfaces` (in the order of its
    /// pcurves) fitted again where their domain doesn't cover its
    /// interval: a clamped pcurve of a closed branch, where the arc runs
    /// across the branch's start.
    pub(crate) fn covered(mut self, surfaces: [&Surface; 2]) -> Result<Self, Failure> {
        for (k, surface) in surfaces.iter().enumerate() {
            let (pc, m) = &self.pcurves[k];
            if pc.period().is_some() {
                continue;
            }
            let [d0, d1] = pc.domain();
            let [a, b] = self.interval.map(|t| m[0] + m[1] * t);
            let slack = 1e-9 * (d1 - d0).abs().max(1.0);
            if a.min(b) >= d0 - slack && a.max(b) <= d1 + slack {
                continue;
            }
            let (fit, bound) = intersect::fit_pcurve(surface, &self.curve, self.interval)?;
            self.pcurves[k] = (Curve2::Nurbs2(std::sync::Arc::new(fit)), [0.0, 1.0]);
            self.tolerance = self.tolerance.max(bound);
        }
        Ok(self)
    }

    /// The same curve run the other way.
    pub(crate) fn reversed(self) -> Self {
        let (curve, c) = self.curve.reversed();
        let flip = |(p, m): (Curve2, [f64; 2])| (p, [m[0] + m[1] * c, -m[1]]);
        let [a, b] = self.pcurves;
        Self {
            curve,
            interval: [c - self.interval[0], c - self.interval[1]],
            pcurves: [flip(a), flip(b)],
            tolerance: self.tolerance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::chain::{Class, chains};
    use crate::blend::fillet::fillet_surface;
    use crate::blend::rail::rail;
    use crate::blend::spec::resolve;
    use crate::blend::spine::spine;
    use crate::blend::tests::{block, edge_of};
    use crate::blend::{BlendKind, EdgeBlend};
    use crate::name::FaceRole;

    #[test]
    fn a_box_edge_ends_in_quarter_circles() {
        let b = block();
        let r = 0.2;
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let specs = resolve(
            &b,
            &[EdgeBlend {
                edge: e,
                kind: BlendKind::Fillet { radius: r },
            }],
        )
        .unwrap();
        let c = chains(&b, &specs).unwrap().remove(0);
        let s = spine(&b, &specs[0], r).unwrap();
        let surfaces = specs[0]
            .faces
            .map(|f| b.surface(b.face(f).unwrap().surface).unwrap().clone());
        let rails = [0, 1].map(|k| rail(&s, &surfaces[k], k, s.range).unwrap());
        let feet = [0, 1].map(|k| s.foot(k, s.mid));
        let (blend, _) = fillet_surface(&s, (&surfaces[0], 0), s.range, feet).unwrap();
        for (v, class) in c.vertices.iter().zip(&c.classes) {
            let Class::End { face, cuts } = *class else {
                panic!()
            };
            let end = b.surface(b.face(face).unwrap().surface).unwrap();
            let x = [0, 1].map(|k| {
                let range = [rails[k].param(s.range[0]), rails[k].param(s.range[1])];
                crossing(
                    &b,
                    &rails[k],
                    range,
                    end,
                    cuts[k],
                    b.vertex(*v).unwrap().point,
                )
                .unwrap()
            });
            let vp = b.vertex(*v).unwrap().point;
            for c in &x {
                // A radius back from the vertex along the cut.
                assert!((c.point.distance(vp) - r).abs() < 1e-12);
            }
            let ec = end_curve(&blend, end, x[0].point, x[1].point, vp, 2.0 * r, &|p| {
                -p.distance(vp)
            })
            .unwrap();
            let Curve::Circle(circle) = &ec.curve else {
                panic!("{:?}", ec.curve)
            };
            assert!((circle.radius() - r).abs() < 1e-12);
            assert!(ec.interval[0] < ec.interval[1]);
            assert!((ec.interval[1] - ec.interval[0] - core::f64::consts::FRAC_PI_2).abs() < 1e-9);
            assert!(ec.curve.point(ec.interval[0]).distance(x[0].point) < 1e-9);
            assert!(ec.curve.point(ec.interval[1]).distance(x[1].point) < 1e-9);
            for (k, srf) in [&blend, end].into_iter().enumerate() {
                let (pc, m) = &ec.pcurves[k];
                for i in 0..=8 {
                    let t = ec.interval[0] + (ec.interval[1] - ec.interval[0]) * i as f64 / 8.0;
                    let uv = pc.point(m[0] + m[1] * t);
                    assert!(srf.point(uv.x, uv.y).distance(ec.curve.point(t)) < 1e-9);
                }
            }
        }
    }

    #[test]
    fn a_rail_that_misses_its_cut_overflows() {
        let b = block();
        let r = 0.2;
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let specs = resolve(
            &b,
            &[EdgeBlend {
                edge: e,
                kind: BlendKind::Fillet { radius: r },
            }],
        )
        .unwrap();
        let s = spine(&b, &specs[0], r).unwrap();
        let surfaces = specs[0]
            .faces
            .map(|f| b.surface(b.face(f).unwrap().surface).unwrap().clone());
        let rl = rail(&s, &surfaces[0], 0, s.range).unwrap();
        // The far edge of the face on -Y: its rail crosses that plane at a
        // radius from the corner, not on this edge.
        let wrong =
            crate::blend::tests::edge_id(&b, edge_of(&b, FaceRole::MinusY, FaceRole::MinusZ));
        let minus_y = b
            .surface(
                b.face(crate::blend::tests::face_of(&b, FaceRole::MinusY))
                    .unwrap()
                    .surface,
            )
            .unwrap();
        let range = [rl.param(s.range[0]), rl.param(s.range[1])];
        assert_eq!(
            crossing(&b, &rl, range, minus_y, wrong, Point3::ORIGIN).unwrap_err(),
            EndFail::Overflow
        );
    }
}
