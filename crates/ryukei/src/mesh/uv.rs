//! A face's parameter plane: how it is scaled, where its seams are, which
//! of its boundary rows collapse to a point, and how model-space points
//! are inverted into it.

use super::bound::{Metric, Targets};
use crate::geom::{Point2, Point3, Surface};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, Sense};

/// The two parameter directions, as indices: 0 is `u`, 1 is `v`.
pub(crate) const U: usize = 0;
pub(crate) const V: usize = 1;

/// What meshing needs to know about one face's parameter plane.
#[derive(Clone, Debug)]
pub(crate) struct FaceGeom<'a> {
    pub(crate) face: FaceId,
    pub(crate) surface: &'a Surface,
    /// The face's normal is the surface's natural normal.
    pub(crate) forward: bool,
    /// The plane the face is meshed in (see [`stretched`]).
    pub(crate) metric: Metric,
    /// Each direction's speed, as a power of two: lengths in the
    /// parameter plane scaled by it roughly match lengths on the surface.
    pub(crate) speed: Metric,
    /// The period of each direction, when it is periodic.
    pub(crate) periods: [Option<f64>; 2],
    /// For a direction the surface is periodic in but the face is not (see
    /// `open_unwound`), where the period holding the face starts.
    pub(crate) wrap: [Option<f64>; 2],
    /// The surface's own periods.
    pub(crate) full: [Option<f64>; 2],
    pub(crate) domain: [[f64; 2]; 2],
    /// Whether the boundary row at each end (`[direction][end]`) of a
    /// direction that is not periodic shrinks to a single point: a
    /// sphere's poles, a cone's apex, a NURBS's collapsed control row.
    pub(crate) collapsed: [[bool; 2]; 2],
}

/// The scale of a parameter direction: its speed at the first sample
/// where that is more than resolution, or 1 where it never is, rounded to
/// the nearest power of two. Scaling by a power of two is exact, so a
/// point and its parameters always agree on which way a triangle winds.
fn scale(speeds: impl Iterator<Item = f64>) -> f64 {
    let s = speeds
        .into_iter()
        .find(|&s| s > LINEAR_RESOLUTION && s.is_finite())
        .unwrap_or(1.0);
    power_of_two(s)
}

/// The most a direction is shortened by [`stretched`].
const MAX_STRETCH: f64 = 1024.0;

/// A stretch is taken only where it saves at least this share of the
/// triangles the plane before it needed (see [`best_stretch`]).
const GAIN: f64 = 1.125;

