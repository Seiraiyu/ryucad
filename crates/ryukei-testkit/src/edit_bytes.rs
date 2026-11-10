//! Direct edits from bytes (design 399, plan 13.D), for the `edit` fuzz
//! target, the `edit_props` property test and replaying what they find:
//! a body as [`crate::tweak_bytes`] makes it (a primitive, a sweep or a
//! boolean's result, plain or blended), and one of
//!
//! - faces deleted: one face, or the feature [`feature_faces`] finds
//!   from it, healed by [`Heal::Auto`], [`Heal::Cap`] or [`Heal::Grow`];
//! - a topology-changing tweak: [`crate::tweak_bytes`]'s move, offset,
//!   replacement or draft with [`Topology::Change`], and half the time
//!   [`Blends::Rebuild`];
//! - blends deleted: every recognised blend no larger than a fraction of
//!   the body's size, or the one holding a chosen face;
//! - a blend resized: the one holding a chosen face (else the first
//!   recognised) made again at its own size, half it or one and a half
//!   times it.
//!
//! [`run`] makes the edit under a [`Work`]; recognition and feature
//! finding run inside it, so a fuzz case's budget covers them.

use crate::blend_bytes::{blend_case, body};
use crate::euler_script::Bytes;
use crate::tweak_bytes::{self, tweak_case};
use ryukei::Error;
use ryukei::blend::{BlendKind, Chamfer, blend};
use ryukei::name::{Journal, Name, OpKey};
use ryukei::topo::Body;
use ryukei::tweak::{
    Blends, DeleteBlends, DeleteSpec, Heal, RecogniseOptions, Topology, delete_blends_with,
    delete_faces_with, draft_with, feature_faces, move_faces_with, offset_faces_with,
    recognise_blends_with, replace_faces_with, resize_blends_with,
};
use ryukei::work::Work;

/// What to do to the body.
pub enum Op {
    /// Delete `face`, or with `feature` the faces [`feature_faces`]
    /// finds from it, healed by `heal`.
    Delete {
        face: Name,
        feature: bool,
        heal: Heal,
    },
    /// A tweak with [`Topology::Change`].
    Tweak(tweak_bytes::Op),
    /// Delete every blend no larger than `max_size`, or the blend holding
    /// `face`.
    DeleteBlends {
        max_size: Option<f64>,
        face: Option<Name>,
    },
    /// Make the blend holding `face` (else the first recognised) again
    /// with its size times `factor`.
    Resize { face: Name, factor: f64 },
}

impl Op {
    /// The operation's name, for reports.
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Delete { feature: false, .. } => "delete",
            Op::Delete { feature: true, .. } => "delete feature",
            Op::Tweak(t) => match t {
                tweak_bytes::Op::Move(_) => "move",
                tweak_bytes::Op::Offset(_) => "offset",
                tweak_bytes::Op::Replace(_) => "replace",
                tweak_bytes::Op::Draft(_) => "draft",
            },
            Op::DeleteBlends { .. } => "delete blends",
            Op::Resize { .. } => "resize",
        }
    }
}

/// An edit to try.
pub struct EditCase {
    pub body: Body,
    pub op: Op,
}

/// How many bytes after the first two are choices; the rest make the
/// body.
const CHOICES: usize = 16;

/// The faces' names, sorted.
fn face_names(b: &Body) -> Vec<Name> {
    let mut n: Vec<Name> = b.faces().map(|f| b.face(f).expect("a face").name).collect();
    n.sort();
    n
}

/// The least side of the body's box, at least 0.1.
fn least_side(b: &Body) -> Option<f64> {
    let bx = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|a, c| a.union(&c))?;
    let d = bx.max() - bx.min();
    Some(d.x.min(d.y).min(d.z).max(0.1))
}

/// An edit from bytes, or `None` when they give no body. The first byte
/// chooses the kind of edit; a tweak takes the rest as
/// [`tweak_case`] does. Otherwise the second byte chooses a plain or a
/// blended body (a blend edit always takes a blended one), the next
/// [`CHOICES`] bytes the face and the edit, and the rest the body.
pub fn edit_case(data: &[u8]) -> Option<EditCase> {
    let (&what, rest) = data.split_first()?;
    if what % 4 == 1 {
        let mut case = tweak_case(rest)?;
        let rebuild = rest.first().is_some_and(|b| b & 2 != 0);
        let blends = if rebuild {
            Blends::Rebuild
        } else {
            Blends::Keep
        };
        match &mut case.op {
            tweak_bytes::Op::Move(s) => (s.topology, s.blends) = (Topology::Change, blends),
            tweak_bytes::Op::Offset(s) => (s.topology, s.blends) = (Topology::Change, blends),
            tweak_bytes::Op::Replace(s) => (s.topology, s.blends) = (Topology::Change, blends),
            tweak_bytes::Op::Draft(s) => (s.topology, s.blends) = (Topology::Change, blends),
        }
        return Some(EditCase {
            body: case.body,
            op: Op::Tweak(case.op),
        });
    }
    let (&how, rest) = rest.split_first()?;
    let (choices, rest) = rest.split_at(rest.len().min(CHOICES));
    // Blend edits always take a blended body; deletes take either.
    let body = if how % 2 == 1 || what % 4 >= 2 {
        let case = blend_case(rest)?;
        blend(OpKey(0x0ff), &case.body, &case.blends).ok()?.0
    } else {
        body(rest)?.0
    };
    let size = least_side(&body)?;
    let faces = face_names(&body);
    let mut b = Bytes::new(choices);
    let face = faces[b.pick(faces.len())];
    let op = match what % 4 {
        0 => {
            // A feature is found from a curved face where there is one (a
            // hole's wall, a boss's side, a fillet), else from the face.
            let feature = b.pick(2) == 0;
            let curved: Vec<Name> = body
                .faces()
                .filter(|&f| {
                    let s = body.face(f).expect("a face").surface;
                    !matches!(body.surface(s), Some(ryukei::geom::Surface::Plane(_)))
                })
                .map(|f| body.face(f).expect("a face").name)
                .collect();
            let face = if feature && !curved.is_empty() {
                curved[b.pick(curved.len())]
            } else {
                face
            };
            Op::Delete {
                face,
                feature,
                heal: [Heal::Auto, Heal::Cap, Heal::Grow][b.pick(3)],
            }
        }
        2 => {
            if b.pick(2) == 0 {
                Op::DeleteBlends {
                    max_size: Some(size * [0.05, 0.2, 1.0][b.pick(3)]),
                    face: None,
                }
            } else {
                Op::DeleteBlends {
                    max_size: None,
                    face: Some(face),
                }
            }
        }
        _ => Op::Resize {
            face,
            factor: [1.0, 0.5, 1.5][b.pick(3)],
        },
    };
    Some(EditCase { body, op })
}

