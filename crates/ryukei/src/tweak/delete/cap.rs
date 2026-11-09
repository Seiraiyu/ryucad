//! The cap: each set's faces go, and each host face loses the holes the
//! set's edges run round, its own surface covering the gap. Nothing is
//! constructed: every face, edge and vertex that stays keeps its geometry
//! bit for bit.

use super::least;
use super::sets::Set;
use crate::check::{Fault, check_pairs_in};
use crate::layout::{Layout, Plan, build_sheet};
use crate::name::{EditRole, Event, Journal, Name, OpKey, Role};
use crate::topo::{Body, BodyKind, FaceId, LoopId};
use crate::work::Meter;
use crate::{Error, TopologyError, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// The scaffold names' role: everything is renamed afterwards.
const ROLE: Role = Role::Edit(EditRole::Edge);

/// `body` with every set in `sets` capped, named as `body` names it.
pub(crate) fn cap(
    key: OpKey,
    body: &Body,
    sets: &[&Set],
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    let deleted: BTreeSet<FaceId> = sets.iter().flat_map(|s| s.faces.iter().copied()).collect();
    let dropped: BTreeSet<LoopId> = sets
        .iter()
        .flat_map(|s| s.hosts.iter().map(|&(_, l)| l))
        .collect();
    let hosts: BTreeSet<FaceId> = sets
        .iter()
        .flat_map(|s| s.hosts.iter().map(|&(g, _)| g))
        .collect();
    let at = least(body, &deleted);
    let uncertain = || Error::from(TweakError::Uncertain { at });
    m.charge(1 + dropped.len() as u64)
        .map_err(|_| uncertain())?;
    let keep: BTreeSet<FaceId> = body.faces().filter(|f| !deleted.contains(f)).collect();
    let mut lay = Layout::default();
    let copied = crate::sheet::copy_where(&mut lay, body, &keep, |l| !dropped.contains(&l));
    let built = if body.kind() == BodyKind::Sheet {
        build_sheet(&lay, key, ROLE).map_err(|_| uncertain())?
    } else {
        solid(key, body, &lay, &copied).ok_or_else(uncertain)??
    };
    let mut out = built.body.clone();
    crate::sheet::name_copied(&mut out, body, &copied, &built);
    if !crate::check::check(&out).is_empty() {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    // Only a capped face can newly cross another, and only over its gap.
    let gap = deleted
        .iter()
        .map(|&f| crate::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))
        .expect("a set holds a face");
    let capped: BTreeSet<Name> = hosts.iter().map(|&g| body.faces[g].name).collect();
    let near = |f: FaceId| crate::intersect::face_box(&out, f).intersects(&gap);
    let is_host = |f: FaceId| capped.contains(&out.faces[f].name);
    let faults = check_pairs_in(&out, m, |a, b| {
        (is_host(a) || is_host(b)) && near(a) && near(b)
    });
    for fault in faults {
        match fault {
            Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b } => {
                let mut faces = [out.faces[a].name, out.faces[b].name];
                faces.sort();
                return Err(TweakError::SelfIntersecting { faces }.into());
            }
            Fault::CannotCheck { a, .. } => {
                return Err(TweakError::Uncertain {
                    at: out.faces[a].name,
                }
                .into());
            }
            _ => {}
        }
    }
    let mut j = Journal::new();
    for n in &capped {
        j.push(Event::Modified(*n));
    }
    let mut gone: Vec<Name> = deleted.iter().map(|&f| body.faces[f].name).collect();
    gone.extend(
        body.edges()
            .map(|e| body.edges[e].name)
            .filter(|&n| out.by_name(n).is_none()),
    );
    gone.extend(
        body.vertices()
            .map(|v| body.vertices[v].name)
            .filter(|&n| out.by_name(n).is_none()),
    );
    for n in gone {
        j.push(Event::Deleted(n));
    }
    Ok((out, j))
}

/// The layout built as a solid: each connected piece the outer shell of
/// a region of its own, or a void where its faces were a cavity's, in the
/// region of the piece holding that region's outer shell. `None` where a
/// region with cavities came apart (which piece holds them is unclear).
fn solid(
    key: OpKey,
    body: &Body,
    lay: &Layout,
    copied: &crate::sheet::Copied,
) -> Option<Result<crate::layout::Built, Error>> {
    let plan = match Plan::new(lay) {
        Ok(p) => p,
        Err(_) => return Some(Err(Error::Topology(TopologyError::Inconsistent))),
    };
    let origin: BTreeMap<usize, FaceId> = copied.faces.iter().map(|&(f, k)| (k, f)).collect();
    let pieces = plan.shells();
    let shell_of = |p: &Vec<usize>| body.faces[origin[&p[0]]].shell;
    let outer = |s: crate::topo::ShellId| {
        let r = body.shells[s].region;
        body.regions[r].first_shell == Some(s)
    };
    let mut shells: Vec<Vec<usize>> = Vec::new();
    let mut outer_of: Vec<Option<usize>> = Vec::new();
    let mut index: BTreeMap<crate::topo::RegionId, Vec<usize>> = BTreeMap::new();
    for p in pieces.iter().filter(|p| outer(shell_of(p))) {
        index
            .entry(body.shells[shell_of(p)].region)
            .or_default()
            .push(shells.len());
        shells.push(p.clone());
        outer_of.push(None);
    }
    for p in pieces.iter().filter(|p| !outer(shell_of(p))) {
        let r = body.shells[shell_of(p)].region;
        match index.get(&r).map(Vec::as_slice) {
            Some([o]) => {
                shells.push(p.clone());
                outer_of.push(Some(*o));
            }
            _ => return None,
        }
    }
    Some(
        plan.build(key, ROLE, &shells, &outer_of)
            .map_err(|_| Error::Topology(TopologyError::Inconsistent)),
    )
}
