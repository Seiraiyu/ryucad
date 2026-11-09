//! The tweak engine: the offset's local build, its plan given motions.

use super::Motion;
use super::classify::{Class, classes_with};
use super::{curves, extend, vertices};
use crate::geom::Surface;
use crate::name::{Event, Journal, Name, OpKey};
use crate::offset::build::{self, Side};
use crate::topo::{Body, FaceId, Sense, VertexId};
use crate::work::Meter;
use crate::{Error, OffsetCase, OffsetError, TweakCase, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// What a tweak changes: each changed face's motion and new surface, and
/// the sense a replaced face takes on its new one.
#[derive(Clone, Debug, Default)]
pub(crate) struct Changes {
    pub(crate) motion: BTreeMap<FaceId, Motion>,
    pub(crate) surface: BTreeMap<FaceId, Surface>,
    pub(crate) senses: BTreeMap<FaceId, Sense>,
    /// Each drafted face's hinge (a draft's only).
    pub(crate) drafts: BTreeMap<FaceId, super::draft::Drafted>,
    /// The parting line's edges, which stay where they are.
    pub(crate) hinges: BTreeSet<crate::topo::EdgeId>,
}

/// The tweaked body and its journal, or the refusal, under `meter`.
pub(crate) fn run(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    built(key, body, changes, meter).map_err(translate)
}

/// [`run`] under `topology`: with `Change`, the re-trim where the fixed
/// engine refuses a change of topology ([`super::retrim::takes`]); its
/// refusal is then the answer. The fixed engine's result stands wherever
/// it builds.
pub(crate) fn run_with(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    topology: super::Topology,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    let fixed = run(key, body, changes, meter);
    match fixed {
        Err(e) if topology == super::Topology::Change && super::retrim::takes(&e) => {
            super::retrim::retrim(key, body, changes, meter)
        }
        r => r,
    }
}

fn built(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    let class = classes_with(body, &changes.motion, &changes.hinges)?;
    let surface = body
        .faces()
        .map(|f| {
            let s = changes
                .surface
                .get(&f)
                .cloned()
                .unwrap_or_else(|| body.surfaces[body.faces[f].surface].clone());
            (f, s)
        })
        .collect();
    let mut plan = crate::offset::classify::Plan::for_tweak(
        body,
        changes.motion.clone(),
        surface,
        changes.senses.clone(),
        curves::offset_classes(body, &changes.motion, &class),
    );
    extend::extend(&mut plan, &class)?;
    let geo = curves::curves(&plan, &class, &changes.drafts, meter)?;
    let rails = curves::rails(&plan, meter)?;
    let vgeo = vertices::vertices(&plan, &class, &geo, &rails, meter)?;
    let side = Side {
        plan: &plan,
        geo: &geo,
        vgeo: &vgeo,
        rails: &rails,
        flip: false,
    };
    let (out, journal) = build::build(key, &side, meter)?;
    Ok((out, kept_changes(body, changes, &class, &vgeo, journal)))
}

/// The journal with `Modified` only for what changed: faces with a
/// motion, edges not kept, vertices that moved.
fn kept_changes(
    body: &Body,
    changes: &Changes,
    class: &BTreeMap<crate::topo::EdgeId, Class>,
    vgeo: &BTreeMap<VertexId, crate::offset::vertices::VertexGeo>,
    journal: Journal,
) -> Journal {
    let mut changed: BTreeSet<Name> = BTreeSet::new();
    for f in changes.motion.keys() {
        changed.insert(body.faces[*f].name);
    }
    for (&e, &c) in class {
        if c != Class::Kept && c != Class::Hinge {
            changed.insert(body.edges[e].name);
        }
    }
    for e in body.edges().filter(|&e| body.is_laminar(e)) {
        let f = body.loops[body.fins[body.edges[e].fin].loop_].face;
        if changes.motion.contains_key(&f) {
            changed.insert(body.edges[e].name);
        }
    }
    for (&v, g) in vgeo {
        let p = body.vertices[v].point;
        if g.points[0] != p && !on_hinges(body, changes, v, g.points[0]) {
            changed.insert(body.vertices[v].name);
        }
    }
    let mut out = Journal::new();
    for e in journal.events() {
        match e {
            Event::Modified(n) if !changed.contains(n) => {}
            other => out.push(other.clone()),
        }
    }
    out
}

/// Whether vertex `v`, found at `x`, stays on its drafted faces' hinges:
/// every changed face round it is drafted and `x` lies within resolution
/// of where it was (found again on the drafted surfaces, it may differ by
/// rounding alone). Other tweaks' vertices are judged by their bits.
fn on_hinges(body: &Body, changes: &Changes, v: VertexId, x: crate::geom::Point3) -> bool {
    let mut any = false;
    for e in body.vertex_edges(v) {
        for fin in body.edge_fins(e) {
            let f = body.loops[body.fins[fin].loop_].face;
            match changes.motion.get(&f) {
                None => {}
                Some(Motion::Draft { .. }) => any = true,
                Some(_) => return false,
            }
        }
    }
    any && x.distance(body.vertices[v].point) <= crate::tol::LINEAR_RESOLUTION
}

/// The shared code's offset refusals as a tweak's.
pub(crate) fn translate(e: Error) -> Error {
    let Error::InvalidOffset(o) = e else {
        return e;
    };
    match o {
        OffsetError::BadDistance => TweakError::BadDistance,
        OffsetError::NoSuchFace { face } => TweakError::NoSuchFace { face },
        OffsetError::FaceCollapses { face } => TweakError::FaceCollapses { face },
        OffsetError::EdgeVanishes { edge } => TweakError::EdgeVanishes { edge },
        OffsetError::SelfIntersecting { faces } => TweakError::SelfIntersecting { faces },
        OffsetError::Uncertain { at } => TweakError::Uncertain { at },
        OffsetError::ToleranceTooLarge { at, needed } => {
            TweakError::ToleranceTooLarge { at, needed }
        }
        OffsetError::Unsupported { at, case } => TweakError::Unsupported {
            at,
            case: match case {
                OffsetCase::LoneVertex => TweakCase::LoneVertex,
                _ => TweakCase::PartlySmooth,
            },
        },
        OffsetError::Empty => TweakError::Uncertain {
            at: Name::from_u128(0),
        },
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::name::{Event, FaceRole, Role};
    use crate::primitive;

    const K: OpKey = OpKey(7);

    #[test]
    fn a_raised_top_builds_a_taller_box_journalling_what_moved() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let n = Name::derive(K, Role::Face(FaceRole::PlusZ), &[]);
        let top = b.faces().find(|&f| b.faces[f].name == n).unwrap();
        let up = Rigid::translation(Vector3::new(0.0, 0.0, 0.1));
        let changes = Changes {
            motion: BTreeMap::from([(top, Motion::Rigid(up))]),
            surface: BTreeMap::from([(top, b.surfaces[b.faces[top].surface].transformed(&up))]),
            ..Changes::default()
        };
        // unmetered: a unit test.
        let (o, j) = run(OpKey(8), &b, &changes, &crate::work::Meter::free()).unwrap();
        assert_eq!(
            (o.faces().count(), o.edges().count(), o.vertices().count()),
            (6, 12, 8)
        );
        let modified = j
            .events()
            .iter()
            .filter(|e| matches!(e, Event::Modified(_)))
            .count();
        // The top, its 4 edges and 4 carried sides, its 4 corners.
        assert_eq!(modified, 13);
        assert!(o.by_name(n).is_some());
        let top_z = o
            .vertices()
            .map(|v| o.vertices[v].point.z)
            .fold(0.0, f64::max);
        assert!((top_z - 0.5).abs() < 1e-9);
    }
}
