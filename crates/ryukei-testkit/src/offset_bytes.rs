//! Offsets and shells from bytes, for the `offset` fuzz target and for
//! replaying what it has found: a body (as [`crate::blend_bytes`] makes
//! it, or that body blended), a distance on a coarse grid of fractions of
//! its size either way, now and then a face at a distance of its own or
//! at zero, and an offset or a shell.

use crate::blend_bytes::{blend_case, body};
use crate::euler_script::Bytes;
use ryukei::blend::blend;
use ryukei::name::{Name, OpKey};
use ryukei::offset::{OffsetSpec, ShellSpec};
use ryukei::topo::Body;

/// What to do to the body.
pub enum Op {
    Offset(OffsetSpec),
    Shell(ShellSpec),
}

/// An offset or a shell to try.
pub struct OffsetCase {
    pub body: Body,
    pub op: Op,
}

/// An offset or a shell from bytes, or `None` when they give no body.
pub fn offset_case(data: &[u8]) -> Option<OffsetCase> {
    let (&how, rest) = data.split_first()?;
    let (body, rest) = if how % 2 == 1 {
        // A blended body: its fillets' and chamfers' faces offset too.
        let case = blend_case(rest)?;
        let b = blend(OpKey(0x0ff), &case.body, &case.blends).ok()?.0;
        (b, &[][..])
    } else {
        body(rest)?
    };
    let mut b = Bytes::new(rest);
    let size = {
        let bx = body
            .faces()
            .map(|f| ryukei::intersect::face_box(&body, f))
            .reduce(|a, c| a.union(&c))?;
        let d = bx.max() - bx.min();
        d.x.min(d.y).min(d.z).max(0.1)
    };
    let d = size * [0.01, 0.03, 0.1, 0.2, 0.4][b.pick(5)];
    let mut faces: Vec<Name> = body.faces().map(|f| body.face(f).expect("a face").name).collect();
    faces.sort();
    let mut some = |b: &mut Bytes| faces.remove(b.pick(faces.len().max(1)).min(faces.len() - 1));
    let op = match b.pick(4) {
        0 => Op::Shell(ShellSpec {
            thickness: d,
            outward: b.pick(4) == 0,
            open: if b.pick(3) == 0 { Vec::new() } else { vec![some(&mut b)] },
            faces: Vec::new(),
        }),
        k => {
            let distance = if k == 1 { d } else { -d };
            let own = match b.pick(3) {
                0 => vec![(some(&mut b), 0.0)],
                1 => vec![(some(&mut b), distance * 0.5)],
                _ => Vec::new(),
            };
            Op::Offset(OffsetSpec {
                distance,
                faces: own,
            })
        }
    };
    Some(OffsetCase { body, op })
}
