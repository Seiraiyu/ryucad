//! Floating-point estimates of the least distance between two bodies and
//! where it is: what [`super::distance`] starts from and takes its nearest
//! points from. Exactness comes from the comparisons, never from here.

use ryu_board::geom::{Nm, Point, round_nm};

use super::core::Core;
use super::distance::Distance;
use super::pair;
use super::{Body, Shell, Solid};
use crate::tree::Packed;

type P = (f64, f64);

/// Two points moved towards each other by `wa / 2` and `wb / 2` along
/// the line between them; where that overlaps, one point in both.
fn thicken(g: f64, p: P, q: P, wa: Nm, wb: Nm) -> (f64, P, P) {
    let (ha, hb) = (wa as f64 / 2.0, wb as f64 / 2.0);
    if g <= 0.0 {
        return (0.0, p, p);
    }
    let dir = ((q.0 - p.0) / g, (q.1 - p.1) / g);
    let d = g - ha - hb;
    if d <= 0.0 {
        let k = ha.min(g);
        let m = (p.0 + k * dir.0, p.1 + k * dir.1);
        return (0.0, m, m);
    }
    (
        d,
        (p.0 + ha * dir.0, p.1 + ha * dir.1),
        (q.0 - hb * dir.0, q.1 - hb * dir.1),
    )
}

fn pf(p: Point) -> P {
    (p.x as f64, p.y as f64)
}

fn far(a: P, b: P) -> f64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    ryu_math::sqrt(dx * dx + dy * dy)
}

/// The best found so far: a distance and its two points.
type Best = Option<(f64, P, P)>;

fn better(best: &mut Best, c: (f64, P, P)) {
    if best.is_none_or(|b| c.0 < b.0) {
        *best = Some(c);
    }
}

/// Two pieces, if they can come nearer than `limit`: the cheap lower
/// bound rules most out before any candidate is made.
fn least(a: &Core, b: &Core, limit: f64, best: &mut Best, flip: bool) {
    if pair::lower(a, b).0 > limit {
        return;
    }
    for c in pair::candidates(a, b) {
        let (d, p, q) = pair::approx(&c);
        if d < limit {
            better(best, if flip { (d, q, p) } else { (d, p, q) });
        }
    }
}

/// The limit a walk goes on with: none once touching is found, since
/// nothing is nearer.
fn open(limit: f64) -> Option<f64> {
    (limit > 0.0).then_some(limit)
}

fn now(best: &Best, limit: f64) -> f64 {
    best.map_or(limit, |b| b.0.min(limit))
}

/// A shell's outlines and a piece: the shell's pieces nearest first, while
/// their boxes come within the best so far.
fn region_core(s: &Shell, c: &Core, limit: f64, best: &mut Best, flip: bool) {
    let one = Packed::new(vec![(c.bounds(), 0)]);
    s.tree.nearest_pairs(
        &one,
        limit,
        |k, r, l| s.lower_to(k, r, l),
        |_, r, _| pair::lower_to_box(c, r),
        |k, _| {
            least(&s.cores[k as usize], c, now(best, limit), best, flip);
            open(now(best, limit))
        },
    );
}

/// Two solids' distance and nearest points in floating point, where they
/// come nearer than `limit` (the best between the bodies so far).
fn solid_estimate(a: &Solid, b: &Solid, limit: f64) -> Best {
    let mut best = None;
    match (a, b) {
        (Solid::Thick { core: c, w }, Solid::Thick { core: d, w: v }) => {
            least(c, d, limit + (w + v) as f64 / 2.0, &mut best, false);
            best.map(|(g, p, q)| thicken(g, p, q, *w, *v))
        }
        (Solid::Region(s), Solid::Thick { core, w })
        | (Solid::Thick { core, w }, Solid::Region(s)) => {
            let flip = matches!(a, Solid::Thick { .. });
            if s.contains(core.probe()) {
                let p = pf(core.probe());
                return Some((0.0, p, p));
            }
            // The probes' distance is an outline-to-piece distance.
            let u = far(pf(s.probe), pf(core.probe())).min(limit + *w as f64 / 2.0);
            region_core(s, core, u, &mut best, flip);
            best.map(|(g, p, q)| {
                if flip {
                    thicken(g, p, q, *w, 0)
                } else {
                    thicken(g, p, q, 0, *w)
                }
            })
        }
        (Solid::Region(s), Solid::Region(t)) => {
            for (inner, outer) in [(s, t), (t, s)] {
                if outer.contains(inner.probe) {
                    let p = pf(inner.probe);
                    return Some((0.0, p, p));
                }
            }
            // Pairs of pieces nearest first, while their boxes come within
            // the best so far.
            let u = far(pf(s.probe), pf(t.probe)).min(limit);
            s.tree.nearest_pairs(
                &t.tree,
                u,
                |i, r, l| s.lower_to(i, r, l),
                |j, r, l| t.lower_to(j, r, l),
                |i, j| {
                    least(
                        &s.cores[i as usize],
                        &t.cores[j as usize],
                        now(&best, u),
                        &mut best,
                        false,
                    );
                    open(now(&best, u))
                },
            );
            best
        }
    }
}

fn round(p: P) -> Point {
    Point::new(round_nm(p.0), round_nm(p.1))
}

/// The least distance between two bodies in floating point, with its
/// nearest points and solids; `exact` false. Pairs of solids, and of
/// their pieces, are taken nearest boxes first and only while their boxes
/// lie nearer than the best so far: a branch and bound over both trees.
pub fn estimate(a: &Body, b: &Body) -> Option<Distance> {
    let mut best: Option<(f64, P, P, usize, usize)> = None;
    // Two probes are a distance the least can't pass: only solids whose
    // boxes come that close to a solid of the other body take part, and
    // the best so far narrows it.
    let u = far(pf(a.solids.first()?.probe()), pf(b.solids.first()?.probe())) + 1.0;
    a.tree.nearest_pairs(
        &b.tree,
        u,
        |i, r, l| a.lower_to(i, r, l),
        |j, r, l| b.lower_to(j, r, l),
        |i, j| {
            let (i, j) = (i as usize, j as usize);
            let limit = best.map_or(u, |m| m.0);
            if let Some((d, p, q)) = solid_estimate(&a.solids[i], &b.solids[j], limit)
                && best.is_none_or(|m| d < m.0)
            {
                best = Some((d, p, q, i, j));
            }
            open(best.map_or(u, |m| m.0))
        },
    );
    let (d, p, q, i, j) = best?;
    Some(Distance {
        nm: round_nm(d),
        exact: false,
        bound: 0,
        a: round(p),
        b: round(q),
        a_solid: i,
        b_solid: j,
    })
}
