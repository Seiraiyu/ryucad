//! Race body 1573 (boolean seed 885: a cylinder, and a lens of two
//! spheres whose rim circle passes 4 cm from the smaller sphere's poles)
//! sewn back from its own faces, as `ryukei-bench`'s sewing race cuts it
//! (#373).

use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{SewOptions, sew};
use ryukei::name::OpKey;
use ryukei::topo::{Body, BodyKind};
use ryukei_testkit::Lcg;

/// Healing's cap: the race sews with it.
const TOLERANCE: f64 = 1e-4;

fn body() -> Body {
    let mut g = Lcg::new(885);
    let bytes: Vec<u8> = (0..96).map(|_| (g.next_u64() >> 56) as u8).collect();
    let case = ryukei_testkit::boolean_bytes::boolean_case(&bytes).expect("seed 885's case");
    ryukei::boolean::boolean(OpKey(9), case.op, &case.target, &case.tools())
        .expect("seed 885's boolean")
        .0
}

/// `b`'s faces as sheets, each moved by up to `scale` of a tenth of the
/// cap along a direction of its own, as the race cuts body `seed`.
fn cut(b: &Body, seed: u64, scale: f64) -> Vec<Body> {
    let mut g = Lcg::new(seed);
    let mut unit = || (g.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    b.faces()
        .map(|f| {
            let s = ryukei::sheet::extract(OpKey(23), b, &[b.face(f).unwrap().name])
                .unwrap()
                .0;
            let dir = Vector3::new(unit() - 0.5, unit() - 0.5, unit() - 0.5);
            let by = dir.normalize().map_or(Vector3::new(0.0, 0.0, 0.0), |d| {
                d.get() * (0.1 * TOLERANCE * scale * unit())
            });
            s.transformed(&Rigid::translation(by)).unwrap()
        })
        .collect()
}

/// Unmoved, the rim's copies are one curve, and the use sewing renames
/// keeps its pcurve; moved, healing tightens the rim onto the spheres'
/// circle, the closed edge taking the branch's own range, where its
/// clamped pcurves fit. Either way no pcurve is fitted afresh round the
/// poles, which fails (`NoPcurve` before the fix).
#[test]
fn race_body_1573_sews_back_moved_or_not() {
    let b = body();
    let options = SewOptions {
        tolerance: TOLERANCE,
        solid: true,
    };
    for scale in [0.0, 0.01, 1.0] {
        let (out, _) = sew(OpKey(24), &cut(&b, 1573, scale), &options)
            .unwrap_or_else(|e| panic!("moved by {scale}: {e}"));
        assert_eq!(out.len(), 2, "moved by {scale}");
        for s in &out {
            assert_eq!(s.kind(), BodyKind::Solid, "moved by {scale}");
        }
        let faces: usize = out.iter().map(|s| s.faces().count()).sum();
        assert_eq!(faces, b.faces().count(), "moved by {scale}");
    }
}
