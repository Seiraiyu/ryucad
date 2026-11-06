//! References (design 364, plan 21.A) of the primitives and seeded
//! sweeps, booleans and blends, written as a file must describe them:
//! each a sheet whose topology holds, meshed with no fault, named as the
//! healed body is.

use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::check_topology;
use ryukei::geom::{Box3, Rigid, Vector3};
use ryukei::heal::{HealOptions, ReferenceOptions, heal, reference, seamed};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{Name, OpKey};
use ryukei::primitive;
use ryukei::topo::{Body, BodyKind};
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;
use std::collections::BTreeSet;

/// The bodies the `heal` golden section heals.
pub fn bodies() -> Vec<Body> {
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut out = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ];
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    for seed in 0..12 {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            out.push(b);
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            out.push(b);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
        {
            out.push(b);
        }
    }
    out
}

/// A body's size: the box of points along its edges.
fn size(b: &Body) -> f64 {
    // Nine points along each edge (a face bounded by one closed edge has
    // one vertex, so its vertices' box is a point), else the faces' boxes
    // for a body with no edge (a whole sphere): a face's box can be the
    // whole size box.
    let along = b.edges().flat_map(|e| {
        let edge = b.edge(e).unwrap();
        let curve = b.curve(edge.curve).unwrap();
        let [t0, t1] = edge.interval;
        (0..=8).map(move |i| curve.point(t0 + (t1 - t0) * f64::from(i) / 8.0))
    });
    Box3::from_points(along)
        .or_else(|| {
            b.faces()
                .map(|f| ryukei::intersect::face_box(b, f))
                .reduce(|x, y| x.union(&y))
        })
        .map(|x| (x.max() - x.min()).length())
        .unwrap()
}

/// A mesh's chord for a body: a hundredth of its size, at least what
/// [`MeshParams`] takes.
fn chord(b: &Body) -> f64 {
    (size(b) * 1e-2).max(10.0 * ryukei::tol::LINEAR_RESOLUTION)
}

fn faces(b: &Body) -> BTreeSet<Name> {
    b.faces().map(|f| b.face(f).unwrap().name).collect()
}

#[test]
fn seeded_bodies_build_as_references_named_as_healed() {
    for (i, b) in bodies().iter().enumerate() {
        // A boolean that leaves nothing: healing gives the empty body, and a
        // reference refuses a shape with no face to lay out.
        if b.faces().next().is_none() {
            continue;
        }
        let shape = seamed(b, 1e-7).unwrap();
        let (r, report) = reference(OpKey(3), &shape, &ReferenceOptions::default())
            .unwrap_or_else(|e| panic!("body {i}: {e}"));
        assert_eq!(check_topology(&r), [], "body {i}");
        assert_eq!(r.kind(), BodyKind::Sheet, "body {i}");
        // A file's own bodies: closed, nothing split, dropped or unoriented.
        assert_eq!(report.laminar, [], "body {i}");
        assert_eq!(report.split_faces, [], "body {i}");
        assert_eq!(report.dropped_faces, [], "body {i}");
        assert_eq!(report.unoriented, [], "body {i}");
        let m = tessellate(&r, &MeshParams::new(chord(b), 0.5)).unwrap();
        assert_eq!(m.faults, [], "body {i}");
        let (h, _, healed) = heal(OpKey(3), &shape, &HealOptions::default()).unwrap();
        assert_eq!(faces(&r), faces(&h), "body {i}");
        assert_eq!(report.seams, healed.seams, "body {i}");
    }
}

#[test]
fn a_reference_is_the_same_on_one_thread_and_four() {
    let b = &bodies()[4];
    let shape = seamed(b, 1e-7).unwrap();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| reference(OpKey(3), &shape, &ReferenceOptions::default()).unwrap())
    };
    let (a, ra) = on(1);
    let (b, rb) = on(4);
    assert_eq!(a.dump(), b.dump());
    assert_eq!(ra, rb);
}
