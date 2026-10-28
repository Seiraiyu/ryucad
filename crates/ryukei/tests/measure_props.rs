//! Mass properties against the boolean identity
//! `vol(A) + vol(B) = vol(A ∪ B) + vol(A ∩ B)`, and the first moments
//! likewise, over seeded overlapping primitives: every sum holds within
//! the sum of the four bounds. Release only (minutes in a debug build):
//! `cargo test -p ryukei --release --test measure_props`.

use ryukei::Error;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::measure::{MassOptions, MassProperties, mass};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;

fn solid(g: &mut Lcg, k: u128) -> Body {
    let at = Rigid::translation(Vector3::new(
        g.next_f64(-0.3, 0.3),
        g.next_f64(-0.3, 0.3),
        g.next_f64(-0.3, 0.3),
    ));
    let s = g.next_f64(0.4, 1.0);
    let key = OpKey(k);
    match g.next_u64() % 3 {
        0 => primitive::block(key, s, s * 0.8, s * 0.6, &at),
        1 => primitive::cylinder(key, s / 2.0, s, &at),
        _ => primitive::sphere(key, s / 2.0, &at),
    }
    .unwrap()
    .0
}

fn op(op: BooleanOp, a: &Body, b: &Body) -> Result<Body, Error> {
    let tool = Tool {
        key: ToolKey(1),
        body: b,
        place: Rigid::IDENTITY,
    };
    boolean(OpKey(50), op, a, &[tool]).map(|(b, _)| b)
}

fn weigh(b: &Body) -> MassProperties {
    mass(b, 1.0, &MassOptions::default()).unwrap()
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn volumes_add_up_through_unions_and_intersections() {
    let mut g = Lcg::new(0x6d61_7373);
    let mut checked = 0;
    for k in 0..24u128 {
        let (a, b) = (solid(&mut g, 2 * k + 1), solid(&mut g, 2 * k + 2));
        let (Ok(u), Ok(i)) = (
            op(BooleanOp::Unite, &a, &b),
            op(BooleanOp::Intersect, &a, &b),
        ) else {
            continue;
        };
        if i.faces().next().is_none() {
            continue;
        }
        let [ma, mb, mu, mi] = [&a, &b, &u, &i].map(weigh);
        let lhs = ma.volume.value() + mb.volume.value();
        let rhs = mu.volume.value() + mi.volume.value();
        let bound = [&ma, &mb, &mu, &mi]
            .iter()
            .map(|m| m.volume.bound())
            .sum::<f64>();
        assert!(
            (lhs - rhs).abs() <= bound,
            "case {k}: {lhs} against {rhs} ± {bound}"
        );
        // The quadrature is far better than its bound.
        assert!(
            (lhs - rhs).abs() <= 1e-7 * lhs,
            "case {k}: {lhs} against {rhs}"
        );
        let first = |m: &MassProperties| m.centre.to_vector() * m.volume.value();
        let d = first(&ma) + first(&mb) - first(&mu) - first(&mi);
        assert!(
            d.length() <= 1e-7 * lhs,
            "case {k}: first moments differ by {d:?}"
        );
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} cases built");
}
