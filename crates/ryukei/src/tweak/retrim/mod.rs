//! The local re-trim (design 399): the tweak engine for changes of
//! topology. The faces a tweak changes and their neighbours are the
//! region; every face of it becomes a working face, its surface carried
//! past its domain over the region's box; the faces outside the region
//! stay as they are. The working faces and the outside faces go through
//! the self-union's one-body boolean pipeline ([`crate::repair::resolve`]),
//! and the pieces kept are those the frontier walk reaches from the
//! outside faces (see `walk`). Nothing about which side of a surface is
//! material is guessed: the walk only counts.

pub(crate) mod body;
pub(crate) mod held;
pub(crate) mod names;
pub(crate) mod near;
pub(crate) mod region;
pub(crate) mod seeds;
pub(crate) mod walk;
pub(crate) mod working;

use crate::boolean::build::Kept;
use crate::boolean::table::{Member, Table};
use crate::boolean::{Fail, past_cap};
use crate::check::Fault;
use crate::name::{Journal, Name, OpKey};
use crate::split::Piece;
use crate::topo::{Body, FaceId};
use crate::tweak::engine::Changes;
use crate::work::Meter;
use crate::{Error, TopologyError, TweakError};
use body::{WorkBody, doubled, region_box, work_body};
use region::Region;
use std::collections::BTreeMap;
use std::sync::Mutex;
use walk::WalkError;

/// One build of a rung of the retry ladder: whether it may go on to the
/// next.
pub(crate) enum Rung {
    Done(Box<Result<(Body, Journal), Error>>),
    Next(Error),
    /// The walk couldn't close: climbs the ladder as `Next`.
    Walked(Error),
    /// A first build's `Uncertain` at a pair of its table, held or not
    /// (design 669): answers as `Done` unless the rung is built again
    /// narrowed.
    Uncertain(Error),
    /// A narrowed retry that narrows no working face: never built.
    Unchanged,
}

/// Whether a rung may be built again narrowed (design 669).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Narrow {
    /// Never: each rung is built once (the tests' re-trim without
    /// narrowing).
    #[cfg_attr(not(test), allow(dead_code))]
    Never,
    /// Once its first build refuses `Uncertain` at a pair of its table.
    AfterUncertain,
}

/// Which build of a rung `attempt` makes (design 669).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Build {
    /// The rung's first build: as before, bit for bit.
    First,
    /// Built again narrowed after the first refused `Uncertain`:
    /// `Unchanged` where no narrowing changes a working face.
    Retry,
    /// A rung above one built narrowed: its one build, narrowed, built
    /// whether or not a narrowing changes anything.
    Only,
}

impl Build {
    /// Whether this build applies the narrowings.
    pub(crate) fn narrowed(self) -> bool {
        self != Build::First
    }
}

/// A rung's answer from its first build and, where that refused
/// `Uncertain` at a pair of its table and `narrow` allows, its narrowed
/// build (`again`); and whether the rung was built narrowed. A narrowed
/// build that narrows nothing leaves the first answer standing; any other
/// narrowed outcome is the rung's, a failed walk or `Next` climbing
/// (design 669; plan 20.A, Decision 5).
pub(crate) fn settle(first: Rung, narrow: Narrow, again: impl FnOnce() -> Rung) -> (Rung, bool) {
    let Rung::Uncertain(e) = first else {
        return (first, false);
    };
    if narrow == Narrow::Never {
        return (done(Err(e)), false);
    }
    match again() {
        Rung::Unchanged => (done(Err(e)), false),
        // `attempt` answers a narrowed build's own `Uncertain` as `Done`;
        // kept for the match.
        Rung::Uncertain(e) => (done(Err(e)), true),
        r => (r, true),
    }
}

