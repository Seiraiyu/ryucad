//! Placing tools: each tool's body moved into place, in parallel.

use super::Tool;
use crate::topo::Body;
use crate::{BooleanError, Error};
use rayon::prelude::*;

/// The operands: the target, then every tool moved by its placement.
/// Fails with `OutsideSizeBox` naming the first tool that leaves the
/// size box.
pub(super) fn operands(target: &Body, tools: &[Tool]) -> Result<Vec<Body>, Error> {
    let placed: Vec<Body> = tools
        .par_iter()
        .map(|t| {
            t.body
                .transformed(&t.place)
                .map_err(|_| Error::from(BooleanError::OutsideSizeBox { tool: t.key }))
        })
        // The first failure in order, not the first a thread found.
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<_, _>>()?;
    let mut ops = Vec::with_capacity(tools.len() + 1);
    ops.push(target.clone());
    ops.extend(placed);
    Ok(ops)
}
