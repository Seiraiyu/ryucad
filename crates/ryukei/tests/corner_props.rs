//! Corners on seeded random bodies: random boxes and sweeps, placed
//! anywhere, with two or three of the edges at a vertex of three blended:
//! fillets of one size, of two (one larger), chamfers, or a fillet and a
//! chamfer. Every result is a typed refusal or a body that passes the
//! checker and meshes without faults, enclosing a volume within what its
//! edges' corners could take or fill; one thread and many give the same
//! bits. Refusals for overflow are expected (the sizes run large), and
//! for faces the blend can't lie on (swept splines); others must stay
//! rare. This runs in release only (`cargo test -p ryukei --release
//! --test corner_props`).

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::check::{Fault, check, check_intersections};
use ryukei::geom::{Curve, Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId, VertexId};
use ryukei::{BlendError, Error};
use ryukei_testkit::Lcg;
use ryukei_testkit::sweep_bytes::sweep;
use std::f64::consts::PI;

fn volume(b: &Body) -> f64 {
    let m = tessellate(b, &MeshParams::new(1e-4, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

/// A random box or sweep, and its smallest dimension.
fn body(g: &mut Lcg) -> (Body, f64) {
    loop {
        if g.next_u64().is_multiple_of(3) {
            let axis = Vector3::new(g.next_f64(-1.0, 1.0), g.next_f64(-1.0, 1.0), 1.0)
                .normalize()
                .unwrap();
            let t = Vector3::new(
                g.next_f64(-3.0, 3.0),
                g.next_f64(-3.0, 3.0),
                g.next_f64(-3.0, 3.0),
            );
            let at = Rigid::translation(t).then_after(&Rigid::rotation(axis, g.next_f64(0.0, PI)));
            let d = [0, 1, 2].map(|_| g.next_f64(0.5, 2.0));
            let b = primitive::block(OpKey(1), d[0], d[1], d[2], &at).unwrap().0;
            return (b, d[0].min(d[1]).min(d[2]));
        }
        let bytes: Vec<u8> = (0..96).map(|_| g.next_u64() as u8).collect();
        if let Some(found) = sweep(&bytes)
            && found.1 >= 0.2
            && found.0.vertices().any(|v| corner_at(&found.0, v).is_some())
            && found.0.faces().all(|f| {
                let s = found.0.surface(found.0.face(f).unwrap().surface).unwrap();
                !matches!(
                    s,
                    Surface::Nurbs(_) | Surface::Extrusion(_) | Surface::Revolution(_)
                )
            })
        {
            return found;
        }
    }
}

/// The three edges at `v`, when it has three and none is closed.
fn corner_at(b: &Body, v: VertexId) -> Option<[EdgeId; 3]> {
    let edges = b.vertex_edges(v);
    let [x, y, z] = edges[..] else {
        return None;
    };
    let open = |e: EdgeId| {
        let ed = b.edge(e).unwrap();
        ed.start != ed.end
    };
    (open(x) && open(y) && open(z)).then_some([x, y, z])
}

/// What blending an edge by `size` may cut away or fill at most: the
/// square of how far its rails can be set back (a fillet's `r cot(α/2)`
/// for a wedge `α`), along its length and that far past each end.
fn most(b: &Body, e: EdgeId, size: f64) -> f64 {
    let x = b.edge(e).unwrap();
    let c = b.curve(x.curve).unwrap();
    let len = match c {
        Curve::Circle(k) => 2.0 * PI * k.radius(),
        _ => c.point(x.interval[0]).distance(c.point(x.interval[1])),
    };
    let mid = c.point((x.interval[0] + x.interval[1]) / 2.0);
    let normals: Vec<Vector3> = b
        .edge_fins(e)
        .map(|f| {
            let face = b
                .face(b.loop_(b.fin(f).unwrap().loop_).unwrap().face)
                .unwrap();
            let s = b.surface(face.surface).unwrap();
            let (u, v) = s.project(mid).param;
            let n = s.normal(u, v).unwrap().get();
            if face.sense == ryukei::topo::Sense::Forward {
                n
            } else {
                -n
            }
        })
        .collect();
    let between = normals[0].dot(normals[1]).clamp(-1.0, 1.0);
    let between = ryukei::math::acos(between);
    let wedge = PI - between;
    let d = size / ryukei::math::tan(wedge / 2.0).max(1e-3);
    4.0 * d * d * (len + 2.0 * d)
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn corners_of_random_bodies() {
    let mut g = Lcg::new(0x636f_726e_6572);
    let (mut built, mut overflow, mut unsupported, mut other, mut unchecked) = (0, 0, 0, 0, 0);
    for case in 0..96u128 {
        let (b, small) = body(&mut g);
        let corners: Vec<[EdgeId; 3]> = b.vertices().filter_map(|v| corner_at(&b, v)).collect();
        let edges = corners[(g.next_u64() % corners.len() as u64) as usize];
        let size = g.next_f64(0.05, 0.3) * small;
        let kinds: Vec<BlendKind> = match g.next_u64() % 4 {
            // One size.
            0 => vec![BlendKind::Fillet { radius: size }; 3],
            // One larger, rolled over by the others.
            1 => vec![
                BlendKind::Fillet { radius: 2.0 * size },
                BlendKind::Fillet { radius: size },
                BlendKind::Fillet { radius: size },
            ],
            2 => vec![BlendKind::Chamfer(Chamfer::Equal { distance: size }); 3],
            // A fillet and a chamfer trim each other (three are refused).
            _ => vec![
                BlendKind::Fillet { radius: size },
                BlendKind::Chamfer(Chamfer::Equal { distance: size }),
            ],
        };
        let three = kinds.len() == 3 && !g.next_u64().is_multiple_of(3);
        let picked: Vec<EdgeId> = if three {
            edges.to_vec()
        } else {
            edges[..2].to_vec()
        };
        let blends: Vec<EdgeBlend> = picked
            .iter()
            .zip(&kinds)
            .map(|(&e, &kind)| EdgeBlend {
                edge: b.edge(e).unwrap().name,
                kind,
            })
            .collect();
        let key = OpKey(0xc0e0 + case);
        match blend(key, &b, &blends) {
            Ok((r, _)) => {
                built += 1;
                assert_eq!(check(&r), [], "case {case}");
                // Faces never cross; a pair the check can't decide (two
                // fillets of different sizes trimmed against each other,
                // whose surfaces' intersection has a node where their
                // rails cross) is rare.
                let faults = check_intersections(&r);
                assert!(
                    faults
                        .iter()
                        .all(|f| matches!(f, Fault::CannotCheck { .. })),
                    "case {case}: {faults:?}"
                );
                if !faults.is_empty() {
                    unchecked += 1;
                    eprintln!("case {case}: {faults:?}");
                }
                let (before, after) = (volume(&b), volume(&r));
                let bound: f64 = picked
                    .iter()
                    .zip(&kinds)
                    .map(|(&e, k)| match k {
                        BlendKind::Fillet { radius } => most(&b, e, *radius),
                        _ => most(&b, e, size),
                    })
                    .sum();
                assert!(after != before, "case {case}");
                assert!(
                    (after - before).abs() < bound + 1e-4,
                    "case {case}: {before} {after} {bound}"
                );
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap();
                let (one, _) = pool.install(|| blend(key, &b, &blends)).unwrap();
                assert_eq!(one.dump(), r.dump(), "case {case}");
            }
            Err(Error::InvalidBlend(
                BlendError::Overflow { .. } | BlendError::RadiusTooLarge { .. },
            )) => overflow += 1,
            Err(Error::InvalidBlend(BlendError::Unsupported { at })) => {
                unsupported += 1;
                eprintln!("case {case}: unsupported at {at}");
            }
            Err(Error::InvalidBlend(e)) => {
                other += 1;
                eprintln!("case {case}: {e:?}");
            }
            Err(e) => panic!("case {case}: {e:?}"),
        }
    }
    eprintln!(
        "built {built}, overflowed {overflow}, unsupported {unsupported}, refused {other}, unchecked {unchecked}"
    );
    assert!(built >= 64, "only {built} built");
    assert!(unchecked <= 1, "{unchecked} not checked");
    assert!(
        unsupported <= 8 && other <= 6,
        "{unsupported} {other} refused"
    );
}