/// The ladder (design 669): rungs 0 to 3 in order, each built by
/// `build(rung, which)`, `None` once the region can't grow. Each rung's
/// first build, and after an `Uncertain` its narrowed retry
/// ([`settle`]); once a rung was built narrowed, every later rung is built
/// narrowed only: its first build would carry the same faces farther over
/// a bigger region. `Next` and failed walks climb, and the last answers
/// when the ladder ends.
pub(crate) fn climb(
    narrow: Narrow,
    mut build: impl FnMut(usize, Build) -> Option<Rung>,
) -> Result<(Body, Journal), Error> {
    let mut last = None;
    let mut narrowed = false;
    for rung in 0..4 {
        let r = if narrowed {
            let Some(r) = build(rung, Build::Only) else {
                break;
            };
            r
        } else {
            let Some(first) = build(rung, Build::First) else {
                break;
            };
            let (r, now) = settle(first, narrow, || {
                build(rung, Build::Retry).expect("a rung built once builds again")
            });
            narrowed = now;
            r
        };
        match r {
            Rung::Done(r) => return *r,
            Rung::Next(e) | Rung::Walked(e) => last = Some(e),
            // `settle` answers it; kept for the match.
            Rung::Uncertain(e) => return Err(e),
            Rung::Unchanged => {}
        }
    }
    Err(last.expect("one rung ran"))
}

/// How many adjusted builds a narrowed rung may make (design 724): a
/// work bound, not a tolerance. Each costs at most a rung's cap.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const ADJUSTS: usize = 3;

/// A narrowed rung's answer (design 724, plan 22.A). `first` is its first
/// narrowed build and what that build asks: `Some` adjustments where its
/// walk failed `Clipped` at a working face with room to give. The rung is
/// built again by `again` with what each build asks, at most [`ADJUSTS`]
/// times. The first adjusted build that builds (closes and checks clean)
/// answers, and so does one after which the meter is stopped (a cancel
/// or a halt answers at once). Otherwise the first answer stands, so
/// refusals keep their names and a failed walk climbs as before.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn adjusting(
    first: (Rung, Option<near::Adjusts>),
    stopped: &dyn Fn() -> bool,
    mut again: impl FnMut(&near::Adjusts) -> (Rung, Option<near::Adjusts>),
) -> Rung {
    let (first, mut ask) = first;
    for _ in 0..ADJUSTS {
        let Some(adjusts) = ask.take() else {
            break;
        };
        let (r, next) = again(&adjusts);
        if matches!(&r, Rung::Done(d) if d.is_ok()) || stopped() {
            return r;
        }
        ask = next;
    }
    first
}

/// A rung that answers.
fn done(r: Result<(Body, Journal), Error>) -> Rung {
    Rung::Done(Box::new(r))
}

/// [`retrim_with`], built again narrowed after an `Uncertain`.
pub(crate) fn retrim(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    retrim_with(key, body, changes, Narrow::AfterUncertain, m)
}

/// `body` tweaked by `changes` with its topology free to change, named
/// from `key`. Refusals: `NotASolid` for a sheet, `LaminarFace` for a
/// region face with a laminar edge, `GapNotHealed`, `HealAmbiguous`,
/// `RegionTooSmall` and `ExtensionUnsupported` from the walk,
/// `SelfIntersecting`, `Uncertain` and `ToleranceTooLarge` from the
/// result's checks. Where a rung's first build refuses `Uncertain` at a
/// pair of its table, `narrow` says whether it is built again narrowed
/// (period-aware turns and stops at held seams), under its own
/// `Scope::Rung` cap; once one is, every later rung is built narrowed
/// only (design 669).
pub(crate) fn retrim_with(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    narrow: Narrow,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    body.require_solid("re-trim")?;
    let changed = changes
        .motion
        .keys()
        .chain(&changes.deleted)
        .copied()
        .collect();
    let mut region = region::region(body, &changed);
    let reach = reach(body, changes);
    // The ladder: the region and its box; the box doubled; the region
    // grown by a ring, twice. It stops once the region is every face.
    // `at` is the rung the region and box were made for.
    let mut at: Option<(usize, crate::geom::Box3)> = None;
    climb(narrow, |rung, which| {
        if at.map(|a| a.0) != Some(rung) {
            // Polled, never charged: a rung's working faces and their
            // sheets are unmetered (13.D: NIST FTC 10's blends ran 17
            // minutes past a cancel here).
            if m.poll().is_err() {
                return Some(done(Err(Error::Cancelled)));
            }
            if rung >= 2 {
                if region.whole(body) {
                    return None;
                }
                region = region.grown(body);
            }
            if let Err(e) = laminar_free(body, &region) {
                return Some(done(Err(e)));
            }
            let mut bx = region_box(body, &region, reach);
            if rung == 1 {
                bx = doubled(&bx);
            }
            at = Some((rung, bx));
        }
        let bx = at.map(|a| a.1).expect("made above");
        // Each build under its own cap: one that can't settle refuses
        // `Uncertain` in bounded units, as the caps do.
        Some(m.capped(crate::work::Scope::Rung, || {
            let build = || attempt(key, body, changes, &region, &bx, rung, which, m);
            if which.narrowed() {
                build()
            } else {
                m.first_build(build)
            }
        }))
    })
}

