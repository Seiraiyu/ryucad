//! Curve–surface intersection.
//!
//! The curve over its interval becomes exact rational Bézier pieces
//! ([`Chain3`]); on each, the surface's implicit form gives a polynomial
//! whose roots are the hits, isolated by [`Bernstein::roots`]. A simple
//! root is a crossing. A stretch rounding can't sign is taken to its
//! certified distance from the surface: within the caller's tolerance it
//! is a contact (a point, or a stretch lying on the surface), and beyond
//! it the answer is [`Failure::Uncertain`]. Which side the curve comes
//! from and leaves to is read from certified signs of the implicit form
//! on either side, so a touch and a crossing are told apart by rule.

use super::Failure;
use super::implicit::Implicit;
use crate::geom::bernstein::{Bernstein, Root};
use crate::geom::chain3::{Chain3, Piece3};
use crate::geom::{Curve, Point3, Surface};
use crate::interval::Interval;
use crate::predicates::Sign;
use crate::tol::LINEAR_RESOLUTION;
use crate::work::{Meter, Scope};

/// Which side of a surface a curve is on just before or after a hit:
/// the side its natural normal points to (`Positive`) or the other, or
/// past the end of the curve's interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Negative,
    Positive,
    Beyond,
}

/// Where a curve meets a surface.
#[derive(Clone, Debug, PartialEq)]
pub enum CurveHit {
    /// The curve meets the surface at `t`, within the tolerance, coming
    /// from `before` and leaving to `after`: a crossing when they are
    /// opposite, a touch when they are the same. A hit within tolerance of
    /// an end of the interval is at that end exactly, with `Beyond` there.
    Point {
        t: f64,
        point: Point3,
        before: Side,
        after: Side,
    },
    /// The curve lies within the tolerance of the surface from `t[0]` to
    /// `t[1]`.
    On { t: [f64; 2] },
}

/// Every place where `curve` over `[t0, t1]` (`t0 < t1`) meets `surface`,
/// in increasing parameter. `tolerance` (at least
/// [`LINEAR_RESOLUTION`]) is the explicit rule for contact: a stretch
/// certified within it of the surface touches it, and hits closer than it
/// to each other are one.
///
/// A pipe is met by [`super::curve_pipe`], and any other surface with no
/// implicit form (a NURBS, an extrusion, a revolution) by certified
/// subdivision of the curve's and the surface's parameters, by the same
/// rules.
///
/// Fails with `Uncertain` where rounding leaves a stretch that is neither
/// certified apart from the surface nor within the tolerance of it.
pub fn curve_surface(
    curve: &Curve,
    range: [f64; 2],
    surface: &Surface,
    tolerance: f64,
) -> Result<Vec<CurveHit>, Failure> {
    // unmetered: the public function takes no work
    curve_surface_in(curve, range, surface, tolerance, &Meter::free())
}

/// [`curve_surface`] charging `m`, under the Search cap: one unit for the
/// call (a closed form costs a unit too), then each part searched.
pub(crate) fn curve_surface_in(
    curve: &Curve,
    [t0, t1]: [f64; 2],
    surface: &Surface,
    tolerance: f64,
    m: &Meter,
) -> Result<Vec<CurveHit>, Failure> {
    if let Surface::Pipe(_) = surface {
        return super::pipe_cs::curve_pipe_in(curve, [t0, t1], surface, tolerance, m);
    }
    m.charge(1)?;
    m.capped(Scope::Search, || {
        search(curve, [t0, t1], surface, tolerance, m)
    })
}

/// [`curve_surface_in`]'s search of any surface but a pipe.
fn search(
    curve: &Curve,
    [t0, t1]: [f64; 2],
    surface: &Surface,
    tolerance: f64,
    m: &Meter,
) -> Result<Vec<CurveHit>, Failure> {
    let Some(imp) = Implicit::of(surface) else {
        if let Some(hits) = on_affine_twin(curve, [t0, t1], surface, tolerance, m)? {
            return Ok(hits);
        }
        return super::sheet_cs::curve_sheet(curve, [t0, t1], surface, tolerance, m).or_else(|e| {
            // A curve along a bounded surface's domain edge lies on the
            // side of every cell beside it: on the surface carried past
            // its edges it lies inside. What is found there must lie on
            // the surface itself.
            let Failure::Uncertain = e else {
                return Err(e);
            };
            let Some(ext) = super::ss::extended(surface, 16.0) else {
                return Err(e);
            };
            let hits = super::sheet_cs::curve_sheet(curve, [t0, t1], &ext, tolerance, m)?;
            on_surface(curve, surface, tolerance.max(LINEAR_RESOLUTION), hits)
        });
    };
    let tolerance = tolerance.max(LINEAR_RESOLUTION);
    let chain = Chain3::new(curve, t0, t1);
    let ctx = Context {
        curve,
        range: [t0, t1],
        surface,
        imp: &imp,
        tolerance,
        meter: m,
    };
    let mut found = Vec::new();
    for (i, piece) in chain.pieces.iter().enumerate() {
        ctx.candidates(&chain, i, piece, &mut found)?;
    }
    // An end within tolerance of the surface touches it, even where the
    // curve stops just short of a root.
    for t in [t0, t1] {
        let q = curve.point(t);
        let f = imp.value_at(q).mag();
        if imp.distance_bound(f, imp.local(q.to_vector())) <= tolerance {
            found.push(Candidate {
                t: [t, t],
                simple: None,
                tangent: None,
            });
        }
    }
    let merged = ctx.merge(&chain, found);
    let mut out = Vec::with_capacity(merged.len());
    for (k, c) in merged.iter().enumerate() {
        let lo = if k == 0 { t0 } else { merged[k - 1].t[1] };
        let hi = merged.get(k + 1).map_or(t1, |n| n.t[0]);
        out.push(ctx.classify(c, [lo, hi])?);
    }
    Ok(out)
}

