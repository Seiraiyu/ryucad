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
    let n = polynomial_surface(rng);
    let weights = (0..n.weights().len())
        .map(|_| rng.range(0.2, 4.0))
        .collect();
    NurbsSurface::new(
        n.knots().clone(),
        n.control_points().collect(),
        Some(weights),
    )
    .expect("a surface")
}

/// [`surface`]'s knots and control points, every weight 1.
fn polynomial_surface(rng: &mut Rng) -> NurbsSurface {
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
    NurbsSurface::new(knots, pts, None).expect("a surface")
}

/// `x` lies in `iv` grown by `slack`.
fn holds(iv: Iv, x: f64, slack: f64) -> bool {
    iv.lo() - slack <= x && x <= iv.hi() + slack
}

/// Over seeded rational and polynomial patches and boxes small against them, the
/// Taylor enclosure holds the surface's point and derivatives at sampled
/// parameters of the box. The sampled values are f64 evaluations, so each
/// is allowed the width of the certified cut's enclosure at that very
/// point (which holds the true value) plus a few ulps of its magnitude:
/// rounding of the reference, not of the enclosure.
#[test]
fn enclosures_hold_sampled_values() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut checked, mut taken) = (0usize, 0usize);
    // Rational patches, then polynomial ones, which take no quotient rule
    // (#308).
    for case in 0..180 {
        let n = if case < 120 {
            surface(&mut rng)
        } else {
            polynomial_surface(&mut rng)
        };
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
            for (index, patch) in n.patches().iter().enumerate() {
                if let (Some(s), Some(t)) = (
                    super::super::local(patch.range[0], range[0]),
                    super::super::local(patch.range[1], range[1]),
                ) && jets(n.id(), index, patch, s, t).is_some()
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
    assert_eq!(step(0.0, 1.0 / 16.0), sp(2, 0));
    assert_eq!(step(0.0, 0.25), sp(2, 0));
    assert_eq!(step(1.0 / 16.0, 3.0 / 16.0), sp(2, 0));
    assert_eq!(step(1.0 - 1.0 / 8.0, 1.0), sp(2, 6));
    assert_eq!(step(1.0, 1.0), sp(2, 6));
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

/// A cell from the table, from a fresh build, or after its shard was
/// emptied gives the same bits.
#[test]
fn cells_are_the_same_however_found() {
    let mut rng = Rng(11);
    let n = surface(&mut rng);
    let patch = &n.patches()[0];
    let (ks, kt) = (Span { level: 2, k: 3 }, Span { level: 1, k: 0 });
    let bits = |c: &Cell| {
        c.values
            .iter()
            .flat_map(|v| {
                v.mids
                    .iter()
                    .flatten()
                    .chain(v.err.iter())
                    .map(|x| x.to_bits())
            })
            .collect::<Vec<u64>>()
    };
    let fresh = build(patch, ks, kt).expect("finite");
    let a = cell(n.id(), 0, patch, ks, kt).expect("finite");
    let b = cell(n.id(), 0, patch, ks, kt).expect("finite");
    assert!(Arc::ptr_eq(&a, &b), "the second is the table's");
    assert_eq!(bits(&fresh), bits(&a));
    for shard in &TABLE.shards {
        let mut s = shard.lock().expect("unpoisoned");
        s.young.clear();
        s.old.clear();
        s.bytes = 0;
    }
    RECENT.with(|r| r.borrow_mut().clear());
    let c = cell(n.id(), 0, patch, ks, kt).expect("finite");
    assert!(!Arc::ptr_eq(&a, &c), "built again");
    assert_eq!(bits(&fresh), bits(&c));
}

/// A polynomial patch's jets, its first three components' enclosures
/// (#308), lie within what the quotient rule gives on the same cell, which
/// divides by an enclosure of 1 and subtracts the point times enclosures
/// of 0: never wider, so a mesh tested by them is never finer. And they
/// hold sampled values as the quotient's do.
#[test]
fn polynomial_jets_are_no_wider_than_the_quotients() {
    let mut rng = Rng(0x308_0001);
    let (mut compared, mut narrower) = (0usize, 0usize);
    for _ in 0..120 {
        let n = polynomial_surface(&mut rng);
        for (index, patch) in n.patches().iter().enumerate() {
            assert!(patch.pts.iter().all(|h| h.w == 1.0));
            for _ in 0..8 {
                let f = [0.0, 1e-9, 1e-4, 0.01, 0.05, 0.2][rng.below(6)];
                let g = [0.0, 1e-9, 1e-4, 0.01, 0.05, 0.2][rng.below(6)];
                let (s0, t0) = (rng.range(0.0, 1.0 - f), rng.range(0.0, 1.0 - g));
                let (s, t) = ([s0, s0 + f], [t0, t0 + g]);
                let (Some(ks), Some(kt)) = (step(s[0], s[1]), step(t[0], t[1])) else {
                    continue;
                };
                let Some(c) = cell(n.id(), index, patch, ks, kt) else {
                    continue;
                };
                assert!(c.polynomial);
                let (Some(p), Some(q)) = (
                    enclose(&c, patch.range, [s, t], [ks, kt], true),
                    enclose(&c, patch.range, [s, t], [ks, kt], false),
                ) else {
                    continue;
                };
                for k in 0..6 {
                    for i in 0..3 {
                        assert!(
                            q[k][i].lo() <= p[k][i].lo() && p[k][i].hi() <= q[k][i].hi(),
                            "jet {k}[{i}]: {:?} not within {:?}",
                            p[k][i],
                            q[k][i]
                        );
                        compared += 1;
                        narrower += usize::from(p[k][i] != q[k][i]);
                    }
                }
            }
        }
    }
    assert!(compared > 20_000, "only {compared} compared");
    assert!(
        narrower > compared / 2,
        "only {narrower} of {compared} narrower"
    );
}

/// A block whose top is a bicubic spline of three by four pieces, bumped
/// unevenly, so its mesh asks for many cells of many patches.
fn spline_block() -> crate::topo::Body {
    use crate::geom::{Rigid, Surface};
    use crate::topo::Sense;
    let (dx, dy, dz) = (0.03, 0.02, 0.01);
    let (mut b, _) =
        crate::primitive::block(crate::name::OpKey(5), dx, dy, dz, &Rigid::IDENTITY).unwrap();
    let top = b
        .faces()
        .find(|&f| {
            b.face_loops(f).all(|l| {
                b.loop_fins(l).all(|fin| {
                    let v = b.fin_start(fin).unwrap();
                    (b.vertex(v).unwrap().point.z - dz).abs() < 1e-12
                })
            })
        })
        .unwrap();
    let ku = KnotVector::clamped(3, &[0.0, 0.3, 0.7, 1.0], &[4, 1, 1, 4]).unwrap();
    let kv = KnotVector::clamped(3, &[0.0, 0.2, 0.5, 0.8, 1.0], &[4, 1, 1, 1, 4]).unwrap();
    // Control points at the Greville abscissae, so `x` and `y` are linear
    // in the parameters and the block's edges are parameter lines.
    let greville = |knots: &[f64], mults: &[usize]| -> Vec<f64> {
        let full: Vec<f64> = knots
            .iter()
            .zip(mults)
            .flat_map(|(&k, &m)| core::iter::repeat_n(k, m))
            .collect();
        (0..full.len() - 4)
            .map(|i| (full[i + 1] + full[i + 2] + full[i + 3]) / 3.0)
            .collect()
    };
    let gu = greville(&[0.0, 0.3, 0.7, 1.0], &[4, 1, 1, 4]);
    let gv = greville(&[0.0, 0.2, 0.5, 0.8, 1.0], &[4, 1, 1, 1, 4]);
    let mut pts = Vec::new();
    let (nu, nv) = (ku.control_count(), kv.control_count());
    for (i, &u) in gu.iter().enumerate() {
        for (j, &v) in gv.iter().enumerate() {
            let edge = i == 0 || j == 0 || i == nu - 1 || j == nv - 1;
            let z = dz
                + if edge {
                    0.0
                } else {
                    0.002 * (1.0 + (i * 7 + j * 3) as f64 % 5.0)
                };
            let (x, y) = (dx * u, dy * v);
            pts.push(Point3::new(x, y, z));
        }
    }
    let s = NurbsSurface::new([ku, kv], pts, None).unwrap();
    let id = b.add_surface(Surface::Nurbs(Arc::new(s)));
    crate::euler::set_surface(&mut b, top, id, Sense::Forward).unwrap();
    crate::pcurve::fill(&mut b).unwrap();
    b
}

/// Every bit of a mesh, in order.
fn mesh_bits(m: &crate::mesh::BodyMesh) -> Vec<u64> {
    let mut out = vec![m.triangles.len() as u64, m.faults.len() as u64];
    out.extend(
        m.positions
            .iter()
            .flat_map(|p| p.to_array().map(f64::to_bits)),
    );
    out.extend(m.triangles.iter().flatten().map(|&i| u64::from(i)));
    out
}

/// Cells evicted mid-mesh and built again, and meshes on one thread and on
/// eight, give the same mesh, bit for bit: a cell is a function of its key,
/// so neither the table's limit nor which thread asked first shows.
#[test]
fn eviction_and_threads_leave_the_mesh_alone() {
    use crate::mesh::{MeshParams, tessellate};
    let body = spline_block();
    let params = MeshParams::new(2e-6, 0.3);
    let on = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| tessellate(&body, &params).unwrap())
    };
    let one = mesh_bits(&on(1));
    assert!(one[0] > 5_000, "only {} triangles", one[0]);
    assert_eq!(one, mesh_bits(&on(8)), "one thread and eight");
    // A limit so small every cell is evicted as soon as another is kept.
    let cap = CAP.swap(1, Ordering::Relaxed);
    let tiny = (mesh_bits(&on(1)), mesh_bits(&on(8)));
    CAP.store(cap, Ordering::Relaxed);
    assert_eq!(one, tiny.0, "evicted, one thread");
    assert_eq!(one, tiny.1, "evicted, eight threads");
}

