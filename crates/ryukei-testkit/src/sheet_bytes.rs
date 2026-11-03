//! Sheets from bytes, for the `sheet` fuzz target and for replaying what
//! it finds (design 295, plan 12.B): faces taken out of a body
//! [`crate::blend_bytes::body`] makes (`sheet::extract` and `remove`), a
//! surface of one of its faces bounded by a box of its parameters
//! (`sheet::from_surface`), a profile's planar faces (`sweep::planar`),
//! or open chains (beside a loop, now and then) extruded or revolved as a
//! surface; or (plan 12.C) a body's faces, each its own sheet moved apart
//! by a fraction of the tolerance, sewn back as a closed or open sheet
//! (`heal::sew`); or (plan 12.D) a plane sheet across a body trimmed by
//! it (`boolean` with `Intersect` or `Subtract`), or the body divided by
//! the sheet (`boolean::divide`), whose pieces are solids. Later plans of
//! phase 12 do more to the sheets.

use crate::blend_bytes::body;
use crate::euler_script::Bytes;
use crate::sweep_bytes::shape;
use core::f64::consts::{PI, TAU};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean, divide};
use ryukei::geom::Plane;
use ryukei::geom::{
    Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Rigid, Vector2, Vector3,
};
use ryukei::heal::{SewOptions, sew};
use ryukei::name::{Name, OpKey};
use ryukei::sheet::{extract, from_surface, remove};
use ryukei::sweep::{
    Chain, ChainKey, Profile, Segment, SegmentKey, ValidProfile, extrude, planar, revolve,
};
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};

/// What sheets are made with.
const KEY: OpKey = OpKey(0x5348_4254);

/// The bodies the bytes make (a sheet, or a division's pieces, which are
/// solids), and the size of their smallest feature, for a chord to mesh
/// them at; `None` where the bytes make nothing or a typed refusal. Any
/// other error panics: it is a bug.
pub fn sheet(data: &[u8]) -> Option<(Vec<Body>, f64)> {
    let (&kind, rest) = data.split_first()?;
    let made = match kind % 7 {
        0 => one(faces(rest)?),
        1 => one(bounded(rest)?),
        2 => one(planar_faces(rest)?),
        3 => one(swept(rest)?),
        4 => one(sewn(rest)?),
        5 => one(trimmed(rest)?),
        _ => divided(rest)?,
    };
    match made {
        Ok(found) => Some(found),
        Err(
            Error::InvalidSheet(_)
            | Error::InvalidProfile(_)
            | Error::InvalidSweep(_)
            | Error::InvalidHeal(_)
            | Error::InvalidBoolean(
                BooleanError::Uncertain { .. }
                | BooleanError::Unsupported { .. }
                | BooleanError::NonManifold { .. }
                | BooleanError::ToolDoesNotDivide { .. },
            )
            | Error::Degenerate
            | Error::NonFinite,
        ) => None,
        Err(e) => panic!("unexpected {e:?}"),
    }
}

type Made = Result<(Body, f64), Error>;
type Many = Result<(Vec<Body>, f64), Error>;

fn one(made: Made) -> Many {
    made.map(|(b, small)| (vec![b], small))
}

/// A body and a plane sheet across it: through its box's middle, moved
/// along its normal (picked from the bytes) by up to a third of the box,
/// twice its size each way.
fn across(data: &[u8]) -> Option<(Body, Body)> {
    let (b, rest) = body(data)?;
    let x = b
        .faces()
        .map(|f| ryukei::intersect::face_box(&b, f))
        .reduce(|x, y| x.union(&y))?;
    let mut bytes = Bytes::new(rest);
    let size = (x.max() - x.min()).length();
    let n = Vector3::new(
        bytes.range(-1.0, 1.0),
        bytes.range(-1.0, 1.0),
        bytes.range(0.2, 1.0),
    );
    let mid = x.min() + (x.max() - x.min()) * 0.5;
    let origin = mid + n.normalize()?.get() * (bytes.range(-1.0, 1.0) * size / 3.0);
    let frame = Frame::new(origin, n, Vector3::new(1.0, 0.3, 0.1)).ok()?;
    let sheet = from_surface(
        KEY,
        &ryukei::geom::Surface::Plane(Plane::new(frame)),
        [[-size, size], [-size, size]],
    )
    .ok()?
    .0;
    Some((b, sheet))
}

