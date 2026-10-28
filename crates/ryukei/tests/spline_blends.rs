//! Blends on faces of every surface: primitives with their curves and
//! surfaces swapped for exact NURBS forms (so the analytic volumes are
//! the oracle), spline extrusions and revolutions, lofts and imported
//! splines. Every result passes the checker and the self-intersection
//! check, meshes without faults, and encloses the volume it should.
//! This runs in release only (`cargo test -p ryukei --release --test
//! spline_blends`).

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::check::{check, check_intersections};
use ryukei::geom::Rigid;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::mesh::volume_and_area;
use ryukei_testkit::nurbs::convert_body;
use std::f64::consts::PI;

const K: OpKey = OpKey(0xb0);

/// `b` with every curve and surface its exact NURBS form.
fn nurbs(mut b: Body) -> Body {
    convert_body(&mut b).unwrap();
    b
}

/// The volume a sound body's mesh encloses.
fn volume(b: &Body, chord: f64) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(chord, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    volume_and_area(&m).0
}

fn near(got: f64, want: f64, tol: f64) {
    assert!((got - want).abs() <= tol, "{got} against {want}");
}

fn fillet(edge: Name, radius: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius },
    }
}

/// A primitive's edge between two faces.
fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [
        Name::derive(key, Role::Face(a), &[]),
        Name::derive(key, Role::Face(b), &[]),
    ];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