/// How far a changed face's vertex moves, at most, and at least ten
/// resolutions.
fn reach(body: &Body, changes: &Changes) -> f64 {
    reach_of(body, changes, &|_| true)
}

/// [`reach`] over the changed faces `keep` takes (a near box's gap, plan
/// 20.B). Over every face it is `reach`, bit for bit: the same faces in
/// the same order.
pub(crate) fn reach_of(body: &Body, changes: &Changes, keep: &dyn Fn(FaceId) -> bool) -> f64 {
    let mut r: f64 = 10.0 * crate::tol::LINEAR_RESOLUTION;
    // A gap is as wide as the box of the faces taken away.
    if let Some(g) = changes
        .deleted
        .iter()
        .filter(|&&f| keep(f))
        .map(|&f| crate::intersect::face_box(body, f))
        .reduce(|a, b| a.union(&b))
    {
        r = r.max(g.min().distance(g.max()));
    }
    for (&f, mo) in changes.motion.iter().filter(|&(&f, _)| keep(f)) {
        // An offset moves every point by its distance (`apply` needs the
        // normal for that, which a vertex alone doesn't give).
        if let crate::tweak::Motion::Offset(d) = mo {
            r = r.max(d.abs());
            continue;
        }
        let new = changes
            .surface
            .get(&f)
            .unwrap_or(&body.surfaces[body.faces[f].surface]);
        let sense = changes
            .senses
            .get(&f)
            .copied()
            .unwrap_or(body.faces[f].sense);
        for l in body.face_loops(f) {
            for fin in body.loop_fins(l) {
                let e = &body.edges[body.fins[fin].edge];
                for v in [e.start, e.end] {
                    let p = body.vertices[v].point;
                    let (q, _) =
                        crate::tweak::motion::apply(mo, new, sense, p, crate::geom::Vector3::ZERO);
                    r = r.max(p.distance(q));
                }
            }
        }
    }
    r
}

/// `LaminarFace` for a region face with a laminar edge.
fn laminar_free(body: &Body, region: &Region) -> Result<(), Error> {
    for &f in &region.faces {
        if body.face_loops(f).any(|l| {
            body.loop_fins(l)
                .any(|fin| body.is_laminar(body.fins[fin].edge))
        }) {
            return Err(TweakError::LaminarFace {
                face: body.faces[f].name,
            }
            .into());
        }
    }
    Ok(())
}

/// One rung: the working body, the table, the walk, the result's checks.
/// `rung` says how a near box is doubled (rung 1) in a narrowed build.
#[allow(clippy::too_many_arguments)]
fn attempt(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    region: &Region,
    bx: &crate::geom::Box3,
    rung: usize,
    which: Build,
    m: &Meter,
) -> Rung {
    attempt_with(key, body, changes, region, bx, rung, which, None, m)
}

/// [`attempt`] with the pins forced (`Some`, the tests only) or as
/// [`near::pins`] gives them (`None`).
#[allow(clippy::too_many_arguments)]
fn attempt_with(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    region: &Region,
    bx: &crate::geom::Box3,
    rung: usize,
    which: Build,
    forced: Option<&std::collections::BTreeSet<FaceId>>,
    m: &Meter,
) -> Rung {
    let none = near::Adjusts::new();
    attempt_adjusted(
        key, body, changes, region, bx, rung, which, forced, &none, &mut None, m,
    )
}