/// The plane a face is meshed in: each direction scaled by its speed,
/// then one direction shortened by a power of two `f` up to
/// [`MAX_STRETCH`], so that triangles round in the plane run `f` times
/// longer that way on the surface. Waldron's bound holds in any scaling
/// of the plane, so every triangle is certified as in the speeds' plane;
/// only their shapes change (issue #288; design 296's anisotropy).
///
/// The plane is the one that needs the fewest triangles by an estimate
/// from the face's samples (the middle of its box and a 3 × 3 grid over
/// it): at each, the triangle's radius in the plane that the chord allows
/// (`M R² / 2 = chord`, `M` the largest eigenvalue of `|S_kk|` scaled to
/// the plane, as [`super::bound::second_derivative`] takes it) and that
/// the angle allows (the unit normal, turning at `|N_u|` and `|N_v|`, turns
/// by at most `angle` across `2R`), the lesser of the two; a triangle
/// covers `f R²` of the speeds' plane. Curved one way only (a cylinder, a
/// cone, an extrusion), a face takes the most stretch along its straight
/// direction. Where the face's bound over its whole box (what certifies,
/// cell by cell, near what samples miss: a twist between them) allows a
/// smaller stretch the same way by the chord's test alone, that is taken;
/// the bound asking for the other way, or none, keeps the speeds.
///
/// No direction is shortened past the face's extent that way in the
/// speeds' plane over the chord (as a power of two), so the face is never
/// narrower in the plane than the chord: a face a millimetre long along
/// its straight direction, stretched further, refines forever (ryukei
/// 11.B).
///
/// A face with a pole or an apex keeps its speeds (`collapsed`), as does
/// one whose box bound leaves the normal's turn unbounded (where `S_u ×
/// S_v` may vanish its triangles are certified near a degenerate row the
/// samples don't see), and one where a sample's normal is undefined.
fn stretched(
    speed: Metric,
    evals: &[crate::geom::SurfaceEval],
    bound: &crate::geom::SurfaceBound,
    t: &Targets,
    extent: [f64; 2],
) -> Metric {
    if !(bound.turn_u.is_finite() && bound.turn_v.is_finite()) {
        return speed;
    }
    // At each sample: `|S_uu|`, `|S_uv|`, `|S_vv|` and the unit normal's
    // turning rates along u and v (`|w_k|`'s part across the normal over
    // `|w|`, `w = S_u × S_v`).
    let mut samples: Vec<([f64; 3], [f64; 2])> = Vec::with_capacity(evals.len());
    for e in evals {
        let w = e.du.cross(e.dv);
        let Some(n) = w.normalize() else {
            return speed;
        };
        let n = n.get();
        let rate = |wk: crate::geom::Vector3| (wk - n * wk.dot(n)).length() / w.length();
        samples.push((
            [e.duu.length(), e.duv.length(), e.dvv.length()],
            [
                rate(e.duu.cross(e.dv) + e.du.cross(e.duv)),
                rate(e.duv.cross(e.dv) + e.du.cross(e.dvv)),
            ],
        ));
    }
    if samples.is_empty() {
        return speed;
    }
    let count = |m: Metric| {
        let f = (speed.a / m.a) * (speed.b / m.b);
        samples
            .iter()
            .map(|&([duu, duv, dvv], [tu, tv])| {
                let k = largest(duu / (m.a * m.a), duv / (m.a * m.b), dvv / (m.b * m.b));
                let turn = crate::math::sqrt((tu / m.a) * (tu / m.a) + (tv / m.b) * (tv / m.b));
                let by_chord = if k > 0.0 {
                    2.0 * t.chord / k
                } else {
                    f64::INFINITY
                };
                let by_angle = if turn > 0.0 {
                    let r = t.angle / (2.0 * turn);
                    r * r
                } else {
                    f64::INFINITY
                };
                let r2 = by_chord.min(by_angle);
                if r2.is_finite() { 1.0 / (f * r2) } else { 0.0 }
            })
            .sum::<f64>()
    };
    let by_samples = best_stretch(speed, |m| {
        let c = count(m);
        if c > 0.0 { 1.0 / c } else { f64::INFINITY }
    });
    let by_bound = best_stretch(speed, |m| {
        let f = (speed.a / m.a) * (speed.b / m.b);
        let k = super::bound::second_derivative(bound, m);
        if k > 0.0 { f / k } else { f64::INFINITY }
    });
    match (by_samples, by_bound) {
        (Some((u, f)), Some((v, g))) if u == v => {
            // Never shorten a direction below the chord in the plane: a
            // face straight one way but a millimetre long that way,
            // shortened past what the chord resolves, refines forever
            // (ryukei 11.B's hang).
            let along = if u { extent[0] } else { extent[1] };
            let room = along / t.chord;
            let most = if room >= 2.0 {
                floor_power_of_two(room)
            } else {
                1.0
            };
            let f = f.min(g).min(most);
            if f > 1.0 {
                shortened(speed, u, f)
            } else {
                speed
            }
        }
        _ => speed,
    }
}

/// The largest power of two at most `x >= 1`.
fn floor_power_of_two(x: f64) -> f64 {
    let p = power_of_two(x);
    if p > x { p / 2.0 } else { p }
}

