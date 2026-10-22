//! Blends on seeded random primitives: random boxes, cylinders and
//! frustums, placed anywhere, with random edges blended by random
//! fillets and chamfers, some meeting at a corner. Every result is a
//! typed refusal or a body that
//! passes the checker and meshes without faults, enclosing less than the
//! body it came from (every edge here is convex) and more than the body
//! less the corners its edges cut; one thread and many give the same
//! bits. Refusals for overflow are expected (the sizes run large);
//! others must stay rare. Blending and meshing many bodies is slow in a
//! debug build, so this runs in release only (`cargo test -p ryukei
//! --release --test blend_props`).

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::check::{Fault, check, check_intersections};
use ryukei::geom::{Curve, Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId};
use ryukei::{BlendError, Error};
use ryukei_testkit::Lcg;
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

/// A random placement within a few units of the origin.
fn place(g: &mut Lcg) -> Rigid {
    let axis = loop {
        let v = Vector3::new(
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
            g.next_f64(-1.0, 1.0),
        );
        if v.length() > 0.2 {
            break v.normalize().unwrap();
        }
    };
    let t = Vector3::new(
        g.next_f64(-3.0, 3.0),
        g.next_f64(-3.0, 3.0),
        g.next_f64(-3.0, 3.0),
    );
    Rigid::translation(t).then_after(&Rigid::rotation(axis, g.next_f64(0.0, PI)))
}

/// A random primitive and its smallest dimension.
fn body(g: &mut Lcg) -> (Body, f64) {
    let at = place(g);
    match g.next_u64() % 3 {
        0 => {
            let d = [0, 1, 2].map(|_| g.next_f64(0.5, 2.0));
            let b = primitive::block(OpKey(1), d[0], d[1], d[2], &at).unwrap().0;
            (b, d[0].min(d[1]).min(d[2]))
        }
        1 => {
            let (r, h) = (g.next_f64(0.4, 1.2), g.next_f64(0.4, 2.0));
            let b = primitive::cylinder(OpKey(1), r, h, &at).unwrap().0;
            (b, r.min(h))
        }
        _ => {
            let (r0, r1, h) = (
                g.next_f64(0.6, 1.2),
                g.next_f64(0.3, 0.5),
                g.next_f64(0.5, 1.5),
            );
            let b = primitive::frustum(OpKey(1), r0, r1, h, &at).unwrap().0;
            (b, r1.min(h))
        }
    }
}

/// One edge, or two: half the time sharing no vertex, a quarter of the
/// time meeting at a corner, and whether they meet.
fn edges(g: &mut Lcg, b: &Body) -> (Vec<EdgeId>, bool) {
    let all: Vec<EdgeId> = b.edges().collect();
    let first = all[(g.next_u64() % all.len() as u64) as usize];
    let ends = |e: EdgeId| {
        let x = b.edge(e).unwrap();
        [x.start, x.end]
    };
    let pick = |g: &mut Lcg, meet: bool| -> Option<EdgeId> {
        let list: Vec<EdgeId> = all
            .iter()
            .copied()
            .filter(|&e| e != first && ends(e).iter().any(|v| ends(first).contains(v)) == meet)
            .collect();
        (!list.is_empty()).then(|| list[(g.next_u64() % list.len() as u64) as usize])
    };
    match g.next_u64() % 4 {
        0 | 1 => (vec![first], false),
        2 => match pick(g, false) {
            Some(e) => (vec![first, e], false),
            None => (vec![first], false),
        },
        _ => match pick(g, true) {
            Some(e) => (vec![first, e], true),
            None => (vec![first], false),
        },
    }
}

/// What blending an edge may cut away at most: its square corner, the
/// size each way, along its length and a size past each end.
fn corner(b: &Body, e: EdgeId, size: f64) -> f64 {
    let x = b.edge(e).unwrap();
    let c = b.curve(x.curve).unwrap();
    let len = match c {
        Curve::Circle(k) => 2.0 * PI * k.radius(),
        _ => c.point(x.interval[0]).distance(c.point(x.interval[1])),
    };
    size * size * (len + 2.0 * size)
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn blends_of_random_primitives() {
    let mut g = Lcg::new(0x626c_656e_6473);
    let (mut built, mut overflow, mut other, mut unchecked) = (0, 0, 0, 0);
    for case in 0..48 {
        let (b, small) = body(&mut g);
        let (picked, _) = edges(&mut g, &b);
        let size = g.next_f64(0.02, 0.8) * small;
        let kind = match g.next_u64() % 3 {
            0 => BlendKind::Fillet { radius: size },
            1 => BlendKind::Chamfer(Chamfer::Equal { distance: size }),
            _ => {
                let fin = b.edge_fins(picked[0]).next().unwrap();
                let face = b.loop_(b.fin(fin).unwrap().loop_).unwrap().face;
                BlendKind::Chamfer(Chamfer::Distances {
                    first: size,
                    second: size * g.next_f64(0.5, 1.0),
                    first_face: b.face(face).unwrap().name,
                })
            }
        };
        let blends: Vec<EdgeBlend> = picked
            .iter()
            .map(|&e| EdgeBlend {
                edge: b.edge(e).unwrap().name,
                kind,
            })
            .collect();
        // A second edge's chamfer names a face of the first: give it two
        // equal distances instead.
        let blends: Vec<EdgeBlend> = blends
            .into_iter()
            .enumerate()
            .map(|(i, x)| match (i, x.kind) {
                (1, BlendKind::Chamfer(Chamfer::Distances { first, .. })) => EdgeBlend {
                    kind: BlendKind::Chamfer(Chamfer::Equal { distance: first }),
                    ..x
                },
                _ => x,
            })
            .collect();
        let key = OpKey(0xb1e0 + case);
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
                let most: f64 = picked.iter().map(|&e| corner(&b, e, size)).sum();
                assert!(after < before, "case {case}: {before} {after}");
                assert!(
                    after > before - most - 1e-4,
                    "case {case}: {before} {after} {most}"
                );
                // One thread gives the same bits.
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
            Err(Error::InvalidBlend(e)) => {
                other += 1;
                eprintln!("case {case}: {e:?}");
            }
            Err(e) => panic!("case {case}: {e:?}"),
        }
    }
    eprintln!("built {built}, overflowed {overflow}, refused {other}, unchecked {unchecked}");
    assert!(built >= 16, "only {built} built");
    assert!(unchecked <= 1, "{unchecked} not checked");
    assert!(overflow >= 1, "{overflow}");
    assert!(other <= 4, "{other} refused");
}
