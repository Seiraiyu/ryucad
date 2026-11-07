//! Fixtures for the crate's own unit tests.

use crate::blend::{BlendKind, EdgeBlend, blend};
use crate::geom::{Box3, Curve2, Frame, Line2, Point2, Point3, Surface, Vector3};
use crate::intersect::face_box;
use crate::name::{Name, OpKey};
use crate::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use crate::topo::{Body, FaceId};
use std::f64::consts::PI;
use std::sync::OnceLock;

fn square() -> Loop {
    let pts = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    Loop {
        key: LoopKey(1),
        segments,
    }
}

fn section(h: f64, turn: f64) -> ValidProfile {
    let (s, c) = crate::math::sin_cos(turn);
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(c, s, 0.0),
        )
        .unwrap(),
        loops: vec![square()],
    })
    .unwrap()
}

/// The twisted loft of `spline_blends.rs` (a unit square turned a
/// twelfth of a turn over a unit height), unblended.
pub(crate) fn twisted_loft() -> Body {
    let opts = LoftOptions {
        closed: false,
        start: None,
        end: None,
    };
    loft(
        OpKey(5),
        &[section(0.0, 0.0), section(1.0, PI / 6.0)],
        &opts,
    )
    .unwrap()
    .0
}

/// The twisted loft with every edge filleted at 0.05 under `OpKey(0xb0)`,
/// built once.
pub(crate) fn filleted_loft() -> &'static Body {
    static BODY: OnceLock<Body> = OnceLock::new();
    BODY.get_or_init(|| {
        let b = twisted_loft();
        let all: Vec<Name> = b.edges().map(|e| b.edge(e).unwrap().name).collect();
        let blends: Vec<EdgeBlend> = all
            .iter()
            .map(|&edge| EdgeBlend {
                edge,
                kind: BlendKind::Fillet { radius: 0.05 },
            })
            .collect();
        blend(OpKey(0xb0), &b, &blends).unwrap().0
    })
}

fn corner_faces() -> [FaceId; 2] {
    let b = filleted_loft();
    [16, 17].map(|i| b.faces().find(|f| f.index() == i).unwrap())
}

/// The surfaces of faces 16 and 17 of [`filleted_loft`]: the fillets of two
/// bottom cap edges, pipes footed on the cap, meeting at a ball corner
/// (issue #177).
pub(crate) fn footed_corner_pair() -> (Surface, Surface) {
    let b = filleted_loft();
    let [x, y] = corner_faces().map(|f| b.surface(b.face(f).unwrap().surface).unwrap().clone());
    for s in [&x, &y] {
        assert!(matches!(s, Surface::Pipe(p) if p.rail().is_none()), "{s:?}");
    }
    (x, y)
}

/// The union of [`footed_corner_pair`]'s faces' boxes.
pub(crate) fn footed_corner_region() -> Box3 {
    let b = filleted_loft();
    let [x, y] = corner_faces();
    face_box(b, x).union(&face_box(b, y))
}