/// `speed` with `u` (or else `v`) shortened `f` times.
fn shortened(speed: Metric, u: bool, f: f64) -> Metric {
    if u {
        Metric {
            a: speed.a / f,
            b: speed.b,
        }
    } else {
        Metric {
            a: speed.a,
            b: speed.b / f,
        }
    }
}

/// The direction (`true` for `u`) and the power of two `f` from 2 to
/// [`MAX_STRETCH`] whose plane `score`s best, `None` where none beats the
/// speeds' plane; a larger `f`, or the other direction, is taken only
/// where it scores [`GAIN`] times the best so far, so near ties keep the
/// smaller stretch.
fn best_stretch(speed: Metric, score: impl Fn(Metric) -> f64) -> Option<(bool, f64)> {
    let base = score(speed);
    if !(base > 0.0 && base.is_finite()) {
        return None;
    }
    let mut best = (base, None);
    for u in [true, false] {
        let mut f = 2.0;
        while f <= MAX_STRETCH {
            let s = score(shortened(speed, u, f));
            if s > best.0 * GAIN {
                best = (s, Some((u, f)));
            }
            f *= 2.0;
        }
    }
    best.1
}

/// The largest eigenvalue of `[[p, q], [q, r]]`.
fn largest(p: f64, q: f64, r: f64) -> f64 {
    let half = (p - r) / 2.0;
    (p + r) / 2.0 + crate::math::sqrt(half * half + q * q)
}

/// The power of two nearest `x > 0` (by ratio).
pub(crate) fn power_of_two(x: f64) -> f64 {
    let e = ((x.to_bits() >> 52) & 0x7ff) as i64 - 1023;
    let below = f64::from_bits(((e + 1023) as u64) << 52);
    if x >= below * core::f64::consts::SQRT_2 {
        below * 2.0
    } else {
        below
    }
}

