//! Bit-exact shapes on every platform and CPU level: `tests/golden/board.txt`
//! holds the hashes of seeded symbols' shapes, seeded features' shapes, and
//! placements at awkward angles. After a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryu-board --test determinism` and review
//! the diff like any other change.

use ryu_board::geom::{Mdeg, Place, Point};
use ryu_board::model::Library;
use ryu_board::shape::{feature_shape, symbol_shape};
use ryu_board::testing::{Golden, Lcg, seeded_features, seeded_symbol};

#[test]
fn shapes_are_bit_exact() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/board.txt"
    ));
    {
        let mut s = golden.section("symbols");
        let mut g = Lcg::new(0x676f_6c64);
        for _ in 0..1000 {
            s.shape(&symbol_shape(&seeded_symbol(&mut g)));
        }
    }
    {
        let mut s = golden.section("features");
        let mut g = Lcg::new(0x6665_6174);
        let f = seeded_features(&mut g, 64, 2000, 50_000_000);
        let lib = Library::default();
        for i in 0..f.features().len() {
            s.shape(feature_shape(&f, i, &lib));
        }
    }
    {
        let mut s = golden.section("placements");
        let mut g = Lcg::new(0x706c_6163);
        for _ in 0..10_000 {
            let p = Place {
                at: Point::new(g.range(-1 << 40, 1 << 39), g.range(-1 << 39, 1 << 39)),
                rot: Mdeg::new(g.range(0, 359_999)),
                mirror: g.coin(),
                scale: if g.coin() {
                    Some(g.range(1, 3_000_000))
                } else {
                    None
                },
            };
            let q = p.point(Point::new(
                g.range(-1 << 38, 1 << 38),
                g.range(-1 << 38, 1 << 38),
            ));
            s.i64(q.x);
            s.i64(q.y);
        }
    }
    golden.finish();
}