/// The remainder's radius by upper ends alone (#308) is the float the
/// interval expressions it replaced gave, bit for bit: over values from
/// zero and below `TINY` to near overflow, infinities and NaN, with
/// half-widths as `jets` makes them (upper ends, so at least `TINY`).
#[test]
fn radii_by_upper_ends_match_the_interval_expressions() {
    let up = |x: Iv| x.hi();
    let old = |cell: &Cell,
               [k, ku, kv, kuu, kuv, kvv]: [usize; 6],
               c: usize,
               g: [f64; 2],
               [hu, hv]: [f64; 2]| {
        let err = pt(cell.values[k].err[c]);
        let first = up(err + pt(cell.mags[ku][c]) * pt(hu) + pt(cell.mags[kv][c]) * pt(hv));
        let gu = pt(g[0].abs()) + pt(cell.values[ku].err[c]);
        let gv = pt(g[1].abs()) + pt(cell.values[kv].err[c]);
        let rest = pt(cell.mags[kuu][c]) * pt(hu) * pt(hu)
            + pt(2.0) * pt(cell.mags[kuv][c]) * pt(hu) * pt(hv)
            + pt(cell.mags[kvv][c]) * pt(hv) * pt(hv);
        let second = up(err + gu * pt(hu) + gv * pt(hv) + pt(0.5) * rest);
        first.min(second)
    };
    let specials = [
        0.0,
        -0.0,
        1e-300,
        crate::interval::TINY,
        crate::interval::TINY / 3.0,
        f64::MIN_POSITIVE,
        1e-160,
        1e-12,
        1.0,
        1e150,
        1e300,
        f64::MAX,
        f64::INFINITY,
        f64::NAN,
    ];
    let mut rng = Rng(0x308_5eed);
    let value = |rng: &mut Rng| {
        if rng.below(3) == 0 {
            specials[rng.below(specials.len())]
        } else {
            // Any exponent, so products under- and overflow.
            f64::from_bits(rng.next() >> 1)
        }
    };
    let empty = || Values {
        rows: 0,
        cols: 0,
        mids: Vec::new(),
        err: [0.0; 4],
    };
    for _ in 0..200_000 {
        let mut cell = Cell {
            at: Vector3::new(0.0, 0.0, 0.0),
            polynomial: false,
            values: core::array::from_fn(|_| empty()),
            mags: [[0.0; 4]; 15],
        };
        for v in &mut cell.values {
            v.err = core::array::from_fn(|_| value(&mut rng).abs());
        }
        for m in &mut cell.mags {
            *m = core::array::from_fn(|_| value(&mut rng).abs());
        }
        let k = rng.below(6);
        let (i, j) = ORDERS[k];
        let n = [
            k,
            idx(i + 1, j),
            idx(i, j + 1),
            idx(i + 2, j),
            idx(i + 1, j + 1),
            idx(i, j + 2),
        ];
        let c = rng.below(4);
        let sign = |x: f64, rng: &mut Rng| if rng.below(2) == 0 { x } else { -x };
        let g = [
            sign(value(&mut rng), &mut rng),
            sign(value(&mut rng), &mut rng),
        ];
        // Half-widths are upper ends `up` gave, so at least `TINY`.
        let h = [
            (pt(value(&mut rng).abs()) * pt(1.0)).hi(),
            (pt(value(&mut rng).abs()) * pt(1.0)).hi(),
        ];
        let (a, b) = (old(&cell, n, c, g, h), radius(&cell, n, c, g, h));
        assert!(
            a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
            "{a:e} against {b:e} for {n:?} {c} {g:?} {h:?}"
        );
    }
}
