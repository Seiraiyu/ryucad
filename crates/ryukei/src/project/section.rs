//! Sections (design 326, phase 4): a body cut by a plane, through the
//! boolean, and the faces the cut makes (the cap a drawing hatches).

use crate::Error;
use crate::boolean::{BooleanOp, Tool, boolean_in};
use crate::geom::{Box3, Frame, Plane, Point3, Rigid, Vector3};
use crate::intersect::face_box;
use crate::name::ToolKey;
use crate::name::{Event, Journal, Name, OpKey};
use crate::primitive;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use crate::work::{Meter, Work};
use std::collections::BTreeSet;

/// What a section subtracts: a block whose base lies on the plane.
pub(crate) struct HalfSpace {
    pub body: Body,
    /// The name of the face on the plane, before the boolean places it.
    pub base: Name,
}

/// The box of a body's faces (each face's enclosure), or `None` for a
/// body with no faces.
pub(crate) fn extent(body: &Body) -> Option<(Point3, Point3)> {
    let b: Box3 = body
        .faces()
        .map(|f| face_box(body, f))
        .reduce(|a, b| a.union(&b))?;
    Some((b.min(), b.max()))
}

/// Whether every vertex of `f`'s loops lies within resolution of the
/// plane through `p0` square to `n`.
pub(crate) fn on_plane(b: &Body, f: FaceId, p0: Point3, n: Vector3) -> bool {
    b.face_loops(f).all(|l| {
        let lp = &b.loops[l];
        lp.vertex
            .map(|v| ((b.vertices[v].point - p0).dot(n)).abs() <= LINEAR_RESOLUTION)
            .unwrap_or(true)
            && b.loop_fins(l).all(|fin| {
                let v = b.start_of(fin);
                ((b.vertices[v].point - p0).dot(n)).abs() <= LINEAR_RESOLUTION
            })
    })
}

/// The half-space in front of `plane` (along its frame's `z`, the side a
/// section cuts away), as a block a hundredth of the body's diagonal past
/// its box every other way. `None` when nothing of the body's box lies
/// more than resolution in front of the plane: the plane touches the
/// body at most, and the section is the body as it is.
pub(crate) fn half_space(
    key: OpKey,
    body: &Body,
    plane: &Plane,
) -> Result<Option<HalfSpace>, Error> {
    let frame = plane.frame();
    let (p0, n) = (frame.origin(), frame.z().get());
    let Some((lo, hi)) = extent(body) else {
        return Ok(None);
    };
    let mut far = f64::NEG_INFINITY;
    for x in [lo.x, hi.x] {
        for y in [lo.y, hi.y] {
            for z in [lo.z, hi.z] {
                far = far.max((Point3::new(x, y, z) - p0).dot(n));
            }
        }
    }
    if far <= LINEAR_RESOLUTION {
        return Ok(None);
    }
    let diagonal = (hi - lo).length();
    let margin = diagonal.max(1e-6) * 0.01;
    // The box's middle brought onto the plane.
    let middle = Point3::new(
        (lo.x + hi.x) / 2.0,
        (lo.y + hi.y) / 2.0,
        (lo.z + hi.z) / 2.0,
    );
    let foot = middle - n * (middle - p0).dot(n);
    let hint = if n.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let axes = Frame::new(foot, n, hint)?;
    let w = diagonal / 2.0 + margin;
    let (x, y) = (axes.x().get(), axes.y().get());
    let corner = foot - x * w - y * w;
    let place = Rigid::from_frame(&Frame::new(corner, n, x)?);
    let (tool, _) = primitive::block(key, 2.0 * w, 2.0 * w, far + margin, &place)?;
    let base = tool
        .faces()
        .find(|&f| on_plane(&tool, f, p0, n))
        .map(|f| tool.faces[f].name)
        .expect("a block has a face on its base");
    Ok(Some(HalfSpace { body: tool, base }))
}