fn tool(body: &Body) -> Tool<'_> {
    Tool {
        key: ToolKey(0x5348),
        body,
        place: Rigid::IDENTITY,
    }
}

/// A plane sheet across a body, trimmed by it: what is inside, or (by
/// the byte after) outside.
fn trimmed(data: &[u8]) -> Option<Made> {
    let (b, sheet) = across(data)?;
    let op = if data.last().is_some_and(|x| x % 2 == 0) {
        BooleanOp::Intersect
    } else {
        BooleanOp::Subtract
    };
    let made = boolean(KEY, op, &sheet, &[tool(&b)]);
    Some(match made {
        Ok((s, _)) if s.faces().next().is_none() => return None,
        Ok((s, _)) => Ok((s, 0.08)),
        Err(e) => Err(e),
    })
}

/// A body divided by a plane sheet across it: every piece.
fn divided(data: &[u8]) -> Option<Many> {
    let (b, sheet) = across(data)?;
    Some(divide(KEY, &b, &[tool(&sheet)]).map(|(d, _)| {
        let mut all = d.front;
        all.extend(d.back);
        all.extend(d.apart);
        (all, 0.08)
    }))
}

/// Some of a body's faces, chosen by the bits of the bytes after it,
/// extracted or removed.
fn faces(data: &[u8]) -> Option<Made> {
    let (b, rest) = body(data)?;
    let mut bytes = Bytes::new(rest);
    let names: Vec<Name> = b.faces().map(|f| b.face(f).expect("live").name).collect();
    let chosen: Vec<Name> = names
        .iter()
        .filter(|_| bytes.byte().is_multiple_of(2))
        .copied()
        .collect();
    let made = if bytes.byte().is_multiple_of(2) {
        extract(KEY, &b, &chosen)
    } else {
        remove(KEY, &b, &chosen)
    };
    Some(made.map(|(s, _)| (s, 0.08)))
}

/// A body's faces, some left out by the bits after it, each extracted as
/// a sheet of its own and moved along a direction of its own by up to a
/// tenth of the tolerance, sewn as sheets (a closed piece stays a closed
/// sheet): the first piece.
fn sewn(data: &[u8]) -> Option<Made> {
    let (b, rest) = body(data)?;
    let mut bytes = Bytes::new(rest);
    let tolerance = [1e-6, 1e-5, 1e-4][usize::from(bytes.byte() % 3)];
    let mut sheets = Vec::new();
    for f in b.faces() {
        // One in five left out (none once the bytes run out).
        if bytes.byte() % 5 == 4 {
            continue;
        }
        let name = b.face(f).expect("live").name;
        let s = match extract(KEY, &b, &[name]) {
            Ok((s, _)) => s,
            Err(e) => return Some(Err(e)),
        };
        let dir = Vector3::new(
            bytes.range(-1.0, 1.0),
            bytes.range(-1.0, 1.0),
            bytes.range(0.1, 1.0),
        );
        let by = dir.normalize().map_or(Vector3::new(0.0, 0.0, 0.0), |d| {
            d.get() * (tolerance * 0.1 * f64::from(bytes.byte()) / 255.0)
        });
        sheets.push(s.transformed(&Rigid::translation(by)).ok()?);
    }
    if sheets.is_empty() {
        return None;
    }
    let options = SewOptions {
        tolerance,
        solid: false,
    };
    Some(sew(KEY, &sheets, &options).map(|(mut pieces, _)| (pieces.remove(0), 0.08)))
}

