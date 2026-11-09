//! Deleted faces in connected sets, and the loops of the faces round each
//! set that run wholly along it: where a cap would close it.

use super::hole::is_hole;
use crate::geom::Surface;
use crate::topo::{Body, EdgeId, FaceId, LoopId};
use std::collections::{BTreeMap, BTreeSet};

/// One connected set of deleted faces.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Set {
    pub(crate) faces: BTreeSet<FaceId>,
    /// Edges between a face of the set and a surviving face.
    pub(crate) boundary: BTreeSet<EdgeId>,
    /// Loops of surviving faces every edge of which the set's faces use.
    pub(crate) hosts: Vec<(FaceId, LoopId)>,
}

/// The faces using edge `e`.
pub(crate) fn faces_of(body: &Body, e: EdgeId) -> Vec<FaceId> {
    body.edge_fins(e)
        .map(|fin| body.loops[body.fins[fin].loop_].face)
        .collect()
}

/// `deleted` in sets joined across shared edges, each in face order, the
/// sets in the order of their least face.
pub(crate) fn sets(body: &Body, deleted: &BTreeSet<FaceId>) -> Vec<Set> {
    let mut parent: BTreeMap<FaceId, FaceId> = deleted.iter().map(|&f| (f, f)).collect();
    fn find(p: &mut BTreeMap<FaceId, FaceId>, mut f: FaceId) -> FaceId {
        while p[&f] != f {
            let up = p[&p[&f]];
            p.insert(f, up);
            f = up;
        }
        f
    }
    for e in body.edges() {
        let fs: Vec<FaceId> = faces_of(body, e)
            .into_iter()
            .filter(|f| deleted.contains(f))
            .collect();
        for w in fs.windows(2) {
            let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
            if a != b {
                parent.insert(a.max(b), a.min(b));
            }
        }
    }
    let mut by_root: BTreeMap<FaceId, BTreeSet<FaceId>> = BTreeMap::new();
    for &f in deleted {
        let r = find(&mut parent, f);
        by_root.entry(r).or_default().insert(f);
    }
    by_root
        .into_values()
        .map(|faces| {
            let boundary: BTreeSet<EdgeId> = body
                .edges()
                .filter(|&e| {
                    let fs = faces_of(body, e);
                    fs.iter().any(|f| faces.contains(f)) && fs.iter().any(|f| !deleted.contains(f))
                })
                .collect();
            let mut hosts = Vec::new();
            let around: BTreeSet<FaceId> = boundary
                .iter()
                .flat_map(|&e| faces_of(body, e))
                .filter(|f| !deleted.contains(f))
                .collect();
            for g in around {
                for l in body.face_loops(g) {
                    let mut fins = body.loop_fins(l).peekable();
                    if fins.peek().is_none() {
                        continue;
                    }
                    if fins.all(|fin| boundary.contains(&body.fins[fin].edge)) {
                        hosts.push((g, l));
                    }
                }
            }
            Set {
                faces,
                boundary,
                hosts,
            }
        })
        .collect()
}

impl Set {
    /// Whether a cap closes the set: every boundary edge on a host loop,
    /// each a hole of its face, the face keeping a loop or closing up.
    pub(crate) fn closable(&self, body: &Body) -> bool {
        let on: BTreeSet<EdgeId> = self
            .hosts
            .iter()
            .flat_map(|&(_, l)| body.loop_fins(l).map(|fin| body.fins[fin].edge))
            .collect();
        if on != self.boundary {
            return false;
        }
        self.hosts.iter().all(|&(g, l)| {
            let left = body.face_loops(g).filter(|&k| k != l).count();
            let closes = matches!(
                body.surfaces[body.faces[g].surface],
                Surface::Sphere(_) | Surface::Torus(_)
            );
            // Loops this set takes from the same face all go together.
            let taken = self.hosts.iter().filter(|&&(h, _)| h == g).count();
            (left + 1 > taken || closes) && is_hole(body, g, l)
        })
    }
}
