//! ryukei phase 23's twin of a partial blend delete (design 919, plan
//! 23.C): a slab [`LENGTH`] long and [`HEIGHT`] high, extruded
//! [`WIDTH`], with a disc of radius [`DISC`] floating [`GAP`] above its
//! top, joined to it by two concave fillets of radius [`BRIDGE`] (each
//! rolling on the slab's top and the disc's cylinder), and the slab's
//! bottom -x edge rounded [`FILLET`]. A bridge's supports never meet:
//! the top's plane passes below the disc, so deleting a bridge refuses.
//! A partial delete of the blends up to [`MAX_SIZE`] deletes the small
//! fillet and keeps both bridges. (The disc reads as a fillet too, rolling
//! on the two bridges, larger than `MAX_SIZE`.)
//!
//! The plan's first twin, a slab's full round, is no blend to ryukei:
//! recognition takes no fillet turning within `FULL_ROUND` of a half turn
//! (a slot's round end), so neither it nor the plan's slot was usable.

use core::f64::consts::PI;
use ryukei::Error;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Vector2, Vector3};
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use ryukei::topo::{Body, FaceId};

/// The slab's length along x.
pub const LENGTH: f64 = 0.06;
/// The slab's height along y, the profile's v.
pub const HEIGHT: f64 = 0.01;
/// The extrusion's depth along z.
pub const WIDTH: f64 = 0.04;
/// The disc's radius.
pub const DISC: f64 = 0.01;
/// How far the disc floats above the slab's top.
pub const GAP: f64 = 0.002;
/// The two concave fillets' radius (more than half the gap, so they
/// reach the disc).
pub const BRIDGE: f64 = 0.004;
/// The small fillet's radius.
pub const FILLET: f64 = 0.002;
/// The largest blend the twin's deletes take: the bridges and the
/// fillet, not the disc.
pub const MAX_SIZE: f64 = 0.005;

/// The disc's centre, in the profile.
fn disc_centre() -> Point2 {
    Point2::new(LENGTH / 2.0, HEIGHT + GAP + DISC)
}

/// How far each bridge's centre lies from the disc's along x: each is
/// `BRIDGE` above the top and `DISC + BRIDGE` from the disc's centre.
fn reach() -> f64 {
    let rise = DISC + GAP - BRIDGE;
    ryukei::math::sqrt((DISC + BRIDGE) * (DISC + BRIDGE) - rise * rise)
}

/// The profile's arcs measure their angle from straight down.
fn down() -> Vector2 {
    Vector2::new(0.0, -1.0)
}

/// The angle, from straight down, of direction `d` in a frame turning
/// `ccw` (its y towards +u) or not (towards -u).
fn angle(d: Vector2, ccw: bool) -> f64 {
    let across = if ccw { d.x } else { -d.x };
    ryukei::math::atan2(across, -d.y)
}

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

fn line(key: u128, p: Point2, q: Point2) -> Result<Segment, Error> {
    Ok(seg(
        key,
        Curve2::Line2(Line2::through(p, q)?),
        0.0,
        p.distance(q),
    ))
}