/// The faces of `body` the boolean made of the half-space's base: its
/// placed name, the pieces it was split into, and what they were merged
/// into, followed through `journal` until nothing more is named. In
/// `FaceId` order.
pub(crate) fn caps(body: &Body, journal: &Journal, base: Name) -> Vec<FaceId> {
    let mut names = BTreeSet::from([base]);
    loop {
        let before = names.len();
        for e in journal.events() {
            match e {
                Event::Placed { from, to, .. } if names.contains(from) => {
                    names.insert(*to);
                }
                Event::Split { parent, children } if names.contains(parent) => {
                    names.extend(children.iter().copied());
                }
                Event::Merged { parents, child } if parents.iter().any(|p| names.contains(p)) => {
                    names.insert(*child);
                }
                _ => {}
            }
        }
        if names.len() == before {
            break;
        }
    }
    body.faces()
        .filter(|&f| names.contains(&body.faces[f].name))
        .collect()
}

/// The key the half-space is placed under in a section's boolean: its
/// entities' names derive from the section's key and this.
pub const SECTION_TOOL: ToolKey = ToolKey(0x5ec7_10c0);

/// A body cut by a plane.
#[derive(Clone, Debug)]
pub struct Section {
    /// What lies on and behind the plane (against its normal).
    pub body: Body,
    /// The boolean's journal: what happened to the body's names, and the
    /// half-space's entities `Placed` under [`SECTION_TOOL`].
    pub journal: Journal,
    /// The faces lying on the plane that the cut made (the cap a drawing
    /// hatches), in `FaceId` order. A sheet's section has none.
    pub faces: Vec<FaceId>,
}

/// [`section_with`] with no cancel or budget.
pub fn section(key: OpKey, body: &Body, plane: &Plane) -> Result<Section, Error> {
    section_with(key, body, plane, &Work::unlimited())
}

/// `body` (a solid or a sheet) less what lies in front of `plane`, the
/// side its frame's `z` points to (as ryucad's section planes point to
/// what is cut away), through [`crate::boolean::boolean`]: every
/// certificate of the boolean holds, and refusals are its
/// (`Error::InvalidBoolean`), with `Cancelled` and `BudgetExceeded` as
/// every `_with` call. A plane the body only touches, or lies wholly
/// behind, gives the body as it is with an empty journal; one with the
/// body wholly in front gives the empty body. Names are the boolean's:
/// the body's entities keep theirs, the cap's are the half-space's base
/// `Placed` under `key` and [`SECTION_TOOL`], split by what cut it.
pub fn section_with(key: OpKey, body: &Body, plane: &Plane, work: &Work) -> Result<Section, Error> {
    crate::work::run(work, |m| section_in(key, body, plane, m))
}

