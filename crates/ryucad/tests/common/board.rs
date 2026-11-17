//! A seeded board as STEP (#813, design 698): `distinct` components from
//! `ryukei_testkit::race`, each its own product, placed `copies` times in
//! all under one root (placement `k` uses component `k % distinct`), each
//! turned about all three axes, written by ryustep in millimetres. Race
//! bodies a STEP round trip refuses are left out, as `step_bar` does.

use ryukei::geom::{Rigid, UnitVector3, Vector3};
use ryustep::{Child, LengthUnit, Product, ReadOptions, WriteModel, WriteOptions};

/// Placement `k`: on a grid 40 wide and 10 m apart about the origin
/// (1,000 placements stand within ±200 m), turned about x, then y, then
/// z by angles that differ between neighbours.
pub fn place(k: usize) -> Rigid {
    let turn = |axis: UnitVector3, step: usize, n: usize| {
        Rigid::rotation(axis, 0.3 + 0.7 * ((k * step) % n) as f64)
    };
    let r = turn(UnitVector3::Z, 1, 7)
        .then_after(&turn(UnitVector3::Y, 3, 5).then_after(&turn(UnitVector3::X, 5, 11)));
    let t = Rigid::translation(Vector3::new(
        10.0 * (k % 40) as f64 - 200.0,
        10.0 * (k / 40) as f64 - 125.0,
        0.0,
    ));
    t.then_after(&r)
}

fn model(bodies: &[ryukei::topo::Body], copies: usize) -> WriteModel {
    let mut products: Vec<Product> = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| Product {
            id: format!("c{i}"),
            name: format!("c{i}"),
            bodies: vec![b.clone()],
            ..Default::default()
        })
        .collect();
    let children = (0..copies)
        .map(|k| Child {
            product: k % bodies.len(),
            place: place(k),
            id: String::new(),
        })
        .collect();
    products.push(Product {
        id: "root".into(),
        name: "root".into(),
        children,
        ..Default::default()
    });
    WriteModel { products }
}

fn options() -> WriteOptions {
    WriteOptions {
        unit: LengthUnit::Millimeter,
        time_stamp: "2026-11-17T00:00:00".into(),
        ..WriteOptions::default()
    }
}

/// The board's file.
pub fn board(distinct: usize, copies: usize) -> Vec<u8> {
    let mut bodies = ryukei_testkit::race::writable(distinct + 8);
    let first = ryustep::write(&model(&bodies, bodies.len()), &options()).unwrap();
    let read = ryustep::read(
        &first,
        &ReadOptions {
            key: ryukei::name::OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let refused: Vec<String> = read
        .bodies
        .iter()
        .filter(|b| b.result.is_err())
        .filter_map(|b| b.definition)
        .map(|d| read.products.definitions[d].product_id.to_string())
        .collect();
    let mut i = 0;
    bodies.retain(|_| {
        i += 1;
        !refused.contains(&format!("c{}", i - 1))
    });
    assert!(bodies.len() >= distinct, "{} of the race write", bodies.len());
    bodies.truncate(distinct);
    ryustep::write(&model(&bodies, copies), &options()).unwrap()
}
