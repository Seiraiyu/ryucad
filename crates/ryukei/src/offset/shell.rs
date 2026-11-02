//! Shells: a solid hollowed to a wall of a thickness, open where asked.

use super::OffsetSpec;
use crate::boolean::{BooleanOp, Tool, boolean_in};
use crate::geom::Rigid;
use crate::name::{Journal, Name, OpKey, ToolKey};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::Body;
use crate::work::{Meter, Work};
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
/// whatever [`offset_body`] or the boolean refuses. A sheet is
/// `NotASolid`.
pub fn shell(key: OpKey, body: &Body, spec: &ShellSpec) -> Result<(Body, Journal), Error> {
    shell_with(key, body, spec, &Work::unlimited())
}

/// [`shell`] under `work`, the offset and the boolean one call under one
/// budget: refused with `Cancelled` or `BudgetExceeded` when it stops,
/// building nothing.
pub fn shell_with(
    key: OpKey,
    body: &Body,
    spec: &ShellSpec,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |meter| shell_in(key, body, spec, meter))
}

fn shell_in(
    key: OpKey,
    body: &Body,
    spec: &ShellSpec,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    body.require_solid("shell")?;
    let t = spec.thickness;
    if !(t.is_finite() && t >= 10.0 * LINEAR_RESOLUTION) {
        return Err(OffsetError::BadDistance.into());
    }
    let sign = if spec.outward { 1.0 } else { -1.0 };
    let mut faces: Vec<(Name, f64)> = spec.faces.iter().map(|&(f, d)| (f, sign * d)).collect();
    for &f in &spec.open {
        if body
            .by_name(f)
            .is_none_or(|e| !matches!(e, crate::topo::EntityId::Face(_)))
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
    let moved = match super::local(offset_key(key), body, &spec, meter) {
        Ok((m, _)) => m,
        Err(e) if super::global_applies(&e) => {
            match global_shell(key, body, spec.distance, &spec, meter)? {
                Some(m) => m,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let place = Rigid::IDENTITY;
    if spec.distance < 0.0 {
        let tool = Tool {
            key: TOOL,
            body: &moved,
            place,
        };
        boolean_in(key, BooleanOp::Subtract, body, &[tool], meter)
    } else {
        let tool = Tool {
            key: TOOL,
            body,
            place,
        };
        boolean_in(key, BooleanOp::Subtract, &moved, &[tool], meter)
    }
}

/// The key of the prisms a global shell extends its open faces by.
fn extension_key(key: OpKey) -> OpKey {
    let mut bytes = b"ryukei.shell.extension\0".to_vec();
    bytes.extend_from_slice(&key.0.to_le_bytes());
    OpKey::from_bytes(&bytes)
}

/// The offset a global shell subtracts or subtracts from (design 299):
/// every face at the thickness. Inwards, each open face (planar) is first
/// carried out by a prism the thickness deep, so the body's offset ends
/// on its plane; outwards, only a shell with no open face is global.
/// `None` where the global shell doesn't apply: faces of their own
/// thickness, an open face that isn't planar or whose prism meets the
/// body elsewhere, or an open face outwards.
fn global_shell(
    key: OpKey,
    body: &Body,
    d: f64,
    spec: &OffsetSpec,
    meter: &Meter,
) -> Result<Option<Body>, Error> {
    let open: Vec<Name> = spec
        .faces
        .iter()
        .filter(|x| x.1 == 0.0)
        .map(|x| x.0)
        .collect();
    if spec.faces.iter().any(|x| x.1 != 0.0) || (d > 0.0 && !open.is_empty()) {
        return Ok(None);
    }
    let mut prisms = Vec::new();
    for &f in &open {
        let Some(p) = prism(key, body, f, -d)? else {
            return Ok(None);
        };
        prisms.push(p);
    }
    let extended = if prisms.is_empty() {
        body.clone()
    } else {
        let tools: Vec<Tool> = prisms
            .iter()
            .enumerate()
            .map(|(i, p)| Tool {
                key: ToolKey(2 + i as u128),
                body: p,
                place: Rigid::IDENTITY,
            })
            .collect();
        boolean_in(extension_key(key), BooleanOp::Unite, body, &tools, meter)?.0
    };
    let uniform = OffsetSpec {
        distance: d,
        faces: Vec::new(),
    };
    Ok(super::global::global_in(offset_key(key), &extended, &uniform, meter)?.map(|x| x.0))
}

/// The open face `f` of `body` carried out along its outward normal by
/// `depth`: a prism of its loops, where it is planar. `None` where it
/// isn't, or where a fin has no pcurve.
fn prism(key: OpKey, body: &Body, f: Name, depth: f64) -> Result<Option<Body>, Error> {
    use crate::geom::Surface;
    use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use crate::topo::{EntityId, Sense};
    let Some(EntityId::Face(face)) = body.by_name(f) else {
        return Ok(None);
    };
    let fc = &body.faces[face];
    let Surface::Plane(plane) = &body.surfaces[fc.surface] else {
        return Ok(None);
    };
    let mut loops = Vec::new();
    for (i, l) in body.face_loops(face).enumerate() {
        let mut segments = Vec::new();
        for fin in body.loop_fins(l) {
            let fi = &body.fins[fin];
            let Some(pc) = fi.pcurve else {
                return Ok(None);
            };
            let e = &body.edges[fi.edge];
            let [a, b] = e.interval.map(|t| pc.param(t));
            let (t0, t1) = if fi.sense == Sense::Forward {
                (a, b)
            } else {
                (b, a)
            };
            segments.push(Segment {
                key: SegmentKey(e.name.to_u128()),
                curve: body.curves2[pc.curve].clone(),
                t0,
                t1,
            });
        }
        loops.push(Loop {
            key: LoopKey(i as u128 + 1),
            segments,
        });
    }
    let profile = ValidProfile::new(Profile {
        frame: *plane.frame(),
        loops,
    })?;
    let n = plane.frame().z().get();
    let n = if fc.sense == Sense::Forward { n } else { -n };
    Ok(Some(
        extrude(extension_key(key), &profile, n, 0.0, depth)?.0,
    ))
}
