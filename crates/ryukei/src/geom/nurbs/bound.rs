//! Certified derivative bounds for NURBS over a parameter range.
//!
//! Each Bézier piece the range overlaps is cut down to the overlap by de
//! Casteljau subdivision in homogeneous coordinates, all in interval
//! arithmetic. The derivatives of the homogeneous numerator `A = Σ B w P`
//! and denominator `W = Σ B w` are polynomials whose ranges lie within
//! the hulls of their difference nets; the quotient rule, again in
//! intervals, bounds the rational derivatives. The control points are
//! moved so the hull's centre is the origin first, which keeps the
//! quotient's intervals tight. Every operation rounds outward, so the
//! bounds hold for the true values.

use super::bezier::{BezierCurve, BezierPatch};
use super::{NurbsCurve, NurbsSurface};
use crate::geom::Vector3;
use crate::geom::bound2::{CurveBound, SurfaceBound};
use crate::interval::Interval;

type Iv = Interval;

/// A homogeneous control point `(w x, w y, w z, w)`.
type H = [Iv; 4];

/// An interval vector.
type V3 = [Iv; 3];

fn pt(x: f64) -> Iv {
    Iv::point(x)
}

fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: V3, s: Iv) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn cross3(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Divides by a positive interval; unbounded if it may hold zero.
fn div3(a: V3, w: Iv) -> V3 {
    let whole = Iv::new(f64::NEG_INFINITY, f64::INFINITY).expect("ordered");
    a.map(|x| x.checked_div(w).unwrap_or(whole))
}

/// The greatest magnitude of an interval.
fn mag(x: Iv) -> f64 {
    x.lo().abs().max(x.hi().abs())
}

/// The least magnitude of an interval: 0 when it holds zero.
fn mig(x: Iv) -> f64 {
    if x.lo() > 0.0 {
        x.lo()
    } else if x.hi() < 0.0 {
        -x.hi()
    } else {
        0.0
    }
}

/// An upper bound on the length of every vector in the box.
fn norm_hi(a: V3) -> f64 {
    let s = a
        .iter()
        .fold(pt(0.0), |acc, &x| acc + pt(mag(x)) * pt(mag(x)));
    s.sqrt().map_or(f64::INFINITY, Iv::hi)
}

/// A lower bound on the length of every vector in the box.
fn norm_lo(a: V3) -> f64 {
    let s = a
        .iter()
        .fold(pt(0.0), |acc, &x| acc + pt(mig(x)) * pt(mig(x)));
    s.sqrt().map_or(0.0, Iv::lo)
}

/// The interval hull of two intervals.
fn hull(a: Iv, b: Iv) -> Iv {
    Iv::new(a.lo().min(b.lo()), a.hi().max(b.hi())).expect("ordered")
}

/// The componentwise hull of a list of homogeneous points.
fn hull4(pts: impl IntoIterator<Item = H>) -> H {
    pts.into_iter()
        .reduce(|a, b| core::array::from_fn(|k| hull(a[k], b[k])))
        .expect("non-empty")
}

fn lerp4(a: H, b: H, s: Iv) -> H {
    core::array::from_fn(|k| a[k] + (b[k] - a[k]) * s)
}

/// The control points of the part `[s0, s1]` of a Bézier curve over
/// `[0, 1]`, by de Casteljau subdivision.
fn sub_curve(pts: &[H], s0: f64, s1: f64) -> Vec<H> {
    // The part after s0.
    let mut right = pts.to_vec();
    if s0 > 0.0 {
        right = split(&right, pt(s0)).1;
    }
    if s1 < 1.0 {
        // s1 in the right part's own parameter.
        let s = (pt(s1) - pt(s0))
            .checked_div(pt(1.0) - pt(s0))
            .expect("s0 < 1");
        right = split(&right, s).0;
    }
    right
}

/// De Casteljau at `s`: the left and right control points.
fn split(pts: &[H], s: Iv) -> (Vec<H>, Vec<H>) {
    let n = pts.len();
    let mut work = pts.to_vec();
    let mut left = Vec::with_capacity(n);
    let mut right = vec![work[n - 1]; n];
    left.push(work[0]);
    right[n - 1] = work[n - 1];
    for level in 1..n {
        for i in 0..n - level {
            work[i] = lerp4(work[i], work[i + 1], s);
        }
        left.push(work[0]);
        right[n - 1 - level] = work[n - 1 - level];
    }
    (left, right)
}

fn hom(p: Vector3, w: f64) -> H {
    [pt(w) * pt(p.x), pt(w) * pt(p.y), pt(w) * pt(p.z), pt(w)]
}

/// The parts of `[t0, t1]` inside a spline's domain, unwrapped into it
/// when periodic.
fn clip(domain: [f64; 2], period: Option<f64>, [t0, t1]: [f64; 2]) -> Vec<[f64; 2]> {
    let [d0, d1] = domain;
    match period {
        Some(p) if t1 - t0 < p => {
            let a = d0 + (t0 - d0).rem_euclid(p);
            let b = a + (t1 - t0);
            if b <= d1 {
                vec![[a, b]]
            } else {
                vec![[a, d1], [d0, (b - p).min(d1)]]
            }
        }
        Some(_) => vec![domain],
        None => vec![[t0.max(d0), t1.min(d1).max(t0.max(d0))]],
    }
}

/// The local parameters `[s0, s1]` of `[a, b]` within a piece's range,
/// widened by an ulp each way to cover their rounding, or `None` when they
/// don't overlap.
fn local([r0, r1]: [f64; 2], [a, b]: [f64; 2]) -> Option<[f64; 2]> {
    if b < r0 || a > r1 {
        return None;
    }
    let s0 = ((a.max(r0) - r0) / (r1 - r0)).next_down().max(0.0);
    let s1 = ((b.min(r1) - r0) / (r1 - r0)).next_up().min(1.0);
    Some([s0, s1])
}

/// The net `c (x[i + 1] - x[i])`: a Bézier curve's derivative, with `c`
/// its degree over its parameter length.
fn diffs(x: &[H], c: Iv) -> Vec<H> {
    x.windows(2)
        .map(|w| core::array::from_fn(|k| (w[1][k] - w[0][k]) * c))
        .collect()
}

/// The hull of a net, or zero for an empty one (the derivative of a
/// constant).
fn hull_or_zero(x: &[H]) -> H {
    if x.is_empty() {
        [pt(0.0); 4]
    } else {
        hull4(x.iter().copied())
    }
}

/// The hull of the part `[s0, s1]` of a curve net.
fn part_hull(x: &[H], [s0, s1]: [f64; 2]) -> H {
    if x.is_empty() {
        return [pt(0.0); 4];
    }
    hull4(sub_curve(x, s0, s1))
}

/// The spatial and weight parts of a homogeneous interval point.
fn parts(h: H) -> (V3, Iv) {
    ([h[0], h[1], h[2]], h[3])
}

/// Moves homogeneous points so that `c` becomes the origin: `w x - w c`.
fn centred(pts: &[H], c: Vector3) -> Vec<H> {
    let c = [pt(c.x), pt(c.y), pt(c.z)];
    pts.iter()
        .map(|h| {
            [
                h[0] - h[3] * c[0],
                h[1] - h[3] * c[1],
                h[2] - h[3] * c[2],
                h[3],
            ]
        })
        .collect()
}

/// The centre of the box of the points' Euclidean midpoints.
fn centre(pts: &[H]) -> Vector3 {
    let mid = |h: &H, k: usize| 0.5 * (h[k].lo() + h[k].hi()) / (0.5 * (h[3].lo() + h[3].hi()));
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for h in pts {
        for k in 0..3 {
            lo[k] = lo[k].min(mid(h, k));
            hi[k] = hi[k].max(mid(h, k));
        }
    }
    Vector3::new(
        0.5 * (lo[0] + hi[0]),
        0.5 * (lo[1] + hi[1]),
        0.5 * (lo[2] + hi[2]),
    )
}

/// The box of the Euclidean control points: with positive weights the
/// piece lies in their convex hull.
fn point_hull(pts: &[H]) -> V3 {
    let e = hull4(pts.iter().map(|h| {
        let (a, w) = parts(*h);
        let p = div3(a, w);
        [p[0], p[1], p[2], w]
    }));
    [e[0], e[1], e[2]]
}

/// Bounds on one Bézier curve piece over `[s0, s1]`, its control points
/// already in homogeneous intervals.
///
/// The derivative nets are taken over the whole piece and only then cut
/// down to `[s0, s1]`: cutting first and differencing after would divide
/// the rounding of the cut by the part's length, which can be tiny.
fn curve_piece(pts: &[H], range: [f64; 2], s: [f64; 2]) -> CurveBound {
    let sub = sub_curve(pts, s[0], s[1]);
    let at = centre(&sub);
    let whole = centred(pts, at);
    let n = (pts.len() - 1) as f64;
    let Some(inv) = pt(1.0).checked_div(pt(range[1]) - pt(range[0])) else {
        return CurveBound {
            d2: f64::INFINITY,
            turn: f64::INFINITY,
        };
    };
    let net1 = diffs(&whole, pt(n) * inv);
    let net2 = diffs(&net1, pt(n - 1.0) * inv);
    let c = point_hull(&centred(&sub, at));
    let (a1, w1) = parts(part_hull(&net1, s));
    let (a2, w2) = parts(part_hull(&net2, s));
    let (_, w) = parts(hull4(sub.iter().copied()));
    // C' = (A' - W' C) / W;  C'' = (A'' - 2 W' C' - W'' C) / W.
    let d1 = div3(sub3(a1, scale3(c, w1)), w);
    let d2 = div3(sub3(sub3(a2, scale3(d1, pt(2.0) * w1)), scale3(c, w2)), w);
    let d2_hi = norm_hi(d2);
    let speed = norm_lo(d1);
    let turn = if speed > 0.0 {
        (pt(norm_hi(d1)) * pt(d2_hi))
            .checked_div(pt(speed) * pt(speed))
            .map_or(f64::INFINITY, Iv::hi)
    } else {
        f64::INFINITY
    };
    CurveBound { d2: d2_hi, turn }
}

/// Enclosures of one Bézier piece's point and first three derivatives
/// over `[s0, s1]`, as [`curve_piece`] bounds the first two: the nets of
/// the whole piece, centred on the part, then cut down, and the quotient
/// rule `C''' = (A''' - 3 W' C'' - 3 W'' C' - W''' C) / W`.
fn jet_piece(pts: &[H], range: [f64; 2], s: [f64; 2]) -> [V3; 4] {
    let sub = sub_curve(pts, s[0], s[1]);
    let at = centre(&sub);
    let whole = centred(pts, at);
    let n = (pts.len() - 1) as f64;
    let Some(inv) = pt(1.0).checked_div(pt(range[1]) - pt(range[0])) else {
        let all = Iv::new(f64::NEG_INFINITY, f64::INFINITY).expect("ordered");
        return [[all; 3]; 4];
    };
    let net1 = diffs(&whole, pt(n) * inv);
    let net2 = diffs(&net1, pt(n - 1.0) * inv);
    let net3 = diffs(&net2, pt(n - 2.0) * inv);
    let c = point_hull(&centred(&sub, at));
    let (a1, w1) = parts(part_hull(&net1, s));
    let (a2, w2) = parts(part_hull(&net2, s));
    let (a3, w3) = parts(part_hull(&net3, s));
    let (_, w) = parts(hull4(sub.iter().copied()));
    let d1 = div3(sub3(a1, scale3(c, w1)), w);
    let d2 = div3(sub3(sub3(a2, scale3(d1, pt(2.0) * w1)), scale3(c, w2)), w);
    let d3 = div3(
        sub3(
            sub3(sub3(a3, scale3(d2, pt(3.0) * w1)), scale3(d1, pt(3.0) * w2)),
            scale3(c, w3),
        ),
        w,
    );
    let at = [pt(at.x), pt(at.y), pt(at.z)];
    [add3(c, at), d1, d2, d3]
}

/// The componentwise hull of two interval vectors.
fn hull3(a: V3, b: V3) -> V3 {
    core::array::from_fn(|k| hull(a[k], b[k]))
}

impl NurbsCurve {
    /// Enclosures of the point and its first three derivatives at every
    /// parameter of `range`, clipped to the domain (a periodic range
    /// wraps): the hull of [`jet_piece`] over the pieces it overlaps.
    pub(crate) fn jet_box(&self, range: [f64; 2]) -> [V3; 4] {
        let mut out: Option<[V3; 4]> = None;
        for piece in self.pieces() {
            let pts: Vec<H> = piece.pts.iter().map(|h| hom(h.p, h.w)).collect();
            for r in clip(self.domain(), self.period(), range) {
                if let Some(s) = local(piece.range, r) {
                    let j = jet_piece(&pts, piece.range, s);
                    out = Some(match out {
                        None => j,
                        Some(o) => core::array::from_fn(|k| hull3(o[k], j[k])),
                    });
                }
            }
        }
        out.unwrap_or_else(|| {
            let [d0, d1] = self.domain();
            let t = range[0].clamp(d0, d1);
            self.jet_box([t, t])
        })
    }
}

fn worse(a: CurveBound, b: CurveBound) -> CurveBound {
    CurveBound {
        d2: a.d2.max(b.d2),
        turn: a.turn.max(b.turn),
    }
}

fn curve_bound(pieces: &[BezierCurve<Vector3>], ranges: &[[f64; 2]]) -> CurveBound {
    let mut out = CurveBound { d2: 0.0, turn: 0.0 };
    for piece in pieces {
        let pts: Vec<H> = piece.pts.iter().map(|h| hom(h.p, h.w)).collect();
        for &r in ranges {
            if let Some(s) = local(piece.range, r) {
                out = worse(out, curve_piece(&pts, piece.range, s));
            }
        }
    }
    out
}

impl NurbsCurve {
    /// Bounds on the derivatives over the part of `range` inside the
    /// domain (a periodic range wraps).
    pub fn derivative_bound(&self, range: [f64; 2]) -> CurveBound {
        curve_bound(self.pieces(), &clip(self.domain(), self.period(), range))
    }
}

/// A row-major net of `rows × cols` points.
struct Net {
    pts: Vec<H>,
    rows: usize,
    cols: usize,
}

impl Net {
    fn at(&self, i: usize, j: usize) -> H {
        self.pts[i * self.cols + j]
    }

    /// The derivative net along the rows (`u`), times `c`.
    fn diff_u(&self, c: Iv) -> Self {
        let rows = self.rows.saturating_sub(1);
        let pts = (0..rows)
            .flat_map(|i| {
                (0..self.cols).map(move |j| {
                    let (a, b) = (self.at(i + 1, j), self.at(i, j));
                    core::array::from_fn(|k| (a[k] - b[k]) * c)
                })
            })
            .collect();
        Self {
            pts,
            rows,
            cols: self.cols,
        }
    }

    /// The derivative net along the columns (`v`), times `c`.
    fn diff_v(&self, c: Iv) -> Self {
        let cols = self.cols.saturating_sub(1);
        let pts = (0..self.rows)
            .flat_map(|i| {
                (0..cols).map(move |j| {
                    let (a, b) = (self.at(i, j + 1), self.at(i, j));
                    core::array::from_fn(|k| (a[k] - b[k]) * c)
                })
            })
            .collect();
        Self {
            pts,
            rows: self.rows,
            cols,
        }
    }

    /// The net of the part `[s, t]` of the patch.
    fn part(&self, s: [f64; 2], t: [f64; 2]) -> Self {
        if self.pts.is_empty() {
            return Self {
                pts: Vec::new(),
                rows: self.rows,
                cols: self.cols,
            };
        }
        let cols: Vec<Vec<H>> = (0..self.cols)
            .map(|j| {
                let line: Vec<H> = (0..self.rows).map(|i| self.at(i, j)).collect();
                sub_curve(&line, s[0], s[1])
            })
            .collect();
        let pts = (0..self.rows)
            .flat_map(|i| {
                let line: Vec<H> = cols.iter().map(|c| c[i]).collect();
                sub_curve(&line, t[0], t[1])
            })
            .collect();
        Self {
            pts,
            rows: self.rows,
            cols: self.cols,
        }
    }
}

/// Enclosures of one Bézier patch's point and its first and second
/// derivatives over a local box `[s, t]`: `[S, Su, Sv, Suu, Suv, Svv]`,
/// the point relative to the returned centre. As for curves, the
/// derivative nets are taken over the whole patch before the cut. `None`
/// where the patch's parameter ranges can't be inverted.
fn patch_jets(patch: &BezierPatch, s: [f64; 2], t: [f64; 2]) -> Option<(Vector3, [V3; 6])> {
    let [p, q] = patch.degrees;
    let net = Net {
        pts: patch.pts.iter().map(|h| hom(h.p, h.w)).collect(),
        rows: p + 1,
        cols: q + 1,
    };
    let sub = net.part(s, t);
    let at = centre(&sub.pts);
    let whole = Net {
        pts: centred(&net.pts, at),
        rows: net.rows,
        cols: net.cols,
    };
    let [ru, rv] = patch.range;
    let iu = pt(1.0).checked_div(pt(ru[1]) - pt(ru[0]))?;
    let iv = pt(1.0).checked_div(pt(rv[1]) - pt(rv[0]))?;
    let (fp, fq) = (p as f64, q as f64);
    let nu = whole.diff_u(pt(fp) * iu);
    let nv = whole.diff_v(pt(fq) * iv);
    let nuu = nu.diff_u(pt(fp - 1.0) * iu);
    let nvv = nv.diff_v(pt(fq - 1.0) * iv);
    let nuv = nu.diff_v(pt(fq) * iv);
    let hull = |n: &Net| parts(hull_or_zero(&n.part(s, t).pts));
    let c = point_hull(&centred(&sub.pts, at));
    let (_, w) = parts(hull4(sub.pts.iter().copied()));
    let ((au, wu), (av, wv)) = (hull(&nu), hull(&nv));
    let ((auu, wuu), (auv, wuv), (avv, wvv)) = (hull(&nuu), hull(&nuv), hull(&nvv));
    // The quotient rule, as for curves, with the mixed term
    // Suv = (Auv - Wu Sv - Wv Su - Wuv S) / W.
    let su = div3(sub3(au, scale3(c, wu)), w);
    let sv = div3(sub3(av, scale3(c, wv)), w);
    let suu = div3(sub3(sub3(auu, scale3(su, pt(2.0) * wu)), scale3(c, wuu)), w);
    let svv = div3(sub3(sub3(avv, scale3(sv, pt(2.0) * wv)), scale3(c, wvv)), w);
    let suv = div3(
        sub3(
            sub3(sub3(auv, scale3(sv, wu)), scale3(su, wv)),
            scale3(c, wuv),
        ),
        w,
    );
    Some((at, [c, su, sv, suu, suv, svv]))
}

/// Bounds on one Bézier patch over a local box `[s, t]`, and on its
/// first derivatives' lengths `|S_u|` and `|S_v|`.
fn patch_piece(patch: &BezierPatch, s: [f64; 2], t: [f64; 2]) -> (SurfaceBound, [f64; 2]) {
    let Some((_, [_, su, sv, suu, suv, svv])) = patch_jets(patch, s, t) else {
        return (
            SurfaceBound {
                duu: f64::INFINITY,
                duv: f64::INFINITY,
                dvv: f64::INFINITY,
                turn_u: f64::INFINITY,
                turn_v: f64::INFINITY,
            },
            [f64::INFINITY; 2],
        );
    };
    // The unit normal n = N / |N| turns at most |N_u| / |N| along u, with
    // N_u = Suu × Sv + Su × Suv (and likewise along v).
    let n_min = norm_lo(cross3(su, sv));
    let turn = |d: V3| {
        if n_min > 0.0 {
            pt(norm_hi(d))
                .checked_div(pt(n_min))
                .map_or(f64::INFINITY, Iv::hi)
        } else {
            f64::INFINITY
        }
    };
    (
        SurfaceBound {
            duu: norm_hi(suu),
            duv: norm_hi(suv),
            dvv: norm_hi(svv),
            turn_u: turn(add3(cross3(suu, sv), cross3(su, suv))),
            turn_v: turn(add3(cross3(suv, sv), cross3(su, svv))),
        },
        [norm_hi(su), norm_hi(sv)],
    )
}

impl NurbsSurface {
    /// Bounds on the derivatives over the part of the box inside the
    /// domain (periodic directions wrap).
    pub fn derivative_bound(&self, range: [[f64; 2]; 2]) -> SurfaceBound {
        self.derivative_bounds(range).0
    }

    /// Bounds on the first derivatives' lengths, `|S_u|` and `|S_v|`,
    /// over the part of the box inside the domain.
    pub fn first_bound(&self, range: [[f64; 2]; 2]) -> [f64; 2] {
        self.derivative_bounds(range).1
    }

    /// [`Self::derivative_bound`] and [`Self::first_bound`] at once.
    pub fn derivative_bounds(&self, [ru, rv]: [[f64; 2]; 2]) -> (SurfaceBound, [f64; 2]) {
        let (pu, pv) = self.periods();
        let [du, dv] = self.domain();
        let (cu, cv) = (clip(du, pu, ru), clip(dv, pv, rv));
        let mut out = SurfaceBound {
            duu: 0.0,
            duv: 0.0,
            dvv: 0.0,
            turn_u: 0.0,
            turn_v: 0.0,
        };
        let mut first = [0.0f64; 2];
        for patch in self.patches() {
            for &a in &cu {
                let Some(s) = local(patch.range[0], a) else {
                    continue;
                };
                for &b in &cv {
                    let Some(t) = local(patch.range[1], b) else {
                        continue;
                    };
                    let (x, d) = patch_piece(patch, s, t);
                    first = [first[0].max(d[0]), first[1].max(d[1])];
                    out = SurfaceBound {
                        duu: out.duu.max(x.duu),
                        duv: out.duv.max(x.duv),
                        dvv: out.dvv.max(x.dvv),
                        turn_u: out.turn_u.max(x.turn_u),
                        turn_v: out.turn_v.max(x.turn_v),
                    };
                }
            }
        }
        (out, first)
    }

    /// Enclosures of the point and its first and second derivatives at
    /// every parameter of the box, `[S, Su, Sv, Suu, Suv, Svv]`, clipped
    /// to the domain (periodic directions wrap): the hull of each Bézier
    /// patch's enclosures over the part of the box it holds. Unbounded
    /// where a patch's ranges can't be inverted.
    pub(crate) fn jet_box(&self, [ru, rv]: [[f64; 2]; 2]) -> [V3; 6] {
        let (pu, pv) = self.periods();
        let [du, dv] = self.domain();
        let (cu, cv) = (clip(du, pu, ru), clip(dv, pv, rv));
        let all = Iv::new(f64::NEG_INFINITY, f64::INFINITY).expect("ordered");
        let mut out: Option<[V3; 6]> = None;
        for patch in self.patches() {
            for &a in &cu {
                let Some(s) = local(patch.range[0], a) else {
                    continue;
                };
                for &b in &cv {
                    let Some(t) = local(patch.range[1], b) else {
                        continue;
                    };
                    let j = match patch_jets(patch, s, t) {
                        Some((at, mut j)) => {
                            j[0] = add3(j[0], [pt(at.x), pt(at.y), pt(at.z)]);
                            j
                        }
                        None => [[all; 3]; 6],
                    };
                    out = Some(match out {
                        None => j,
                        Some(o) => core::array::from_fn(|k| hull3(o[k], j[k])),
                    });
                }
            }
        }
        out.unwrap_or_else(|| {
            let u = ru[0].clamp(du[0], du[1]);
            let v = rv[0].clamp(dv[0], dv[1]);
            self.jet_box([[u, u], [v, v]])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_bounds_hold() {
        // A torus's exact rational form: |S_u| and |S_v| at a grid of
        // points of each box stay within the box's bound.
        use crate::geom::{Frame, Surface, Torus};
        let torus = Surface::Torus(Torus::new(Frame::WORLD, 2.0, 0.5).unwrap());
        let tau = core::f64::consts::TAU;
        let (n, _) = torus.to_nurbs([[0.0, tau], [0.0, tau]]).unwrap();
        for i in 0..8 {
            for j in 0..8 {
                let (u0, v0) = (tau * f64::from(i) / 8.0, tau * f64::from(j) / 8.0);
                let b = [[u0, u0 + 0.3], [v0, v0 + 0.5]];
                let [bu, bv] = n.first_bound(b);
                for a in 0..=4 {
                    for c in 0..=4 {
                        let (u, v) = (u0 + 0.3 * f64::from(a) / 4.0, v0 + 0.5 * f64::from(c) / 4.0);
                        let e = n.eval(u, v);
                        assert!(e.du.length() <= bu && e.dv.length() <= bv, "{u} {v}");
                    }
                }
                assert!(bu.is_finite() && bv.is_finite());
            }
        }
    }

    #[test]
    fn clipping_ranges() {
        assert_eq!(clip([0.0, 4.0], None, [1.0, 2.0]), [[1.0, 2.0]]);
        assert_eq!(clip([0.0, 4.0], None, [-1.0, 5.0]), [[0.0, 4.0]]);
        assert_eq!(clip([0.0, 4.0], Some(4.0), [5.0, 6.0]), [[1.0, 2.0]]);
        assert_eq!(
            clip([0.0, 4.0], Some(4.0), [3.0, 5.0]),
            [[3.0, 4.0], [0.0, 1.0]]
        );
        assert_eq!(clip([0.0, 4.0], Some(4.0), [-1.0, 9.0]), [[0.0, 4.0]]);
    }

    #[test]
    fn local_parameters_cover_the_range() {
        assert_eq!(local([1.0, 3.0], [4.0, 5.0]), None);
        let [s0, s1] = local([1.0, 3.0], [1.5, 2.0]).unwrap();
        assert!((0.249_999..=0.25).contains(&s0) && (0.5..0.500_001).contains(&s1));
        assert_eq!(local([1.0, 3.0], [0.0, 9.0]), Some([0.0, 1.0]));
    }

    #[test]
    fn subdivision_keeps_the_curve() {
        // The quadratic (t, t², 0) with unit weights.
        let pts = [
            hom(Vector3::new(0.0, 0.0, 0.0), 1.0),
            hom(Vector3::new(0.5, 0.0, 0.0), 1.0),
            hom(Vector3::new(1.0, 1.0, 0.0), 1.0),
        ];
        let sub = sub_curve(&pts, 0.25, 0.75);
        // Its ends are the curve at 0.25 and 0.75.
        assert!(sub[0][0].contains(0.25) && sub[0][1].contains(0.0625));
        assert!(sub[2][0].contains(0.75) && sub[2][1].contains(0.5625));
    }
}