/// A range inside `[lo, hi]` (each end clamped to ±4), or a period's worth
/// from anywhere when `period` is given: now and then the whole of it.
fn range(b: &mut Bytes, [lo, hi]: [f64; 2], period: Option<f64>) -> [f64; 2] {
    match period {
        Some(p) => {
            let start = b.range(-p, p);
            let span = if b.byte().is_multiple_of(4) {
                p
            } else {
                b.range(0.05, 1.0) * p
            };
            [start, start + span]
        }
        None => {
            let (lo, hi) = (lo.max(-4.0), hi.min(4.0));
            let a = b.range(lo, hi);
            let z = b.range(lo, hi);
            [a.min(z), a.max(z)]
        }
    }
}

/// One of a body's faces' surfaces, bounded by a box of its parameters.
fn bounded(data: &[u8]) -> Option<Made> {
    let (b, rest) = body(data)?;
    let mut bytes = Bytes::new(rest);
    let faces: Vec<_> = b.faces().collect();
    if faces.is_empty() {
        return None;
    }
    let f = faces[bytes.pick(faces.len())];
    let surface = b.surface(b.face(f)?.surface)?.clone();
    let domain = surface.domain();
    let (pu, pv) = surface.periods();
    let rect = [
        range(&mut bytes, domain[0], pu),
        range(&mut bytes, domain[1], pv),
    ];
    Some(from_surface(KEY, &surface, rect).map(|(s, _)| (s, 0.08)))
}

/// A frame anywhere, tilted as the bytes say.
fn frame(b: &mut Bytes) -> Frame {
    let axis = Vector3::new(b.range(-1.0, 1.0), b.range(-1.0, 1.0), 1.0)
        .normalize()
        .expect("z is 1");
    let shift = Vector3::new(b.range(-5.0, 5.0), b.range(-5.0, 5.0), b.range(-5.0, 5.0));
    let at = Rigid::translation(shift).then_after(&Rigid::rotation(axis, b.range(-3.2, 3.2)));
    Frame::new(
        at.apply_point(Point3::ORIGIN),
        at.apply_vector(Vector3::new(0.0, 0.0, 1.0)),
        at.apply_vector(Vector3::new(1.0, 0.0, 0.0)),
    )
    .expect("a rigid frame")
}

/// One to three loops' planar faces.
fn planar_faces(data: &[u8]) -> Option<Made> {
    let mut b = Bytes::new(data);
    let frame = frame(&mut b);
    let mut loops = Vec::new();
    let mut small = f64::INFINITY;
    for key in 1..=1 + b.pick(3) as u128 {
        let (lp, s) = shape(&mut b, key)?;
        loops.push(lp);
        small = small.min(s);
    }
    Some(
        ValidProfile::new(Profile { frame, loops })
            .and_then(|p| planar(KEY, &p))
            .map(|(s, _)| (s, small)),
    )
}

/// An open chain from bytes, keyed `key`: a polyline of one to four
/// segments, or an arc; and its smallest dimension.
fn chain(b: &mut Bytes, key: u128) -> Option<(Chain, f64)> {
    let k = key * 32;
    let start = Point2::new(b.range(-3.0, 3.0), b.range(-3.0, 3.0));
    let mut small = f64::INFINITY;
    let segments = if b.byte().is_multiple_of(3) {
        let r = b.range(0.05, 2.0);
        let f = Frame2::new(start, Vector2::new(1.0, 0.0), true).ok()?;
        let a0 = b.range(-PI, PI);
        let a1 = a0 + b.range(-0.95, 0.95) * TAU;
        small = r.min(r * (a1 - a0).abs());
        vec![Segment {
            key: SegmentKey(k),
            curve: Curve2::Circle2(Circle2::new(f, r).ok()?),
            t0: a0,
            t1: a1,
        }]
    } else {
        let mut at = start;
        let mut out = Vec::new();
        for i in 0..1 + b.pick(4) {
            let next = at + Vector2::new(b.range(-2.0, 2.0), b.range(-2.0, 2.0));
            let curve = Curve2::Line2(Line2::through(at, next).ok()?);
            small = small.min(at.distance(next));
            out.push(Segment {
                key: SegmentKey(k + i as u128),
                curve,
                t0: 0.0,
                t1: at.distance(next),
            });
            at = next;
        }
        out
    };
    Some((
        Chain {
            key: ChainKey(key),
            segments,
        },
        small,
    ))
}

