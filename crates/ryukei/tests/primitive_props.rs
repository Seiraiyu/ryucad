//! Every primitive at any placement and size is valid, and its names
//! depend only on its key.

use proptest::prelude::*;
use ryukei::Error;
use ryukei::check::check;
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::name::{Event, Journal, OpKey};
use ryukei::primitive::{block, cone, cylinder, frustum, sphere, torus};
use ryukei::topo::Body;

fn placement() -> impl Strategy<Value = Rigid> {
    let coord = || -300.0f64..300.0;
    (
        coord(),
        coord(),
        coord(),
        -1.0f64..1.0,
        -1.0f64..1.0,
        0.1f64..1.0,
        -10.0f64..10.0,
    )
        .prop_map(|(x, y, z, ax, ay, az, angle)| {
            let axis = Vector3::new(ax, ay, az).normalize().unwrap();
            Rigid::translation(Vector3::new(x, y, z)).then_after(&Rigid::rotation(axis, angle))
        })
}

fn dim() -> impl Strategy<Value = f64> {
    prop_oneof![1e-3f64..1.0, 1.0f64..100.0]
}

/// One of the primitives, by index, with dimensions drawn from `d`.
fn build(which: u8, d: [f64; 3], key: OpKey, at: &Rigid) -> Result<(Body, Journal), Error> {
    let [a, b, c] = d;
    match which % 7 {
        0 => block(key, a, b, c, at),
        1 => cylinder(key, a, b, at),
        2 => cone(key, a, b, at),
        3 => frustum(key, a, a + b, c, at),
        4 => frustum(key, a + b, a, c, at),
        5 => sphere(key, a, at),
        _ => torus(key, a + b, b, at),
    }
}

fn names(j: &Journal) -> Vec<u128> {
    j.events()
        .iter()
        .map(|e| match e {
            Event::Created(n) => n.to_u128(),
            other => panic!("a primitive reported {other:?}"),
        })
        .collect()
}

proptest! {
    #[test]
    fn primitives_are_valid_anywhere(which in 0u8..7, d in [dim(), dim(), dim()], at in placement()) {
        // Large shapes far out may not fit: that is the only failure.
        match build(which, d, OpKey(1), &at) {
            Err(e) => prop_assert_eq!(e, Error::OutsideSizeBox),
            Ok((body, journal)) => {
                prop_assert_eq!(check(&body), []);
                let (here, j0) = build(which, d, OpKey(1), &Rigid::IDENTITY).unwrap();
                prop_assert_eq!(names(&journal), names(&j0));
                prop_assert_eq!(here.counts(), body.counts());
            }
        }
    }

    #[test]
    fn same_inputs_same_body(which in 0u8..7, d in [dim(), dim(), dim()], key in any::<u128>()) {
        let at = Rigid::translation(Vector3::new(1.0, 2.0, 3.0));
        let (a, ja) = build(which, d, OpKey(key), &at).unwrap();
        let (b, jb) = build(which, d, OpKey(key), &at).unwrap();
        prop_assert_eq!(a.dump(), b.dump());
        prop_assert_eq!(&ja, &jb);
        // Another key renames everything and changes nothing else.
        let (c, jc) = build(which, d, OpKey(key ^ 1), &at).unwrap();
        prop_assert_eq!(a.dump_unnamed(), c.dump_unnamed());
        for n in names(&jc) {
            prop_assert!(!names(&ja).contains(&n));
        }
    }

    #[test]
    fn compact_keeps_names_and_validity(which in 0u8..7, d in [dim(), dim(), dim()]) {
        let (mut body, journal) = build(which, d, OpKey(5), &Rigid::IDENTITY).unwrap();
        let counts = body.counts();
        body.compact();
        prop_assert_eq!(body.counts(), counts);
        prop_assert_eq!(check(&body), []);
        for e in journal.events() {
            if let Event::Created(n) = e {
                prop_assert!(body.by_name(*n).is_some());
            }
        }
    }
}

#[test]
fn a_vertex_sits_where_expected() {
    let at = Rigid::translation(Vector3::new(1.0, 2.0, 3.0));
    let (b, _) = cylinder(OpKey(1), 2.0, 5.0, &at).unwrap();
    let points: Vec<Point3> = b.vertices().map(|v| b.vertex(v).unwrap().point).collect();
    assert_eq!(
        points,
        [Point3::new(3.0, 2.0, 3.0), Point3::new(3.0, 2.0, 8.0)]
    );
}