impl<'a> FaceGeom<'a> {
    pub(crate) fn new(b: &'a Body, f: FaceId, t: &Targets) -> Self {
        let face = &b.faces[f];
        let surface = &b.surfaces[face.surface];
        let (pu, pv) = surface.periods();
        let domain = surface.domain();
        let forward = face.sense == Sense::Forward;
        let Opened {
            periods,
            wrap,
            seams,
        } = open_unwound(b, f, surface, [pu, pv], forward);
        let mut domain = domain;
        for k in 0..2 {
            if let (Some(s), Some(p)) = (seams[k], periods[k]) {
                domain[k] = [s, s + p];
            }
        }
        let collapsed = collapsed_rows(surface);
        // The parameter box of the face's vertices (or of its domain when
        // it has none), within the domain's finite part.
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for l in b.face_loops(f) {
            let lp = &b.loops[l];
            let corners = lp
                .vertex
                .into_iter()
                .chain(b.loop_fins(l).map(|fin| b.start_of(fin)));
            for v in corners {
                let (u, w) = surface.project(b.vertices[v].point).param;
                for (k, x) in [u, w].into_iter().enumerate() {
                    lo[k] = lo[k].min(x);
                    hi[k] = hi[k].max(x);
                }
            }
        }
        // A box flat in a direction (a face bounded by one closed edge has
        // one vertex) takes in points along the edges too: its middle
        // alone may sit anywhere, a sphere's near its pole, where the
        // speed would scale the whole face as if it were all that narrow.
        if (0..2).any(|k| lo[k] == hi[k]) {
            for l in b.face_loops(f) {
                for fin in b.loop_fins(l) {
                    let e = &b.edges[b.fins[fin].edge];
                    let [t0, t1] = e.interval;
                    for x in [0.25, 0.5, 0.75] {
                        let p = b.curves[e.curve].point(t0 + (t1 - t0) * x);
                        let (u, w) = surface.project(p).param;
                        for (k, x) in [u, w].into_iter().enumerate() {
                            lo[k] = lo[k].min(x);
                            hi[k] = hi[k].max(x);
                        }
                    }
                }
            }
        }
        let range: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let [d0, d1] = domain[k];
            let finite = |x: f64, y: f64| if x.is_finite() { x } else { y };
            if lo[k] <= hi[k] {
                [lo[k], hi[k]]
            } else {
                [finite(d0, -1.0), finite(d1, 1.0)]
            }
        });
        // The middle of the box first, then the centres of a 3 × 3 grid of
        // cells over it.
        let at = |k: usize, f: f64| range[k][0] + (range[k][1] - range[k][0]) * f;
        let cells = [1.0 / 6.0, 0.5, 5.0 / 6.0];
        let evals: Vec<_> = core::iter::once((0.5, 0.5))
            .chain(
                cells
                    .iter()
                    .flat_map(|&x| cells.iter().map(move |&y| (x, y))),
            )
            .map(|(x, y)| surface.eval(at(U, x), at(V, y)))
            .collect();
        let speed = Metric {
            a: scale(evals.iter().map(|e| e.du.length())),
            b: scale(evals.iter().map(|e| e.dv.length())),
        };
        let poles = collapsed.iter().flatten().any(|&c| c);
        let metric = if poles {
            speed
        } else {
            // The face's box in the speeds' plane.
            let extent = [
                (range[U][1] - range[U][0]) * speed.a,
                (range[V][1] - range[V][0]) * speed.b,
            ];
            stretched(speed, &evals, &surface.derivative_bound(range), t, extent)
        };
        Self {
            face: f,
            surface,
            forward,
            metric,
            speed,
            periods,
            wrap,
            full: [pu, pv],
            domain,
            collapsed,
        }
    }

    /// Whether the face is meshed in a plane stretched from its speeds.
    pub(crate) fn stretched(&self) -> bool {
        self.metric != self.speed
    }

    /// The speed of direction `k`.
    fn scale_of(&self, k: usize) -> f64 {
        if k == U { self.speed.a } else { self.speed.b }
    }

    /// The seam of a periodic direction: the start of its domain.
    pub(crate) fn seam(&self, k: usize) -> Option<f64> {
        self.periods[k].map(|_| self.domain[k][0])
    }

    /// The parameters of the nearest point of the surface to `p`, and its
    /// distance. Periodic parameters are in `[seam, seam + period)`, and
    /// parameters within resolution of a seam or of a collapsed row are
    /// moved exactly onto it.
    pub(crate) fn invert(&self, p: Point3) -> (Point2, f64) {
        let foot = self.surface.project(p);
        let (u, v) = foot.param;
        (self.snap(Point2::new(u, v)), foot.distance)
    }

    /// Moves parameters within resolution of a seam or collapsed row onto
    /// it; periodic ones into `[seam, seam + period)`.
    pub(crate) fn snap(&self, p: Point2) -> Point2 {
        let mut x = [p.x, p.y];
        for (k, x) in x.iter_mut().enumerate() {
            let close = |a: f64, b: f64| (a - b).abs() * self.scale_of(k) <= LINEAR_RESOLUTION;
            if let (Some(period), Some(seam)) = (self.periods[k], self.seam(k)) {
                *x = seam + (*x - seam).rem_euclid(period);
                if close(*x, seam) || close(*x, seam + period) {
                    *x = seam;
                }
            } else if let (Some(base), Some(period)) = (self.wrap[k], self.full[k]) {
                *x = base + (*x - base).rem_euclid(period);
            } else {
                for end in 0..2 {
                    if self.collapsed[k][end] && close(*x, self.domain[k][end]) {
                        *x = self.domain[k][end];
                    }
                }
            }
        }
        Point2::new(x[0], x[1])
    }

    /// The collapsed row `(direction, end)` that parameters lie exactly
    /// on, if any: there the other parameter is meaningless.
    pub(crate) fn pole(&self, p: Point2) -> Option<(usize, usize)> {
        let x = [p.x, p.y];
        (0..2)
            .flat_map(|k| (0..2).map(move |e| (k, e)))
            .find(|&(k, e)| self.collapsed[k][e] && x[k] == self.domain[k][e])
    }
}

