//! Sheet-metal parts made by extruding bent strips: a profile of straight
//! runs and arcs `t` thick, its material on the left of a path from the
//! origin along `x`, extruded along `z`. They are exact sheet metal with no
//! flange operation: each run is a flange, each arc a bend.

use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2, Vector3};
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use ryukei::topo::Body;

/// One stretch of a strip's path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Leg {
    /// Straight on by a length: on the path's side, the run between the
    /// bend lines either end.
    Run(f64),
    /// A turn by `angle` (left when positive) with inner radius `radius`.
    Turn { angle: f64, radius: f64 },
}

/// The segments of a path `offset` to the left of the strip's, as curves
/// with their parameter ranges, from the start.
fn path(legs: &[Leg], t: f64, offset: f64) -> Vec<(Curve2, f64, f64)> {
    let mut p = Point2::new(0.0, 0.0);
    let mut h = Vector2::new(1.0, 0.0);
    let left = |h: Vector2| Vector2::new(-h.y, h.x);
    let at = |p: Point2, v: Vector2| Point2::new(p.x + v.x, p.y + v.y);
    let mut out = Vec::new();
    for leg in legs {
        match *leg {
            Leg::Run(l) => {
                let line = Line2::new(at(p, left(h) * offset), h).expect("a direction");
                out.push((Curve2::Line2(line), 0.0, l));
                p = at(p, h * l);
            }
            Leg::Turn { angle, radius } => {
                let s = angle.signum();
                // The path is the outer side of a left turn, the inner of
                // a right one.
                let rp = if s > 0.0 { radius + t } else { radius };
                let c = at(p, left(h) * (s * rp));
                let x = Vector2::new(p.x - c.x, p.y - c.y) * (1.0 / rp);
                let frame = Frame2::new(c, x, s > 0.0).expect("a direction");
                let circle = Circle2::new(frame, rp - s * offset).expect("a radius");
                out.push((Curve2::Circle2(circle), 0.0, angle.abs()));
                let (sn, cs) = ryukei::math::sin_cos(angle);
                let turned = |v: Vector2| Vector2::new(v.x * cs - v.y * sn, v.x * sn + v.y * cs);
                p = at(c, turned(x) * rp);
                h = turned(h);
            }
        }
    }
    out
}

/// A strip `t` thick along `legs`, extruded `w` along `z`, keyed `key`.
pub fn strip(key: OpKey, legs: &[Leg], t: f64, w: f64) -> Body {
    let bottom = path(legs, t, 0.0);
    let top = path(legs, t, t);
    let mut segments = Vec::new();
    let mut push = |c: Curve2, t0: f64, t1: f64| {
        let key = SegmentKey(segments.len() as u128 + 1);
        segments.push(Segment {
            key,
            curve: c,
            t0,
            t1,
        });
    };
    for (c, t0, t1) in &bottom {
        push(c.clone(), *t0, *t1);
    }
    let end = |p: &[(Curve2, f64, f64)]| {
        let (c, _, e) = p.last().expect("a leg");
        c.point(*e)
    };
    let (a, b) = (end(&bottom), end(&top));
    push(
        Curve2::Line2(Line2::through(a, b).expect("apart")),
        0.0,
        a.distance(b),
    );
    for (c, t0, t1) in top.iter().rev() {
        push(c.clone(), *t1, *t0);
    }
    let start = top[0].0.point(top[0].1);
    push(
        Curve2::Line2(Line2::through(start, Point2::new(0.0, 0.0)).expect("apart")),
        0.0,
        t,
    );
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .expect("a strip");
    extrude(key, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, w)
        .expect("an extrusion")
        .0
}