/// One or two chains, now and then beside a loop, extruded or revolved as
/// a surface.
fn swept(data: &[u8]) -> Option<Made> {
    let mut b = Bytes::new(data);
    let frame = frame(&mut b);
    let mut small = f64::INFINITY;
    let mut chains = Vec::new();
    for key in 1..=1 + b.pick(2) as u128 {
        let (c, s) = chain(&mut b, key)?;
        chains.push(c);
        small = small.min(s);
    }
    let mut loops = Vec::new();
    if b.byte().is_multiple_of(4) {
        let (lp, s) = shape(&mut b, 9)?;
        loops.push(lp);
        small = small.min(s);
    }
    let profile = match ValidProfile::surface(Profile { frame, loops }, chains) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let made = if b.byte().is_multiple_of(2) {
        let dir = frame.z().get()
            + frame.x().get() * b.range(-2.0, 2.0)
            + frame.y().get() * b.range(-2.0, 2.0);
        let start = b.range(-2.0, 2.0);
        extrude(KEY, &profile, dir, start, start + b.range(-0.1, 3.0))
    } else {
        let a = b.range(-PI, PI);
        let dir = frame.x().get() * ryukei::math::cos(a) + frame.y().get() * ryukei::math::sin(a);
        let through = frame.to_world(b.range(-4.0, 4.0), b.range(-4.0, 4.0), 0.0);
        let Ok(axis) = Axis::new(through, dir) else {
            return None;
        };
        let start = b.range(-PI, PI);
        let span = if b.byte().is_multiple_of(4) {
            TAU
        } else {
            b.range(-0.1, 7.0)
        };
        revolve(KEY, &profile, axis, start, start + span)
    };
    Some(made.map(|(s, _)| (s, small)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lcg;
    use ryukei::check::{Fault, check, check_intersections};
    use ryukei::mesh::{MeshParams, check_mesh, tessellate};
    use ryukei::topo::BodyKind;

    /// What the fuzz target asks of every sheet, on seeded bytes of each
    /// kind: a typed refusal, or a sheet that checks clean and meshes.
    #[test]
    fn seeded_bytes_make_sound_sheets_or_refusals() {
        let mut made = [0; 7];
        for seed in 0..70u64 {
            let mut g = Lcg::new(seed);
            let mut data: Vec<u8> = (0..96).map(|_| g.next_u64() as u8).collect();
            data[0] = (seed % 7) as u8;
            let Some((bodies, small)) = sheet(&data) else {
                continue;
            };
            made[(seed % 7) as usize] += 1;
            for body in bodies {
                let kind = if seed % 7 == 6 {
                    BodyKind::Solid
                } else {
                    BodyKind::Sheet
                };
                assert_eq!(body.kind(), kind, "seed {seed}");
                assert_eq!(check(&body), [], "seed {seed}");
                for fault in check_intersections(&body) {
                    assert!(
                        matches!(fault, Fault::CannotCheck { .. }),
                        "seed {seed}: {fault:?}"
                    );
                }
                let params = MeshParams {
                    max_triangles: 20_000,
                    ..MeshParams::new((small / 8.0).clamp(1e-3, 0.05), 0.5)
                };
                match tessellate(&body, &params) {
                    Ok(m) => {
                        assert_eq!(m.faults, [], "seed {seed}");
                        assert_eq!(check_mesh(&body, &m), Ok(()), "seed {seed}");
                    }
                    Err(Error::MeshTooLarge { .. }) => {}
                    Err(e) => panic!("seed {seed}: unexpected {e:?}"),
                }
            }
        }
        // Every kind makes some sheets.
        assert!(made.iter().all(|&n| n > 0), "{made:?}");
    }
}
