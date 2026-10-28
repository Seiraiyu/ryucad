//! Certifying a sweep along an open path embedded: the map carrying the
//! profile's plane along the path, `Φ(x, s)`, is one to one over a
//! rectangle holding the profile and the whole path. Every face of the
//! body is the image of a piece of that domain (a side of a segment times
//! a leg's range, a cap of the profile at an end), so faces of an
//! embedded sweep meet only where their pieces meet: along the edges and
//! at the vertices they share. The self-intersection check, whose pairs of
//! long thin swept faces go to the four-parameter tracer (seconds to
//! minutes a pair, issue #133), is then not needed.
//!
//! For fixed `s` the map is a similarity of the plane, one to one. Two
//! sections at `s ≠ r` are apart when their boxes are; otherwise, over a
//! run of the path holding both, `g(s) = N(r) · (Φ(x, s) - Φ(x, r))`
//! (`N(r)` the normal of the section at `r`) vanishes at `s = r`, and its
//! derivative `N(r) · Φ_s(x, s)` is certified of one sign for every `r`
//! and `s` in the run, so `Φ(x, s)` never lies on the section at `r`. The
//! map is affine in `x` for fixed `s`, so that sign is certified at the
//! rectangle's four corners. The path is cut into pieces, halved where
//! either test fails; past the budget the caller checks every pair of
//! faces as before. Sections either side of a joint between legs agree to
//! rounding, which this ignores, as the builder does.
//!
//! The same enclosures show a sweep that folds: where `N(s) · Φ_s(x, s)`,
//! the Jacobian's sign, is certified one way at a vertex of a lump and the
//! other way at a vertex of the same lump (at any `s`), the lump runs back
//! over itself somewhere between, as a profile does beyond a bend's centre
//! of curvature.

use super::{Leg, Point3, ValidProfile};
use crate::geom::path_sweep::Carry;
use crate::geom::{Box3, FrameLaw, Point2, Vector3};
use crate::interval::Interval;
use crate::sweep::SegmentKey;

/// The most pieces of the path looked at before giving up.
const MAX_PIECES: usize = 2048;

/// The most rounds of halving.
const MAX_ROUNDS: u32 = 24;

/// The pieces each leg starts in.
const START_PIECES: usize = 4;

/// The rectangle's corners, which come first among the points carried.
const CORNERS: usize = 4;

type IVec = [Interval; 3];

/// How one leg carries a point of the profile's plane: `c(s) + Σⱼ (aⱼ +
/// σ(s) bⱼ) fⱼ(s)`, as [`crate::geom::Sweep`] carries its profile.
struct LegMap {
    carry: Carry,
    range: [f64; 2],
    /// The centre's place in the start frame.
    a: [f64; 3],
    /// Each point's place about the centre in the start frame: the
    /// rectangle's corners, then the profile's vertices.
    b: Vec<[f64; 3]>,
    /// The section's normal in the start frame.
    n: [f64; 3],
    /// `σ` at the leg's start is 1; its rate along the leg.
    rate: f64,
}

/// A piece of a leg's range, with enclosures over it.
struct Piece {
    leg: usize,
    s: [f64; 2],
    bx: Box3,
    /// The section's normal.
    n: IVec,
    /// Each point's velocity `Φ_s`.
    v: Vec<IVec>,
}

fn pt(x: f64) -> Interval {
    Interval::point(x)
}

fn hull(a: IVec, b: IVec) -> IVec {
    [a[0].hull(b[0]), a[1].hull(b[1]), a[2].hull(b[2])]
}

fn dot(a: IVec, b: IVec) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// What the map is certified to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Verdict {
    /// One to one: the faces meet only where they share edges and
    /// vertices.
    Embedded,
    /// It folds: `N(s) · Φ_s` is certified of one sign at a profile vertex
    /// and of the other at another (or at another place along the path),
    /// so the sweep runs back over itself. The leg named is one where it
    /// runs against the profile's motion at the start (`sign`).
    Folds(SegmentKey),
    /// Neither could be certified within the budget.
    Unknown,
}

