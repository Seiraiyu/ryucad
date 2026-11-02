//! Edge flanges: bends and flanges added on straight sheet edges.
//!
//! Each [`EdgeFlange`](super::EdgeFlange) is planned in the frame of its
//! edge ([`plan`]): `s` along the edge from its start vertex, `x` into the
//! base from the wall the edge bounds, `h` along the side's outward normal,
//! the way the flange folds.
//!
//! Each flange's tools are exact sweeps ([`tools`]), keyed from the call's
//! key and the edge's name, so moving or resizing a flange renames nothing
//! but what it makes new or takes away. Two booleans apply them: every cut
//! is subtracted at once (under the key `SheetMetalRole::Cut` derives from
//! the call's), then every bend and flange is united (under the call's own
//! key), so tools' entities are `Placed` under their `ToolKey`s and the
//! base's keep their names. The result must recognize as sheet metal, with
//! each planned bend's inner face on a bend; the journal is the two
//! booleans', then each new bend's inner and outer faces `Created`.

pub(crate) mod plan;
pub(crate) mod tools;

#[cfg(test)]
mod tests;

use super::recognize::read;
use super::{EdgeFlange, SheetOptions};
use crate::boolean::{BooleanOp, Tool, boolean};
use crate::geom::{Rigid, Surface};
use crate::name::{Event, Journal, Name, OpKey, Role, SheetMetalRole, ToolKey};
use crate::topo::Body;
use crate::{Error, SheetMetalError};

/// The key `role` derives for the flange on `edge`: a tool's sweep and its
/// `ToolKey` share its value.
fn derived(key: OpKey, role: SheetMetalRole, edge: Option<Name>) -> u128 {
    let parents: Vec<Name> = edge.into_iter().collect();
    Name::derive(key, Role::SheetMetal(role), &parents).to_u128()
}

/// Adds the bends and flanges `flanges` describe to `body`, a sheet-metal
/// part: see the module's documentation. Refusals are
/// `Error::InvalidSheetMetal`, made before anything is built where
/// planning can tell, else the booleans' own (`Error::InvalidBoolean`) or
/// `NotSheetAfter`.
pub fn flange(
    key: OpKey,
    body: &Body,
    flanges: &[EdgeFlange],
    options: &SheetOptions,
) -> Result<(Body, Journal), Error> {
    let model = read(body, options)?.model;
    let tol = super::recognize::tolerance(options)?;
    let planned = plan::plan(body, &model, flanges, tol)?;
    // Every tool, before anything is applied.
    let mut cuts = Vec::new();
    let mut adds = Vec::new();
    let mut bends = Vec::new();
    for p in &planned {
        let k = |role| derived(key, role, Some(p.edge));
        if let Some(c) = tools::cut(OpKey(k(SheetMetalRole::Cut)), p)? {
            cuts.push((ToolKey(k(SheetMetalRole::Cut)), c));
        }
        let bend = tools::bend(OpKey(k(SheetMetalRole::Bend)), p)?;
        bends.push((ToolKey(k(SheetMetalRole::Bend)), faces_on(&bend, p.radius)));
        adds.push((ToolKey(k(SheetMetalRole::Bend)), bend));
        let f = tools::flange(OpKey(k(SheetMetalRole::Flange)), p)?;
        adds.push((ToolKey(k(SheetMetalRole::Flange)), f));
    }
    let mut journal = Journal::new();
    let cut_key = OpKey(derived(key, SheetMetalRole::Cut, None));
    let base = if cuts.is_empty() {
        body.clone()
    } else {
        let (b, j) = boolean(cut_key, BooleanOp::Subtract, body, &placed(&cuts))?;
        journal.append(j);
        b
    };
    let (out, j) = boolean(key, BooleanOp::Unite, &base, &placed(&adds))?;
    // What each bend's faces were named once placed.
    let mut renamed = std::collections::BTreeMap::new();
    for e in j.events() {
        if let Event::Placed { tool, from, to } = e {
            renamed.insert((*tool, *from), *to);
        }
    }
    journal.append(j);
    let after = match read(&out, options) {
        Ok(r) => r.model,
        Err(fault) => {
            return Err(SheetMetalError::NotSheetAfter {
                fault: Box::new(fault),
            }
            .into());
        }
    };
    for (tool, [inner, outer]) in bends {
        let name = |n: Name| renamed.get(&(tool, n)).copied().unwrap_or(n);
        let (inner, outer) = (name(inner), name(outer));
        if !after.bends.iter().any(|b| b.inner.contains(&inner)) {
            return Err(SheetMetalError::NotSheetAfter {
                fault: Box::new(SheetMetalError::NoSuchBend { name: inner }),
            }
            .into());
        }
        journal.push(Event::Created(inner));
        journal.push(Event::Created(outer));
    }
    Ok((out, journal))
}

/// Tools in place where they were made.
fn placed(tools: &[(ToolKey, Body)]) -> Vec<Tool<'_>> {
    tools
        .iter()
        .map(|(k, b)| Tool {
            key: *k,
            body: b,
            place: Rigid::IDENTITY,
        })
        .collect()
}

/// A bend tool's inner and outer cylinder faces: radius `r`, and the other.
fn faces_on(bend: &Body, r: f64) -> [Name; 2] {
    let mut inner = None;
    let mut outer = None;
    for f in bend.faces() {
        let face = &bend.faces[f];
        if let Surface::Cylinder(c) = &bend.surfaces[face.surface] {
            if (c.radius() - r).abs() <= crate::tol::LINEAR_RESOLUTION {
                inner = Some(face.name);
            } else {
                outer = Some(face.name);
            }
        }
    }
    [inner.expect("an inner side"), outer.expect("an outer side")]
}