/// The slab and the disc with their bridges, before the small fillet.
pub fn bridged() -> Body {
    let c = disc_centre();
    let s = reach();
    let rise = DISC + GAP - BRIDGE;
    let right = Point2::new(c.x + s, HEIGHT + BRIDGE);
    let left = Point2::new(c.x - s, HEIGHT + BRIDGE);
    // Where each bridge meets the disc, from the disc's centre and from
    // the bridge's.
    let to_right = Vector2::new(s, -rise);
    let to_left = Vector2::new(-s, -rise);
    let frame = |o: Point2, ccw: bool| Frame2::new(o, down(), ccw).expect("a frame");
    let arc = |o: Point2, r: f64, ccw: bool| {
        Curve2::Circle2(Circle2::new(frame(o, ccw), r).expect("a circle"))
    };
    let k = 0x23c70;
    let segments = vec![
        line(k, Point2::new(0.0, 0.0), Point2::new(LENGTH, 0.0)),
        line(k + 1, Point2::new(LENGTH, 0.0), Point2::new(LENGTH, HEIGHT)),
        line(
            k + 2,
            Point2::new(LENGTH, HEIGHT),
            Point2::new(c.x + s, HEIGHT),
        ),
        // The right bridge, clockwise from its foot to the disc.
        Ok(seg(
            k + 3,
            arc(right, BRIDGE, false),
            0.0,
            angle(Vector2::new(-to_right.x, -to_right.y), false),
        )),
        // The disc over its top, from the right bridge to the left.
        Ok(seg(
            k + 4,
            arc(c, DISC, true),
            angle(to_right, true),
            2.0 * PI + angle(to_left, true),
        )),
        // The left bridge, from the disc down to its foot.
        Ok(seg(
            k + 5,
            arc(left, BRIDGE, true),
            angle(Vector2::new(-to_left.x, -to_left.y), true),
            0.0,
        )),
        line(
            k + 6,
            Point2::new(c.x - s, HEIGHT),
            Point2::new(0.0, HEIGHT),
        ),
        line(k + 7, Point2::new(0.0, HEIGHT), Point2::new(0.0, 0.0)),
    ];
    let profile = Profile {
        frame: Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .expect("a frame"),
        loops: vec![Loop {
            key: LoopKey(1),
            segments: segments
                .into_iter()
                .collect::<Result<_, _>>()
                .expect("segments"),
        }],
    };
    let valid = ValidProfile::new(profile).expect("the profile");
    extrude(
        OpKey(0x23c7),
        &valid,
        Vector3::new(0.0, 0.0, 1.0),
        0.0,
        WIDTH,
    )
    .expect("the extrusion")
    .0
}

/// The bridged slab with its bottom -x edge rounded [`FILLET`].
pub fn body() -> Body {
    let b = bridged();
    // The bottom and the -x end, flat at y = 0 and at x = 0.
    let flat = |f: FaceId, y: bool| {
        let bx = ryukei::intersect::face_box(&b, f);
        let (lo, hi) = (bx.min(), bx.max());
        if y {
            lo.y.abs() < 1e-9 && hi.y.abs() < 1e-9
        } else {
            lo.x.abs() < 1e-9 && hi.x.abs() < 1e-9
        }
    };
    let face = |fin| {
        let l = b.fin(fin).expect("a fin").loop_;
        b.loop_(l).expect("a loop").face
    };
    let e = b
        .edges()
        .find(|&e| {
            let fs: Vec<FaceId> = b.edge_fins(e).map(face).collect();
            fs.iter().any(|&f| flat(f, true)) && fs.iter().any(|&f| flat(f, false))
        })
        .expect("the bottom's -x edge");
    let edge = b.edge(e).expect("an edge").name;
    blend(
        OpKey(0x23ca),
        &b,
        &[EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius: FILLET },
        }],
    )
    .expect("the fillet")
    .0
}

/// The volume once the small fillet is deleted and the bridges kept: the
/// bridged slab's, from its profile's area. The slab, the disc less the
/// cap the bridges hide under it, and the two bridges' regions (each a
/// trapezoid from the top to the disc less the bridge's sector).
pub fn kept_volume() -> f64 {
    let s = reach();
    let rise = DISC + GAP - BRIDGE;
    // A bridge's sector angle: from straight down round to the disc.
    let bridge_turn = PI - ryukei::math::atan2(s, rise);
    // The disc's arc runs round its top between the two contacts.
    let contact = ryukei::math::atan2(s, rise);
    let disc_arc = 2.0 * PI - 2.0 * contact;
    // Area by the shoelace over the profile's arcs and lines: the region
    // above the top line, under the disc's arc and the bridges, is the
    // polygon (left foot, left centre, disc centre, right centre, right
    // foot) plus the disc's sector less the bridges' sectors.
    let c = disc_centre();
    let poly = [
        (c.x - s, HEIGHT),
        (c.x + s, HEIGHT),
        (c.x + s, HEIGHT + BRIDGE),
        (c.x, c.y),
        (c.x - s, HEIGHT + BRIDGE),
    ];
    let mut twice = 0.0;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        twice += a.0 * b.1 - b.0 * a.1;
    }
    let above = twice.abs() / 2.0 + 0.5 * DISC * DISC * disc_arc
        - 2.0 * 0.5 * BRIDGE * BRIDGE * bridge_turn;
    (LENGTH * HEIGHT + above) * WIDTH
}