/// Judges the sweep the legs make of `profile` along an open path:
/// whether it is one to one over a rectangle holding the profile, or
/// folds, or neither could be certified. `sign` is `+1` when the profile
/// starts out moving along its normal, `-1` against it.
pub(super) fn judge(
    profile: &ValidProfile,
    legs: &[Leg],
    o: Point3,
    law: FrameLaw,
    sign: f64,
) -> Verdict {
    let frame = profile.frame();
    let mut lo = Point2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut points = Vec::new();
    let mut lumps = Vec::new();
    for (k, lump) in profile.lumps().iter().enumerate() {
        for s in &lump.outer.segments {
            let b = s.bounds();
            lo = Point2::new(lo.x.min(b.min().x), lo.y.min(b.min().y));
            hi = Point2::new(hi.x.max(b.max().x), hi.y.max(b.max().y));
        }
        points.extend(lump.outer.vertices.iter().copied());
        lumps.resize(points.len(), k);
    }
    if !(lo.x.is_finite() && lo.y.is_finite() && hi.x.is_finite() && hi.y.is_finite()) {
        return Verdict::Unknown;
    }
    // The rectangle's corners, then the profile's vertices.
    let at = |x: f64, y: f64| frame.to_world(x, y, 0.0);
    let mut carried = vec![
        at(lo.x, lo.y),
        at(hi.x, lo.y),
        at(hi.x, hi.y),
        at(lo.x, hi.y),
    ];
    carried.extend(points.iter().map(|p| at(p.x, p.y)));
    let mut maps = Vec::with_capacity(legs.len());
    for leg in legs {
        let Some(map) = leg_map(leg, &carried, frame.z().get(), o, law) else {
            return Verdict::Unknown;
        };
        maps.push(map);
    }
    let mut pieces = Vec::new();
    for (k, m) in maps.iter().enumerate() {
        let [s0, s1] = m.range;
        for i in 0..START_PIECES {
            let a = s0 + (s1 - s0) * i as f64 / START_PIECES as f64;
            let b = if i + 1 == START_PIECES {
                s1
            } else {
                s0 + (s1 - s0) * (i + 1) as f64 / START_PIECES as f64
            };
            let Some(p) = piece(&maps, k, [a, b]) else {
                return Verdict::Unknown;
            };
            pieces.push(p);
        }
    }
    for _ in 0..MAX_ROUNDS {
        if let Some(leg) = folds(&pieces, &lumps, sign) {
            return Verdict::Folds(legs[leg].segment.key);
        }
        let bad = failing(&pieces);
        if bad.is_empty() {
            return Verdict::Embedded;
        }
        if pieces.len() + bad.len() > MAX_PIECES {
            return Verdict::Unknown;
        }
        let mut next = Vec::with_capacity(pieces.len() + bad.len());
        for (i, p) in pieces.into_iter().enumerate() {
            if bad.binary_search(&i).is_err() {
                next.push(p);
                continue;
            }
            let [a, b] = p.s;
            let m = a + (b - a) / 2.0;
            if !(a < m && m < b) {
                return Verdict::Unknown;
            }
            for s in [[a, m], [m, b]] {
                let Some(q) = piece(&maps, p.leg, s) else {
                    return Verdict::Unknown;
                };
                next.push(q);
            }
        }
        pieces = next;
    }
    Verdict::Unknown
}

/// The first leg where `N(s) · Φ_s` is certified against `sign` at a
/// profile vertex, when it is also certified with `sign` at a vertex of
/// the same lump (`lumps` gives each vertex's): the Jacobian of the map
/// changes sign over the lump swept along the path, so it folds.
fn folds(pieces: &[Piece], lumps: &[usize], sign: f64) -> Option<usize> {
    let n = lumps.last().map_or(0, |&k| k + 1);
    (0..n).find_map(|k| {
        let mine: Vec<usize> = (0..lumps.len()).filter(|&i| lumps[i] == k).collect();
        folds_in(pieces, &mine, sign)
    })
}

/// [`folds`] for one lump's vertices, by index.
fn folds_in(pieces: &[Piece], vertices: &[usize], sign: f64) -> Option<usize> {
    let mut with = false;
    let mut against = None;
    for p in pieces {
        for &w in vertices.iter().map(|&i| &p.v[CORNERS + i]) {
            let d = dot(p.n, w);
            let s = if d.lo() > 0.0 {
                1.0
            } else if d.hi() < 0.0 {
                -1.0
            } else {
                continue;
            };
            if s == sign {
                with = true;
            } else if against.is_none() {
                against = Some(p.leg);
            }
        }
    }
    against.filter(|_| with)
}

