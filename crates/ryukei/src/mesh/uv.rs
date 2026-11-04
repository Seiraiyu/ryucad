//! A face's parameter plane: how it is scaled, where its seams are, which
//! of its boundary rows collapse to a point, and how model-space points
//! are inverted into it.

use super::bound::Metric;
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
    /// Whether a vertex within its tolerance of a collapsed row is put
    /// on the row (see [`FaceGeom::on_row`]): a second try's, never the
    /// first's.
    pub(crate) tube_rows: bool,
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

/// How many times the less curved direction is shortened, at the
/// least, before a face is meshed in a stretched plane.
const STRETCH: f64 = 8.0;

/// The plane a face is meshed in: each direction scaled by its speed,
/// and where the surface curves far more one way than the other, the
/// less curved direction shortened by `sqrt(κ_more / κ_less)` as a power
/// of two, when that is at least [`STRETCH`]. Each direction's curvature
/// is read as `|S_kk| / |S_k|²` at every one of the face's samples (the
/// middle of its box and a 3 × 3 grid over it), never at one point (a
/// normal curvature vanishes where a torus's tube is level), and the
/// ratio is the least of the samples', the same direction less curved at
/// all of them. The factor is also no more than the face's bounds allow
/// (`sqrt` of `|S_kk|`'s bound over the speed squared one way against
/// the other's, over the face's box, the same direction less curved):
/// triangles are certified by those bounds, and stretching a direction
/// whose bound is far looser than its samples (a sweep's offset)
/// multiplies that bound by the factor squared. A triangle's deviation goes as `κ` times its extent
/// squared, so in that plane the chord allows about the same extent both
/// ways: on a thin tube swept round a wide ring (issue #288: a 2.8 cm
/// profile turned about an axis 4 m off), triangles run along the ring
/// as far as its curvature allows, not only as far as the tube's.
/// Waldron's bound holds in any scaling of the plane, so every triangle
/// is certified as before; only their shapes change. A plane, cylinder
/// or cone (curved one way only), a face with a collapsed row (a pole or
/// an apex), and faces curved within 64 times alike both ways keep their
/// speeds.
fn stretched(
    speed: Metric,
    evals: &[crate::geom::SurfaceEval],
    bound: &crate::geom::SurfaceBound,
    poles: bool,
) -> Metric {
    if poles {
        return speed;
    }
    // The bounds' ratio, the less curved direction first.
    let (bu, bv) = (
        bound.duu / (speed.a * speed.a),
        bound.dvv / (speed.b * speed.b),
    );
    if !(bu > 0.0 && bv > 0.0 && bu.is_finite() && bv.is_finite()) {
        return speed;
    }
    let curvature = |d2: crate::geom::Vector3, d1: crate::geom::Vector3| {
        let s = d1.length_squared();
        if s > 0.0 { d2.length() / s } else { 0.0 }
    };
    let mut least = f64::INFINITY;
    let mut along_u = None;
    for e in evals {
        let (ku, kv) = (curvature(e.duu, e.du), curvature(e.dvv, e.dv));
        if !(ku > 0.0 && kv > 0.0 && ku.is_finite() && kv.is_finite()) {
            return speed;
        }
        let u = ku < kv;
        if *along_u.get_or_insert(u) != u {
            return speed;
        }
        least = least.min(ku.max(kv) / ku.min(kv));
    }
    let Some(u) = along_u else {
        return speed;
    };
    if (bu < bv) != u {
        return speed;
    }
    let f = power_of_two(crate::math::sqrt(least.min(bu.max(bv) / bu.min(bv))));
    if f < STRETCH {
        return speed;
    }
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
    pub(crate) fn new(b: &'a Body, f: FaceId) -> Self {
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
            stretched(speed, &evals, &surface.derivative_bound(range), poles)
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
            tube_rows: false,
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

    /// A second try for a face whose domain couldn't be built: its seam
    /// moved off its loops where they graze the surface's own (see
    /// [`grazed_seam`]), and its tolerant vertices within their tolerance
    /// of a collapsed row put on it (see [`FaceGeom::on_row`]). `None`
    /// where neither changes anything.
    pub(crate) fn reseamed(&self, b: &Body) -> Option<Self> {
        let mut g = self.clone();
        let seams = grazed_seam(b, self.face, self.surface, self.periods);
        let moved = match (0..2).find(|&k| seams[k].is_some()) {
            Some(k) => {
                let (s, p) = (seams[k]?, self.periods[k]?);
                g.domain[k] = [s, s + p];
                true
            }
            None => false,
        };
        g.tube_rows = true;
        let rows = b
            .face_loops(self.face)
            .flat_map(|l| b.loop_fins(l))
            .any(|fin| {
                let v = b.edges[b.fins[fin].edge].start;
                let end = b.edges[b.fins[fin].edge].end;
                [v, end].into_iter().any(|v| {
                    let uv = g.invert(b.vertices[v].point).0;
                    g.pole(uv).is_none() && g.on_row(b, v, uv) != uv
                })
            });
        (moved || rows).then_some(g)
    }

    /// The parameters `uv` of vertex `v`, on the collapsed row its point
    /// lies within its tolerance of (and resolution) if [`Self::tube_rows`]
    /// is set, else as they are. In the tube model a vertex lies within
    /// its tolerance of its true place: a vertex sewing left a few microns
    /// off a sphere's pole, the corner where its loop passes through the
    /// pole, is that pole, and a loop through it doesn't wind round it
    /// (issue #348).
    pub(crate) fn on_row(&self, b: &Body, v: crate::topo::VertexId, uv: Point2) -> Point2 {
        if !self.tube_rows {
            return uv;
        }
        let vertex = &b.vertices[v];
        let reach = vertex.tolerance.get() + LINEAR_RESOLUTION;
        for k in 0..2 {
            for end in 0..2 {
                if !self.collapsed[k][end] {
                    continue;
                }
                let mut x = [uv.x, uv.y];
                x[k] = self.domain[k][end];
                let row = self.surface.point(x[0], x[1]);
                if row.distance(vertex.point) <= reach {
                    return Point2::new(x[0], x[1]);
                }
            }
        }
        uv
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

/// How far past a seam (as a share of the period) a loop that winds
/// round nothing may reach before the seam is moved off it (see
/// [`grazed_seam`]).
const GRAZE: f64 = 1e-3;

/// On a surface meshed periodic one way only, the seam of a face whose loops
/// wind round nothing but come within [`GRAZE`] of the period of the
/// surface's seam, past it or short of it: there cutting the face at the
/// seam leaves slivers rounding can't settle (fillets' quarter rounds in
/// a file, one with an edge 15 nm past the seam of its cylinder, others
/// starting 1e-13 rad off it, issue #449), or a loop across the seam whose
/// crossing is a tolerant vertex just off it, so that no point of the
/// loop is on the seam where it crosses (a sewn cone's, issue #348); the
/// seam goes in the widest gap the loops leave instead. Where loops wind
/// round, a vertex within the graze of the seam is such a crossing too
/// (the loop jumps across the seam between two edges' ends, a tolerant
/// vertex apart, where no edge's samples meet it: a sewn cone's band,
/// #348), and the seam goes in the widest gap between the vertices and
/// the loops that don't wind, where every loop crosses it inside an edge.
/// Any other face keeps the surface's own seam.
fn grazed_seam(
    b: &Body,
    f: FaceId,
    surface: &Surface,
    periods: [Option<f64>; 2],
) -> [Option<f64>; 2] {
    const SAMPLES: usize = 16;
    let mut seams = [None; 2];
    let ks: Vec<usize> = (0..2).filter(|&k| periods[k].is_some()).collect();
    let [k] = ks[..] else {
        return seams;
    };
    let period = periods[k].expect("periodic");
    let seam = surface.domain()[k][0];
    let mut extents: Vec<[f64; 2]> = Vec::new();
    let mut grazes = false;
    let reach = GRAZE * period;
    let near = |xi: f64| {
        let r = (xi - seam) / period;
        (r - r.round()).abs() * period < reach
    };
    for l in b.face_loops(f) {
        let mut x: Vec<f64> = Vec::new();
        // The index in `x` of each fin's first sample: a vertex.
        let mut corners: Vec<usize> = Vec::new();
        for fin in b.loop_fins(l) {
            corners.push(x.len());
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
                let raw = [u, v][k];
                x.push(match x.last() {
                    Some(&p) => super::edge::unwrap(raw, p, Some(period)),
                    None => raw,
                });
            }
        }
        let (Some(&first), Some(&last)) = (x.first(), x.last()) else {
            continue;
        };
        let close = super::edge::unwrap(first, last, Some(period));
        if ((close - first) / period).round() != 0.0 {
            // A loop winding round: only its vertices are kept clear.
            for &c in &corners {
                grazes |= near(x[c]);
                extents.push([x[c], x[c]]);
            }
            continue;
        }
        let lo = x.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        // Whether any sample lies within the graze of a seam line: the
        // loop's extent reaching only that far past the line, or stopping
        // that close short of it, or the loop crossing it at a point that
        // close (a tolerant vertex sewing left a few microns across the
        // seam it lay on, issue #348).
        grazes |= x.iter().copied().any(near);
        extents.push([lo, hi]);
    }
    if grazes {
        seams[k] = gap(&extents, period);
    }
    seams
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
        let g = FaceGeom::new(&b, side(&b));
        assert!(matches!(g.surface, Surface::Cylinder(_)));
        assert!(g.forward);
        assert_eq!(g.periods, [Some(TAU), None]);
        assert_eq!(g.seam(U), Some(0.0));
        assert_eq!(g.seam(V), None);
        assert_eq!(g.collapsed, [[false; 2]; 2]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 1.0));
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
    fn a_sphere_has_poles() {
        let (b, _) = primitive::sphere(OpKey(1), 2.0, &Rigid::IDENTITY).unwrap();
        let g = FaceGeom::new(&b, side(&b));
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
        let g = FaceGeom::new(&b, f);
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
        let g = FaceGeom::new(&b, f);
        assert_eq!(g.periods, [Some(TAU), None]);
        assert_eq!(g.collapsed, [[false; 2]; 2]);
        assert_eq!((g.metric.a, g.metric.b), (2.0, 1.0));
        let (uv, _) = g.invert(Point3::new(2.0, -1e-10, 1.0));
        assert_eq!(uv, Point2::new(0.0, 1.0));
    }
}
