//! Unchanged faces whose surfaces must reach past their domains.

use super::classify::Class;
use super::curves::vertex_reach;
use crate::geom::Surface;
use crate::intersect::{carried_by, face_rect};
use crate::offset::classify::Plan;
use crate::topo::{EdgeId, FaceId, VertexId};
use crate::{Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// How far past each end of each direction of `whole` a face's parameter
/// box `rect` must be carried, for points `reach` away in model space
/// (three times it, in the parameter, by `speed` each way): only at a
/// bounded direction's end the box comes within that of.
pub(crate) fn margins(
    rect: [[f64; 2]; 2],
    whole: [[f64; 2]; 2],
    periods: [Option<f64>; 2],
    speed: [f64; 2],
    reach: f64,
) -> [[f64; 2]; 2] {
    core::array::from_fn(|k| {
        if periods[k].is_some() || speed[k] <= 0.0 {
            return [0.0, 0.0];
        }
        let m = 3.0 * reach / speed[k];
        let near = |a: f64, b: f64| (a - b).abs() <= m;
        [
            if near(rect[k][0], whole[k][0]) {
                m
            } else {
                0.0
            },
            if near(rect[k][1], whole[k][1]) {
                m
            } else {
                0.0
            },
        ]
    })
}

/// Carries, in `plan`, each unchanged face on a bounded surface whose
/// edges move past its domain's ends where its box comes near them.
pub(crate) fn extend(plan: &mut Plan, class: &BTreeMap<EdgeId, Class>) -> Result<(), Error> {
    let body = plan.body;
    let mut faces: Vec<FaceId> = body
        .faces()
        .filter(|f| !plan.motion.contains_key(f))
        .filter(|&f| {
            matches!(
                body.surfaces[body.faces[f].surface],
                Surface::Nurbs(_) | Surface::Extrusion(_) | Surface::Revolution(_)
            )
        })
        .collect();
    faces.sort_by_key(|&f| body.faces[f].name);
    for f in faces {
        let mut vs: BTreeSet<VertexId> = BTreeSet::new();
        let mut moving = false;
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let e = body.fins[fin].edge;
                if class.get(&e).is_some_and(|c| *c != Class::Kept) {
                    moving = true;
                }
                vs.insert(body.edges[e].start);
                vs.insert(body.edges[e].end);
            }
        }
        if !moving {
            continue;
        }
        let reach = vs
            .iter()
            .map(|&v| vertex_reach(plan, v))
            .fold(0.0, f64::max);
        let s = &body.surfaces[body.faces[f].surface];
        let whole = s.domain();
        let (pu, pv) = s.periods();
        let rect = face_rect(body, f).unwrap_or(whole);
        let mid = s.eval(
            (rect[0][0] + rect[0][1]) / 2.0,
            (rect[1][0] + rect[1][1]) / 2.0,
        );
        let speed = [mid.du.length(), mid.dv.length()];
        let m = margins(rect, whole, [pu, pv], speed, reach);
        if m == [[0.0; 2]; 2] {
            continue;
        }
        let carried = carried_by(s, m).ok_or(TweakError::ExtensionUnsupported {
            face: body.faces[f].name,
        })?;
        plan.surface.insert(f, carried);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margins_reach_past_the_ends_a_box_comes_near() {
        let whole = [[0.0, 4.0], [0.0, 4.0]];
        // Speed 1 and a reach of 0.25: margins of 0.75 where the box
        // comes within 0.75 of an end (it touches u's top, v's bottom).
        let rect = [[3.5, 4.0], [0.0, 0.5]];
        let m = margins(rect, whole, [None, None], [1.0, 1.0], 0.25);
        assert_eq!(m, [[0.0, 0.75], [0.75, 0.0]]);
        // A periodic direction is never carried.
        let m = margins(rect, whole, [Some(4.0), None], [1.0, 1.0], 0.25);
        assert_eq!(m, [[0.0, 0.0], [0.75, 0.0]]);
        // Clear of every end: nothing.
        let m = margins(
            [[1.5, 2.5], [1.5, 2.5]],
            whole,
            [None, None],
            [1.0, 1.0],
            0.25,
        );
        assert_eq!(m, [[0.0; 2]; 2]);
    }
}