/// A face on a surface periodic both ways has a domain only when its
/// loops cut it open in one direction: a torus face between two circles
/// (a ring turned whole, or a pipe's elbow) is a strip periodic only the
/// way its loops run. A direction no loop winds round is meshed as open,
/// its parameters wrapped into the one period that holds the face, which
/// starts just below where the face does: below the loop the face lies
/// above (on the left of a loop winding round, by the face's sense), or
/// below the face's single loop. The windings and extents are read from
/// the face's edges, sampled, inverted and unwrapped. A face whose loops
/// wind both ways keeps both periods (and is refused, as before), and so
/// does one whose loops are all holes (a torus dented by a boolean),
/// which covers the surface both ways round like a face with no loops.
///
/// How a face's periodic directions are meshed.
struct Opened {
    /// The periods to mesh with.
    periods: [Option<f64>; 2],
    /// For an opened direction, the start of its period.
    wrap: [Option<f64>; 2],
    /// Where to put the seams of a direction kept periodic, if not at the
    /// surface's own: between the loops, so none crosses one.
    seams: [Option<f64>; 2],
}

/// Returns the periods to mesh with, for an opened direction the start
/// of its period, and for a face kept periodic both ways where to put
/// its seams.
fn open_unwound(
    b: &Body,
    f: FaceId,
    surface: &Surface,
    periods: [Option<f64>; 2],
    forward: bool,
) -> Opened {
    const SAMPLES: usize = 16;
    let none = Opened {
        periods,
        wrap: [None; 2],
        seams: [None; 2],
    };
    let [Some(pu), Some(pv)] = periods else {
        return none;
    };
    let period = [pu, pv];
    // Each loop: its winding both ways, its unwrapped extent, and whether
    // it is a hole (it runs clockwise about what it encloses, seen from
    // the face's side, so the face is outside it).
    let mut loops: Vec<([i64; 2], [[f64; 2]; 2])> = Vec::new();
    let mut holes = 0;
    for l in b.face_loops(f) {
        let mut x: Vec<[f64; 2]> = Vec::new();
        for fin in b.loop_fins(l) {
            let e = &b.edges[b.fins[fin].edge];
            let curve = &b.curves[e.curve];
            let [t0, t1] = e.interval;
            for i in 0..SAMPLES {
                let s = i as f64 / SAMPLES as f64;
                let s = if b.fins[fin].sense == Sense::Forward {
                    s
                } else {
                    1.0 - s
                };
                let (u, v) = surface.project(curve.point(t0 + (t1 - t0) * s)).param;
                let raw = [u, v];
                let next = match x.last() {
                    Some(p) => {
                        core::array::from_fn(|k| super::edge::unwrap(raw[k], p[k], Some(period[k])))
                    }
                    None => raw,
                };
                x.push(next);
            }
        }
        let (Some(first), Some(last)) = (x.first(), x.last()) else {
            continue;
        };
        let wind: [i64; 2] = core::array::from_fn(|k| {
            let close = super::edge::unwrap(first[k], last[k], Some(period[k]));
            ((close - first[k]) / period[k]).round() as i64
        });
        let extent: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let lo = x.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
            let hi = x.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max);
            [lo, hi]
        });
        if wind == [0, 0] {
            let n = x.len();
            let area: f64 = (0..n)
                .map(|i| x[i][0] * x[(i + 1) % n][1] - x[(i + 1) % n][0] * x[i][1])
                .sum();
            if (area < 0.0) == forward {
                holes += 1;
            }
        }
        loops.push((wind, extent));
    }
    // A face whose loops are all holes covers the surface both ways
    // round, like a face with no loops: both directions stay periodic,
    // with the seams in gaps between the holes.
    if holes == loops.len() {
        let seams: [Option<f64>; 2] = core::array::from_fn(|k| {
            let extents: Vec<[f64; 2]> = loops.iter().map(|(_, e)| e[k]).collect();
            gap(&extents, period[k])
        });
        return Opened {
            periods,
            wrap: [None; 2],
            seams,
        };
    }
    if loops.is_empty() {
        return none;
    }
    let Some(k) = (0..2).find(|&k| loops.iter().all(|(w, _)| w[k] == 0)) else {
        return none;
    };
    let j = 1 - k;
    // The face is on the left of each loop when it uses the surface's
    // normal: above (in v) a loop running +u, and above (in u) a loop
    // running -v.
    let up = if k == V { 1 } else { -1 };
    let up = if forward { up } else { -up };
    let below = loops.iter().find(|(w, _)| w[j] == up);
    let lo = match below {
        Some((_, extent)) => extent[k][0],
        None => {
            let widest = loops
                .iter()
                .max_by(|a, b| (a.1[k][1] - a.1[k][0]).total_cmp(&(b.1[k][1] - b.1[k][0])))
                .expect("a loop");
            widest.1[k][0]
        }
    };
    // The period starts a little way below the loop's lowest sample: its
    // true lowest point may lie between samples. A quarter of the gap the
    // loops leave, or 5% of the period, is clear of the face.
    // Each loop's extent moved by whole periods to start at or above `lo`.
    let span = loops
        .iter()
        .map(|(_, e)| {
            let [a, b] = e[k];
            b + ((lo - a) / period[k]).ceil() * period[k]
        })
        .fold(f64::NEG_INFINITY, f64::max)
        - lo;
    // A band that spirals round the surface (at each height less than a
    // turn wide, but more than a turn in all) fits no window of one
    // period, and one within a tenth of a turn of that leaves too little
    // margin for its loops' sampled extent: both directions stay
    // periodic, with the seam across it in a gap between its loops,
    // which then cross only the other seam.
    if span >= 0.9 * period[k] {
        let extents: Vec<[f64; 2]> = loops.iter().map(|(_, e)| e[k]).collect();
        if let Some(seam) = gap(&extents, period[k]) {
            let mut seams = [None; 2];
            seams[k] = Some(seam);
            return Opened {
                periods,
                wrap: [None; 2],
                seams,
            };
        }
        // Loops covering every height of `k` between them leave it no
        // gap: the seam of the other direction goes in a gap of its own,
        // and the loops, which then fit between its seams, are cut at
        // `k`'s seam, as on a cylinder. Opening `k` instead would start
        // its period inside a loop.
        let extents: Vec<[f64; 2]> = loops.iter().map(|(_, e)| e[j]).collect();
        if let Some(seam) = gap(&extents, period[j]) {
            let mut seams = [None; 2];
            seams[j] = Some(seam);
            return Opened {
                periods,
                wrap: [None; 2],
                seams,
            };
        }
    }
    let margin = (0.25 * (period[k] - span))
        .min(0.05 * period[k])
        .max(1e-9 * period[k]);
    let mut open = periods;
    open[k] = None;
    let mut wrap = [None; 2];
    wrap[k] = Some(lo - margin);
    Opened {
        periods: open,
        wrap,
        seams: [None; 2],
    }
}

