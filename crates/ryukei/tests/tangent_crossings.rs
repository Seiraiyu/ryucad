//! Tangent crossings of pipes on curved splines (issue #177, plan 15.D):
//! the fillets of a twisted loft's cap edges are pipes footed on the
//! caps, and two of them meet at a ball corner touching tangentially at
//! the corner triangle's vertex and again where their tubes' tops meet.
//! Release only (`cargo test -p ryukei --release --test
//! tangent_crossings`).

use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::geom::{Curve2, Frame, Line2, Point2, Point3, Surface, Vector3};
use ryukei::intersect::{SurfaceHit, face_box, surface_surface};
use ryukei::name::{Name, OpKey};
use ryukei::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use ryukei::topo::{Body, FaceId};
use std::f64::consts::PI;

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
    let (s, c) = ryukei::math::sin_cos(turn);
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

/// The twisted loft with every edge filleted at 0.05, as
/// `spline_blends.rs` makes it.
fn filleted() -> Body {
    let opts = LoftOptions {
        closed: false,
        start: None,
        end: None,
    };
    let b = loft(
        OpKey(5),
        &[section(0.0, 0.0), section(1.0, PI / 6.0)],
        &opts,
    )
    .unwrap()
    .0;
    let all: Vec<Name> = b.edges().map(|e| b.edge(e).unwrap().name).collect();
    let blends: Vec<EdgeBlend> = all
        .iter()
        .map(|&edge| EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius: 0.05 },
        })
        .collect();
    blend(OpKey(0xb0), &b, &blends).unwrap().0
}

/// Pairs of footed pipe faces (cap-edge fillets) sharing a vertex.
fn footed_pairs(b: &Body) -> Vec<(FaceId, FaceId)> {
    let footed = |f: FaceId| matches!(b.surface(b.face(f).unwrap().surface), Some(Surface::Pipe(p)) if p.rail().is_none());
    let vertices = |f: FaceId| -> Vec<_> {
        b.face_loops(f)
            .flat_map(|l| {
                b.loop_fins(l)
                    .filter_map(|x| b.fin_start(x))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let faces: Vec<FaceId> = b.faces().filter(|&f| footed(f)).collect();
    let mut out = Vec::new();
    for (i, &x) in faces.iter().enumerate() {
        for &y in &faces[i + 1..] {
            let vy = vertices(y);
            if vertices(x).iter().any(|v| vy.contains(v)) {
                out.push((x, y));
            }
        }
    }
    out
}

#[test]
#[ignore = "until Task 5 (#177)"]
fn footed_pipes_at_a_ball_corner_meet_through_their_tangent_points() {
    let b = filleted();
    let pairs = footed_pairs(&b);
    assert_eq!(pairs.len(), 8, "{pairs:?}");
    for (x, y) in pairs {
        let [sx, sy] = [x, y].map(|f| b.surface(b.face(f).unwrap().surface).unwrap());
        let region = face_box(&b, x).union(&face_box(&b, y));
        let hit = surface_surface(sx, sy, &region).unwrap_or_else(|e| panic!("{x:?} {y:?}: {e:?}"));
        let SurfaceHit::Meet { branches, .. } = hit else {
            panic!("{x:?} {y:?}: coincident")
        };
        assert!(!branches.is_empty(), "{x:?} {y:?}");
        // Every branch lies on both surfaces.
        for br in &branches {
            for i in 0..=16 {
                let t = br.range[0] + (br.range[1] - br.range[0]) * i as f64 / 16.0;
                let p = br.curve.point(t);
                for s in [sx, sy] {
                    let d = s.project(p).point.distance(p);
                    assert!(d <= br.tolerance + 1e-9, "{x:?} {y:?}: {d}");
                }
            }
        }
    }
}