/// The hits found on a spline carried on past its domain that lie on the
/// spline itself, within `tolerance` of it; one farther off by less than
/// twice that can't be decided (`Uncertain`). A stretch on it must hold
/// at both ends and its middle.
fn on_surface(
    curve: &Curve,
    surface: &Surface,
    tolerance: f64,
    hits: Vec<CurveHit>,
) -> Result<Vec<CurveHit>, Failure> {
    let on = |p: Point3| -> Result<bool, Failure> {
        let d = surface.distance(p);
        if d <= tolerance {
            Ok(true)
        } else if d > 2.0 * tolerance {
            Ok(false)
        } else {
            Err(Failure::Uncertain)
        }
    };
    let mut out = Vec::with_capacity(hits.len());
    for h in hits {
        let keep = match h {
            CurveHit::Point { point, .. } => on(point)?,
            CurveHit::On { t: [a, b] } => {
                let ends = [a, (a + b) / 2.0, b].map(|t| on(curve.point(t)));
                match ends {
                    [Ok(true), Ok(true), Ok(true)] => true,
                    [Ok(false), Ok(false), Ok(false)] => false,
                    _ => return Err(Failure::Uncertain),
                }
            }
        };
        if keep {
            out.push(h);
        }
    }
    Ok(out)
}

/// The parameters inside `[t0, t1]` of a NURBS curve's joints: knots of
/// multiplicity at least its degree (a periodic curve's first knot moved
/// by whole periods into the range), where the implicit search, which
/// judges touches at a curve's ends, can miss one (a vee dipping half a
/// tolerance above a flat spline, which the sheet search finds).
fn joints(curve: &Curve, [t0, t1]: [f64; 2]) -> Vec<f64> {
    let Curve::Nurbs(n) = curve else {
        return Vec::new();
    };
    let k = n.knots();
    let p = k.degree();
    let mut out = Vec::new();
    for (&x, &m) in k.knots().iter().zip(k.mults()) {
        if m < p {
            continue;
        }
        match k.period() {
            Some(q) if q > 0.0 => {
                let mut y = x + ((t0 - x) / q).ceil() * q;
                while y < t1 {
                    if y > t0 {
                        out.push(y);
                    }
                    y += q;
                }
            }
            _ => {
                if x > t0 && x < t1 {
                    out.push(x);
                }
            }
        }
    }
    out
}

