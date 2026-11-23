//! Partial blend deletes (design 919, plan 23.C): what `delete_blends`
//! picks, deleted set by set and then unit by unit, keeping what refuses.

use super::{DeleteBlends, Recognition};
use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, BodyKind, FaceId};
use crate::work::Meter;
use crate::{Error, TweakError};
use std::collections::BTreeSet;

/// A partial delete's report: what the units' indices point into, the
/// units deleted and those kept, each by least face name.
#[derive(Clone, Debug, PartialEq)]
pub struct PartialDelete {
    pub recognition: Recognition,
    pub deleted: Vec<BlendUnit>,
    pub kept: Vec<KeptUnit>,
}

/// The smallest group `delete_blends` deletes together: one picked blend
/// closed over its chain and the corners it reaches. `blends` index
/// [`Recognition::blends`]; `faces` are sorted.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendUnit {
    pub blends: Vec<usize>,
    pub faces: Vec<Name>,
}

/// A unit kept, with the last error it refused with.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptUnit {
    pub unit: BlendUnit,
    pub cause: Error,
}

/// The units of `picked`: each picked blend not yet in a unit closed
/// alone (`remake::close`), by least face name.
pub(crate) fn units(rec: &Recognition, picked: &BTreeSet<usize>) -> Vec<BlendUnit> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &i in picked {
        if seen.contains(&i) {
            continue;
        }
        let (blends, faces) = super::remake::close(rec, BTreeSet::from([i]));
        seen.extend(blends.iter().copied());
        out.push(BlendUnit {
            blends: blends.into_iter().collect(),
            faces: faces.into_iter().collect(),
        });
    }
    out.sort_by_key(|u| u.faces[0]);
    out
}

/// Whether any of `steps` touched a face of `near`: modified, deleted,
/// split, merged or replaced it (Decision 4).
pub(crate) fn touched(near: &BTreeSet<Name>, steps: &[Journal]) -> bool {
    steps.iter().flat_map(|j| j.events()).any(|e| match e {
        Event::Modified(n) | Event::Deleted(n) => near.contains(n),
        Event::Split { parent, .. } => near.contains(parent),
        Event::Merged { parents, .. } => parents.iter().any(|p| near.contains(p)),
        Event::Replaced { old, .. } => near.contains(old),
        _ => false,
    })
}

/// The faces of `body` sharing a vertex with a face named in `faces`,
/// those left out.
fn neighbours(body: &Body, faces: &[Name]) -> BTreeSet<Name> {
    use crate::tweak::retrim::region::{face_vertices, vertex_faces};
    let mut out = BTreeSet::new();
    for f in body.faces().filter(|&f| faces.contains(&body.faces[f].name)) {
        for v in face_vertices(body, f) {
            for g in vertex_faces(body, v) {
                out.insert(body.faces[g].name);
            }
        }
    }
    out.retain(|n| !faces.contains(n));
    out
}

/// A unit not yet deleted: its last cause, the faces round it at that
/// try, how many steps had built by then, and whether it was tried.
struct Pending {
    unit: BlendUnit,
    cause: Error,
    near: BTreeSet<Name>,
    at: usize,
    tried: bool,
}

impl Pending {
    /// A unit of a set that refused (or that a stop kept from being
    /// tried): tried again in the first pass unless the call stopped.
    fn new(unit: BlendUnit, cause: Error, tried: bool) -> Self {
        Pending {
            unit,
            cause,
            near: BTreeSet::new(),
            at: 0,
            tried,
        }
    }
}

/// Whether `m` stopped: `Err(Cancelled)` for a cancel (the call answers
/// it), `Ok(Some(budget))` for a budget spent, else `Ok(None)`.
fn stopped(m: &Meter) -> Result<Option<Error>, Error> {
    match m.stop_cause() {
        Some(Error::Cancelled) => Err(Error::Cancelled),
        s => Ok(s),
    }
}