fn section_in(key: OpKey, body: &Body, plane: &Plane, m: &Meter) -> Result<Section, Error> {
    let Some(h) = half_space(key, body, plane)? else {
        return Ok(Section {
            body: body.clone(),
            journal: Journal::new(),
            faces: Vec::new(),
        });
    };
    let tool = Tool {
        key: SECTION_TOOL,
        body: &h.body,
        place: Rigid::IDENTITY,
    };
    let (cut, journal) = boolean_in(key, BooleanOp::Subtract, body, &[tool], m)?;
    let faces = caps(&cut, &journal, h.base);
    Ok(Section {
        body: cut,
        journal,
        faces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, Point3, Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    fn plane(origin: Point3, normal: Vector3) -> Plane {
        let hint = if normal.x.abs() < 0.9 {
            Vector3::new(1.0, 0.0, 0.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        Plane::new(Frame::new(origin, normal, hint).unwrap())
    }

    #[test]
    fn the_half_space_stands_on_the_plane_in_front_of_it() {
        let (b, _) = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
        let p = plane(Point3::new(0.02, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
        let h = half_space(OpKey(2), &b, &p).unwrap().unwrap();
        // Its base lies on the plane, and every vertex is on it or in
        // front of it.
        let base = h
            .body
            .faces()
            .find(|f| h.body.faces[*f].name == h.base)
            .unwrap();
        assert!(on_plane(
            &h.body,
            base,
            Point3::new(0.02, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0)
        ));
        for (_, v) in h.body.vertices.live() {
            assert!(v.point.x >= 0.02 - 1e-15, "{:?}", v.point);
        }
        // It covers the body's far side.
        let (lo, hi) = extent(&h.body).unwrap();
        assert!(lo.y < 0.0 && hi.y > 0.03 && lo.z < 0.0 && hi.z > 0.01 && hi.x > 0.04);
    }

    #[test]
    fn a_plane_on_a_face_or_past_the_body_needs_no_half_space() {
        let (b, _) = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
        let top = plane(Point3::new(0.0, 0.0, 0.01), Vector3::new(0.0, 0.0, 1.0));
        assert!(half_space(OpKey(2), &b, &top).unwrap().is_none());
        let below = plane(Point3::new(0.0, 0.0, 0.0), Vector3::new(0.0, 0.0, -1.0));
        assert!(half_space(OpKey(2), &b, &below).unwrap().is_none());
        assert!(
            half_space(OpKey(2), &crate::topo::Body::new(), &top)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn caps_follow_the_base_through_placing_splits_and_merges() {
        use crate::name::{Event, Journal, Name, ToolKey};
        let (b, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
        let faces: Vec<_> = b.faces().collect();
        let name = |i: usize| b.faces[faces[i]].name;
        let (base, placed, piece) = (Name::from_u128(1), Name::from_u128(2), Name::from_u128(3));
        let mut j = Journal::new();
        // Out of order on purpose: the merge names the piece before the
        // split makes it.
        j.push(Event::merged(piece, name(4), name(2)));
        j.push(Event::Split {
            parent: placed,
            children: vec![name(0), piece],
        });
        j.push(Event::Placed {
            tool: ToolKey(1),
            from: base,
            to: placed,
        });
        j.push(Event::Modified(name(5)));
        assert_eq!(caps(&b, &j, base), vec![faces[0], faces[2]]);
        assert_eq!(caps(&b, &Journal::new(), base), vec![]);
    }

    use crate::measure::{Entity, Properties, properties};

    fn area(b: &crate::topo::Body, f: FaceId) -> f64 {
        match properties(b, Entity::Face(f)).unwrap() {
            Properties::Face { area, .. } => area.value(),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn a_block_cut_in_half_keeps_its_near_half_and_names_its_cap() {
        let (b, _) = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
        let p = plane(Point3::new(0.01, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
        let s = section(OpKey(3), &b, &p).unwrap();
        assert_eq!(crate::check::check(&s.body), []);
        assert_eq!(s.body.counts().faces, 6);
        assert_eq!(s.faces.len(), 1);
        assert!((area(&s.body, s.faces[0]) - 0.03 * 0.01).abs() < 1e-15);
        for (_, v) in s.body.vertices.live() {
            assert!(v.point.x <= 0.01 + 1e-15);
        }
    }

    #[test]
    fn planes_that_miss_keep_the_body_and_planes_behind_it_empty_it() {
        let (b, _) = primitive::block(OpKey(1), 0.04, 0.03, 0.01, &Rigid::IDENTITY).unwrap();
        let past = plane(Point3::new(0.0, 0.0, 0.02), Vector3::new(0.0, 0.0, 1.0));
        let s = section(OpKey(3), &b, &past).unwrap();
        assert_eq!(s.body.dump(), b.dump());
        assert!(s.journal.is_empty() && s.faces.is_empty());
        let behind = plane(Point3::new(0.0, 0.0, -0.01), Vector3::new(0.0, 0.0, 1.0));
        let s = section(OpKey(3), &b, &behind).unwrap();
        assert_eq!(s.body.faces().count(), 0);
        assert!(s.faces.is_empty());
    }
}