/// [`attempt_with`] with each working face's near box adjusted by
/// `adjusts` (design 724, plan 22.A); `ask` is set to what a failed walk
/// asks for an adjusted build ([`ask_room`]), and left `None` otherwise.
#[allow(clippy::too_many_arguments)]
fn attempt_adjusted(
    key: OpKey,
    body: &Body,
    changes: &Changes,
    region: &Region,
    bx: &crate::geom::Box3,
    rung: usize,
    which: Build,
    forced: Option<&std::collections::BTreeSet<FaceId>>,
    adjusts: &near::Adjusts,
    ask: &mut Option<near::Adjusts>,
    m: &Meter,
) -> Rung {
    let narrowed = which.narrowed();
    let mut ws = BTreeMap::new();
    let absorbed = absorbed(body, changes, region);
    let absorbing: std::collections::BTreeSet<FaceId> = absorbed.values().copied().collect();
    // Faces left as they are: on their own surfaces, standing for
    // themselves.
    let still = |f: FaceId| {
        region.faces.contains(&f)
            && !changes.deleted.contains(&f)
            && !changes.motion.contains_key(&f)
            && !changes.surface.contains_key(&f)
            && !absorbed.contains_key(&f)
    };
    // The seams kept faces were tangent along, all along or in places,
    // across the frontier too (design 642).
    let held = held::held(body, region, &still);
    // Faces a narrowed build pins (design 683, plan 21.A): left as they
    // are and touching no change, built as their input faces with the
    // faces outside. None in a first build. The working body is built
    // over the region less them.
    let pins = match forced {
        Some(p) => p.clone(),
        None => near::pins(body, region, &still, &absorbing, which),
    };
    let rest = region.without(body, &pins);
    // Whether a narrowed build narrowed anything; a pin does.
    let mut narrowing = !pins.is_empty();
    for &f in region
        .faces
        .iter()
        .filter(|f| !changes.deleted.contains(f) && !absorbed.contains_key(f) && !pins.contains(f))
    {
        if m.poll().is_err() {
            return done(Err(Error::Cancelled));
        }
        let mo = changes.motion.get(&f);
        let sense = changes
            .senses
            .get(&f)
            .copied()
            .unwrap_or(body.faces[f].sense);
        let over = |b: &crate::geom::Box3| {
            working::working(
                body,
                f,
                changes.surface.get(&f),
                sense,
                working::same_params(mo),
                b,
            )
        };
        // Its turn as a first build gives it, and in a narrowed build
        // read round the circle (design 669). A face standing for others
        // on its surface covers them all: its whole turn (NIST FTC 09's
        // hole halves).
        let turned = |w: &working::Working, circular: bool| {
            let mut r = w.rect;
            if !absorbing.contains(&f) {
                for (k, t) in working::delete_turn(body, changes, f, &w.surface, circular)
                    .into_iter()
                    .enumerate()
                {
                    if let Some(t) = t {
                        r[k] = t;
                    }
                }
            }
            r
        };
        let mut w = match over(bx) {
            Ok(w) => w,
            Err(e) => return done(Err(e)),
        };
        let first = turned(&w, false);
        w.rect = first;
        if narrowed {
            // Built narrowed: carried over its near box only (plan 20.B),
            // turned round the circle, then ended at its held seams whose
            // ends can't move (design 642), iso stops first and plane
            // stops where none applied.
            let before = w.surface.clone();
            // Adjusted after a narrowed build failed (design 724): as
            // before where `adjusts` names nothing.
            let near = near::adjusted(
                &near::near_box(body, changes, region, f, bx, rung),
                bx,
                adjusts.get(&f).copied(),
            );
            if near != *bx {
                w = match over(&near) {
                    Ok(w) => w,
                    Err(e) => return done(Err(e)),
                };
            }
            let round = turned(&w, true);
            w.rect = held::iso_stops(body, f, &held, &w.surface, round);
            if let Some(cut) = held::plane_stops(
                body::sheet_key(key, body.faces[f].name),
                body,
                f,
                &held,
                &w.surface,
                round,
                w.rect,
                bx,
                m,
            ) {
                w.cut = Some(cut);
            }
            narrowing |= w.rect != first || w.surface != before || w.cut.is_some();
        }
        ws.insert(f, w);
    }
    // A narrowed retry that narrows nothing would only repeat the first.
    if which == Build::Retry && !narrowing {
        return Rung::Unchanged;
    }
    let w = match work_body(key, body, &rest, &ws, m) {
        Ok(w) => w.pinning(&pins),
        Err(e) => return Rung::Next(e),
    };
    let walked: Mutex<Option<(WalkError, FaceId)>> = Mutex::new(None);
    // The working-body faces whose clipped sides the failing piece
    // reached (design 724, plan 22.A): an ask's room goes to them too.
    let clip_owners: Mutex<std::collections::BTreeSet<FaceId>> = Mutex::new(Default::default());
    let select = |_: &[Body], table: &Table, pieces: &[Piece], _: &[usize], m: &Meter| {
        let arr = seeds::arrangement(&w, table, pieces);
        let outside = seeds::outside_seeds(&w, pieces);
        // Design 399's rule 3: region faces with no frontier edge seeded by
        // the piece holding their old interior point. A moved face's seed
        // says it survives, which a face moved out of the body doesn't: so
        // the walk runs with every such seed, then with the unchanged
        // faces' alone. A region with no outside face takes the plan's
        // one shell seed instead (below).
        let mut tries: Vec<Vec<usize>> = vec![outside.clone()];
        // The seeds rule 3's shortcut alone chose (plan 21.C).
        let mut shortcut = std::collections::BTreeSet::new();
        // A delete whose region is the whole body: every face it keeps
        // seeds by rule 3 too, not one face alone (#692: a drilled plate's
        // side deleted closed from the hole's wall alone, its plug).
        if outside.is_empty() && !changes.deleted.is_empty() && changes.motion.is_empty() {
            let fs = seeds::face_seeds(body, changes, &w, &arr, table, pieces, m);
            if fs.same.len() > 1 {
                tries = vec![fs.same];
                shortcut = fs.shortcut;
            }
        }
        if !outside.is_empty() {
            let fs = seeds::face_seeds(body, changes, &w, &arr, table, pieces, m);
            shortcut = fs.shortcut;
            let (same, moved) = (fs.same, fs.moved);
            if !same.is_empty() || !moved.is_empty() {
                let mut with_same = outside.clone();
                with_same.extend(&same);
                let mut all = with_same.clone();
                all.extend(&moved);
                tries = if moved.is_empty() {
                    vec![with_same]
                } else {
                    vec![all, with_same]
                };
            }
        }
        let mut kept = Err(WalkError::Open { piece: 0 });
        let mut seeds = Vec::new();
        for (k, t) in tries.iter().enumerate() {
            let r = walk::walk(&arr, t, m);
            let stop = r.is_ok() || matches!(r, Err(WalkError::Halted { .. }));
            // The first try's failure is the one reported.
            if stop || k == 0 {
                kept = r;
                seeds = t.clone();
            }
            if stop {
                break;
            }
        }
        // Slits and seeds (design 683, plan 21.C, question 983): every
        // seed set failed. The first set whose seeds alone over-use an
        // edge walks once more without the shortcut's seeds there that
        // don't hold their old face; its answer stands only if it closes
        // (or halts), else the first try's failure is reported.
        if !shortcut.is_empty()
            && matches!(
                kept,
                Err(WalkError::Open { .. }
                    | WalkError::Ambiguous { .. }
                    | WalkError::Clipped { .. })
            )
        {
            let ops = [w.body.clone()];
            let mut holds =
                |i: usize| seeds::inside_old(body, changes, &w, &ops, table, &pieces[i]);
            if let Some((fewer, r)) = seeds::walk_again(&arr, &tries, &shortcut, &mut holds, m) {
                kept = r;
                seeds = fewer;
            }
        }
        // Shells no outside face reaches take a seed each, once.
        if let Ok(k) = &kept {
            let seen: Vec<bool> = k.iter().map(Option::is_some).collect();
            let more = seeds::shell_seeds(body, changes, &w, &arr, table, pieces, &seen, m);
            if !more.is_empty() {
                seeds.extend(more);
                kept = walk::walk(&arr, &seeds, m);
            }
        }
        match kept {
            Ok(k) => Ok(pieces
                .iter()
                .zip(k)
                .filter_map(|(p, keep)| {
                    keep.map(|reversed| Kept {
                        piece: p.clone(),
                        reversed,
                    })
                })
                .collect()),
            Err(e) => {
                let (WalkError::Open { piece }
                | WalkError::Ambiguous { piece }
                | WalkError::Clipped { piece }
                | WalkError::Halted { piece }) = e;
                if let WalkError::Clipped { piece } = e {
                    *clip_owners.lock().expect("unpoisoned") = arr.pieces[piece]
                        .1
                        .iter()
                        .filter(|(g, _)| arr.clipped.contains(g))
                        .flat_map(|&(g, _)| &table.edges[g].members)
                        .filter_map(|mm| match mm {
                            Member::Edge(e) if w.clipped.contains(&e.edge) => Some(e.edge),
                            _ => None,
                        })
                        .flat_map(|e| {
                            w.body
                                .edge_fins(e)
                                .map(|fin| w.body.loops[w.body.fins[fin].loop_].face)
                        })
                        .collect();
                }
                let f = pieces[piece].face;
                *walked.lock().expect("unpoisoned") = Some((e, f.face));
                // A marker: the walk's own error is read from `walked`.
                Err(Fail::Uncertain(f, f))
            }
        }
    };
    let pairs = w.pairs();
    // Kept faces tangent along an input edge meet along it (#652): those
    // left as they are, and the faces outside the region, held by `held`.
    let seams: crate::intersect::Seams = w
        .seams(body, &held, narrowed)
        .into_iter()
        .filter(|(k, _)| pairs.contains(k))
        .collect();
    let resolved =
        crate::repair::resolve_seamed(key, &w.body, &pairs, &seams, &select, &|_| false, m);
    let name_of = |f: FaceId| w.body.faces[f].name;
    let (out, journal) = match (resolved, walked.lock().expect("unpoisoned").take()) {
        // A halt answers at once (`work::run` reports it); every other
        // walk failure climbs the ladder.
        (_, Some((e @ WalkError::Halted { .. }, f))) => {
            return done(Err(walk_error(&w, e, f)));
        }
        (_, Some((e, f))) => {
            // A narrowed clip at a working face asks room for it and its
            // working neighbours (design 724, plan 22.A).
            let owners = clip_owners.lock().expect("unpoisoned").clone();
            *ask = ask_room(&w, e, f, &owners, which, |clipped| {
                let working: std::collections::BTreeSet<FaceId> = ws.keys().copied().collect();
                near::room(body, &working, clipped, bx, adjusts, &|g| {
                    near::near_box(body, changes, region, g, bx, rung)
                })
            });
            return Rung::Walked(walk_error(&w, e, f));
        }
        (Ok(Some(r)), None) => r,
        (Ok(None), None) => {
            let least = region.changed.iter().map(|&f| body.faces[f].name).min();
            return Rung::Next(
                TweakError::GapNotHealed {
                    face: least.unwrap_or(Name::from_u128(0)),
                }
                .into(),
            );
        }
        (Err(Fail::Inconsistent), None) => {
            return done(Err(Error::Topology(TopologyError::Inconsistent)));
        }
        (Err(Fail::Uncertain(a, _)), None) => {
            let e: Error = TweakError::Uncertain {
                at: name_of(a.face),
            }
            .into();
            // A first build's `Uncertain` at any pair may be built again
            // narrowed (design 669); a narrowed build's is its answer.
            if which == Build::First {
                return Rung::Uncertain(e);
            }
            return done(Err(e));
        }
        (Err(Fail::Unsupported(a, _)), None) => {
            return done(Err(TweakError::Uncertain {
                at: name_of(a.face),
            }
            .into()));
        }
        (Err(Fail::NonManifold { faces, .. }), None) => {
            let at = faces
                .first()
                .map_or(Name::from_u128(0), |f| name_of(f.face));
            return Rung::Next(TweakError::HealAmbiguous { face: at }.into());
        }
    };
    let faults = crate::check::check(&out);
    if !faults.is_empty() {
        // Inside out: a face carried past the opposite side (#704); the
        // solid between them would have to collapse.
        if faults.iter().all(crate::check::inside_out) {
            let face = region
                .changed
                .iter()
                .map(|&f| body.faces[f].name)
                .min()
                .unwrap_or(Name::from_u128(0));
            return done(Err(TweakError::FaceCollapses { face }.into()));
        }
        return done(Err(Error::Topology(TopologyError::Inconsistent)));
    }
    // Only pairs with a face the re-trim made: the faces outside the
    // region come back as they were (it never cuts them), so their pairs
    // are the input's (NIST CTC 04: 4 s of a fillet's 8.2 s delete).
    let outside: std::collections::BTreeSet<Name> = body
        .faces()
        .filter(|f| !region.faces.contains(f))
        .map(|f| body.faces[f].name)
        .collect();
    let made = |f: FaceId| !outside.contains(&out.faces[f].name);
    for fault in crate::check::check_pairs_in(&out, m, |a, b| made(a) || made(b)) {
        match fault {
            Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b } => {
                let mut faces = [out.faces[a].name, out.faces[b].name];
                faces.sort();
                return Rung::Next(TweakError::SelfIntersecting { faces }.into());
            }
            Fault::CannotCheck { a, .. } => {
                return done(Err(TweakError::Uncertain {
                    at: out.faces[a].name,
                }
                .into()));
            }
            _ => {}
        }
    }
    if let Some((at, needed)) = past_cap(&out) {
        return done(Err(TweakError::ToleranceTooLarge { at, needed }.into()));
    }
    // A curved result inside out, as #704's planar one is.
    if !crate::check::curved_orientation(&out).is_empty() {
        let face = region
            .changed
            .iter()
            .map(|&f| body.faces[f].name)
            .min()
            .unwrap_or(Name::from_u128(0));
        return done(Err(TweakError::FaceCollapses { face }.into()));
    }
    let mut out = out;
    let moved = changes.motion.keys().map(|&f| body.faces[f].name).collect();
    let mut journal = names::finish(key, body, &w, &mut out, journal, &moved);
    // Faces absorbed into a neighbour on their surface are merged into it.
    let mut into: BTreeMap<Name, Vec<Name>> = BTreeMap::new();
    for (&g, &a) in &absorbed {
        into.entry(body.faces[a].name)
            .or_default()
            .push(body.faces[g].name);
    }
    for (a, mut parents) in into {
        if out.by_name(a).is_some() {
            parents.push(a);
            parents.sort();
            journal.push(crate::name::Event::Merged { parents, child: a });
        }
    }
    done(Ok((out, journal)))
}

