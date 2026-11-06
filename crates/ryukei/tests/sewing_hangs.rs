//! Race bodies 1010 (boolean seed 564: a sphere face, a cone and two
//! planes) and 1503 (boolean seed 844: a sphere face, a cylinder and a
//! plane) sewn back from their own faces, moved as `ryukei-bench`'s
//! sewing race moves them (#370). Neither call returned.
//!
//! Body 1010 sews back on main as it is. Body 1503's sphere face, once
//! sewn, has an edge ending a few micrometres from the sphere's pole and
//! swinging round it, so in the face's parameters the edge's polyline
//! crosses itself there. The mesher asked the edge for samples at the
//! crossing every round, doubling them for twelve rounds in each of shell
//! facing's five meshes, each round slower. It now asks for none where
//! both segments are already shorter than refinement's floor or their
//! edge's tube, and the call ends: shell facing has no mesh of that face,
//! so healing was `Uncertain` in about two seconds (release). Since #497
//! the edge's samples within its tube of the pole run straight into it
//! (`domain::into_pole`), so the face meshes and body 1503 sews back.

use ryukei::Error;
use ryukei::heal::{SewOptions, sew};
use ryukei::name::OpKey;
use ryukei::topo::{Body, BodyKind};
use ryukei_testkit::race::{boolean_body, cut};

/// Healing's cap: the race sews with it.
const TOLERANCE: f64 = 1e-4;

fn sewn(seed: u64, index: u64) -> (Body, Result<Vec<Body>, Error>) {
    let b = boolean_body(seed).expect("the seed's boolean");
    let sheets = cut(&b, index, TOLERANCE).expect("every face a sheet");
    let options = SewOptions {
        tolerance: TOLERANCE,
        solid: true,
    };
    let out = sew(OpKey(24), &sheets, &options).map(|x| x.0);
    (b, out)
}

#[test]
fn race_body_1010_sews_back() {
    let (b, out) = sewn(564, 1010);
    let out = out.unwrap_or_else(|e| panic!("race body 1010: {e}"));
    let faces: usize = out.iter().map(|s| s.faces().count()).sum();
    assert_eq!(faces, b.faces().count());
    for s in &out {
        assert_eq!(s.kind(), BodyKind::Solid);
    }
}

#[test]
fn race_body_1503_sews_back() {
    use ryukei::check::check;
    use ryukei::mesh::{MeshParams, check_mesh, tessellate};
    let (b, out) = sewn(844, 1503);
    let out = out.unwrap_or_else(|e| panic!("race body 1503: {e}"));
    let faces: usize = out.iter().map(|s| s.faces().count()).sum();
    assert_eq!(faces, b.faces().count());
    for s in &out {
        assert_eq!(s.kind(), BodyKind::Solid);
        assert!(check(s).is_empty(), "{:?}", check(s));
        for chord in [1e-1, 1e-2, 1e-3, 1e-4] {
            let m = tessellate(s, &MeshParams::new(chord, 0.5)).unwrap();
            assert!(m.faults.is_empty(), "{chord}: {:?}", m.faults);
            check_mesh(s, &m).unwrap();
        }
    }
}