/// The leg's map, or `None` where its frame can't be certified.
fn leg_map(
    leg: &Leg,
    points: &[Point3],
    normal: Vector3,
    o: Point3,
    law: FrameLaw,
) -> Option<LegMap> {
    let s = &leg.segment;
    let carry = Carry::new(s.curve.clone(), s.range, leg.start.axes[0], law, leg.twist).ok()?;
    let [s0, s1] = s.range;
    let axes = carry.axes_at(s0)?;
    let c0 = s.curve.point(s0);
    let centre = leg.start.centre(o);
    let a = axes.map(|f| f.dot(centre - c0));
    let b = points
        .iter()
        .map(|&p| {
            let q = leg.start.point(o, p);
            axes.map(|f| f.dot(q - centre))
        })
        .collect();
    let nl = leg.start.rigid.apply_vector(normal);
    let n = axes.map(|f| f.dot(nl));
    Some(LegMap {
        carry,
        range: s.range,
        a,
        b,
        n,
        rate: (leg.ratio - 1.0) / (s1 - s0),
    })
}

/// Enclosures over `s` of a leg's section normal and its rate, and of
/// each point's place, velocity and acceleration.
struct Enclosed {
    n: IVec,
    dn: IVec,
    points: Vec<[IVec; 3]>,
}

/// A leg's enclosures over `s`; `None` where the frame can't be
/// enclosed.
fn enclose(m: &LegMap, s: [f64; 2]) -> Option<Enclosed> {
    let (c, f) = m.carry.jet_box(s)?;
    let s0 = m.range[0];
    let sigma = |x: f64| 1.0 + m.rate * (x - s0);
    let (x, y) = (sigma(s[0]), sigma(s[1]));
    let sig = Interval::new(x.min(y), x.max(y))?;
    // The frame and the path are jets in `s`, their second variable.
    let jets = |j: usize| -> [IVec; 3] {
        let g = &f[j];
        [
            [g[0].v, g[1].v, g[2].v],
            [g[0].d[1], g[1].d[1], g[2].d[1]],
            [g[0].h[2], g[1].h[2], g[2].h[2]],
        ]
    };
    let mut n = [pt(0.0); 3];
    let mut dn = [pt(0.0); 3];
    for j in 0..3 {
        let [fj, dj, _] = jets(j);
        for i in 0..3 {
            n[i] = n[i] + pt(m.n[j]) * fj[i];
            dn[i] = dn[i] + pt(m.n[j]) * dj[i];
        }
    }
    let mut points = Vec::with_capacity(m.b.len());
    for b in &m.b {
        let mut p: IVec = [c[0].v, c[1].v, c[2].v];
        let mut d: IVec = [c[0].d[1], c[1].d[1], c[2].d[1]];
        let mut dd: IVec = [c[0].h[2], c[1].h[2], c[2].h[2]];
        for (j, (&aj, &bj)) in m.a.iter().zip(b).enumerate() {
            let coef = pt(aj) + sig * pt(bj);
            let rb = pt(m.rate * bj);
            let [fj, dj, ddj] = jets(j);
            for i in 0..3 {
                p[i] = p[i] + coef * fj[i];
                d[i] = d[i] + coef * dj[i] + rb * fj[i];
                dd[i] = dd[i] + coef * ddj[i] + pt(2.0) * rb * dj[i];
            }
        }
        points.push([p, d, dd]);
    }
    Some(Enclosed { n, dn, points })
}

/// The tighter of an enclosure and the mean value form about the middle:
/// `x(m) + x'(S) (S - m)`.
fn centred(naive: IVec, mid: IVec, rate: IVec, h: Interval) -> IVec {
    core::array::from_fn(|i| {
        let mv = mid[i] + rate[i] * h;
        let lo = naive[i].lo().max(mv.lo());
        let hi = naive[i].hi().min(mv.hi());
        Interval::new(lo, hi).unwrap_or(naive[i])
    })
}