/// [`super::delete_blends_partial_with`]'s body (design 919, plan 23.C).
pub(crate) fn delete_blends_partial(
    key: OpKey,
    body: &Body,
    spec: &DeleteBlends,
    m: &Meter,
) -> Result<(Body, Journal, PartialDelete), Error> {
    use crate::tweak::delete::{delete_faces_in, grow, sets};
    use crate::tweak::{DeleteSpec, Heal};
    if let Some(s) = spec.max_size
        && !(s.is_finite() && s >= 0.0)
    {
        return Err(TweakError::BadDistance.into());
    }
    let rec = super::recognise::recognise(body, &spec.options, m)?;
    let picked = super::remake::pick(&rec, spec)?;
    if picked.is_empty() {
        let report = PartialDelete {
            recognition: rec,
            deleted: vec![],
            kept: vec![],
        };
        return Ok((body.clone(), Journal::new(), report));
    }
    if body.kind() != BodyKind::Solid {
        return Err(Error::NotASolid {
            op: "delete faces by growth",
        });
    }
    let all_units = units(&rec, &picked);
    let (_, faces) = super::remake::close(&rec, picked);
    // The sets, as `delete_faces_in` grows them: in order of least name.
    let ids: BTreeSet<FaceId> = body
        .faces()
        .filter(|&f| faces.contains(&body.faces[f].name))
        .collect();
    let all = sets::sets(body, &ids);
    let mut order: Vec<(BTreeSet<Name>, Option<Error>)> = all
        .iter()
        .map(|s| {
            let names: BTreeSet<Name> = s.faces.iter().map(|&f| body.faces[f].name).collect();
            // What `delete_faces_in` refuses before building, set by set.
            let least = *names.iter().next().expect("a set holds a face");
            let refused = if s.boundary.is_empty() {
                Some(TweakError::GapNotHealed { face: least }.into())
            } else if s.closable(body) {
                Some(TweakError::GrowWouldCap { face: least }.into())
            } else {
                None
            };
            (names, refused)
        })
        .collect();
    order.sort_by_key(|(n, _)| n.iter().next().copied());
    let _ = m.charge(order.len() as u64);
    let mut stop = stopped(m)?;
    let (mut out, mut journal) = (body.clone(), Journal::new());
    let mut steps: Vec<Journal> = Vec::new();
    // Each set's outcome: `None` built, else why it refused, with the
    // faces round it then and how many steps had built.
    type Refusal = (Error, BTreeSet<Name>, usize);
    let mut outcomes: Vec<Option<Refusal>> = Vec::new();
    for (names, refused) in &order {
        if let Some(s) = &stop {
            outcomes.push(Some((s.clone(), BTreeSet::new(), steps.len())));
            continue;
        }
        let ids: BTreeSet<FaceId> = out
            .faces()
            .filter(|&f| names.contains(&out.faces[f].name))
            .collect();
        let r = match refused {
            Some(e) => Err(e.clone()),
            // A growth before it took this set's faces away: refused as
            // `delete_faces_in` refuses it; its units find their faces gone.
            None if ids.is_empty() => Err(TweakError::GapNotHealed {
                face: *names.iter().next().expect("a set holds a face"),
            }
            .into()),
            None => grow::grow(key, &out, &ids, m),
        };
        match r {
            Ok((grown, j)) => {
                out = grown;
                for e in j.events() {
                    if !journal.events().contains(e) {
                        journal.push(e.clone());
                    }
                }
                steps.push(j);
                outcomes.push(None);
            }
            Err(e) => {
                stop = stopped(m)?;
                let near: Vec<Name> = names.iter().copied().collect();
                let near = neighbours(&out, &near);
                outcomes.push(Some((stop.clone().unwrap_or(e), near, steps.len())));
            }
        }
    }
    // A unit is deleted where every set holding its faces built.
    let mut deleted: Vec<BlendUnit> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    for unit in all_units {
        let held: Vec<usize> = (0..order.len())
            .filter(|&i| unit.faces.iter().any(|f| order[i].0.contains(f)))
            .collect();
        let Some((cause, near, at)) = held.iter().find_map(|&i| outcomes[i].clone()) else {
            deleted.push(unit);
            continue;
        };
        // A set of this one unit alone refused, building, as the unit
        // would: tried again only after a neighbour changes (Decision 4).
        let alone = stop.is_none()
            && held.len() == 1
            && order[held[0]].1.is_none()
            && order[held[0]].0.len() == unit.faces.len();
        if alone {
            pending.push(Pending {
                near,
                at,
                unit,
                cause,
                tried: true,
            });
        } else {
            pending.push(Pending::new(unit, cause, stop.is_some()));
        }
    }
    let whole = pending.is_empty();
    // Unit by unit, again while a pass deletes something (Decision 4).
    while stop.is_none() && !pending.is_empty() {
        let mut any = false;
        for p in pending.iter_mut() {
            if stop.is_some() {
                break;
            }
            if p.tried && !touched(&p.near, &steps[p.at..]) {
                continue;
            }
            let present: Vec<Name> = p
                .unit
                .faces
                .iter()
                .copied()
                .filter(|&f| out.by_name(f).is_some())
                .collect();
            let r = if present.is_empty() {
                // A neighbour's growth took its faces: deleted already.
                Ok(None)
            } else {
                let spec = DeleteSpec {
                    faces: present,
                    heal: Heal::Grow,
                };
                delete_faces_in(key, &out, &spec, m).map(Some)
            };
            match r {
                Ok(built) => {
                    if let Some((o, j)) = built {
                        out = o;
                        steps.push(j);
                    }
                    p.at = usize::MAX; // settled
                    any = true;
                }
                Err(e) => {
                    stop = stopped(m)?;
                    p.cause = stop.clone().unwrap_or(e);
                    p.near = neighbours(&out, &p.unit.faces);
                    p.at = steps.len();
                    p.tried = true;
                }
            }
        }
        let (done, rest): (Vec<Pending>, Vec<Pending>) =
            pending.into_iter().partition(|p| p.at == usize::MAX);
        deleted.extend(done.into_iter().map(|p| p.unit));
        pending = rest;
        if !any {
            break;
        }
    }
    // A stop keeps every unit that would have been tried again with it.
    if let Some(s) = &stop {
        for p in &mut pending {
            if !p.tried || touched(&p.near, &steps[p.at..]) {
                p.cause = s.clone();
            }
        }
    }
    deleted.sort_by_key(|u| u.faces[0]);
    let mut kept: Vec<KeptUnit> = pending
        .into_iter()
        .map(|p| KeptUnit {
            unit: p.unit,
            cause: p.cause,
        })
        .collect();
    kept.sort_by_key(|k| k.unit.faces[0]);
    let journal = if whole {
        // `delete_faces_in`'s last step: a `Modified` name gone is gone.
        let mut j = Journal::new();
        for e in journal.events() {
            if !matches!(e, Event::Modified(n) if out.by_name(*n).is_none()) {
                j.push(e.clone());
            }
        }
        j
    } else {
        let refs: Vec<&Journal> = steps.iter().collect();
        super::remake::net(body, &out, &refs, &BTreeSet::new())
    };
    let report = PartialDelete {
        recognition: rec,
        deleted,
        kept,
    };
    Ok((out, journal, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::{BlendKind, EdgeBlend, blend};
    use crate::geom::Rigid;
    use crate::name::{FaceRole, OpKey, Role};
    use crate::primitive;
    use crate::tweak::{DeleteSpec, Heal, RecogniseOptions};
    use crate::work::{Work, run_keeping};

    /// A block 1 × 0.6 × 0.4 with two edges that share no vertex rounded
    /// 0.05: the top's front edge and the bottom's back edge.
    pub(crate) fn two_rounds() -> Body {
        let k = OpKey(0x23c0);
        let (b, _) = primitive::block(k, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let face = |r| Name::derive(k, Role::Face(r), &[]);
        let edge = |a, c| {
            let (a, c) = (face(a), face(c));
            b.edges()
                .find(|&e| {
                    let n: Vec<Name> = b
                        .edge_fins(e)
                        .map(|fin| b.faces[b.loops[b.fins[fin].loop_].face].name)
                        .collect();
                    n.contains(&a) && n.contains(&c)
                })
                .map(|e| b.edges[e].name)
                .unwrap()
        };
        let edges = [
            EdgeBlend {
                edge: edge(FaceRole::PlusZ, FaceRole::MinusY),
                kind: BlendKind::Fillet { radius: 0.05 },
            },
            EdgeBlend {
                edge: edge(FaceRole::MinusZ, FaceRole::PlusY),
                kind: BlendKind::Fillet { radius: 0.05 },
            },
        ];
        blend(OpKey(0x23c1), &b, &edges).unwrap().0
    }

    /// Design 919, plan 23.C: units partition the closed picks, each the
    /// closure of its least blend alone, by least face name.
    #[test]
    fn units_partition_the_closed_picks() {
        let b = two_rounds();
        let rec = crate::tweak::recognise_blends(&b, &RecogniseOptions::default()).unwrap();
        assert_eq!(rec.blends.len(), 2);
        let picked: BTreeSet<usize> = (0..rec.blends.len()).collect();
        let us = units(&rec, &picked);
        assert_eq!(us.len(), 2);
        let (_, all) = super::super::remake::close(&rec, picked);
        let mut seen = BTreeSet::new();
        for u in &us {
            for f in &u.faces {
                assert!(seen.insert(*f), "{f:?} in two units");
            }
        }
        assert_eq!(seen, all);
        assert!(us.windows(2).all(|w| w[0].faces[0] < w[1].faces[0]));
    }

    /// Plan 23.C: where every set builds, the partial delete is
    /// `delete_blends`, bit for bit, every unit deleted and none kept.
    #[test]
    fn where_every_set_builds_it_is_delete_blends() {
        let b = two_rounds();
        let spec = DeleteBlends {
            max_size: Some(0.1),
            ..Default::default()
        };
        // unmetered: a unit test.
        let m = Meter::free();
        let (o, j) = super::super::remake::delete_blends(OpKey(0x23c2), &b, &spec, &m).unwrap();
        let (o2, j2, r) = delete_blends_partial(OpKey(0x23c2), &b, &spec, &m).unwrap();
        assert_eq!(o.dump(), o2.dump());
        assert_eq!(j.events(), j2.events());
        assert_eq!(r.deleted.len(), 2);
        assert!(r.kept.is_empty());
    }

    /// Plan 23.C: a budget that runs out keeps what is not yet settled,
    /// with the budget as its cause, and what was built stands. The two
    /// rounds are one set each; a budget of recognition, the first set's
    /// growth and a little more deletes it and keeps the second. Every
    /// input face is in the result or in the journal, and the kept unit's
    /// faces keep their names.
    #[test]
    fn a_budget_that_runs_out_keeps_the_rest() {
        let b = two_rounds();
        let spec = DeleteBlends {
            max_size: Some(0.1),
            ..Default::default()
        };
        let w = Work::unlimited();
        let rec = crate::tweak::recognise_blends_with(&b, &RecogniseOptions::default(), &w)
            .unwrap();
        let first = units(&rec, &(0..2).collect())[0].clone();
        let one = crate::tweak::delete_faces_with(
            OpKey(0x23c2),
            &b,
            &DeleteSpec {
                faces: first.faces.clone(),
                heal: Heal::Grow,
            },
            &w,
        )
        .unwrap();
        let budget = w.spent() + 64;
        let w = Work::unlimited().with_budget(budget);
        let (o, j, r) =
            run_keeping(&w, |m| delete_blends_partial(OpKey(0x23c2), &b, &spec, m)).unwrap();
        assert_eq!(r.deleted, vec![first]);
        assert_eq!(r.kept.len(), 1);
        assert!(
            matches!(r.kept[0].cause, Error::BudgetExceeded { .. }),
            "{:?}",
            r.kept[0].cause
        );
        assert_eq!(o.dump(), one.0.dump(), "the first set's body stands");
        for f in &r.kept[0].unit.faces {
            assert!(o.by_name(*f).is_some(), "{f:?}");
        }
        for f in b.faces() {
            let n = b.faces[f].name;
            let told = j.events().iter().any(|e| match e {
                Event::Deleted(x) | Event::Modified(x) => *x == n,
                Event::Split { parent, .. } => *parent == n,
                Event::Merged { parents, .. } => parents.contains(&n),
                Event::Replaced { old, .. } => *old == n,
                _ => false,
            });
            assert!(o.by_name(n).is_some() || told, "{n:?} unaccounted");
        }
    }

    /// Plan 23.C, Decision 4: a unit is tried again only after a step
    /// that built touched a face it shares a vertex with.
    #[test]
    fn a_unit_is_retried_only_after_a_neighbour_changed() {
        let n = |i: u128| Name::from_u128(i);
        let near = BTreeSet::from([n(1), n(2)]);
        let mut j = Journal::new();
        j.push(Event::Modified(n(3)));
        assert!(!touched(&near, std::slice::from_ref(&j)));
        j.push(Event::Replaced {
            old: n(2),
            new: vec![n(9)],
        });
        assert!(touched(&near, &[j]));
        let mut k = Journal::new();
        k.push(Event::Split {
            parent: n(1),
            children: vec![n(7), n(8)],
        });
        assert!(touched(&near, &[Journal::new(), k]));
        let mut k = Journal::new();
        k.push(Event::merged(n(2), n(5), n(6)));
        assert!(touched(&near, &[k]));
        let mut k = Journal::new();
        k.push(Event::Deleted(n(1)));
        assert!(touched(&near, &[k]));
    }

    /// A cancel answers `Cancelled`, whatever was built.
    #[test]
    fn a_cancel_answers_cancelled() {
        let b = two_rounds();
        let spec = DeleteBlends {
            max_size: Some(0.1),
            ..Default::default()
        };
        let flag = std::sync::atomic::AtomicBool::new(true);
        let w = Work::unlimited().with_cancel(&flag);
        let r = run_keeping(&w, |m| delete_blends_partial(OpKey(0x23c2), &b, &spec, m));
        assert_eq!(r.map(|_| ()), Err(Error::Cancelled));
    }
}
