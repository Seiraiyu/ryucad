//! Every predicate against the exact rational reference. Inputs are raw
//! f64 bit patterns, kept only when finite and inside the size box.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryukei::geom::{Point2, Point3};
use ryukei::predicates::{Sign, incircle, insphere, orient2d, orient3d};
use ryukei::tol::SIZE_BOX_HALF;
use ryukei_testkit::exact;
use std::cmp::Ordering;

fn sign(o: Ordering) -> Sign {
    match o {
        Ordering::Less => Sign::Negative,
        Ordering::Equal => Sign::Zero,
        Ordering::Greater => Sign::Positive,
    }
}

fuzz_target!(|coords: [f64; 15]| {
    if !coords.iter().all(|c| c.is_finite() && c.abs() <= SIZE_BOX_HALF) {
        return;
    }
    let p3: Vec<Point3> = coords.chunks(3).map(|c| Point3::new(c[0], c[1], c[2])).collect();
    let p2: Vec<Point2> = coords.chunks(2).take(4).map(|c| Point2::new(c[0], c[1])).collect();
    let a3: Vec<[f64; 3]> = p3.iter().map(|p| p.to_array()).collect();
    let a2: Vec<[f64; 2]> = p2.iter().map(|p| [p.x, p.y]).collect();

    assert_eq!(orient2d(p2[0], p2[1], p2[2]), sign(exact::orient2d(a2[0], a2[1], a2[2])));
    assert_eq!(
        incircle(p2[0], p2[1], p2[2], p2[3]),
        sign(exact::incircle(a2[0], a2[1], a2[2], a2[3]))
    );
    assert_eq!(
        orient3d(p3[0], p3[1], p3[2], p3[3]),
        sign(exact::orient3d(a3[0], a3[1], a3[2], a3[3]))
    );
    assert_eq!(
        insphere(p3[0], p3[1], p3[2], p3[3], p3[4]),
        sign(exact::insphere(a3[0], a3[1], a3[2], a3[3], a3[4]))
    );
});
