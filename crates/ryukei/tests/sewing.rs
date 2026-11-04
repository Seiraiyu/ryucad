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

/// Sews race body `index` from its moved faces and
/// requires solids holding every face.
fn sews_back(index: u64, b: &ryukei::topo::Body) {
    let sheets = race::cut(b, index, TOLERANCE).expect("every face a sheet");
    let options = SewOptions {
        tolerance: TOLERANCE,
        solid: true,
    };
    let (out, _) =
        sew(OpKey(24), &sheets, &options).unwrap_or_else(|e| panic!("race body {index}: {e}"));
    for s in &out {
        assert_eq!(s.kind(), BodyKind::Solid, "{index}: {:?}", s.counts());
    }
    let faces: usize = out.iter().map(|s| s.faces().count()).sum();
    assert_eq!(faces, b.faces().count(), "race body {index}");
}

/// Race bodies whose loops cross a seam or a pole of a periodic face at a
/// vertex (#348): moved, the vertex lies microns off the seam or the pole
/// (within its tolerance, past resolution), so no point of the loop is on
/// the seam where it crosses, or the loop passes beside the pole and
/// seems to wind round it. On 300 (boolean seed 165) a cone face's loop
/// that winds round nothing spans two strips; on 574 (boolean seed 318) a
/// cone's loop winding round has no seam point at all; on 990 (boolean
/// seed 552) a sphere face's loop through its pole winds once. The
/// mesher refused the face at every chord, so shell facing had no volume
/// sign. Its second try now moves the seam off the face's vertices, and
/// puts a vertex within its tolerance of a pole on the pole.
#[test]
fn loops_crossed_at_a_moved_vertex_sew_back() {
    for (index, seed) in [(300, 165), (574, 318), (990, 552)] {
        let b = race::boolean_body(seed).expect("the seed builds");
        sews_back(index, &b);
    }
}

/// Race body 482 (blend seed 267) has a ruled chamfer of 1,240 spans
/// whose rails nearly meet: the normal's turn bound there is loose, and
/// the mesh estimate put 14.5 million triangles on a face that meshes in
/// a few hundred, past shell facing's budget (#371).
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in a debug build: run it in release")]
fn race_body_482_sews_back() {
    let b = race::blend_body(267).expect("seed 267 blends");
    sews_back(482, &b);
}