/// Enclosures over `s` of leg `leg`; `None` where the frame can't be
/// enclosed.
fn piece(maps: &[LegMap], leg: usize, s: [f64; 2]) -> Option<Piece> {
    let m = &maps[leg];
    let whole = enclose(m, s)?;
    let mid = s[0] + (s[1] - s[0]) / 2.0;
    let at = enclose(m, [mid, mid])?;
    let h = Interval::new(s[0] - mid, s[1] - mid)?;
    let n = centred(whole.n, at.n, whole.dn, h);
    let mut bx: Option<Box3> = None;
    let mut v = Vec::with_capacity(m.b.len());
    for (k, (w, a)) in whole.points.iter().zip(&at.points).enumerate() {
        // For each `s` the section is the image of the rectangle, held
        // in the box of its corners.
        if k < CORNERS {
            let p = centred(w[0], a[0], w[1], h);
            let pb = Box3::new(
                Point3::new(p[0].lo(), p[1].lo(), p[2].lo()),
                Point3::new(p[0].hi(), p[1].hi(), p[2].hi()),
            );
            bx = Some(bx.map_or(pb, |x| x.union(&pb)));
        }
        v.push(centred(w[1], a[1], w[2], h));
    }
    Some(Piece {
        leg,
        s,
        bx: bx?,
        n,
        v,
    })
}

/// Whether `N(r) · Φ_s(x, s)` keeps one sign over a run, given the hulls
/// over it of the normal and of the corners' velocities.
fn one_sign(n: IVec, v: &[IVec]) -> bool {
    let signs: Vec<Interval> = v.iter().map(|&w| dot(n, w)).collect();
    signs.iter().all(|x| x.lo() > 0.0) || signs.iter().all(|x| x.hi() < 0.0)
}

/// The pieces (sorted) in a pair that can't be told apart: neither are
/// their boxes apart nor is the run between them certified one to one.
fn failing(pieces: &[Piece]) -> Vec<usize> {
    let n = pieces.len();
    // The last piece of the run from each piece that is one to one.
    let mut reach = vec![None; n];
    for i in 0..n {
        let mut h = pieces[i].n;
        let mut w = pieces[i].v[..CORNERS].to_vec();
        if !one_sign(h, &w) {
            continue;
        }
        let mut j = i;
        while j + 1 < n {
            let q = &pieces[j + 1];
            let h2 = hull(h, q.n);
            let w2: Vec<IVec> = w.iter().zip(&q.v).map(|(&a, &b)| hull(a, b)).collect();
            if !one_sign(h2, &w2) {
                break;
            }
            (h, w) = (h2, w2);
            j += 1;
        }
        reach[i] = Some(j);
    }
    let mut bad = Vec::new();
    for i in 0..n {
        let Some(r) = reach[i] else {
            bad.push(i);
            continue;
        };
        for j in r + 1..n {
            if pieces[i].bx.intersects(&pieces[j].bx) {
                bad.push(i);
                bad.push(j);
            }
        }
    }
    bad.sort_unstable();
    bad.dedup();
    bad
}

#[cfg(test)]
mod tests {
    use super::super::{AlongOptions, legs};
    use super::*;
    use crate::geom::{Circle, Curve, Frame, Line};
    use crate::sweep::Path;
    use crate::sweep::path::{PathSegment, ValidPath};
    use crate::sweep::profile::tests::{profile, square};
    use core::f64::consts::FRAC_PI_2;

    /// Up `z` by 1, then a quarter turn of radius 1 towards `x`.
    fn bend() -> ValidPath {
        let up = Vector3::new(0.0, 0.0, 1.0);
        let turn = Frame::new(
            Point3::new(1.0, 0.0, 1.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(-1.0, 0.0, 0.0),
        )
        .unwrap();
        ValidPath::new(&Path {
            segments: vec![
                PathSegment {
                    key: SegmentKey(1),
                    curve: Curve::Line(Line::new(Point3::ORIGIN, up).unwrap()),
                    range: [0.0, 1.0],
                },
                PathSegment {
                    key: SegmentKey(2),
                    curve: Curve::Circle(Circle::new(turn, 1.0).unwrap()),
                    range: [0.0, FRAC_PI_2],
                },
            ],
        })
        .unwrap()
    }

    fn verdict(x: f64) -> Verdict {
        let p = ValidProfile::new(profile(vec![square(1, 10, (x, -0.1), 0.2)])).unwrap();
        let opts = AlongOptions::default();
        let legs = legs(&p, &bend(), &opts, Point3::ORIGIN).unwrap();
        judge(&p, &legs, Point3::ORIGIN, opts.law, 1.0)
    }

    /// A square round the path's start is carried one to one; one wholly
    /// beyond the bend's centre goes up the first leg and back down
    /// round the bend, which is named.
    #[test]
    fn inside_the_bend_is_embedded_beyond_it_folds() {
        assert_eq!(verdict(-0.1), Verdict::Embedded);
        assert_eq!(verdict(1.4), Verdict::Folds(SegmentKey(2)));
    }
}