/// The middle of the widest stretch of a period that none of `extents`
/// (unwrapped, each shorter than a period) covers, if any.
fn gap(extents: &[[f64; 2]], period: f64) -> Option<f64> {
    // Each extent a period either side too, so one that runs past the end
    // of the period covers the start of it.
    let mut spans: Vec<[f64; 2]> = extents
        .iter()
        .flat_map(|&[lo, hi]| {
            let a = lo.rem_euclid(period);
            [-period, 0.0, period].map(|k| [a + k, a + k + (hi - lo)])
        })
        .collect();
    spans.sort_by(|x, y| x[0].total_cmp(&y[0]));
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for s in spans {
        match merged.last_mut() {
            Some(m) if s[0] <= m[1] => m[1] = m[1].max(s[1]),
            _ => merged.push(s),
        }
    }
    // The gaps between merged spans whose middles lie in one period.
    let mut best: Option<(f64, f64)> = None;
    for w in merged.windows(2) {
        let (end, next) = (w[0][1], w[1][0]);
        let (width, mid) = (next - end, (end + next) / 2.0);
        if (0.0..period).contains(&mid) && best.is_none_or(|(b, _)| width > b) {
            best = Some((width, mid));
        }
    }
    best.filter(|&(w, _)| w > 1e-6 * period).map(|(_, m)| m)
}

