//! Face tweaks and drafts from bytes, for the `tweak` fuzz target, the
//! `tweak_props` property test and replaying what they find: a body as
//! [`crate::offset_bytes`] makes it (a primitive, a sweep or a boolean's
//! result, as [`crate::blend_bytes::body`] gives it, or that body
//! blended), one or two of its faces, and a move, an offset, a
//! replacement or a draft, on a coarse grid of fractions of the body's
//! size and of angles.

use crate::blend_bytes::{blend_case, body};
use crate::euler_script::Bytes;
use ryukei::blend::blend;
use ryukei::geom::{Point3, Rigid, UnitVector3, Vector3};
use ryukei::name::{Name, OpKey};
use ryukei::topo::{Body, FaceId};
use ryukei::tweak::{
    DraftFace, DraftSpec, Hinge, MoveSpec, OffsetFacesSpec, ReplaceSpec, Topology,
};

/// What to do to the body.
pub enum Op {
    Move(MoveSpec),
    Offset(OffsetFacesSpec),
    Replace(ReplaceSpec),
    Draft(DraftSpec),
}

impl Op {
    /// The operation's name, for reports.
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Move(_) => "move",
            Op::Offset(_) => "offset",
            Op::Replace(_) => "replace",
            Op::Draft(_) => "draft",
        }
    }
}

/// A tweak or a draft to try.
pub struct TweakCase {
    pub body: Body,
    pub op: Op,
}

/// How many bytes after the first one are choices; the rest make the
/// body.
const CHOICES: usize = 24;

/// A turn by `angle` about the line through `c` along `axis`.
fn turn_about(c: Point3, axis: Vector3, angle: f64) -> Rigid {
    let to = Rigid::translation(c.to_vector());
    let back = Rigid::translation(-c.to_vector());
    let axis = axis.normalize().expect("a unit axis");
    to.then_after(&Rigid::rotation(axis, angle).then_after(&back))
}

/// The middle of a box's corners.
fn middle(lo: Point3, hi: Point3) -> Point3 {
    Point3::new(
        (lo.x + hi.x) / 2.0,
        (lo.y + hi.y) / 2.0,
        (lo.z + hi.z) / 2.0,
    )
}

/// A tweak or a draft from bytes, or `None` when they give no body.
/// The first byte chooses a plain or a blended body, the next
/// [`CHOICES`] bytes the faces and the operation, and the rest the body.
pub fn tweak_case(data: &[u8]) -> Option<TweakCase> {
    let (&how, rest) = data.split_first()?;
    let (choices, rest) = rest.split_at(rest.len().min(CHOICES));
    let body = if how % 2 == 1 {
        // A blended body: its fillets' and chamfers' faces take part.
        let case = blend_case(rest)?;
        blend(OpKey(0x0ff), &case.body, &case.blends).ok()?.0
    } else {
        body(rest)?.0
    };
    let bx = body
        .faces()
        .map(|f| ryukei::intersect::face_box(&body, f))
        .reduce(|a, c| a.union(&c))?;
    let (lo, hi) = (bx.min(), bx.max());
    let d = hi - lo;
    let size = d.x.min(d.y).min(d.z).max(0.1);
    let mid = middle(lo, hi);
    let mut b = Bytes::new(choices);
    let mut faces: Vec<(Name, FaceId)> = body
        .faces()
        .map(|f| (body.face(f).expect("a face").name, f))
        .collect();
    faces.sort_by_key(|&(n, _)| n);
    let first = faces[b.pick(faces.len())];
    let mut chosen = vec![first.0];
    if b.pick(3) == 0 {
        let other = faces[b.pick(faces.len())];
        if other.0 != first.0 {
            chosen.push(other.0);
        }
    }
    let step = size * [0.01, 0.03, 0.1, 0.2][b.pick(4)];
    let axis = [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    ][b.pick(3)];
    let sign = if b.pick(2) == 0 { 1.0 } else { -1.0 };
    let tangent = b.pick(2) == 0;
    let turn = [0.01, 0.05][b.pick(2)] * sign;
    let op = match b.pick(4) {
        0 => {
            let motion = if b.pick(2) == 0 {
                Rigid::translation(axis * (sign * step))
            } else {
                turn_about(mid, axis, turn)
            };
            Op::Move(MoveSpec {
                faces: chosen,
                motion,
                tangent,
                topology: Topology::Keep,
            })
        }
        1 => Op::Offset(OffsetFacesSpec {
            faces: chosen.iter().map(|&n| (n, sign * step)).collect(),
            tangent,
            topology: Topology::Keep,
        }),
        2 => {
            // The first face's own surface turned a little about the
            // middle of the face's box, so it passes near the face.
            let f = body.face(first.1).expect("a face");
            let s = body.surface(f.surface).expect("a surface");
            let fb = ryukei::intersect::face_box(&body, first.1);
            let at = middle(fb.min(), fb.max());
            Op::Replace(ReplaceSpec {
                faces: vec![(first.0, s.transformed(&turn_about(at, axis, turn)))],
                topology: Topology::Keep,
            })
        }
        _ => {
            let pull = UnitVector3::exact(axis * sign).expect("a unit axis");
            let angle = [0.01, 0.05, 0.2][b.pick(3)] * if b.pick(4) == 0 { -1.0 } else { 1.0 };
            let hinge = if b.pick(3) == 0 {
                // A parting line: one edge of the first face.
                let edges: Vec<Name> = body
                    .face_loops(first.1)
                    .flat_map(|l| body.loop_fins(l))
                    .map(|fin| {
                        let e = body.fin(fin).expect("a fin").edge;
                        body.edge(e).expect("an edge").name
                    })
                    .collect();
                Hinge::PartingLine(vec![*edges.get(b.pick(edges.len().max(1)))?])
            } else {
                Hinge::Neutral {
                    origin: [mid, lo][b.pick(2)],
                }
            };
            Op::Draft(DraftSpec {
                pull,
                angle,
                hinge,
                faces: chosen
                    .iter()
                    .map(|&face| DraftFace {
                        face,
                        angle: None,
                        flip: false,
                    })
                    .collect(),
                tangent,
                topology: Topology::Keep,
            })
        }
    };
    Some(TweakCase { body, op })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lcg;

    /// Seeded bytes give cases of every kind, and the same bytes give the
    /// same case.
    #[test]
    fn seeded_bytes_give_every_kind() {
        let mut kinds = std::collections::BTreeMap::new();
        for seed in 0..64u64 {
            let mut g = Lcg::new(seed);
            let data: Vec<u8> = (0..96).map(|_| (g.next_u64() >> 56) as u8).collect();
            let Some(case) = tweak_case(&data) else {
                continue;
            };
            *kinds.entry(case.op.kind()).or_insert(0) += 1;
            let again = tweak_case(&data).expect("the same case");
            assert_eq!(again.body.dump(), case.body.dump(), "seed {seed}");
            assert_eq!(again.op.kind(), case.op.kind(), "seed {seed}");
        }
        for k in ["move", "offset", "replace", "draft"] {
            assert!(
                kinds.get(k).copied().unwrap_or(0) > 0,
                "no {k} in {kinds:?}"
            );
        }
    }
}
