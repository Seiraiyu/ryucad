use super::*;
use crate::geom::nurbs::NurbsSurface;
use crate::geom::{KnotVector, Point3};

/// xorshift64, for seeded patches.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A seeded rational spline: degrees 1 to 7 each way, one to three
/// pieces, control points at a scale from a micrometre to a metre (so the
/// rounding term matters at both ends), weights from 0.2 to 4.
fn surface(rng: &mut Rng) -> NurbsSurface {
    let scale = [1e-6, 1e-3, 0.05, 1.0][rng.below(4)];
    let mut knots = Vec::new();
    for _ in 0..2 {
        let p = 1 + rng.below(7);
        let pieces = 1 + rng.below(3);
        let mut ks = vec![0.0];
        for i in 1..pieces {
            ks.push(i as f64 + rng.range(-0.3, 0.3));
        }
        ks.push(pieces as f64);
        let width = [1.0, 1e-3, 7.5][rng.below(3)];
        let ks: Vec<f64> = ks.iter().map(|k| k * width).collect();
        let mut mults = vec![p + 1];
        mults.extend(core::iter::repeat_n(p, pieces - 1));
        mults.push(p + 1);
        knots.push(KnotVector::clamped(p, &ks, &mults).expect("knots"));
    }
    let knots: [KnotVector; 2] = knots.try_into().expect("two");
    let n = knots[0].control_count() * knots[1].control_count();
    let pts = (0..n)
        .map(|_| {
            Point3::new(
                scale * rng.range(-1.0, 1.0),
                scale * rng.range(-1.0, 1.0),
                scale * rng.range(-1.0, 1.0),
            )
        })
        .collect();
    let weights = (0..n).map(|_| rng.range(0.2, 4.0)).collect();
    NurbsSurface::new(knots, pts, Some(weights)).expect("a surface")
}

/// `x` lies in `iv` grown by `slack`.
fn holds(iv: Iv, x: f64, slack: f64) -> bool {
    iv.lo() - slack <= x && x <= iv.hi() + slack
}

/// Over seeded rational patches and boxes small against them, the
/// Taylor enclosure holds the surface's point and derivatives at sampled
/// parameters of the box. The sampled values are f64 evaluations, so each
/// is allowed the width of the certified cut's enclosure at that very
/// point (which holds the true value) plus a few ulps of its magnitude:
/// rounding of the reference, not of the enclosure.
#[test]
fn enclosures_hold_sampled_values() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut checked, mut taken) = (0usize, 0usize);
    for _ in 0..120 {
        let n = surface(&mut rng);
        let [du, dv] = n.domain();
        for _ in 0..12 {
            // Boxes from a point to most of the domain.
            let fu = [0.0, 1e-9, 1e-4, 0.01, 0.05, 0.3, 0.9][rng.below(7)];
            let fv = [0.0, 1e-9, 1e-4, 0.01, 0.05, 0.3, 0.9][rng.below(7)];
            let wu = (du[1] - du[0]) * fu;
            let wv = (dv[1] - dv[0]) * fv;
            let u0 = rng.range(du[0], du[1] - wu);
            let v0 = rng.range(dv[0], dv[1] - wv);
            let range = [[u0, u0 + wu], [v0, v0 + wv]];
            let small = n.jet_box_small(range);
            // Count the boxes the Taylor path served.
            for patch in n.patches() {
                if let (Some(s), Some(t)) = (
                    super::super::local(patch.range[0], range[0]),
                    super::super::local(patch.range[1], range[1]),
                ) && jets(patch, s, t).is_some()
                {
                    taken += 1;
                }
            }
            for i in 0..=4 {
                for j in 0..=4 {
                    let (a, b) = if i == 4 && j == 4 {
                        (rng.unit(), rng.unit())
                    } else {
                        (f64::from(i) / 3.0, f64::from(j) / 3.0)
                    };
                    let u = (u0 + wu * a.min(1.0)).min(range[0][1]);
                    let v = (v0 + wv * b.min(1.0)).min(range[1][1]);
                    let e = n.eval(u, v);
                    let at = n.jet_box([[u, u], [v, v]]);
                    let vals = [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv];
                    for k in 0..6 {
                        let x = [vals[k].x, vals[k].y, vals[k].z];
                        for c in 0..3 {
                            let slack = at[k][c].hi() - at[k][c].lo() + 1e-14 * x[c].abs();
                            assert!(
                                holds(small[k][c], x[c], slack),
                                "jet {k}[{c}] at ({u}, {v}) of {range:?}: {} not in {:?} \
                                 (cut {:?})",
                                x[c],
                                small[k][c],
                                at[k][c]
                            );
                            // Both enclose the true value: they meet.
                            assert!(
                                small[k][c].lo() <= at[k][c].hi()
                                    && at[k][c].lo() <= small[k][c].hi(),
                                "jet {k}[{c}] at ({u}, {v}): {:?} misses {:?}",
                                small[k][c],
                                at[k][c]
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(checked > 400_000, "only {checked} values checked");
    assert!(taken > 1_000, "the Taylor path served only {taken} boxes");
}

/// A box takes the smallest cell holding it, each direction alone; a box
/// no wider than half a cell at a level lies in a cell of that level.
#[test]
fn boxes_find_their_cells() {
    let sp = |level, k| Some(Span { level, k });
    assert_eq!(step(0.0, 1.0 / 16.0), sp(3, 0));
    assert_eq!(step(0.0, 0.125), sp(3, 0));
    assert_eq!(step(1.0 / 32.0, 3.0 / 32.0), sp(3, 0));
    assert_eq!(step(1.0 - 1.0 / 16.0, 1.0), sp(3, 14));
    assert_eq!(step(1.0, 1.0), sp(3, 14));
    assert_eq!(step(0.3, 0.6), sp(1, 1));
    assert_eq!(step(0.1, 0.9), sp(0, 0));
    assert_eq!(step(0.0, 1.0), sp(0, 0));
    assert_eq!(step(-0.1, 0.0), None);
    for level in 0..LEVELS {
        let half = 1.0 / (2u64 << level) as f64;
        for k in 0..200 {
            let s0 = f64::from(k) / 200.0 * (1.0 - half);
            let span = step(s0, s0 + half).expect("a cell");
            assert!(span.level >= level, "{s0} at {level}: {span:?}");
            let [lo, hi] = span.range();
            assert!(lo <= s0 && s0 + half <= hi);
        }
    }
    // Every cell has its own place in a direction's list.
    let mut seen = [false; ALONG];
    for level in 0..LEVELS {
        for k in 0..steps(level) {
            let i = Span { level, k }.index();
            assert!(!seen[i]);
            seen[i] = true;
        }
    }
    assert!(seen.iter().all(|&x| x));
}

/// The enclosure of a degenerate box is tight: a few ulps of the values'
/// size, far narrower than the cut over a cell.
#[test]
fn point_boxes_are_tight() {
    let mut rng = Rng(7);
    for _ in 0..20 {
        let n = surface(&mut rng);
        let [du, dv] = n.domain();
        let (u, v) = (rng.range(du[0], du[1]), rng.range(dv[0], dv[1]));
        let j = n.jet_box_small([[u, u], [v, v]]);
        let e = n.eval(u, v);
        let p = e.point.to_vector();
        for (c, x) in [p.x, p.y, p.z].into_iter().enumerate() {
            let w = j[0][c].hi() - j[0][c].lo();
            assert!(w <= 1e-9 * (1.0 + x.abs()) * 1e3, "{w} at {x}");
        }
    }
}