/// Which boundary rows of a surface collapse to a point, `[direction][end]`.
/// An extrusion or a pipe has none; a revolution's are its `v` ends on
/// the axis.
pub(crate) fn collapsed_rows(s: &Surface) -> [[bool; 2]; 2] {
    match s {
        Surface::Sphere(_) => [[false; 2], [true, true]],
        Surface::Cone(_) => [[false; 2], [true, false]],
        Surface::Revolution(r) => [[false; 2], r.poles()],
        Surface::Nurbs(n) => {
            let [nu, nv] = n.counts();
            let (pu, pv) = n.periods();
            let row =
                |pts: Vec<Point3>| pts.iter().all(|p| p.distance(pts[0]) <= LINEAR_RESOLUTION);
            let u_row =
                |i: usize| pu.is_none() && row((0..nv).map(|j| n.control_point(i, j)).collect());
            let v_row =
                |j: usize| pv.is_none() && row((0..nu).map(|i| n.control_point(i, j)).collect());
            [[u_row(0), u_row(nu - 1)], [v_row(0), v_row(nv - 1)]]
        }
        _ => [[false; 2]; 2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::name::OpKey;
    use crate::primitive;
    use core::f64::consts::{FRAC_PI_2, TAU};
    use std::sync::Arc;

    fn side(b: &Body) -> FaceId {
        b.faces().next().unwrap()
    }

    #[test]
    fn a_cylinder_side() {
        let (b, _) = primitive::cylinder(OpKey(1), 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b), &crate::mesh::edge::tests::targets(1e-3, 0.3));
        assert!(matches!(g.surface, Surface::Cylinder(_)));
        assert!(g.forward);
        assert_eq!(g.periods, [Some(TAU), None]);
        assert_eq!(g.seam(U), Some(0.0));
        assert_eq!(g.seam(V), None);
        assert_eq!(g.collapsed, [[false; 2]; 2]);
        assert_eq!((g.speed.a, g.speed.b), (2.0, 1.0));
        // Straight along v: stretched that way as far as it goes.
        assert_eq!((g.metric.a, g.metric.b), (2.0, 1.0 / MAX_STRETCH));
        // A point just short of a full turn lands on the seam.
        let p = Point3::new(2.0, -1e-10, 1.0);
        let (uv, d) = g.invert(p);
        assert_eq!(uv, Point2::new(0.0, 1.0));
        assert!(d <= 1e-15);
        let (uv, _) = g.invert(Point3::new(0.0, -2.0, 1.0));
        assert!((uv.x - 1.5 * core::f64::consts::PI).abs() <= 1e-15);
    }

    #[test]
    fn scales_are_powers_of_two() {
        assert_eq!(power_of_two(1.0), 1.0);
        assert_eq!(power_of_two(1.4), 1.0);
        assert_eq!(power_of_two(1.5), 2.0);
        assert_eq!(power_of_two(0.3), 0.25);
        assert_eq!(power_of_two(700.0), 512.0);
    }

    #[test]
    fn a_thin_ring_is_stretched_along_its_ring() {
        // A 2 cm tube turned 2 m round: triangles may run far further
        // along the ring than round the tube.
        let (b, _) = primitive::torus(OpKey(1), 2.0, 0.02, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b), &crate::mesh::edge::tests::targets(1e-3, 0.3));
        assert!(g.metric.a < g.speed.a, "{:?} {:?}", g.metric, g.speed);
        assert_eq!(g.metric.b, g.speed.b);
        // Exact powers of two either way.
        assert_eq!(power_of_two(g.speed.a / g.metric.a), g.speed.a / g.metric.a);
    }

    #[test]
    fn a_short_straight_face_is_shortened_no_further_than_its_chord() {
        // A cylinder a millimetre tall (ryukei 11.B's hang): its side is
        // straight along v, but only a millimetre long that way.
        use crate::mesh::edge::tests::targets;
        let (b, _) = primitive::cylinder(OpKey(1), 0.5, 1e-3, &Rigid::IDENTITY).unwrap();
        // The chord as long as the side: no stretch.
        let g = FaceGeom::new(&b, side(&b), &targets(1e-3, 0.3));
        assert_eq!(g.metric, g.speed);
        // A hundredth of it: shortened 64 times, the largest power of two
        // leaving the side at least a chord long in the plane.
        let g = FaceGeom::new(&b, side(&b), &targets(1e-5, 0.3));
        assert_eq!((g.metric.a, g.speed.b / g.metric.b), (g.speed.a, 64.0));
    }

    #[test]
    fn a_round_torus_and_a_plane_keep_their_speeds() {
        let (b, _) = primitive::torus(OpKey(1), 2.0, 1.0, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b), &crate::mesh::edge::tests::targets(1e-3, 0.3));
        assert_eq!(g.metric, g.speed);
        let (b, _) = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        for f in b.faces() {
            let g = FaceGeom::new(&b, f, &crate::mesh::edge::tests::targets(1e-3, 0.3));
            assert_eq!(g.metric, g.speed);
        }
    }

    #[test]
    fn a_sphere_has_poles() {
        let (b, _) = primitive::sphere(OpKey(1), 2.0, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b), &crate::mesh::edge::tests::targets(1e-3, 0.3));
        assert_eq!(g.collapsed, [[false; 2], [true, true]]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 2.0));
        let (uv, _) = g.invert(Point3::new(0.0, 0.0, 2.0 + 1e-12));
        assert_eq!(uv.y, FRAC_PI_2);
        assert_eq!(g.pole(uv), Some((V, 1)));
        assert_eq!(g.pole(Point2::new(1.0, 0.3)), None);
    }

    #[test]
    fn a_nurbs_cone_has_a_collapsed_row() {
        let (mut b, _) = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let f = side(&b);
        let s = b.faces[f].surface;
        let (n, _) = b.surfaces[s].to_nurbs([[0.0, 0.0], [-2.0, 0.0]]).unwrap();
        let id = b.add_surface(Surface::Nurbs(Arc::new(n)));
        crate::euler::set_surface(&mut b, f, id, Sense::Forward).unwrap();
        let g = FaceGeom::new(&b, f, &crate::mesh::edge::tests::targets(1e-3, 0.3));
        assert_eq!(g.periods[V], None);
        // The cone runs from its apex at v = -2 to its base at v = 0.
        assert_eq!(g.collapsed, [[false; 2], [true, false]]);
    }

    #[test]
    fn an_extruded_side_is_like_a_cylinder() {
        use crate::geom::{Circle, Curve, Extrusion, Frame, Vector3};
        let (mut b, _) = primitive::cylinder(OpKey(1), 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        let f = side(&b);
        let circle = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        let e = Extrusion::new(circle, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let id = b.add_surface(Surface::Extrusion(Arc::new(e)));
        crate::euler::set_surface(&mut b, f, id, Sense::Forward).unwrap();
        let g = FaceGeom::new(&b, f, &crate::mesh::edge::tests::targets(1e-3, 0.3));
        assert_eq!(g.periods, [Some(TAU), None]);
        assert_eq!(g.collapsed, [[false; 2]; 2]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 1.0 / MAX_STRETCH));
        let (uv, _) = g.invert(Point3::new(2.0, -1e-10, 1.0));
        assert_eq!(uv, Point2::new(0.0, 1.0));
    }
}
