use super::*;
use crate::geom::sheet::Sheet;
use crate::geom::{
    Circle, Curve, Cylinder, Extrusion, Frame, KnotVector, NurbsSurface, Revolution, Sphere,
};
use crate::math;
use core::f64::consts::{PI, TAU};
use std::sync::Arc;

/// A rational bicubic patch of two by three spans, gently waved.
pub(crate) fn wavy() -> Surface {
    let ku = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
    let kv = KnotVector::clamped(3, &[0.0, 0.3, 0.7, 1.0], &[4, 1, 1, 4]).unwrap();
    let mut pts = Vec::new();
    let mut w = Vec::new();
    for i in 0..5 {
        for j in 0..6 {
            let (x, y) = (i as f64 * 0.5, j as f64 * 0.4);
            pts.push(Point3::new(
                x,
                y,
                0.3 * math::sin(x * 1.3) * math::cos(y + 0.2),
            ));
            w.push(1.0 + 0.3 * ((i + 2 * j) % 3) as f64);
        }
    }
    Surface::Nurbs(Arc::new(NurbsSurface::new([ku, kv], pts, Some(w)).unwrap()))
}

fn tilted() -> Frame {
    Frame::new(
        Point3::new(0.3, -0.2, 0.5),
        Vector3::new(1.0, 0.4, 0.2),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap()
}

/// A NURBS cylinder of radius 0.7 over half a turn and a unit height.
pub(crate) fn tube() -> Surface {
    let c = Surface::Cylinder(Cylinder::new(tilted(), 0.7).unwrap());
    let (n, _) = c.to_nurbs([[0.0, PI], [-0.5, 0.5]]).unwrap();
    Surface::Nurbs(Arc::new(n))
}

/// An extrusion of a NURBS circle, and a revolution of a circle off the
/// axis (a torus as a swept surface).
fn swept() -> [Surface; 2] {
    let circle = Circle::new(tilted(), 0.8).unwrap();
    let (nc, _) = circle.to_nurbs().unwrap();
    let ex = Extrusion::new(Curve::Nurbs(Arc::new(nc)), Vector3::new(1.0, 0.5, 0.3)).unwrap();
    let meridian = Frame::new(
        Point3::new(2.0, 0.0, 0.3),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let ring = Circle::new(meridian, 0.5).unwrap();
    let rev = Revolution::new(Frame::WORLD, Curve::Circle(ring)).unwrap();
    [
        Surface::Extrusion(Arc::new(ex)),
        Surface::Revolution(Arc::new(rev)),
    ]
}

/// Each base with a box of its parameters and distances either way that
/// keep it regular.
fn cases() -> Vec<(Surface, Cell, [f64; 2])> {
    let [ex, rev] = swept();
    vec![
        (wavy(), [[0.0, 1.0], [0.0, 1.0]], [0.2, -0.2]),
        (tube(), [[0.0, 1.0], [-0.5, 0.5]], [0.3, -0.5]),
        (ex, [[0.5, 4.0], [-1.0, 1.0]], [0.3, -0.5]),
        (rev, [[0.0, TAU], [0.0, TAU]], [0.2, -0.3]),
    ]
}

fn grid(c: Cell) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for i in 0..=5 {
        for k in 0..=5 {
            out.push((
                c[0][0] + (c[0][1] - c[0][0]) * (0.05 + 0.18 * f64::from(i)),
                c[1][0] + (c[1][1] - c[1][0]) * (0.05 + 0.18 * f64::from(k)),
            ));
        }
    }
    out
}

#[test]
fn points_are_the_base_moved_along_its_normal() {
    for (base, cell, ds) in cases() {
        for d in ds {
            let o = Offset::new(base.clone(), d, cell).unwrap();
            for (u, v) in grid(cell) {
                let n = base.normal(u, v).unwrap().get();
                let want = base.point(u, v) + n * d;
                assert!(o.point(u, v).distance(want) <= 1e-12, "{base:?} by {d}");
                assert_eq!(o.normal(u, v).unwrap(), base.normal(u, v).unwrap());
                // The natural normal along O_u × O_v is the base's.
                let e = o.eval(u, v);
                assert!(e.du.cross(e.dv).dot(n) > 0.0, "{base:?} by {d} at {u}, {v}");
            }
        }
    }
}

#[test]
fn derivatives_are_the_surfaces() {
    let h = 1e-5;
    for (base, cell, ds) in cases() {
        for d in ds {
            let o = Offset::new(base.clone(), d, cell).unwrap();
            for (u, v) in grid(cell) {
                let e = o.eval(u, v);
                let at_u = |a: f64| o.eval(u + a, v);
                let at_v = |a: f64| o.eval(u, v + a);
                let close = |a: Vector3, b: Vector3| (a - b).length() <= 1e-5 * (1.0 + b.length());
                assert!(close(e.du, (at_u(h).point - at_u(-h).point) / (2.0 * h)));
                assert!(close(e.dv, (at_v(h).point - at_v(-h).point) / (2.0 * h)));
                assert!(close(e.duu, (at_u(h).du - at_u(-h).du) / (2.0 * h)));
                assert!(close(e.duv, (at_v(h).du - at_v(-h).du) / (2.0 * h)));
                assert!(close(e.dvv, (at_v(h).dv - at_v(-h).dv) / (2.0 * h)));
            }
        }
    }
}

/// Every value and derivative at a grid of each box lies in the box's
/// enclosures.
#[test]
fn enclosures_hold_over_boxes() {
    for (base, cell, ds) in cases() {
        for d in ds {
            let o = Surface::Offset(Arc::new(Offset::new(base.clone(), d, cell).unwrap()));
            let sheet = Sheet::of(&o);
            let at = |c: [f64; 2], x: f64| c[0] + (c[1] - c[0]) * x;
            for (x, y, w) in [(0.1, 0.2, 0.05), (0.35, 0.25, 0.3), (0.0, 0.0, 1.0)] {
                let b = [
                    [at(cell[0], x), at(cell[0], x + w)],
                    [at(cell[1], y), at(cell[1], y + w)],
                ];
                let p = sheet.jet_box(b).unwrap();
                for (u, v) in grid(b) {
                    let e = o.eval(u, v);
                    let want = [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv];
                    for (c, pc) in p.iter().enumerate() {
                        let got = [pc.v, pc.d[0], pc.d[1], pc.h[0], pc.h[1], pc.h[2]];
                        for (g, w) in got.iter().zip(want) {
                            let x = w.to_array()[c];
                            let slack = 1e-9 * (1.0 + x.abs());
                            assert!(
                                g.lo() - slack <= x && x <= g.hi() + slack,
                                "{base:?} by {d} over {b:?}: {x} not in {g:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn derivative_bounds_hold() {
    for (base, cell, ds) in cases() {
        let o = Surface::Offset(Arc::new(Offset::new(base, ds[1], cell).unwrap()));
        let b = o.derivative_bound(cell);
        for (u, v) in grid(cell) {
            let e = o.eval(u, v);
            assert!(e.duu.length() <= b.duu * (1.0 + 1e-9) + 1e-12);
            assert!(e.duv.length() <= b.duv * (1.0 + 1e-9) + 1e-12);
            assert!(e.dvv.length() <= b.dvv * (1.0 + 1e-9) + 1e-12);
        }
    }
}

/// A parameter line is a box of no width one way: its derivative bounds
/// are finite and hold along it, on its cells' sides (a seam's line at a
/// period's start, where every cell only touches it) as inside them. Such
/// a box once fell between the cells, its bound infinite, and a fit along
/// it read infinity times no velocity as NaN, which passed.
#[test]
fn parameter_lines_have_bounds() {
    for (base, cell, ds) in cases() {
        let o = Surface::Offset(Arc::new(Offset::new(base, ds[0], cell).unwrap()));
        let mid = |c: [f64; 2]| (c[0] + c[1]) / 2.0;
        for (k, at) in [
            (0, cell[0][0]),
            (0, mid(cell[0])),
            (1, cell[1][0]),
            (1, mid(cell[1])),
        ] {
            let mut b = cell;
            b[k] = [at, at];
            let bound = o.derivative_bound(b);
            assert!(bound.duu.is_finite() && bound.dvv.is_finite(), "{b:?}");
            assert!(Sheet::of(&o).jet_box(b).is_some(), "{b:?}");
            for i in 0..=8 {
                let t = b[1 - k][0] + (b[1 - k][1] - b[1 - k][0]) * f64::from(i) / 8.0;
                let (u, v) = if k == 0 { (at, t) } else { (t, at) };
                let e = o.eval(u, v);
                assert!(e.duu.length() <= bound.duu * (1.0 + 1e-9) + 1e-12);
                assert!(e.dvv.length() <= bound.dvv * (1.0 + 1e-9) + 1e-12);
            }
        }
    }
}

#[test]
fn projection_finds_points_on_it_and_off_it() {
    for (base, cell, ds) in cases() {
        let o = Surface::Offset(Arc::new(Offset::new(base, ds[0], cell).unwrap()));
        for (u, v) in grid(cell).into_iter().step_by(5) {
            let p = o.point(u, v);
            assert!(o.distance(p) <= 1e-9, "{o:?} at {u}, {v}");
            // Off it along the normal, by less than the curvature allows.
            let n = o.normal(u, v).unwrap().get();
            let q = p + n * 0.01;
            let f = o.project(q);
            assert!((f.distance - 0.01).abs() <= 1e-9, "{}", f.distance);
            assert!(f.point.distance(p) <= 1e-7);
        }
    }
}

#[test]
fn folds_and_poles_are_refused() {
    // The tube's radius is 0.7: inward past it folds.
    assert_eq!(
        Offset::new(tube(), -0.75, [[0.0, 1.0], [-0.5, 0.5]]),
        Err(Error::Degenerate)
    );
    assert!(Offset::new(tube(), -0.65, [[0.0, 1.0], [-0.5, 0.5]]).is_ok());
    // The revolution's tube has radius 0.5: towards its core circle past
    // that folds.
    let [_, rev] = swept();
    let (p, n) = (rev.point(0.0, 0.0), rev.normal(0.0, 0.0).unwrap().get());
    let core = Point3::new(2.0, 0.0, 0.3);
    let inward = if n.dot(core - p) > 0.0 { 1.0 } else { -1.0 };
    assert_eq!(
        Offset::new(rev.clone(), 0.6 * inward, [[0.0, TAU], [0.0, TAU]]),
        Err(Error::Degenerate)
    );
    assert!(Offset::new(rev, 0.45 * inward, [[0.0, TAU], [0.0, TAU]]).is_ok());
    // A spline sphere's poles have no certified normal.
    let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let (n, _) = sphere.to_nurbs([[0.0; 2]; 2]).unwrap();
    let n = Surface::Nurbs(Arc::new(n));
    assert_eq!(n.offset(0.1), Some(Err(Error::Degenerate)));
    let band = n.domain();
    let band = [band[0], [band[1][0] + 0.3, band[1][1] - 0.3]];
    assert!(Offset::new(n, 0.1, band).is_ok());
    // Analytic surfaces offset to their own kinds; NaN and empty boxes.
    assert_eq!(
        Offset::new(sphere, 0.1, [[0.0, 1.0], [0.0, 1.0]]),
        Err(Error::Degenerate)
    );
    assert_eq!(
        Offset::new(wavy(), f64::NAN, [[0.0, 1.0], [0.0, 1.0]]),
        Err(Error::NonFinite)
    );
    assert_eq!(
        Offset::new(wavy(), 0.1, [[0.5, 0.5], [0.0, 1.0]]),
        Err(Error::Degenerate)
    );
}

#[test]
fn offsets_of_offsets_add_up() {
    let a = Surface::Offset(Arc::new(
        Offset::new(wavy(), 0.1, [[0.0, 1.0], [0.0, 0.8]]).unwrap(),
    ));
    let b = Offset::new(a.clone(), 0.05, [[0.2, 1.0], [0.0, 1.0]]).unwrap();
    assert_eq!(b.base(), &wavy());
    assert_eq!(b.distance(), 0.1 + 0.05);
    assert_eq!(b.domain(), [[0.2, 1.0], [0.0, 0.8]]);
    let Some(Ok(Surface::Offset(c))) = a.offset(-0.1) else {
        panic!()
    };
    assert_eq!(c.distance(), 0.0);
}

#[test]
fn periodic_boxes_keep_their_period_only_whole() {
    let [_, rev] = swept();
    let whole = Offset::new(rev.clone(), 0.1, [[0.0, TAU], [0.0, TAU]]).unwrap();
    assert_eq!(whole.periods(), (Some(TAU), Some(TAU)));
    assert_eq!(whole.closed_genus(), Some(1));
    // Past the end of a period: the box starts in the period and wraps.
    let part = Offset::new(rev, 0.1, [[TAU - 0.5, TAU + 0.5], [0.0, TAU]]).unwrap();
    assert_eq!(part.periods(), (None, Some(TAU)));
    assert_eq!(part.closed_genus(), None);
    let [lo, hi] = part.domain()[0];
    assert!((hi - lo - 1.0).abs() < 1e-12);
    for u in [lo, lo + 0.4, hi - 0.1] {
        let p = part.point(u, 1.0);
        assert!(part.project(p).distance <= 1e-9);
    }
}

#[test]
fn moved_and_reflected_offsets() {
    let r = Rigid::translation(Vector3::new(0.5, -1.0, 2.0));
    let m = Mirror::new(
        Point3::new(0.5, -1.0, 2.0),
        Vector3::new(1.0, 0.3, -0.6).normalize().unwrap(),
    );
    for (base, cell, ds) in cases() {
        let o = Surface::Offset(Arc::new(Offset::new(base, ds[0], cell).unwrap()));
        let t = o.transformed(&r);
        let (mo, flip) = o.mirrored(&m).unwrap();
        for (u, v) in grid(cell).into_iter().step_by(7) {
            assert!(t.point(u, v).distance(r.apply_point(o.point(u, v))) <= 1e-12);
            let q = flip.apply(crate::geom::Point2::new(u, v));
            let want = m.apply_point(o.point(u, v));
            assert!(mo.point(q.x, q.y).distance(want) <= 1e-12, "{o:?}");
            let sign = if flip.flips_normal() { -1.0 } else { 1.0 };
            let n = m.apply_vector(o.normal(u, v).unwrap().get()) * sign;
            assert!((mo.normal(q.x, q.y).unwrap().get() - n).length() <= 1e-12);
        }
        let corner = r.apply_point(o.point(cell[0][0], cell[1][0]));
        assert!(t.bounds().intersects(&Box3::from_point(corner)));
    }
}

/// The NURBS form keeps within its bound of the offset at the same
/// parameters, its bound within the tolerance asked for; a box off a
/// bounded domain is refused. (The waved patch takes the longest: about
/// 3 s at 1e-4, over 20 s at 1e-5.)
#[test]
fn nurbs_forms_keep_within_their_bound() {
    for (k, (base, cell, ds)) in cases().into_iter().enumerate() {
        let o = Offset::new(base.clone(), ds[0] / 4.0, cell).unwrap();
        let tolerances: &[f64] = if k == 0 { &[1e-4] } else { &[1e-4, 1e-5] };
        for &tolerance in tolerances {
            let (n, r, bound) = o.to_nurbs_within(cell, tolerance).unwrap();
            assert_eq!(r, [crate::geom::Reparam::Identity; 2]);
            assert!(bound <= tolerance, "{bound}");
            assert_eq!(n.domain(), cell);
            for (u, v) in grid(cell) {
                let d = n.point(u, v).distance(o.point(u, v));
                assert!(d <= bound, "{base:?}: {d} past {bound} at {u}, {v}");
            }
        }
    }
    let o = Offset::new(wavy(), 0.05, [[0.0, 1.0], [0.0, 1.0]]).unwrap();
    assert_eq!(
        o.to_nurbs_within([[0.5, 1.5], [0.0, 1.0]], 1e-6).err(),
        Some(Error::Degenerate)
    );
}

/// A NURBS sphere's band clear of its poles.
pub(crate) fn ball() -> (Surface, Cell) {
    let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let (n, _) = sphere.to_nurbs([[0.0; 2]; 2]).unwrap();
    let n = Surface::Nurbs(Arc::new(n));
    let band = n.domain();
    let band = [band[0], [band[1][0] + 0.3, band[1][1] - 0.3]];
    (n, band)
}

/// The three ways over small cells: regular short of the focal
/// surface; turned past a tube's axis (one factor negative); inverted
/// past a sphere's centre (both), where the expanded product cancels
/// (`-1.4` leaves both factors `-0.4`).
#[test]
fn turn_over_reads_the_three_ways() {
    let cell = [[0.4, 0.41], [0.1, 0.11]];
    let o = Offset::new(tube(), 0.1, [[0.0, 1.0], [-0.5, 0.5]]).unwrap();
    let d = o.local_box(cell).unwrap();
    assert_eq!(turn_over(&d, 0.3), Over::Is(Turn::Regular));
    assert_eq!(turn_over(&d, -0.75), Over::Is(Turn::Turned));
    let (b, band) = ball();
    let o = Offset::new(b, 0.1, band).unwrap();
    let d = o.local_box([[1.0, 1.0001], [0.0, 0.0001]]).unwrap();
    assert_eq!(turn_over(&d, 0.1), Over::Is(Turn::Regular));
    assert_eq!(turn_over(&d, -0.5), Over::Is(Turn::Regular));
    assert_eq!(turn_over(&d, -1.4), Over::Is(Turn::Inverted));
    assert_eq!(turn_over(&d, -2.0), Over::Is(Turn::Inverted));
}

/// Offsets past their focal surfaces, each with its way: the tube and
/// the revolution's tube inward past their radii (one factor negative),
/// the ball inward past its centre (both).
fn passing() -> Vec<(Surface, Cell, f64, Turn)> {
    let [_, rev] = swept();
    let (p, n) = (rev.point(0.0, 0.0), rev.normal(0.0, 0.0).unwrap().get());
    let core = Point3::new(2.0, 0.0, 0.3);
    let inward = if n.dot(core - p) > 0.0 { 1.0 } else { -1.0 };
    let (b, band) = ball();
    vec![
        (tube(), [[0.0, 1.0], [-0.5, 0.5]], -0.75, Turn::Turned),
        (rev, [[0.0, TAU], [0.0, TAU]], 0.6 * inward, Turn::Turned),
        (b, band, -2.0, Turn::Inverted),
    ]
}

#[test]
fn offsets_past_their_focal_surface_keep_their_parameters() {
    for (base, cell, d, way) in passing() {
        assert_eq!(Offset::new(base.clone(), d, cell), Err(Error::Degenerate));
        let o = Offset::inverted_new(base.clone(), d, cell)
            .unwrap_or_else(|e| panic!("{way:?} by {d}: {e:?}"));
        assert_eq!(o.turn(), way);
        assert_eq!(o.turned(), way == Turn::Turned);
        assert!(o.passes_focal());
        let sign = if o.turned() { -1.0 } else { 1.0 };
        for (u, v) in grid(cell) {
            let n = base.normal(u, v).unwrap().get();
            let want = base.point(u, v) + n * d;
            assert!(o.point(u, v).distance(want) < 1e-12, "{way:?} {u} {v}");
            // The natural normal is along `O_u × O_v`, and the base's
            // times the turn's sign.
            let m = o.normal(u, v).unwrap().get();
            assert!((m.dot(n) - sign).abs() < 1e-12, "{way:?} {u} {v}");
            let e = o.eval(u, v);
            assert!(e.du.cross(e.dv).dot(m) > 0.0, "{way:?} {u} {v}");
            let s = Sheet::Offset(&o).normal_jets_at(u, v).unwrap();
            let s = Vector3::new(s[0].v, s[1].v, s[2].v);
            assert!(s.dot(m) > 0.0, "{way:?} {u} {v}");
        }
    }
}

#[test]
fn a_regular_offset_is_regular_either_way() {
    for (base, cell, [a, b]) in cases() {
        for d in [a, b] {
            let o = Offset::inverted_new(base.clone(), d, cell).unwrap();
            assert_eq!(o, Offset::new(base.clone(), d, cell).unwrap());
            assert_eq!(o.turn(), Turn::Regular);
        }
    }
}

#[test]
fn an_offset_folding_in_part_is_refused() {
    // A spline cone offset inward by more than its radius at one end and
    // less than at the other folds where its focal line crosses it.
    let c = Surface::Cone(crate::geom::Cone::new(Frame::WORLD, 1.0, 0.4).unwrap());
    let (n, _) = c.to_nurbs([[0.0, PI], [0.0, 1.0]]).unwrap();
    let n = Surface::Nurbs(Arc::new(n));
    let at = |v: f64| {
        let p = n.point(0.0, v);
        math::sqrt(p.x * p.x + p.y * p.y)
    };
    let (lo, hi) = (at(0.0).min(at(1.0)), at(0.0).max(at(1.0)));
    assert!(hi - lo > 0.2, "{lo} {hi}");
    // The focal line is the axis, `r / cos α` along the normal.
    let reach = |r: f64| r / math::cos(0.4);
    let d = -(reach(lo) + reach(hi)) / 2.0;
    let r = Offset::inverted_new(n.clone(), d, [[0.0, PI], [0.0, 1.0]]);
    assert_eq!(r, Err(Error::Degenerate));
    // Past both radii it is turned whole.
    let o = Offset::inverted_new(n, -(reach(hi) + 0.1), [[0.0, PI], [0.0, 1.0]]).unwrap();
    assert!(o.turned());
}

#[test]
fn offsets_of_turned_offsets_add_along_their_normal() {
    let a = Surface::Offset(Arc::new(
        Offset::inverted_new(tube(), -0.75, [[0.0, 1.0], [-0.5, 0.5]]).unwrap(),
    ));
    // Along the turned offset's normal is against the tube's.
    let b = Offset::inverted_new(a.clone(), 0.05, [[0.0, 1.0], [-0.5, 0.5]]).unwrap();
    assert_eq!(b.distance(), -0.75 - 0.05);
    assert!(b.turned());
    // Regular against the turned offset keeps its turn.
    let c = Offset::new(a.clone(), 0.05, [[0.0, 1.0], [-0.5, 0.5]]).unwrap();
    assert_eq!(c, b);
    // The turned offset is a tube of radius 0.05 about the same axis:
    // 0.1 back along its normal passes that axis, regular against the
    // tube (0.65 inward) but not against the turned offset.
    assert_eq!(
        Offset::new(a.clone(), -0.1, [[0.0, 1.0], [-0.5, 0.5]]),
        Err(Error::Degenerate)
    );
    let back = Offset::inverted_new(a, -0.1, [[0.0, 1.0], [-0.5, 0.5]]).unwrap();
    assert_eq!(back.turn(), Turn::Regular);
}

#[test]
fn turned_offsets_move_and_reflect_with_their_turn() {
    let o = Offset::inverted_new(tube(), -0.75, [[0.0, 1.0], [-0.5, 0.5]]).unwrap();
    let r = Rigid::translation(Vector3::new(0.1, 0.2, 0.3));
    assert!(o.transformed(&r).turned());
    let m = Mirror::new(Point3::ORIGIN, UnitVector3::Y);
    let (mo, _) = o.mirrored(&m).unwrap();
    assert!(mo.turned());
    let want = m.apply_point(o.point(0.4, 0.1));
    let q = mo.project(want).param;
    assert!(mo.point(q.0, q.1).distance(want) < 1e-9);
}

/// STEP reads an offset marked `self_intersect` through
/// `Surface::offset_through`: past the focal surface over the base's
/// whole domain where `Surface::offset` refuses, and the same offset
/// where it is regular.
#[test]
fn offset_through_passes_the_focal_surface() {
    let t = tube();
    assert_eq!(t.offset(-0.8), Some(Err(Error::Degenerate)));
    let Some(Ok(Surface::Offset(o))) = t.offset_through(-0.8) else {
        panic!("turned past the axis")
    };
    assert!(o.turned());
    assert_eq!(t.offset_through(0.2), t.offset(0.2));
}

/// #599: an offset's point lies within its base's grown by `|d|`, so
/// every enclosure is cut there and the offset's box lies within its
/// base's box grown by `|d|`; enclosures still hold sampled points.
#[test]
fn enclosures_lie_within_reach_of_the_base() {
    for (base, cell, [a, b]) in cases() {
        for d in [a, b] {
            let o = Offset::new(base.clone(), d, cell).unwrap();
            let reach = base.bounds().grow(d.abs() * (1.0 + 1e-12));
            let within = |b: Box3| {
                b.min().x >= reach.min().x
                    && b.min().y >= reach.min().y
                    && b.min().z >= reach.min().z
                    && b.max().x <= reach.max().x
                    && b.max().y <= reach.max().y
                    && b.max().z <= reach.max().z
            };
            assert!(within(o.bounds()), "{:?} against {reach:?}", o.bounds());
            let sheet = Sheet::of(&base);
            for c in o.cells() {
                let q = o.jet_box(*c).unwrap();
                let p = sheet.jet_box(*c).unwrap();
                for k in 0..3 {
                    assert!(q[k].v.lo() >= p[k].v.lo() - d.abs() * (1.0 + 1e-12));
                    assert!(q[k].v.hi() <= p[k].v.hi() + d.abs() * (1.0 + 1e-12));
                }
            }
        }
    }
}