/// A curve met with a spline whose net is an affine map of the plane
/// ([`crate::pcurve::affine_net`], within [`crate::pcurve::TWIN_DEVIATION`]:
/// a lofted flat face) as with that plane, by its implicit form, keeping
/// the hits in the spline's domain. The sheet search spent most of a
/// flush pin's cut following a curve lying in such a face (#254).
///
/// The spline lies within its deviation of the plane, so a contact within
/// `tolerance - deviation` of the plane is within `tolerance` of the
/// spline, and the plane's natural normal is the spline's (`a × b`), so
/// sides agree. A hit is in the domain when its parameters (a point's, or
/// every corner of an `On` stretch's box: an affine function is extreme
/// over a box at its corners) lie at least twice the tolerance inside it,
/// out when beyond it by that; anything between, and any spline that
/// isn't such a net, is `None`: search the spline.
fn on_affine_twin(
    curve: &Curve,
    [t0, t1]: [f64; 2],
    surface: &Surface,
    tolerance: f64,
    m: &Meter,
) -> Result<Option<Vec<CurveHit>>, Failure> {
    let Some(a) = crate::pcurve::affine_net(surface) else {
        return Ok(None);
    };
    if a.deviation > crate::pcurve::TWIN_DEVIATION {
        return Ok(None);
    }
    let Some(plane) = a.plane() else {
        return Ok(None);
    };
    let tolerance = tolerance.max(LINEAR_RESOLUTION);
    let hits = search(curve, [t0, t1], &plane, tolerance - a.deviation, m)?;
    // A joint within the tolerance of the plane that no hit covers may be
    // a touch the implicit search can't see: search the spline.
    let frame = plane.frame().expect("a plane has a frame");
    for x in joints(curve, [t0, t1]) {
        let q = curve.point(x);
        if frame.to_local(q).z.abs() > tolerance {
            continue;
        }
        let covered = hits.iter().any(|h| match *h {
            CurveHit::On { t: [y0, y1] } => y0 <= x && x <= y1,
            CurveHit::Point { point, .. } => {
                point.distance(q) <= crate::tol::touch_reach(tolerance)
            }
        });
        if !covered {
            return Ok(None);
        }
    }
    let [[u0, u1], [v0, v1]] = surface.domain();
    let (mu, mv) = (
        2.0 * tolerance / a.a.length(),
        2.0 * tolerance / a.b.length(),
    );
    // `Some(true)` well inside, `Some(false)` beyond, `None` near the edge.
    let place = |p: Point3| -> Option<bool> {
        let q = a.local(p);
        if q.x >= u0 + mu && q.x <= u1 - mu && q.y >= v0 + mv && q.y <= v1 - mv {
            Some(true)
        } else if q.x < u0 - mu || q.x > u1 + mu || q.y < v0 - mv || q.y > v1 + mv {
            Some(false)
        } else {
            None
        }
    };
    let mut out = Vec::with_capacity(hits.len());
    for h in hits {
        let keep = match h {
            CurveHit::Point { point, .. } => place(point),
            CurveHit::On { t: [x, y] } => {
                let b = curve.bounds([x, y]);
                let (lo, hi) = (b.min(), b.max());
                let corners: Vec<Option<bool>> = (0..8)
                    .map(|k| {
                        place(Point3::new(
                            if k & 1 == 0 { lo.x } else { hi.x },
                            if k & 2 == 0 { lo.y } else { hi.y },
                            if k & 4 == 0 { lo.z } else { hi.z },
                        ))
                    })
                    .collect();
                if corners.iter().all(|c| *c == Some(true)) {
                    Some(true)
                } else {
                    None
                }
            }
        };
        match keep {
            Some(true) => out.push(h),
            Some(false) => {}
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// How many parts a stretch may be halved into to certify that it lies
/// within the tolerance of the surface.
const MAX_PARTS: u32 = 4096;

/// A stretch of parameter holding a hit: a simple root, or a stretch
/// within tolerance of the surface.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    t: [f64; 2],
    /// `Some(rising)` for one simple root, `None` otherwise.
    simple: Option<bool>,
    /// For a stretch around a double root, where the curve is tangent to
    /// the surface: the parameter of the tangency, the one simple root of
    /// the derivative there.
    tangent: Option<f64>,
}

/// Touching stretches made one.
fn merge_runs(parts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(parts.len());
    for p in parts {
        match out.last_mut() {
            Some(last) if p[0] <= last[1] => last[1] = last[1].max(p[1]),
            _ => out.push(p),
        }
    }
    out
}

/// The local width at which a piece's roots are bracketed: about a
/// thousandth of resolution along the piece.
fn min_width(piece: &Piece3) -> f64 {
    let (lo, hi) = piece.hull();
    let size = (hi - lo).length().max(LINEAR_RESOLUTION);
    (1e-3 * LINEAR_RESOLUTION / size).clamp(1e-15, 1e-6)
}

/// The piece over local `[lo, hi]`.
fn part(piece: &Piece3, lo: f64, hi: f64) -> Piece3 {
    let mut p = piece.clone();
    if hi < 1.0 {
        p = p.split(hi)[0].clone();
    }
    if lo > 0.0 {
        p = p.split(lo / hi)[1].clone();
    }
    p
}

/// The local coordinates of a piece's control hull, as intervals.
fn hull_local(imp: &Implicit, piece: &Piece3) -> [Interval; 3] {
    let mut out = imp.local(piece.pts[0].p);
    for h in &piece.pts[1..] {
        let l = imp.local(h.p);
        for k in 0..3 {
            out[k] = out[k].hull(l[k]);
        }
    }
    out
}

struct Context<'a> {
    curve: &'a Curve,
    range: [f64; 2],
    surface: &'a Surface,
    imp: &'a Implicit,
    tolerance: f64,
    meter: &'a Meter<'a>,
}

impl Context<'_> {
    /// The candidates of piece `i` of the chain.
    fn candidates(
        &self,
        chain: &Chain3,
        i: usize,
        piece: &Piece3,
        out: &mut Vec<Candidate>,
    ) -> Result<(), Failure> {
        let imp = self.imp;
        let (poly, w) = imp.on_piece(piece);
        let wd = (0..imp.degree()).fold(1.0, |a, _| a * w);
        let width = min_width(piece);
        // A piece certified within the tolerance of the surface all along
        // touches it all along, whatever signs rounding gives `f`: on a
        // surface whose implicit form magnifies distance (a cone nearly
        // flat), `f` can be signed on a curve lying in it.
        let mut budget = MAX_PARTS;
        if self.within(&poly, piece, wd, &mut budget) {
            out.push(Candidate {
                t: piece.range,
                simple: None,
                tangent: None,
            });
            return Ok(());
        }
        // Near misses: where the curve comes within the tolerance of the
        // surface without reaching it, at a turning point of the
        // polynomial, it touches by rule.
        for r in poly.deriv().roots(width) {
            let Root::Simple { lo, hi, .. } = r else {
                continue;
            };
            let t = chain.param(i, (lo + hi) / 2.0);
            let q = self.curve.point(t);
            let f = imp.value_at(q);
            if f.sign().is_some()
                && !imp.beyond_apex(q)
                && imp.distance_bound(f.mag(), imp.local(q.to_vector())) <= self.tolerance
            {
                out.push(Candidate {
                    t: [t, t],
                    simple: None,
                    tangent: Some(t),
                });
            }
        }
        for root in poly.roots(width) {
            match root {
                Root::Simple { lo, hi, rising } => {
                    let t = self.polish([chain.param(i, lo), chain.param(i, hi)], rising);
                    if imp.beyond_apex(self.curve.point(t)) {
                        continue;
                    }
                    out.push(Candidate {
                        t: [t, t],
                        simple: Some(rising),
                        tangent: None,
                    });
                }
                Root::Unresolved { lo: l0, hi: h0 } => {
                    // The stretch within the tolerance of the surface, less
                    // what lies on a cone's other nappe (where `f`
                    // vanishes too); anything else is uncertain.
                    let mut budget = MAX_PARTS;
                    let near = self
                        .near_parts(
                            &poly.part(l0, h0),
                            &part(piece, l0, h0),
                            [l0, h0],
                            wd,
                            &mut budget,
                        )
                        .ok_or(Failure::Uncertain)?;
                    for [lo, hi] in merge_runs(near) {
                        // A double root leaves a stretch rounding can't
                        // sign, but the derivative's root inside it is
                        // simple.
                        let pad = (hi - lo).max(width);
                        let (a, b) = ((lo - pad).max(0.0), (hi + pad).min(1.0));
                        let slope = poly.part(a, b).deriv().roots(width / (b - a));
                        let tangent = match slope[..] {
                            [Root::Simple { lo: l, hi: h, .. }] => {
                                let s = a + (b - a) * (l + h) / 2.0;
                                (lo <= s && s <= hi).then(|| chain.param(i, s))
                            }
                            _ => None,
                        };
                        out.push(Candidate {
                            t: [chain.param(i, lo), chain.param(i, hi)],
                            simple: None,
                            tangent,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Candidates in parameter order, with overlapping ones, and ones the
    /// curve between stays within the tolerance of the surface, made one:
    /// two crossings a hair apart, or a crossing beside a near miss, are a
    /// single contact by rule.
    fn merge(&self, chain: &Chain3, mut found: Vec<Candidate>) -> Vec<Candidate> {
        found.sort_by(|a, b| a.t[0].total_cmp(&b.t[0]));
        let mut out: Vec<Candidate> = Vec::with_capacity(found.len());
        for c in found {
            if let Some(last) = out.last_mut()
                && (c.t[0] <= last.t[1] || self.gap_within(chain, [last.t[1], c.t[0]]))
            {
                last.t[1] = last.t[1].max(c.t[1]);
                last.simple = None;
                last.tangent = last.tangent.or(c.tangent);
                continue;
            }
            out.push(c);
        }
        out
    }

    /// True when the curve over `[a, b]` is certified within the
    /// tolerance of the surface.
    fn gap_within(&self, chain: &Chain3, [a, b]: [f64; 2]) -> bool {
        let mut budget = MAX_PARTS;
        chain.pieces.iter().enumerate().all(|(i, piece)| {
            let [p0, p1] = piece.range;
            if p1 <= a || p0 >= b {
                return true;
            }
            let (s0, s1) = (chain.local(i, a.max(p0)), chain.local(i, b.min(p1)));
            if s1 <= s0 {
                return true;
            }
            let (poly, w) = self.imp.on_piece(piece);
            let wd = (0..self.imp.degree()).fold(1.0, |x, _| x * w);
            self.within(&poly.part(s0, s1), &part(piece, s0, s1), wd, &mut budget)
        })
    }

    /// True when the piece, whose polynomial is `poly` and least weight
    /// to the degree `wd`, is certified within the tolerance of the
    /// surface: as a whole, or halved until every part is (the bound is
    /// loose over a long stretch, and near a cone's apex), using up at
    /// most `budget` parts.
    fn within(&self, poly: &Bernstein, piece: &Piece3, wd: f64, budget: &mut u32) -> bool {
        if self.meter.charge(1).is_err() {
            return false;
        }
        let local = hull_local(self.imp, piece);
        let f_max = (poly.range().mag() / wd).next_up();
        if self.imp.distance_bound(f_max, local) <= self.tolerance {
            return true;
        }
        // A middle plainly beyond the tolerance can't be brought in by
        // halving: give up at once.
        let mid = Point3::ORIGIN + piece.split(0.5)[1].pts[0].p;
        if *budget < 2 || self.surface.distance(mid) > 2.0 * self.tolerance {
            return false;
        }
        *budget -= 2;
        let (p0, p1) = poly.split(0.5);
        let [c0, c1] = piece.split(0.5);
        self.within(&p0, &c0, wd, budget) && self.within(&p1, &c1, wd, budget)
    }

    /// The parts of the stretch `[lo, hi]` of a piece (whose polynomial
    /// there is `poly`, and sub-piece `piece`) certified within the
    /// tolerance of the surface, halving where needed; parts certified
    /// past a cone's apex are left out. `None` when a part is neither, or
    /// the budget runs out.
    fn near_parts(
        &self,
        poly: &Bernstein,
        piece: &Piece3,
        [lo, hi]: [f64; 2],
        wd: f64,
        budget: &mut u32,
    ) -> Option<Vec<[f64; 2]>> {
        self.meter.charge(1).ok()?;
        let local = hull_local(self.imp, piece);
        if self.imp.box_beyond_apex(local) {
            return Some(Vec::new());
        }
        let f_max = (poly.range().mag() / wd).next_up();
        if self.imp.distance_bound(f_max, local) <= self.tolerance {
            return Some(vec![[lo, hi]]);
        }
        *budget = budget.checked_sub(2)?;
        let mid = (lo + hi) / 2.0;
        let (p0, p1) = poly.split(0.5);
        let [c0, c1] = piece.split(0.5);
        let mut out = self.near_parts(&p0, &c0, [lo, mid], wd, budget)?;
        out.extend(self.near_parts(&p1, &c1, [mid, hi], wd, budget)?);
        Some(out)
    }

    /// A simple root's bracket `[a, b]` narrowed by bisection on the
    /// certified sign of the implicit form at the curve's points, which
    /// rounds less than the polynomial; the middle of what is left.
    fn polish(&self, [mut a, mut b]: [f64; 2], rising: bool) -> f64 {
        let below = if rising {
            Sign::Negative
        } else {
            Sign::Positive
        };
        for _ in 0..64 {
            let m = (a + b) / 2.0;
            if m <= a || m >= b {
                break;
            }
            match self.imp.value_at(self.curve.point(m)).sign() {
                Some(s) if s == below => a = m,
                Some(Sign::Zero) | None => break,
                Some(_) => b = m,
            }
        }
        (a + b) / 2.0
    }

    /// A candidate as a hit. `room` is the parameter range up to the
    /// neighbouring candidates (or the interval's ends), where the curve
    /// is certified off the surface.
    fn classify(&self, c: &Candidate, room: [f64; 2]) -> Result<CurveHit, Failure> {
        self.meter.charge(1)?;
        let [t0, t1] = self.range;
        let near =
            |a: f64, b: f64| self.curve.point(a).distance(self.curve.point(b)) <= self.tolerance;
        let [a, b] = c.t;
        let mid = c.tangent.unwrap_or((a + b) / 2.0);
        // By rule, a contact shorter than the touch rule's reach
        // ([`crate::tol::touch_reach`], 100 tolerances) is a touch:
        // rounding leaves a zone about that long around a tangency, which
        // a curve truly lying on the surface outruns.
        let short = |x: f64, y: f64| {
            self.curve.point(x).distance(self.curve.point(y))
                <= crate::tol::touch_reach(self.tolerance)
        };
        let point_like = c.simple.is_some()
            || c.tangent.is_some()
            || (0..=8).all(|k| short(mid, a + (b - a) * f64::from(k) / 8.0));
        if !point_like {
            let lo = if near(a, t0) { t0 } else { a };
            let hi = if near(b, t1) { t1 } else { b };
            return Ok(CurveHit::On { t: [lo, hi] });
        }
        // A contact whose zone reaches an end of the interval is at that
        // end: a tangency is placed only to rounding's reach, which can
        // be wider than the tolerance.
        let (at_start, at_end) = (near(mid, t0) || a <= t0, near(mid, t1) || b >= t1);
        let t = if at_start {
            t0
        } else if at_end {
            t1
        } else {
            mid
        };
        let (before, after) = match c.simple {
            Some(rising) if !at_start && !at_end => {
                let (neg, pos) = (Side::Negative, Side::Positive);
                if rising { (neg, pos) } else { (pos, neg) }
            }
            _ => (
                if at_start {
                    Side::Beyond
                } else {
                    self.side(a, room[0])?
                },
                if at_end {
                    Side::Beyond
                } else {
                    self.side(b, room[1])?
                },
            ),
        };
        Ok(CurveHit::Point {
            t,
            point: self.curve.point(t),
            before,
            after,
        })
    }

    /// The side of the surface the curve is on just past `from`, towards
    /// `limit`: the first certified sign of the implicit form, stepping
    /// out by doubling steps, which stay short of `limit` (where the next
    /// candidate starts).
    fn side(&self, from: f64, limit: f64) -> Result<Side, Failure> {
        self.meter.charge(1)?;
        let span = limit - from;
        let mut step = span / 1024.0;
        for _ in 0..10 {
            match self.imp.side_at(self.curve.point(from + step)) {
                Some(Sign::Positive) => return Ok(Side::Positive),
                Some(_) => return Ok(Side::Negative),
                None => step *= 2.0,
            }
        }
        // The middle of the room, last.
        match self.imp.side_at(self.curve.point(from + span / 2.0)) {
            Some(Sign::Positive) => Ok(Side::Positive),
            Some(_) => Ok(Side::Negative),
            None => Err(Failure::Uncertain),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{
        Circle, Cone, Cylinder, Ellipse, Frame, Line, Plane, Sphere, Torus, Vector3,
    };
    use core::f64::consts::{FRAC_PI_4, PI, TAU};
    use std::sync::Arc;

    const TOL: f64 = LINEAR_RESOLUTION;

    /// A spline whose net is affine (the race block's flat bottom) is met
    /// as its plane: a curve lying in it is one `On` stretch over its
    /// range, a line through it one crossing where the spline search puts
    /// it, a line through the plane past the domain nothing, as the spline
    /// search finds (#254).
    #[test]
    fn an_affine_spline_is_met_as_its_plane() {
        use crate::geom::{KnotVector, Line, NurbsCurve, NurbsSurface, Point3, Vector3};
        use std::sync::Arc;
        let ku = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let kv = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
        let gv = [0.0, 1.0 / 3.0, 1.0, 5.0 / 3.0, 2.0];
        let mut pts = Vec::new();
        for u in [0.0, 1.0] {
            for &v in &gv {
                pts.push(Point3::new(40.0 * u, 12.5 * v, 0.0));
            }
        }
        let s = Surface::Nurbs(Arc::new(NurbsSurface::new([ku, kv], pts, None).unwrap()));
        let n = 12;
        let ring: Vec<Point3> = (0..n)
            .map(|i| {
                let t = core::f64::consts::TAU * f64::from(i) / f64::from(n);
                let (si, co) = crate::math::sin_cos(t);
                Point3::new(10.5 + 0.3 * co, 7.5 + 0.3 * si, 0.0)
            })
            .collect();
        let knots: Vec<f64> = (0..=n).map(f64::from).collect();
        let k = KnotVector::periodic(3, &knots, &vec![1; knots.len()]).unwrap();
        let ring = Curve::Nurbs(Arc::new(NurbsCurve::new(k, ring, None).unwrap()));
        let m = Meter::free(); // unmetered: a test
        let twin = on_affine_twin(&ring, [0.0, 12.0], &s, 1e-8, &m)
            .unwrap()
            .expect("met as its plane");
        assert!(matches!(twin.as_slice(), [CurveHit::On { t: [a, b] }] if *a == 0.0 && *b == 12.0));
        let line = Curve::Line(
            Line::new(Point3::new(3.3, 4.1, -1.0), Vector3::new(0.1, 0.1, 1.0)).unwrap(),
        );
        let twin = on_affine_twin(&line, [0.0, 3.0], &s, 1e-8, &m)
            .unwrap()
            .unwrap();
        let spline = super::super::sheet_cs::curve_sheet(&line, [0.0, 3.0], &s, 1e-8, &m).unwrap();
        assert_eq!(twin.len(), 1);
        assert_eq!(spline.len(), 1);
        let (CurveHit::Point { point: p, .. }, CurveHit::Point { point: q, .. }) =
            (&twin[0], &spline[0])
        else {
            panic!("crossings");
        };
        assert!(p.distance(*q) <= 1e-9, "{p:?} {q:?}");
        let outside = Curve::Line(
            Line::new(Point3::new(50.0, 4.0, -1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        );
        assert!(
            on_affine_twin(&outside, [0.0, 3.0], &s, 1e-8, &m)
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    fn line(o: (f64, f64, f64), d: (f64, f64, f64)) -> Curve {
        Curve::Line(Line::new(Point3::new(o.0, o.1, o.2), Vector3::new(d.0, d.1, d.2)).unwrap())
    }

    fn points(hits: &[CurveHit]) -> Vec<(f64, Side, Side)> {
        hits.iter()
            .map(|h| match *h {
                CurveHit::Point {
                    t, before, after, ..
                } => (t, before, after),
                CurveHit::On { .. } => panic!("a stretch: {hits:?}"),
            })
            .collect()
    }

    use Side::{Beyond, Negative, Positive};

    #[test]
    fn a_line_through_a_sphere() {
        let s = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
        let l = line((-5.0, 0.0, 0.0), (1.0, 0.0, 0.0));
        let hits = curve_surface(&l, [0.0, 10.0], &s, TOL).unwrap();
        let p = points(&hits);
        assert_eq!(p.len(), 2);
        assert!((p[0].0 - 3.0).abs() < 1e-12 && (p[1].0 - 7.0).abs() < 1e-12);
        assert_eq!((p[0].1, p[0].2), (Positive, Negative));
        assert_eq!((p[1].1, p[1].2), (Negative, Positive));
        // Missing it.
        let far = line((-5.0, 2.5, 0.0), (1.0, 0.0, 0.0));
        assert!(
            curve_surface(&far, [0.0, 10.0], &s, TOL)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_line_tangent_to_a_cylinder_touches_it() {
        let c = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
        let l = line((-3.0, 1.0, 0.5), (1.0, 0.0, 0.0));
        let p = points(&curve_surface(&l, [0.0, 6.0], &c, TOL).unwrap());
        assert_eq!(p.len(), 1);
        assert!((p[0].0 - 3.0).abs() < 1e-4);
        assert_eq!((p[0].1, p[0].2), (Positive, Positive));
        // Passing a hair outside, within tolerance: a touch by rule.
        let l = line((-3.0, 1.0 + 0.5e-8, 0.5), (1.0, 0.0, 0.0));
        let p = points(&curve_surface(&l, [0.0, 6.0], &c, TOL).unwrap());
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].1, p[0].2), (Positive, Positive));
        let l = line((-3.0, 1.0 + 2e-8, 0.5), (1.0, 0.0, 0.0));
        assert!(curve_surface(&l, [0.0, 6.0], &c, TOL).unwrap().is_empty());
        // Passing a hair inside, within tolerance: a touch too, though the
        // line crosses twice.
        let l = line((-3.0, 1.0 - 0.5e-8, 0.5), (1.0, 0.0, 0.0));
        let p = points(&curve_surface(&l, [0.0, 6.0], &c, TOL).unwrap());
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].1, p[0].2), (Positive, Positive));
        // From inside, a hair short of tangent: two crossings.
        let l = line((-3.0, 1.0 - 1e-6, 0.5), (1.0, 0.0, 0.0));
        let p = points(&curve_surface(&l, [0.0, 6.0], &c, TOL).unwrap());
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn a_touch_reaching_an_end_is_at_the_end() {
        // A line ending where it is tangent to a cylinder: the touch is
        // placed only to rounding's reach, but it is the end exactly.
        let c = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
        let l = line((-3.0, 1.0, 0.5), (1.0, 0.0, 0.0));
        let p = points(&curve_surface(&l, [0.0, 3.0], &c, TOL).unwrap());
        assert_eq!(p, [(3.0, Positive, Beyond)]);
    }

    #[test]
    fn curves_lying_on_surfaces() {
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let l = line((0.0, 0.0, 0.0), (1.0, 2.0, 0.0));
        assert_eq!(
            curve_surface(&l, [-1.0, 2.0], &plane, TOL).unwrap(),
            [CurveHit::On { t: [-1.0, 2.0] }]
        );
        let f = Frame::new(
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let r = 3f64.sqrt();
        let latitude = Curve::Circle(Circle::new(f, r).unwrap());
        let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
        assert_eq!(
            curve_surface(&latitude, [0.0, TAU], &sphere, TOL).unwrap(),
            [CurveHit::On { t: [0.0, TAU] }]
        );
        // A ruling of a cone, down to its apex.
        let cone = Surface::Cone(Cone::new(Frame::WORLD, 1.0, FRAC_PI_4).unwrap());
        let apex = Point3::new(0.0, 0.0, -1.0);
        let ruling = Curve::Line(Line::through(apex, Point3::new(2.0, 0.0, 1.0)).unwrap());
        let len = 8f64.sqrt();
        assert_eq!(
            curve_surface(&ruling, [0.0, len], &cone, TOL).unwrap(),
            [CurveHit::On { t: [0.0, len] }]
        );
    }

    #[test]
    fn hits_at_the_ends_of_the_interval() {
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let l = line((0.0, 0.0, -1.0), (0.0, 0.0, 1.0));
        let p = points(&curve_surface(&l, [0.0, 1.0], &plane, TOL).unwrap());
        assert_eq!(p, [(1.0, Negative, Beyond)]);
        let p = points(&curve_surface(&l, [1.0, 3.0], &plane, TOL).unwrap());
        assert_eq!(p, [(1.0, Beyond, Positive)]);
        // Within tolerance of the end, but not at it.
        let p = points(&curve_surface(&l, [0.0, 1.0 - 0.5e-8], &plane, TOL).unwrap());
        assert_eq!(p, [(1.0 - 0.5e-8, Negative, Beyond)]);
        assert!(
            curve_surface(&l, [0.0, 1.0 - 2e-8], &plane, TOL)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn conics_and_quartics() {
        // A tilted ellipse round a torus's axis crosses its tube four times
        // where sampling sees the implicit form change sign.
        let f = Frame::new(
            Point3::new(0.0, 0.0, 0.2),
            Vector3::new(0.1, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let e = Curve::Ellipse(Ellipse::new(f, 3.6, 2.5).unwrap());
        let torus = Surface::Torus(Torus::new(Frame::WORLD, 3.0, 1.0).unwrap());
        let p = points(&curve_surface(&e, [0.0, TAU], &torus, TOL).unwrap());
        let imp = Implicit::of(&torus).unwrap();
        let mut changes = Vec::new();
        let n = 20_000;
        let sign = |t: f64| imp.value_at(e.point(t)).lo() > 0.0;
        for k in 0..n {
            let (a, b) = (
                TAU * f64::from(k) / f64::from(n),
                TAU * f64::from(k + 1) / f64::from(n),
            );
            if sign(a) != sign(b) {
                changes.push((a + b) / 2.0);
            }
        }
        assert_eq!(p.len(), changes.len(), "{p:?} vs {changes:?}");
        for ((t, before, after), c) in p.iter().zip(&changes) {
            assert!((t - c).abs() < 1e-3);
            assert_ne!(before, after);
            assert!(torus.distance(e.point(*t)) < 1e-12);
        }
        // The same circle as a NURBS meets a plane as the analytic one does.
        let c = Circle::new(f, 2.0).unwrap();
        let tilted = Surface::Plane(Plane::new(
            Frame::new(
                Point3::ORIGIN,
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .unwrap(),
        ));
        let analytic = points(&curve_surface(&Curve::Circle(c), [0.0, TAU], &tilted, TOL).unwrap());
        let (n, _) = c.to_nurbs().unwrap();
        let nurbs = points(
            &curve_surface(&Curve::Nurbs(Arc::new(n.clone())), n.domain(), &tilted, TOL).unwrap(),
        );
        assert_eq!(analytic.len(), 2);
        assert_eq!(nurbs.len(), 2);
        for ((ta, ..), (tn, ..)) in analytic.iter().zip(&nurbs) {
            assert!(Curve::Circle(c).point(*ta).distance(n.point(*tn)) < 1e-12);
        }
        let _ = PI;
    }

    #[test]
    fn a_nurbs_surface_is_met_by_subdivision() {
        let plane = Surface::Plane(Plane::new(Frame::WORLD));
        let (n, _) = plane.to_nurbs([[0.0, 1.0], [0.0, 1.0]]).unwrap();
        let l = line((0.5, 0.5, -1.0), (0.0, 0.0, 1.0));
        let p = points(&curve_surface(&l, [0.0, 2.0], &Surface::Nurbs(Arc::new(n)), TOL).unwrap());
        assert_eq!(p.len(), 1);
        assert!((p[0].0 - 1.0).abs() <= 1e-12);
        assert_eq!((p[0].1, p[0].2), (Negative, Positive));
    }

    #[test]
    fn a_search_halts_at_its_budget() {
        use crate::work::Work;
        let s = Surface::Sphere(Sphere::new(Frame::WORLD, 2.0).unwrap());
        let l = line((-5.0, 0.0, 0.0), (1.0, 0.0, 0.0));
        let w = Work::unlimited();
        let m = Meter::new(&w);
        assert!(curve_surface_in(&l, [0.0, 10.0], &s, TOL, &m).is_ok());
        assert!(m.spent() > 0);
        let w = Work::unlimited().with_budget(0);
        let m = Meter::new(&w);
        assert_eq!(
            curve_surface_in(&l, [0.0, 10.0], &s, TOL, &m).err(),
            Some(Failure::Uncertain)
        );
        assert!(m.halted());
    }
}
