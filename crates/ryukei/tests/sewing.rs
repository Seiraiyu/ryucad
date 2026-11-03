//! Sewing race bodies back from their faces, as `ryukei-bench`'s sewing
//! race cuts them (`ryukei_testkit::race::cut`), for the cases it found.

use ryukei::heal::{SewOptions, sew};
use ryukei::name::OpKey;
use ryukei::topo::BodyKind;
use ryukei_testkit::race;

/// Healing's cap: the race sews with it.
const TOLERANCE: f64 = 1e-4;

/// Race body 1218 (blend seed 675) has two edges about 1e-5 m long,
/// shorter than twice the gap its moved faces leave: sewing must still
/// pair them, once their neighbours have joined their ends, and give the
/// body back whole, not as a sheet with four laminar edges (#374).
#[test]
fn race_body_1218_sews_back_whole() {
    let b = race::blend_body(675).expect("seed 675 blends");
    let sheets = race::cut(&b, 1218, TOLERANCE).expect("every face a sheet");
    let options = SewOptions {
        tolerance: TOLERANCE,
        solid: true,
    };
    let (out, _) = sew(OpKey(24), &sheets, &options).unwrap();
    assert!(!out.is_empty());
    for s in &out {
        assert_eq!(s.kind(), BodyKind::Solid, "{:?}", s.counts());
    }
    let faces: usize = out.iter().map(|s| s.faces().count()).sum();
    assert_eq!(faces, b.faces().count());
}
