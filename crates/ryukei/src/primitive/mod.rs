//! Primitive solids: box (`block`), cylinder, cone, frustum, sphere and
//! torus.
//!
//! Each is built in a fixed local frame entirely through Euler operators,
//! then moved by a rigid placement, then named by role: faces by what they
//! are (`+Z`, `side`, ...), edges and vertices by the faces around them. A
//! primitive's journal lists only `Created` events: faces, then edges, then
//! vertices, each in array order. The names depend only on the key, never
//! on the placement or the dimensions.

mod block;
mod closed;
mod turned;

pub use block::block;
pub use closed::{sphere, torus};
pub use turned::{cone, cylinder, frustum};

use crate::Error;
use crate::geom::{Point3, Rigid};
use crate::name::{Event, FaceRole, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EntityId, FaceId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// A dimension from outside the kernel: finite and at least
/// [`LINEAR_RESOLUTION`].
fn size(x: f64) -> Result<f64, Error> {
    if !x.is_finite() {
        Err(Error::NonFinite)
    } else if x < LINEAR_RESOLUTION {
        Err(Error::Degenerate)
    } else {
        Ok(x)
    }
}

/// Fails unless the placement maps the local box `[lo, hi]` inside the
/// size box. The box is convex, so its corners are enough.
fn check_extent(placement: &Rigid, lo: Point3, hi: Point3) -> Result<(), Error> {
    for i in 0..8 {
        let pick = |bit: usize, a: f64, b: f64| if i & bit == 0 { a } else { b };
        let c = Point3::new(
            pick(1, lo.x, hi.x),
            pick(2, lo.y, hi.y),
            pick(4, lo.z, hi.z),
        );
        let p = placement.apply_point(c);
        Point3::try_new(p.x, p.y, p.z)?;
    }
    Ok(())
}

/// Names every entity by role and reports it: faces from `roles`, edges
/// and vertices from the sorted names of the faces around them. Then
/// gives every fin its exact pcurve.
fn name_by_role(
    body: &mut Body,
    kind: &str,
    key: OpKey,
    roles: &[(FaceId, FaceRole)],
) -> Result<Journal, Error> {
    let mut journal = Journal::new();
    for &(f, role) in roles {
        let name = body.derive_name(kind, key, Role::Face(role), &[]);
        body.rename(EntityId::Face(f), name);
    }
    let face_name = |b: &Body, f: FaceId| b.faces[f].name;
    // The faces around each vertex: every loop that passes through it.
    let mut around: BTreeMap<VertexId, BTreeSet<Name>> = BTreeMap::new();
    for (l, lp) in body.loops.live() {
        let name = face_name(body, lp.face);
        if let Some(v) = lp.vertex {
            around.entry(v).or_default().insert(name);
        }
        for fin in body.loop_fins(l) {
            around.entry(body.start_of(fin)).or_default().insert(name);
        }
    }
    let edges: Vec<_> = body.edges().collect();
    for e in edges {
        let faces: BTreeSet<Name> = body
            .edge_fins(e)
            .map(|f| face_name(body, body.loops[body.fins[f].loop_].face))
            .collect();
        let faces: Vec<Name> = faces.into_iter().collect();
        let name = body.derive_name(kind, key, Role::Edge, &faces);
        body.rename(EntityId::Edge(e), name);
    }
    for (v, faces) in around {
        let faces: Vec<Name> = faces.into_iter().collect();
        let name = body.derive_name(kind, key, Role::Vertex, &faces);
        body.rename(EntityId::Vertex(v), name);
    }
    for (_, f) in body.faces.live() {
        journal.push(Event::Created(f.name));
    }
    for (_, e) in body.edges.live() {
        journal.push(Event::Created(e.name));
    }
    for (_, v) in body.vertices.live() {
        journal.push(Event::Created(v.name));
    }
    crate::pcurve::fill(body)?;
    if cfg!(debug_assertions) {
        let faults = crate::check::check(body);
        assert!(faults.is_empty(), "{kind} primitive is invalid: {faults:?}");
    }
    Ok(journal)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::check::check;
    use crate::geom::{UnitVector3, Vector3};
    use crate::topo::Counts;

    pub(crate) const K: OpKey = OpKey(0x2a);

    /// A placement that moves and turns things so nothing lines up with the
    /// world axes.
    pub(crate) fn tilted() -> Rigid {
        let axis = Vector3::new(1.0, 2.0, 3.0).normalize().unwrap();
        Rigid::translation(Vector3::new(10.0, -20.0, 30.0)).then_after(&Rigid::rotation(axis, 0.7))
    }

    pub(crate) fn solid(v: usize, e: usize, f: usize, l: usize) -> Counts {
        Counts {
            regions: 2,
            shells: 1,
            faces: f,
            loops: l,
            fins: 2 * e,
            edges: e,
            vertices: v,
        }
    }

    /// Checks a primitive's body and journal, and that its names do not
    /// depend on the placement.
    pub(crate) fn sound(
        build: impl Fn(OpKey, &Rigid) -> Result<(Body, Journal), Error>,
        counts: Counts,
    ) -> Body {
        let (b, j) = build(K, &Rigid::IDENTITY).unwrap();
        assert_eq!(b.counts(), counts);
        assert_eq!(check(&b), []);
        assert_eq!(
            j.events().len(),
            counts.faces + counts.edges + counts.vertices
        );
        assert!(j.events().iter().all(|e| matches!(e, Event::Created(_))));
        assert_eq!(b.names.len(), j.events().len());
        let (moved, j2) = build(K, &tilted()).unwrap();
        assert_eq!(check(&moved), []);
        assert_eq!(j, j2);
        let (other, j3) = build(OpKey(7), &Rigid::IDENTITY).unwrap();
        for e in j3.events() {
            assert!(!j.events().contains(e));
        }
        assert_eq!(other.dump_unnamed(), b.dump_unnamed());
        b
    }

    #[test]
    fn extents_must_fit_the_size_box() {
        let lo = Point3::new(0.0, 0.0, 0.0);
        let hi = Point3::new(1.0, 1.0, 1.0);
        assert_eq!(check_extent(&Rigid::IDENTITY, lo, hi), Ok(()));
        let far = Rigid::translation(Vector3::new(499.5, 0.0, 0.0));
        assert_eq!(check_extent(&far, lo, hi), Err(Error::OutsideSizeBox));
        let turned = Rigid::rotation(UnitVector3::Z, f64::NAN);
        assert_eq!(check_extent(&turned, lo, hi), Err(Error::NonFinite));
        assert_eq!(size(0.0), Err(Error::Degenerate));
        assert_eq!(size(f64::INFINITY), Err(Error::NonFinite));
    }
}