/// The area of the corner a fillet of radius `r` takes from a square
/// edge, and how far its centroid is from the edge along each face.
fn corner(r: f64) -> (f64, f64) {
    (
        (1.0 - PI / 4.0) * r * r,
        r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI),
    )
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn one_edge_of_a_spline_box() {
    let k = OpKey(1);
    let b = nurbs(
        primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let (r, _) = blend(
        K,
        &b,
        &[fillet(edge(k, FaceRole::PlusX, FaceRole::PlusZ), 0.25)],
    )
    .unwrap();
    let c = r.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (7, 15, 10));
    let (area, _) = corner(0.25);
    near(volume(&r, 1e-4), 6.0 - area * 2.0, 1e-4);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn one_edge_of_a_spline_box_chamfered() {
    let k = OpKey(1);
    let b = nurbs(
        primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let (r, _) = blend(
        K,
        &b,
        &[EdgeBlend {
            edge: edge(k, FaceRole::PlusX, FaceRole::PlusZ),
            kind: BlendKind::Chamfer(Chamfer::Equal { distance: 0.2 }),
        }],
    )
    .unwrap();
    near(volume(&r, 1e-4), 6.0 - 0.02 * 2.0, 1e-4);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_spline_cylinders_rim() {
    let b = nurbs(
        primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let rim = b.edges().next().unwrap();
    let (r, _) = blend(K, &b, &[fillet(b.edge(rim).unwrap().name, 0.2)]).unwrap();
    let (area, d) = corner(0.2);
    near(volume(&r, 1e-4), PI - 2.0 * PI * (1.0 - d) * area, 5e-4);
}

/// Blends `blends` on `b` and on its exact NURBS form: both sound, with
/// the same counts and volume.
fn as_analytic(b: &Body, blends: &[EdgeBlend], tol: f64) -> Body {
    let (want, _) = blend(K, b, blends).unwrap();
    let (got, _) = blend(K, &nurbs(b.clone()), blends).unwrap();
    let (w, g) = (want.counts(), got.counts());
    assert_eq!(
        (w.faces, w.edges, w.vertices),
        (g.faces, g.edges, g.vertices)
    );
    near(volume(&got, 1e-4), volume(&want, 1e-4), tol);
    got
}

fn all_edges(k: OpKey) -> Vec<Name> {
    use FaceRole::*;
    let mut out = Vec::new();
    for (a, b) in [
        (PlusX, PlusY),
        (PlusX, MinusY),
        (MinusX, PlusY),
        (MinusX, MinusY),
        (PlusX, PlusZ),
        (PlusX, MinusZ),
        (MinusX, PlusZ),
        (MinusX, MinusZ),
        (PlusY, PlusZ),
        (PlusY, MinusZ),
        (MinusY, PlusZ),
        (MinusY, MinusZ),
    ] {
        out.push(edge(k, a, b));
    }
    out
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn every_edge_of_a_spline_box() {
    let k = OpKey(1);
    let (a, b, c) = (1.0, 2.0, 3.0);
    let bx = primitive::block(k, a, b, c, &Rigid::IDENTITY).unwrap().0;
    let r = 0.25;
    let blends: Vec<EdgeBlend> = all_edges(k).into_iter().map(|e| fillet(e, r)).collect();
    let out = as_analytic(&bx, &blends, 2e-4);
    let (x, y, z) = (a - 2.0 * r, b - 2.0 * r, c - 2.0 * r);
    let want = x * y * z
        + 2.0 * r * (x * y + y * z + z * x)
        + PI * r * r * (x + y + z)
        + 4.0 / 3.0 * PI * (r * r * r);
    near(volume(&out, 1e-4), want, 2e-4);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_spline_box_s_top_rim_chamfered_and_filleted() {
    use FaceRole::*;
    let k = OpKey(1);
    let bx = primitive::block(k, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let top = [PlusX, MinusX, PlusY, MinusY].map(|f| edge(k, f, PlusZ));
    let fillets: Vec<EdgeBlend> = top.iter().map(|&e| fillet(e, 0.15)).collect();
    as_analytic(&bx, &fillets, 2e-4);
    let chamfers: Vec<EdgeBlend> = top
        .iter()
        .map(|&e| EdgeBlend {
            edge: e,
            kind: BlendKind::Chamfer(Chamfer::Equal { distance: 0.15 }),
        })
        .collect();
    as_analytic(&bx, &chamfers, 2e-4);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_spline_cylinder_s_rims_and_a_chamfer() {
    let b = primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rims: Vec<Name> = b.edges().map(|e| b.edge(e).unwrap().name).collect();
    as_analytic(&b, &[fillet(rims[0], 0.2), fillet(rims[1], 0.1)], 5e-4);
    as_analytic(
        &b,
        &[EdgeBlend {
            edge: rims[0],
            kind: BlendKind::Chamfer(Chamfer::Equal { distance: 0.2 }),
        }],
        5e-4,
    );
}

mod lofts {
    use super::*;
    use ryukei::geom::{Curve2, Frame, Line2, Point2, Point3, Vector3};
    use ryukei::sweep::{
        LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft,
    };

    /// A square of side `s` centred on its frame's origin.
    fn square(s: f64) -> Loop {
        let h = s / 2.0;
        let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
        let segments = (0..4)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 4]);
                let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
                Segment {
                    key: SegmentKey(10 + i as u128),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect();
        Loop {
            key: LoopKey(1),
            segments,
        }
    }

    /// A section in the plane `z = h`, turned by `turn` about `z`.
    fn section(h: f64, turn: f64, s: f64) -> ValidProfile {
        let (sn, c) = ryukei::math::sin_cos(turn);
        ValidProfile::new(Profile {
            frame: Frame::new(
                Point3::new(0.0, 0.0, h),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(c, sn, 0.0),
            )
            .unwrap(),
            loops: vec![square(s)],
        })
        .unwrap()
    }

    fn lofted(turn: f64) -> Body {
        let opts = LoftOptions {
            closed: false,
            start: None,
            end: None,
        };
        loft(
            OpKey(5),
            &[section(0.0, 0.0, 1.0), section(1.0, turn, 1.0)],
            &opts,
        )
        .unwrap()
        .0
    }

    /// The edges whose ends lie at heights `z0` and `z1`.
    fn edges_between(b: &Body, z0: f64, z1: f64) -> Vec<Name> {
        b.edges()
            .filter(|&e| {
                let ed = b.edge(e).unwrap();
                let [v0, v1] = [ed.start, ed.end].map(|v| b.vertex(v).unwrap().point.z);
                let (lo, hi) = (v0.min(v1), v0.max(v1));
                (lo - z0).abs() < 1e-9 && (hi - z1).abs() < 1e-9
            })
            .map(|e| b.edge(e).unwrap().name)
            .collect()
    }

    /// A prism lofted between equal squares: its sides are flat
    /// splines. Its laterals and its top rim round as a box's do.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "slow in a debug build: CI runs it in release"
    )]
    fn a_lofted_prism() {
        let b = lofted(0.0);
        let laterals = edges_between(&b, 0.0, 1.0);
        assert_eq!(laterals.len(), 4);
        let r = 0.1;
        let (out, _) = blend(
            K,
            &b,
            &laterals.iter().map(|&e| fillet(e, r)).collect::<Vec<_>>(),
        )
        .unwrap();
        near(
            volume(&out, 1e-4),
            1.0 - 4.0 * (1.0 - PI / 4.0) * r * r,
            2e-4,
        );
        let top = edges_between(&b, 1.0, 1.0);
        assert_eq!(top.len(), 4);
        let (out, _) = blend(
            K,
            &b,
            &top.iter().map(|&e| fillet(e, r)).collect::<Vec<_>>(),
        )
        .unwrap();
        let want = {
            let bx = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
                .unwrap()
                .0;
            use FaceRole::*;
            let top = [PlusX, MinusX, PlusY, MinusY].map(|f| edge(OpKey(1), f, PlusZ));
            let (o, _) = blend(
                K,
                &bx,
                &top.iter().map(|&e| fillet(e, r)).collect::<Vec<_>>(),
            )
            .unwrap();
            volume(&o, 1e-4)
        };
        near(volume(&out, 1e-4), want, 2e-4);
    }

    /// The twisted loft's laterals bevelled: ruled chamfers between
    /// curved splines.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "slow in a debug build: CI runs it in release"
    )]
    fn a_twisted_loft_s_laterals_chamfered() {
        let b = lofted(PI / 6.0);
        let laterals = edges_between(&b, 0.0, 1.0);
        let d = 0.05;
        let (out, _) = blend(
            K,
            &b,
            &laterals
                .iter()
                .map(|&e| EdgeBlend {
                    edge: e,
                    kind: BlendKind::Chamfer(Chamfer::Equal { distance: d }),
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let before = volume(&b, 1e-4);
        let after = volume(&out, 1e-4);
        let cut = 4.0 * d * d / 2.0;
        assert!(
            after < before - cut && after > before - 1.3 * cut,
            "{before} {after}"
        );
    }

    /// Every edge of the twisted loft: it builds, but its corners are balls
    /// between pipes on curved splines, which touch tangentially at the
    /// corner triangles' vertices where two intersection branches cross.
    /// The check can't settle those pairs yet (issue #177, plan 8.F), and
    /// nothing else.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "slow in a debug build: CI runs it in release"
    )]
    fn a_twisted_loft_s_every_edge_checks_all_but_its_corners() {
        use ryukei::check::Fault;
        use ryukei::geom::Surface;
        let b = lofted(PI / 6.0);
        let all: Vec<Name> = b.edges().map(|e| b.edge(e).unwrap().name).collect();
        let (out, _) = blend(
            K,
            &b,
            &all.iter().map(|&e| fillet(e, 0.05)).collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(check(&out), []);
        let faults = check_intersections(&out);
        assert_eq!(faults.len(), 8, "{faults:?}");
        let pipe = |f| {
            matches!(
                out.surface(out.face(f).unwrap().surface),
                Some(Surface::Pipe(_))
            )
        };
        for f in faults {
            let Fault::CannotCheck { a, b } = f else {
                panic!("{f:?}")
            };
            assert!(pipe(a) && pipe(b));
        }
    }

    /// The twisted loft's top rim: fillets between its curved sides and
    /// its flat cap are pipes, which would trim each other at the cap's
    /// corners where both touch the cap. Their crossing there is a tangent
    /// one the intersectors can't settle yet (issue #177, plan 8.F), so it
    /// is refused, naming the corner.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "slow in a debug build: CI runs it in release"
    )]
    fn a_twisted_loft_s_top_rim_is_refused_at_its_corners() {
        let b = lofted(PI / 6.0);
        let top = edges_between(&b, 1.0, 1.0);
        let r = blend(
            K,
            &b,
            &top.iter().map(|&e| fillet(e, 0.05)).collect::<Vec<_>>(),
        );
        let Err(ryukei::Error::InvalidBlend(
            ryukei::BlendError::Uncertain { at } | ryukei::BlendError::Unsupported { at },
        )) = r
        else {
            panic!("{:?}", r.map(|_| ()))
        };
        assert!(b.vertices().any(|v| b.vertex(v).unwrap().name == at));
    }

    /// A loft twisted by a sixth of a turn: its sides are curved splines.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "slow in a debug build: CI runs it in release"
    )]
    fn a_twisted_loft_s_laterals() {
        let b = lofted(PI / 6.0);
        let laterals = edges_between(&b, 0.0, 1.0);
        let r = 0.05;
        let (out, _) = blend(
            K,
            &b,
            &laterals.iter().map(|&e| fillet(e, r)).collect::<Vec<_>>(),
        )
        .unwrap();
        let before = volume(&b, 1e-4);
        let after = volume(&out, 1e-4);
        // Each lateral is a little longer than the height.
        let cut = 4.0 * (1.0 - PI / 4.0) * r * r;
        assert!(
            after < before - cut && after > before - 1.3 * cut,
            "{before} {after}"
        );
    }
}

/// A NURBS circle extruded: a cylinder as an extrusion of a spline.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn an_extruded_spline_circle_s_rim() {
    use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Point2, Point3, Vector2, Vector3};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use std::sync::Arc;
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
    let c = Circle2::new(f, 1.0).unwrap();
    let (n, r) = Curve2::Circle2(c).to_nurbs([0.0, 2.0 * PI]).unwrap();
    let [t0, t1] = [r.to_nurbs(0.0), r.to_nurbs(2.0 * PI)];
    let profile = ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(1),
                curve: Curve2::Nurbs2(Arc::new(n)),
                t0,
                t1,
            }],
        }],
    })
    .unwrap();
    let b = extrude(OpKey(4), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
        .unwrap()
        .0;
    assert!(b.faces().any(|f| matches!(
        b.surface(b.face(f).unwrap().surface),
        Some(ryukei::geom::Surface::Extrusion(_))
    )));
    let rim = b
        .edges()
        .find(|&e| {
            let ed = b.edge(e).unwrap();
            (b.vertex(ed.start).unwrap().point.z - 1.0).abs() < 1e-9
        })
        .unwrap();
    let (out, _) = blend(K, &b, &[fillet(b.edge(rim).unwrap().name, 0.2)]).unwrap();
    let (area, d) = corner(0.2);
    near(volume(&out, 1e-4), PI - 2.0 * PI * (1.0 - d) * area, 5e-4);
}
