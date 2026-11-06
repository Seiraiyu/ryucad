//! Tangent chains: the faces a tweak's motion spreads to when asked.

use super::Motion;
use super::classify::tangent;
use crate::Error;
use crate::topo::{Body, FaceId};
use std::collections::{BTreeMap, VecDeque};

/// `motion` grown across every edge whose faces are tangent, repeatedly:
/// each face reached takes the motion of the face it was reached from.
/// A replacement doesn't spread.
pub(crate) fn grow(
    body: &Body,
    mut motion: BTreeMap<FaceId, Motion>,
) -> Result<BTreeMap<FaceId, Motion>, Error> {
    let name = |f: FaceId| body.faces[f].name;
    let mut queue: Vec<FaceId> = motion.keys().copied().collect();
    queue.sort_by_key(|&f| name(f));
    let mut queue: VecDeque<FaceId> = queue.into();
    while let Some(f) = queue.pop_front() {
        let m = motion[&f];
        if m == Motion::Replace {
            continue;
        }
        let mut next: Vec<(FaceId, crate::topo::EdgeId)> = Vec::new();
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let e = body.fins[fin].edge;
                if body.is_laminar(e) {
                    continue;
                }
                for other in body.edge_fins(e) {
                    let g = body.loops[body.fins[other].loop_].face;
                    if g != f && !motion.contains_key(&g) {
                        next.push((g, e));
                    }
                }
            }
        }
        next.sort_by_key(|&(g, e)| (name(g), body.edges[e].name));
        for (g, e) in next {
            if !motion.contains_key(&g) && tangent(body, e)? {
                motion.insert(g, m);
                queue.push_back(g);
            }
        }
    }
    Ok(motion)
}
