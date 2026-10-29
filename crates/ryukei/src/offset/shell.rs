//! Shells: a solid hollowed to a wall of a thickness, open where asked.

use super::{OffsetSpec, offset_body};
use crate::boolean::{BooleanOp, Tool, boolean};
use crate::geom::Rigid;
use crate::name::{Journal, Name, OpKey, ToolKey};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::Body;
use crate::{Error, OffsetError};

/// A shell's wall: `thickness` everywhere but the faces `faces` names,
/// each its own; inwards, or outwards with `outward`; and the faces
/// `open` names left open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellSpec {
    pub thickness: f64,
    pub outward: bool,
    pub open: Vec<Name>,
    pub faces: Vec<(Name, f64)>,
}

/// The key of the offset a shell subtracts or subtracts from.
fn offset_key(key: OpKey) -> OpKey {
    let mut bytes = b"ryukei.shell.offset\0".to_vec();
    bytes.extend_from_slice(&key.0.to_le_bytes());
    OpKey::from_bytes(&bytes)
}

/// The tool key of that offset in the shell's boolean.
const TOOL: ToolKey = ToolKey(1);

/// `body` hollowed by `spec`: inwards, the body less its offset by
/// `-thickness` (each face of `faces` by its own) with the open faces at
/// zero, which lie on the body's own faces so the boolean takes them away
/// and the walls end there; outwards, the offset by `+thickness` less the
/// body. With no open faces, the inner offset is a cavity.
///
/// Inwards, the body's faces keep their names, and the inner faces are
/// the boolean's tool's, `Placed` under the shell's key; outwards, the
/// outer faces keep the body's faces' names and the body's own are the
/// tool's. The journal is the boolean's: the open faces are `Deleted`.
/// Fails with `BadDistance` for a thickness that isn't finite or is under
/// `10 × LINEAR_RESOLUTION`, `NoSuchFace` for an open face that isn't the
/// body's (or is named twice, or also given a thickness), and with
/// whatever [`offset_body`] or the boolean refuses.
pub fn shell(key: OpKey, body: &Body, spec: &ShellSpec) -> Result<(Body, Journal), Error> {
    let t = spec.thickness;
    if !(t.is_finite() && t >= 10.0 * LINEAR_RESOLUTION) {
        return Err(OffsetError::BadDistance.into());
    }
    let sign = if spec.outward { 1.0 } else { -1.0 };
    let mut faces: Vec<(Name, f64)> = spec.faces.iter().map(|&(f, d)| (f, sign * d)).collect();
    for &f in &spec.open {
        if body.by_name(f).is_none_or(|e| !matches!(e, crate::topo::EntityId::Face(_)))
            || faces.iter().any(|x| x.0 == f)
        {
            return Err(OffsetError::NoSuchFace { face: f }.into());
        }
        faces.push((f, 0.0));
    }
    let spec = OffsetSpec {
        distance: sign * t,
        faces,
    };
    let (moved, _) = offset_body(offset_key(key), body, &spec)?;
    let place = Rigid::IDENTITY;
    if spec.distance < 0.0 {
        let tool = Tool {
            key: TOOL,
            body: &moved,
            place,
        };
        boolean(key, BooleanOp::Subtract, body, &[tool])
    } else {
        let tool = Tool {
            key: TOOL,
            body,
            place,
        };
        boolean(key, BooleanOp::Subtract, &moved, &[tool])
    }
}