/// `kind` with its sizes times `factor`.
fn scaled(kind: BlendKind, factor: f64) -> BlendKind {
    match kind {
        BlendKind::Fillet { radius } => BlendKind::Fillet {
            radius: radius * factor,
        },
        BlendKind::Chamfer(Chamfer::Equal { distance }) => BlendKind::Chamfer(Chamfer::Equal {
            distance: distance * factor,
        }),
        BlendKind::Chamfer(Chamfer::Distances {
            first,
            second,
            first_face,
        }) => BlendKind::Chamfer(Chamfer::Distances {
            first: first * factor,
            second: second * factor,
            first_face,
        }),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance,
            angle,
            face,
        }) => BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance: distance * factor,
            angle,
            face,
        }),
    }
}

/// The edit made under `work`: a new body and its journal, or the
/// kernel's refusal.
pub fn run(case: &EditCase, key: OpKey, work: &Work) -> Result<(Body, Journal), Error> {
    let body = &case.body;
    match &case.op {
        Op::Delete {
            face,
            feature,
            heal,
        } => {
            let faces = if *feature {
                feature_faces(body, *face)?
            } else {
                vec![*face]
            };
            delete_faces_with(key, body, &DeleteSpec { faces, heal: *heal }, work)
        }
        Op::Tweak(t) => match t {
            tweak_bytes::Op::Move(s) => move_faces_with(key, body, s, work),
            tweak_bytes::Op::Offset(s) => offset_faces_with(key, body, s, work),
            tweak_bytes::Op::Replace(s) => replace_faces_with(key, body, s, work),
            tweak_bytes::Op::Draft(s) => draft_with(key, body, s, work).map(|(b, j, _)| (b, j)),
        },
        Op::DeleteBlends { max_size, face } => {
            let spec = DeleteBlends {
                max_size: *max_size,
                faces: face.iter().copied().collect(),
                options: RecogniseOptions::default(),
            };
            delete_blends_with(key, body, &spec, work)
        }
        Op::Resize { face, factor } => {
            let opts = RecogniseOptions::default();
            let r = recognise_blends_with(body, &opts, work)?;
            let blend = match r.holding(*face) {
                Some(Ok(i)) => &r.blends[i],
                _ => match r.blends.first() {
                    Some(b) => b,
                    // Nothing to resize: the kernel's own refusal.
                    None => {
                        return resize_blends_with(
                            key,
                            body,
                            &[(*face, BlendKind::Fillet { radius: 1e-3 })],
                            &opts,
                            work,
                        );
                    }
                },
            };
            let sizes = [(blend.faces[0], scaled(blend.kind, *factor))];
            resize_blends_with(key, body, &sizes, &opts, work)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lcg;

    /// Seeded bytes give edits of every kind, and the same bytes give the
    /// same case.
    #[test]
    fn seeded_bytes_give_every_kind() {
        let mut kinds = std::collections::BTreeMap::new();
        for seed in 0..64u64 {
            let mut g = Lcg::new(seed);
            let data: Vec<u8> = (0..96).map(|_| (g.next_u64() >> 56) as u8).collect();
            let Some(case) = edit_case(&data) else {
                continue;
            };
            *kinds.entry(case.op.kind()).or_insert(0) += 1;
            let again = edit_case(&data).expect("the same case");
            assert_eq!(again.body.dump(), case.body.dump(), "seed {seed}");
            assert_eq!(again.op.kind(), case.op.kind(), "seed {seed}");
        }
        for k in ["delete", "delete feature", "delete blends", "resize"] {
            assert!(
                kinds.get(k).copied().unwrap_or(0) > 0,
                "no {k} in {kinds:?}"
            );
        }
        let tweaks: usize = ["move", "offset", "replace", "draft"]
            .iter()
            .map(|k| kinds.get(k).copied().unwrap_or(0))
            .sum();
        assert!(tweaks > 0, "no tweak in {kinds:?}");
    }
}
