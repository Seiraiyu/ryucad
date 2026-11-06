//! Scratch probe for plan 15.D (#177). Not for merging.
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::check::check_intersections;
use ryukei::geom::{Curve2, Frame, Line2, Point2, Point3, Vector3};
use ryukei::name::{Name, OpKey};
use ryukei::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use ryukei::topo::Body;
use std::time::Instant;

fn square(s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
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
    Loop { key: LoopKey(1), segments }
}

fn section(h: f64, turn: f64) -> ValidProfile {
    let (sn, c) = ryukei::math::sin_cos(turn);
    ValidProfile::new(Profile {
        frame: Frame::new(Point3::new(0.0, 0.0, h), Vector3::new(0.0, 0.0, 1.0), Vector3::new(c, sn, 0.0)).unwrap(),
        loops: vec![square(1.0)],
    })
    .unwrap()
}

fn lofted(turn: f64) -> Body {
    let opts = LoftOptions { closed: false, start: None, end: None };
    loft(OpKey(5), &[section(0.0, 0.0), section(1.0, turn)], &opts).unwrap().0
}

fn edges_between(b: &Body, z0: f64, z1: f64) -> Vec<Name> {
    b.edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            let [v0, v1] = [ed.start, ed.end].map(|v| b.vertex(v).unwrap().point.z);
            ((v0.min(v1)) - z0).abs() < 1e-9 && (v0.max(v1) - z1).abs() < 1e-9
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect()
}

fn fillets(es: &[Name], r: f64) -> Vec<EdgeBlend> {
    es.iter().map(|&edge| EdgeBlend { edge, kind: BlendKind::Fillet { radius: r } }).collect()
}

#[test]
#[ignore]
fn probe_every_edge() {
    let b = lofted(std::f64::consts::PI / 6.0);
    let all: Vec<Name> = b.edges().map(|e| b.edge(e).unwrap().name).collect();
    let t = Instant::now();
    let (out, _) = blend(OpKey(0xb0), &b, &fillets(&all, 0.05)).unwrap();
    eprintln!("blend {:.2}s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let f = check_intersections(&out);
    eprintln!("check {:.2}s: {f:?}", t.elapsed().as_secs_f64());
}

#[test]
#[ignore]
fn probe_top_rim() {
    let b = lofted(std::f64::consts::PI / 6.0);
    let top = edges_between(&b, 1.0, 1.0);
    let t = Instant::now();
    let r = blend(OpKey(0xb0), &b, &fillets(&top, 0.05));
    eprintln!("top rim {:.2}s: {:?}", t.elapsed().as_secs_f64(), r.as_ref().map(|_| ()));
    if let Ok((out, _)) = r {
        let t = Instant::now();
        let f = check_intersections(&out);
        eprintln!("check {:.2}s: {f:?}", t.elapsed().as_secs_f64());
    }
}

#[test]
#[ignore]
fn probe_one_pair() {
    use ryukei::intersect::{face_box, surface_surface};
    use ryukei::topo::FaceId;
    let b = lofted(std::f64::consts::PI / 6.0);
    let all: Vec<Name> = b.edges().map(|e| b.edge(e).unwrap().name).collect();
    let (out, _) = blend(OpKey(0xb0), &b, &fillets(&all, 0.05)).unwrap();
    let f = |i: usize| out.faces().find(|x: &FaceId| x.index() == i).unwrap();
    let (a, c) = (f(16), f(17));
    let sa = out.surface(out.face(a).unwrap().surface).unwrap();
    let sc = out.surface(out.face(c).unwrap().surface).unwrap();
    let region = face_box(&out, a).union(&face_box(&out, c));
    eprintln!("region {region:?}");
    let t = Instant::now();
    let r = surface_surface(sa, sc, &region);
    eprintln!("ss {:.2}s: {:?}", t.elapsed().as_secs_f64(), r.as_ref().map(|_| ()));
}
