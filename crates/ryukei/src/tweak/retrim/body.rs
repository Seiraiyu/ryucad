//! The working body: the faces outside the region as they are, and every
//! face of the region as a working face (its working surface over its
//! parameter box, a sheet face of its own), laid out as one sheet. Every
//! face carries its original's name; working faces' sides and corners are
//! scaffolding.

use super::region::Region;
use super::working::Working;
use crate::geom::Box3;
use crate::layout::{self, Boundary, Layout};
use crate::name::{BooleanRole, EditRole, Name, OpKey, Role};
use crate::topo::{Body, EdgeId, EntityId, FaceId, Sense};
use crate::{Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// The working body and what its faces and edges stand for.
#[derive(Clone, Debug)]
pub(crate) struct WorkBody {
    pub(crate) body: Body,
    /// Each face's face in the input.
    pub(crate) origin: BTreeMap<FaceId, FaceId>,
    /// The working faces.
    pub(crate) working: BTreeSet<FaceId>,
    /// Working faces whose surfaces couldn't be carried where needed.
    pub(crate) fixed: BTreeSet<FaceId>,
    /// The working faces' own sides: a kept piece using one reaches past
    /// the region.
    pub(crate) clipped: BTreeSet<EdgeId>,
    /// The frontier edges, as edges of the working body.
    // Read by the tests; the pairs read `frontier_faces`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) frontier: BTreeSet<EdgeId>,
    /// Each frontier edge (the working body's) and its input faces.
    pub(crate) frontier_faces: BTreeMap<EdgeId, BTreeSet<FaceId>>,
    /// The names of the scaffolding: working faces' sides and corners.
    pub(crate) scaffold: BTreeSet<Name>,
}

/// A face's sheet key: unique per face within the re-trim.
fn sheet_key(key: OpKey, face: Name) -> OpKey {
    OpKey(Name::derive(key, Role::Edit(EditRole::Edge), &[face]).to_u128())
}

/// `face` run the other way round: under the other sense, its loops
/// reversed so they still run counterclockwise about its normal.
fn flipped(face: &mut layout::Face) {
    face.sense = match face.sense {
        Sense::Forward => Sense::Reversed,
        Sense::Reversed => Sense::Forward,
    };
    for l in &mut face.loops {
        if let Boundary::Arcs(uses) = l {
            uses.reverse();
            for u in uses.iter_mut() {
                u.sense = match u.sense {
                    Sense::Forward => Sense::Reversed,
                    Sense::Reversed => Sense::Forward,
                };
            }
        }
    }
}

/// The working body of `body` for `region`, its region faces on
/// `working`. A working surface whose box `sheet::from_surface` refuses
/// is `ExtensionUnsupported`; a layout that won't build (a pinch the
/// region missed) is `HealAmbiguous` naming the least region face.
pub(crate) fn work_body(
    key: OpKey,
    body: &Body,
    region: &Region,
    working: &BTreeMap<FaceId, Working>,
    m: &crate::work::Meter,
) -> Result<WorkBody, Error> {
    let mut lay = Layout::default();
    let outside: BTreeSet<FaceId> = body.faces().filter(|f| !region.faces.contains(f)).collect();
    let copied = crate::sheet::copy(&mut lay, body, &outside);
    let mut sheets = Vec::new();
    for (&f, w) in working {
        // Polled before each sheet's fitted sides, which are unmetered.
        m.poll().map_err(|_| Error::Cancelled)?;
        let name = body.faces[f].name;
        let (s, _) = crate::sheet::from_surface(sheet_key(key, name), &w.surface, w.rect)
            .map_err(|_| Error::from(TweakError::ExtensionUnsupported { face: name }))?;
        let only: BTreeSet<FaceId> = s.faces().collect();
        let c = crate::sheet::copy(&mut lay, &s, &only);
        let k = c.faces[0].1;
        if lay.faces[k].sense != w.sense {
            flipped(&mut lay.faces[k]);
        }
        sheets.push((f, s, c));
    }
    let least = region.faces.iter().map(|&f| body.faces[f].name).min();
    let built =
        layout::build_sheet(&lay, key, Role::Boolean(BooleanRole::Merged)).map_err(|_| {
            Error::from(TweakError::HealAmbiguous {
                face: least.unwrap_or(Name::from_u128(0)),
            })
        })?;
    let mut out = built.body.clone();
    crate::sheet::name_copied(&mut out, body, &copied, &built);
    let mut origin = BTreeMap::new();
    for &(f, k) in &copied.faces {
        origin.insert(built.faces[k], f);
    }
    let mut frontier = BTreeSet::new();
    let mut frontier_faces = BTreeMap::new();
    for (&e, &g) in &copied.edges {
        if region.frontier.contains(&e) {
            let we = built.edges[&g];
            frontier.insert(we);
            frontier_faces.insert(
                we,
                body.edge_fins(e)
                    .map(|fin| body.loops[body.fins[fin].loop_].face)
                    .collect(),
            );
        }
    }
    let (mut working_faces, mut fixed, mut clipped, mut scaffold) = (
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    for (f, s, c) in &sheets {
        crate::sheet::name_copied(&mut out, s, c, &built);
        let wf = built.faces[c.faces[0].1];
        out.rename(EntityId::Face(wf), body.faces[*f].name);
        origin.insert(wf, *f);
        working_faces.insert(wf);
        if !working[f].extendable {
            fixed.insert(wf);
        }
        for &g in c.edges.values() {
            let e = built.edges[&g];
            clipped.insert(e);
            scaffold.insert(out.edges[e].name);
        }
        for &g in c.vertices.values() {
            scaffold.insert(out.vertices[built.vertices[&g]].name);
        }
    }
    Ok(WorkBody {
        body: out,
        origin,
        working: working_faces,
        fixed,
        clipped,
        frontier,
        frontier_faces,
        scaffold,
    })
}

/// The box a re-trim works in: its region's faces' box grown by `reach`.
pub(crate) fn region_box(body: &Body, region: &Region, reach: f64) -> Box3 {
    region
        .faces
        .iter()
        .map(|&f| crate::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))
        .expect("a region holds a face")
        .grow(reach)
}

/// `b` doubled about its middle.
pub(crate) fn doubled(b: &Box3) -> Box3 {
    let (lo, hi) = (b.min(), b.max());
    let h = (hi - lo) * 0.5;
    Box3::new(lo - h, hi + h)
}

impl WorkBody {
    /// The pairs of faces the table meets, each in face order: every two
    /// working faces whose boxes meet, and each working face with each
    /// outside face it shares a frontier edge with (which imprints that
    /// edge on it). Outside faces are never met with each other.
    pub(crate) fn pairs(&self) -> BTreeSet<(FaceId, FaceId)> {
        let b = &self.body;
        let boxes: BTreeMap<FaceId, Box3> = b
            .faces()
            .map(|f| (f, crate::intersect::face_box(b, f)))
            .collect();
        let mut out = BTreeSet::new();
        let list: Vec<FaceId> = self.working.iter().copied().collect();
        for (i, &a) in list.iter().enumerate() {
            for &c in &list[i + 1..] {
                if boxes[&a].intersects(&boxes[&c]) {
                    out.insert((a.min(c), a.max(c)));
                }
            }
        }
        for (&e, faces) in &self.frontier_faces {
            // The outside face across the frontier edge.
            let Some(o) = b
                .edge_fins(e)
                .map(|fin| b.loops[b.fins[fin].loop_].face)
                .find(|f| !self.working.contains(f))
            else {
                continue;
            };
            for &wf in &self.working {
                if faces.contains(&self.origin[&wf]) {
                    out.insert((wf.min(o), wf.max(o)));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::region::region;
    use super::super::tests::{K, block, face_of};
    use super::super::working::working;
    use super::*;
    use crate::geom::Point3;
    use crate::name::FaceRole;

    #[test]
    fn a_raised_top_works_on_its_bottom_and_five_working_faces() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let r = region(&b, &BTreeSet::from([top]));
        let bx = Box3::new(Point3::new(-0.2, -0.2, -0.2), Point3::new(1.2, 0.8, 0.8));
        let ws: BTreeMap<FaceId, Working> = r
            .faces
            .iter()
            .map(|&f| {
                (
                    f,
                    working(&b, f, None, b.faces[f].sense, true, &bx).unwrap(),
                )
            })
            .collect();
        // unmetered: a unit test.
        let w = work_body(K, &b, &r, &ws, &crate::work::Meter::free()).unwrap();
        assert_eq!(w.body.kind(), crate::topo::BodyKind::Sheet);
        assert!(crate::check::check_topology(&w.body).is_empty());
        assert_eq!(w.body.faces().count(), 6);
        assert_eq!(w.working.len(), 5);
        // Every face keeps its original's name.
        for f in w.body.faces() {
            let n = w.body.faces[f].name;
            assert_eq!(w.body.faces[f].name, b.faces[w.origin[&f]].name, "{n}");
        }
        assert_eq!(w.frontier.len(), 4);
        // Five rectangles' sides.
        assert_eq!(w.clipped.len(), 20);
    }
    #[test]
    fn working_faces_meet_each_other_and_their_frontier_neighbours() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let r = region(&b, &BTreeSet::from([top]));
        let bx = Box3::new(Point3::new(-0.2, -0.2, -0.2), Point3::new(1.2, 0.8, 0.8));
        let ws: BTreeMap<FaceId, Working> = r
            .faces
            .iter()
            .map(|&f| {
                (
                    f,
                    working(&b, f, None, b.faces[f].sense, true, &bx).unwrap(),
                )
            })
            .collect();
        // unmetered: a unit test.
        let w = work_body(K, &b, &r, &ws, &crate::work::Meter::free()).unwrap();
        let pairs = w.pairs();
        let bottom = *w
            .origin
            .iter()
            .find(|(_, o)| **o == face_of(&b, FaceRole::MinusZ))
            .unwrap()
            .0;
        // Each side with the bottom, across its frontier edge.
        assert_eq!(
            pairs
                .iter()
                .filter(|&&(a, c)| a == bottom || c == bottom)
                .count(),
            4
        );
        // The top with each side, and each side with its two neighbours;
        // opposite sides' boxes are flat slabs apart: 8 pairs.
        let working_pairs = pairs
            .iter()
            .filter(|&&(a, c)| w.working.contains(&a) && w.working.contains(&c))
            .count();
        assert_eq!(working_pairs, 8);
        for &(a, c) in &pairs {
            assert!(a < c);
        }
    }
}