/// Region faces left as they are that share an edge with another such
/// face on the same surface, the same way round: each mapped to the
/// least-named face of its group, whose working face covers them all.
/// Their own working faces would coincide with it (NIST FTC 09's hole
/// walls, each two half cylinders carried a whole turn each, closed two
/// ways).
fn absorbed(body: &Body, changes: &Changes, region: &Region) -> BTreeMap<FaceId, FaceId> {
    let still = |f: FaceId| {
        region.faces.contains(&f)
            && !changes.deleted.contains(&f)
            && !changes.motion.contains_key(&f)
            && !changes.surface.contains_key(&f)
    };
    let mut up: BTreeMap<FaceId, FaceId> = BTreeMap::new();
    fn root(up: &BTreeMap<FaceId, FaceId>, mut f: FaceId) -> FaceId {
        while let Some(&g) = up.get(&f) {
            if g == f {
                break;
            }
            f = g;
        }
        f
    }
    for e in body.edges() {
        let fs: Vec<FaceId> = body
            .edge_fins(e)
            .map(|fin| body.loops[body.fins[fin].loop_].face)
            .collect();
        let [a, b] = fs.as_slice() else { continue };
        let (a, b) = (*a, *b);
        if a == b || !still(a) || !still(b) {
            continue;
        }
        let (fa, fb) = (&body.faces[a], &body.faces[b]);
        if fa.sense != fb.sense || body.surfaces[fa.surface] != body.surfaces[fb.surface] {
            continue;
        }
        let (ra, rb) = (root(&up, a), root(&up, b));
        if ra != rb {
            let (keep, go) = if body.faces[ra].name < body.faces[rb].name {
                (ra, rb)
            } else {
                (rb, ra)
            };
            up.insert(go, keep);
            up.entry(keep).or_insert(keep);
        }
    }
    up.keys()
        .filter_map(|&f| {
            let r = root(&up, f);
            (r != f).then_some((f, r))
        })
        .collect()
}

