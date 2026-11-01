//! A profile's region as a planar sheet (design 295, plan 12.B).

use super::{ValidProfile, extrude};
use crate::error::{Error, SheetError};
use crate::name::{Event, Journal, Name, OpKey, Role, SweepRole};
use crate::topo::Body;

/// How thick the extrude whose start cap is the sheet is: thick enough to
/// extrude (`10 × LINEAR_RESOLUTION`), thin enough to stay in the size
/// box wherever the profile does.
const THICKNESS: f64 = 1e-6;

/// One planar face per lump of `profile`, its holes as inner loops, its
/// front along the profile's normal: a sheet. Every face, edge and vertex
/// is named as an extrude's start cap names them, so the face is the
/// start cap an extrude with `key` would make. A profile with chains, or
/// with no loops, is `SheetError::OpenProfile`.
pub fn planar(key: OpKey, profile: &ValidProfile) -> Result<(Body, Journal), Error> {
    if profile.lumps().is_empty() || !profile.chains().is_empty() {
        return Err(SheetError::OpenProfile.into());
    }
    let loops = profile.loops_only();
    // Extruded against the normal, the start cap lies on the profile's
    // plane facing along it.
    let down = -profile.frame().z().get();
    let (solid, _) = extrude::solid(key, &loops, down, 0.0, THICKNESS)?;
    let caps: Vec<Name> = profile
        .lumps()
        .iter()
        .map(|lump| {
            let outer = Name::from_u128(lump.outer.key.0);
            let lp = Name::derive(key, Role::Sweep(SweepRole::Loop), &[outer]);
            Name::derive(key, Role::Sweep(SweepRole::StartCap), &[lp])
        })
        .collect();
    let (sheet, _) = crate::sheet::extract(key, &solid, &caps)?;
    let mut journal = Journal::new();
    for f in sheet.faces() {
        journal.push(Event::Created(sheet.faces[f].name));
    }
    for e in sheet.edges() {
        journal.push(Event::Created(sheet.edges[e].name));
    }
    for v in sheet.vertices() {
        journal.push(Event::Created(sheet.vertices[v].name));
    }
    Ok((sheet, journal))
}