/// A walk's failure at face `face` of the working body as a refusal.
fn walk_error(w: &WorkBody, e: WalkError, face: FaceId) -> Error {
    let name = w.body.faces[face].name;
    match e {
        WalkError::Open { .. } => TweakError::GapNotHealed { face: name },
        WalkError::Ambiguous { .. } => TweakError::HealAmbiguous { face: name },
        WalkError::Clipped { .. } if w.fixed.contains(&face) => {
            TweakError::ExtensionUnsupported { face: name }
        }
        WalkError::Clipped { .. } => TweakError::RegionTooSmall { face: name },
        WalkError::Halted { .. } => TweakError::Uncertain { at: name },
    }
    .into()
}

/// What a build whose walk failed with `e` at working-body face `f` asks
/// (design 724, plan 22.A): where `e` is `Clipped` and the build is
/// narrowed, `room` for the input faces of `f` and of `owners` (the
/// working-body faces whose clipped sides the failing piece reached) that
/// are working faces and not `fixed`. A first build, any other failure,
/// or only pinned, outside or fixed faces ask nothing.
///
/// The owners depart from plan 728's Decision 3, which asked room for
/// `f` alone: in NIST STC 10 (#658, census VII) the clipped side a piece
/// of plane `a7b86e65` reached was coplanar plane `ccab7076`'s, which
/// shares no vertex with it (design 724's Decision 2: a sheet ending
/// short).
fn ask_room(
    w: &WorkBody,
    e: WalkError,
    f: FaceId,
    owners: &std::collections::BTreeSet<FaceId>,
    which: Build,
    room: impl FnOnce(&std::collections::BTreeSet<FaceId>) -> Option<near::Adjusts>,
) -> Option<near::Adjusts> {
    if !which.narrowed() || !matches!(e, WalkError::Clipped { .. }) {
        return None;
    }
    let clipped: std::collections::BTreeSet<FaceId> = std::iter::once(f)
        .chain(owners.iter().copied())
        .filter(|g| w.working.contains(g) && !w.fixed.contains(g))
        .map(|g| w.origin[&g])
        .collect();
    if clipped.is_empty() {
        return None;
    }
    room(&clipped)
}

/// Whether the fixed engine's refusal `e` means the topology must change
/// (so `Topology::Change` re-trims): an edge vanishing, a vertex
/// splitting, a face collapsing, faces crossing, or a lone vertex. An
/// `Uncertain` at one of `body`'s vertices is the fixed engine's
/// three-face form of a vertex splitting (`tweak::vertices`: a curve there
/// doesn't pass the point the others meet at), so it re-trims too; every
/// other `Uncertain` answers at once.
pub(crate) fn takes(body: &Body, e: &Error) -> bool {
    match e {
        Error::InvalidTweak(TweakError::Uncertain { at }) => {
            matches!(body.by_name(*at), Some(crate::topo::EntityId::Vertex(_)))
        }
        _ => matches!(
            e,
            Error::InvalidTweak(
                TweakError::EdgeVanishes { .. }
                    | TweakError::VertexSplits { .. }
                    | TweakError::FaceCollapses { .. }
                    | TweakError::SelfIntersecting { .. }
                    | TweakError::Unsupported {
                        case: crate::TweakCase::LoneVertex,
                        ..
                    }
            )
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests;
