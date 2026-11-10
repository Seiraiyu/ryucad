//! Deleting blends and making them again: the blends a request picks,
//! closed over their chains and corners, deleted by growth (13.B's
//! `delete_faces`); then, for a resize or a rebuilt tweak, each made
//! again on the sharp edges between its supports by one `blend::blend`,
//! and its faces, edges and vertices given back the names of those they
//! stand in place of.

use super::recognise::{self, size};
use super::{DeleteBlends, RecogniseOptions, Recognised, Recognition};
use crate::blend::{BlendKind, Chamfer, EdgeBlend};
use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, EntityId, FaceId};
use crate::tweak::delete::{DeleteSpec, Heal, delete_faces_in};
use crate::work::Meter;
use crate::{BlendError, BlendLoss, Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// Picked blends closed over their chains and the corners they reach:
/// the blends' indices and every face to delete.
pub(crate) fn close(
    rec: &Recognition,
    picked: BTreeSet<usize>,
) -> (BTreeSet<usize>, BTreeSet<Name>) {
    let mut blends = picked;
    loop {
        let before = blends.len();
        let chains: BTreeSet<usize> = blends.iter().map(|&i| rec.blends[i].chain).collect();
        for (i, b) in rec.blends.iter().enumerate() {
            if chains.contains(&b.chain) {
                blends.insert(i);
            }
        }
        for c in &rec.corners {
            if c.blends.iter().any(|i| blends.contains(i)) {
                blends.extend(c.blends.iter().copied());
            }
        }
        if blends.len() == before {
            break;
        }
    }
    let mut faces: BTreeSet<Name> = blends
        .iter()
        .flat_map(|&i| rec.blends[i].faces.iter().copied())
        .collect();
    for c in &rec.corners {
        if c.blends.iter().any(|i| blends.contains(i)) {
            faces.insert(c.face);
        }
    }
    (blends, faces)
}

/// The blends holding `faces`: `NotABlend` for a face that is neither a
/// blend's nor a corner's.
fn holding(rec: &Recognition, faces: &[Name]) -> Result<BTreeSet<usize>, Error> {
    let mut out = BTreeSet::new();
    for &f in faces {
        match rec.holding(f) {
            Some(Ok(i)) => {
                out.insert(i);
            }
            Some(Err(j)) => out.extend(rec.corners[j].blends.iter().copied()),
            None => return Err(TweakError::NotABlend { face: f }.into()),
        }
    }
    Ok(out)
}

/// `faces` deleted by growth.
fn grow(
    key: OpKey,
    body: &Body,
    faces: &BTreeSet<Name>,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    delete_faces_in(
        key,
        body,
        &DeleteSpec {
            faces: faces.iter().copied().collect(),
            heal: Heal::Grow,
        },
        m,
    )
}

pub(crate) fn delete_blends(
    key: OpKey,
    body: &Body,
    spec: &DeleteBlends,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    if let Some(s) = spec.max_size
        && !(s.is_finite() && s >= 0.0)
    {
        return Err(TweakError::BadDistance.into());
    }
    let rec = recognise::recognise(body, &spec.options, m)?;
    let mut picked = holding(&rec, &spec.faces)?;
    if let Some(s) = spec.max_size {
        for (i, b) in rec.blends.iter().enumerate() {
            if size(&b.kind) <= s {
                picked.insert(i);
            }
        }
    }
    if picked.is_empty() {
        return Ok((body.clone(), Journal::new()));
    }
    let (_, faces) = close(&rec, picked);
    grow(key, body, &faces, m)
}

/// The edges of `body` between a face named `a` and one named `b`.
fn between(body: &Body, [a, b]: [Name; 2]) -> Vec<Name> {
    body.edges()
        .filter(|&e| {
            let names: Vec<Name> = body
                .edge_fins(e)
                .map(|fin| body.faces[body.loops[body.fins[fin].loop_].face].name)
                .collect();
            names.len() == 2 && names.contains(&a) && names.contains(&b)
        })
        .map(|e| body.edges[e].name)
        .collect()
}

/// Why a blend made again was refused, in a word.
fn loss(e: &BlendError) -> BlendLoss {
    match e {
        BlendError::Overflow { .. } => BlendLoss::Overflow,
        BlendError::RadiusTooLarge { .. } => BlendLoss::RadiusTooLarge,
        BlendError::Unsupported { .. } => BlendLoss::Unsupported,
        BlendError::NonManifold { .. } => BlendLoss::NonManifold,
        BlendError::SmoothEdge { .. } => BlendLoss::SmoothEdge,
        _ => BlendLoss::Uncertain,
    }
}

/// The edge a blend refusal names, if any.
fn refused_edge(e: &BlendError) -> Option<Name> {
    match e {
        BlendError::Overflow { edge, .. }
        | BlendError::RadiusTooLarge { edge }
        | BlendError::SmoothEdge { edge }
        | BlendError::BadSize { edge }
        | BlendError::ConvexityChanges { edge } => Some(*edge),
        _ => None,
    }
}

/// `blends` made again on `body`, each with its kind, on the edges
/// between its supports: one blend call. A blend with no edge there, or
/// one the call refuses, is `BlendLost` naming its least face. Where the
/// call refuses `Overflow` because a cap-closed feature (a hole, a boss)
/// stands in a blend's strip on one of its supports, the feature is
/// capped, the blends made on the capped body, and the feature cut or
/// added again by a boolean ([`through_features`]).
fn reblend(
    key: OpKey,
    orig: &Body,
    body: &Body,
    blends: &[(Recognised, BlendKind)],
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    match reblend_plain(key, orig, body, blends, m) {
        Err(r) => match r.overflow(blends) {
            Some((edge, face)) => match through_features(key, orig, body, blends, edge, face, m) {
                Some(done) => done,
                None => Err(r.lost),
            },
            None => Err(r.lost),
        },
        Ok(done) => Ok(done),
    }
}

/// The feature on face `host` (a cap-closed set across one of its inner
/// loops) whose loop lies nearest `edge`: its faces.
fn feature_near(body: &Body, host: Name, edge: Name) -> Option<Vec<Name>> {
    let (Some(EntityId::Face(h)), Some(EntityId::Edge(e))) =
        (body.by_name(host), body.by_name(edge))
    else {
        return None;
    };
    let ed = &body.edges[e];
    let curve = &body.curves[ed.curve];
    let mut best: Option<(f64, Vec<Name>)> = None;
    for l in body.face_loops(h) {
        let fins: Vec<_> = body.loop_fins(l).collect();
        let edges: BTreeSet<_> = fins.iter().map(|&fin| body.fins[fin].edge).collect();
        if edges.contains(&e) {
            continue;
        }
        let Some(seed) = fins
            .first()
            .and_then(|&fin| recognise::across(body, body.fins[fin].edge, h))
        else {
            continue;
        };
        let Ok(faces) = crate::tweak::delete::feature_faces(body, body.faces[seed].name) else {
            continue;
        };
        let d = edges
            .iter()
            .flat_map(|&x| [body.edges[x].start, body.edges[x].end])
            .map(|v| {
                let p = body.vertices[v].point;
                curve.point(curve.project(p).param).distance(p)
            })
            .fold(f64::INFINITY, f64::min);
        if best.as_ref().is_none_or(|(b, _)| d < *b) {
            best = Some((d, faces));
        }
    }
    best.map(|(_, f)| f)
}

/// The blends made through the features standing in their strips: each
/// feature nearest an overflowing rail capped (at most
/// [`FEATURES`] of them), the blends made on the capped body, then the
/// material the features took away subtracted and what they added
/// united, both found from the capped body and `body` by booleans. The
/// features' faces take back their names through the booleans' journals.
/// `None` where no feature is found or the capped body's blends still
/// overflow on no feature.
fn through_features(
    key: OpKey,
    orig: &Body,
    body: &Body,
    blends: &[(Recognised, BlendKind)],
    mut edge: Name,
    mut face: Name,
    m: &Meter,
) -> Option<Result<(Body, Journal), Error>> {
    let mut capped = body.clone();
    let mut taken: Vec<Name> = Vec::new();
    for _ in 0..FEATURES {
        let faces = feature_near(&capped, face, edge)?;
        let spec = DeleteSpec {
            faces,
            heal: Heal::Cap,
        };
        let (next, _) = match delete_faces_in(key, &capped, &spec, m) {
            Ok(r) => r,
            Err(e) => return Some(Err(e)),
        };
        taken.extend(spec.faces);
        capped = next;
        match reblend_plain(key, orig, &capped, blends, m) {
            Err(r) => match r.overflow(blends) {
                Some(at) => (edge, face) = at,
                None => return Some(Err(r.lost)),
            },
            Ok((round, _)) => {
                let r = (|| -> Result<(Body, Journal), Error> {
                    let (out, steps) = recut(key, body, &capped, round, &taken, m)?;
                    let mut out = out;
                    let remade = blends.iter().flat_map(|(b, _)| b.faces.iter().copied());
                    for n in taken.clone().into_iter().chain(remade) {
                        if let Some(x) = follow(n, &steps, &out) {
                            rename(orig, &mut out, x, n);
                        }
                    }
                    Ok((out, Journal::new()))
                })();
                return Some(r);
            }
        }
    }
    None
}

/// The features `taken` (faces of `body`, capped in `capped`) made again
/// on `round`, the capped body blended: what they took away is the lump
/// of `capped` the sheet of their faces cuts off, subtracted; what they
/// added the lump of `body` the sheet of the capped hosts cuts off,
/// united. A lump is the one holding the fewest of the other body's own
/// face names. Returns the body and the journals in order.
fn recut(
    key: OpKey,
    body: &Body,
    capped: &Body,
    round: Body,
    taken: &[Name],
    m: &Meter,
) -> Result<(Body, Vec<Journal>), Error> {
    use crate::boolean::{BooleanOp, Tool, boolean_in, divide_in};
    use crate::geom::Rigid;
    use crate::name::{EditRole, Role, ToolKey};
    let k = |i: u128| {
        OpKey(Name::derive(key, Role::Edit(EditRole::Feature), &[Name::from_u128(i)]).to_u128())
    };
    fn tool(b: &Body) -> Tool<'_> {
        Tool {
            key: ToolKey(1),
            body: b,
            place: Rigid::IDENTITY,
        }
    }
    let own = |b: &Body, of: &Body| {
        of.faces()
            .filter(|&f| b.by_name(of.faces[f].name).is_some())
            .count()
    };
    // The plug: `capped` cut by the sheet of the features' faces.
    let (walls, _) = crate::sheet::extract(k(1), body, taken)?;
    let mut steps = Vec::new();
    let mut out = round;
    if let Ok((d, j)) = divide_in(k(2), capped, &[tool(&walls)], m) {
        let lumps: Vec<&Body> = d.front.iter().chain(&d.back).collect();
        if lumps.len() >= 2 {
            let plug = lumps.iter().min_by_key(|b| own(b, capped)).expect("two");
            steps.push(j);
            let (o, j) = boolean_in(k(3), BooleanOp::Subtract, &out, &[tool(plug)], m)?;
            steps.push(j);
            return Ok((o, steps));
        }
    }
    // An addition: `body` cut by the sheet of the hosts the caps closed.
    let taken_set: BTreeSet<Name> = taken.iter().copied().collect();
    let hosts: Vec<Name> = body
        .faces()
        .filter(|&f| !taken_set.contains(&body.faces[f].name))
        .filter(|&f| neighbours(body, f).iter().any(|n| taken_set.contains(n)))
        .map(|f| body.faces[f].name)
        .filter(|&n| capped.by_name(n).is_some())
        .collect();
    let (lid, _) = crate::sheet::extract(k(4), capped, &hosts)?;
    let (d, j) = divide_in(k(5), body, &[tool(&lid)], m)?;
    let lumps: Vec<&Body> = d.front.iter().chain(&d.back).collect();
    let Some(boss) = lumps.iter().min_by_key(|b| own(b, body)) else {
        return Err(TweakError::Uncertain { at: taken[0] }.into());
    };
    steps.push(j);
    let (o, j) = boolean_in(k(6), BooleanOp::Unite, &out, &[tool(boss)], m)?;
    steps.push(j);
    out = o;
    Ok((out, steps))
}

/// Features a blend made again may be made through.
const FEATURES: usize = 8;

/// The one live face of `out` that name `n` became through `steps`'
/// placings and splits, if there is exactly one.
pub(crate) fn follow(n: Name, steps: &[Journal], out: &Body) -> Option<EntityId> {
    let mut now: BTreeSet<Name> = BTreeSet::from([n]);
    for j in steps {
        let mut next = BTreeSet::new();
        for x in &now {
            let mut went = false;
            for e in j.events() {
                match e {
                    Event::Placed { from, to, .. } if from == x => {
                        next.insert(*to);
                        went = true;
                    }
                    Event::Split { parent, children } if parent == x => {
                        next.extend(children.iter().copied());
                        went = true;
                    }
                    _ => {}
                }
            }
            if !went {
                next.insert(*x);
            }
        }
        // A split after a placing: follow the placed names too.
        for e in j.events() {
            if let Event::Split { parent, children } = e
                && next.contains(parent)
            {
                next.remove(parent);
                next.extend(children.iter().copied());
            }
        }
        now = next;
    }
    let live: Vec<EntityId> = now
        .iter()
        .filter_map(|x| out.by_name(*x))
        .filter(|x| matches!(x, EntityId::Face(_)))
        .collect();
    match live.as_slice() {
        [x] => Some(*x),
        _ => None,
    }
}

/// A blend made again refused: the blend's own refusal, and the
/// `BlendLost` it maps to.
struct Refusal {
    raw: Error,
    lost: Error,
}

impl Refusal {
    /// The edge and face an `Overflow` names, where the face is one of
    /// the blends' supports: a feature on it may stand in the strip.
    fn overflow(&self, blends: &[(Recognised, BlendKind)]) -> Option<(Name, Name)> {
        match self.raw {
            Error::InvalidBlend(BlendError::Overflow { edge, face })
                if blends.iter().any(|(b, _)| b.supports.contains(&face)) =>
            {
                Some((edge, face))
            }
            _ => None,
        }
    }
}

/// [`reblend`] without the features: the blend's refusal beside the
/// `BlendLost` it maps to.
fn reblend_plain(
    key: OpKey,
    orig: &Body,
    body: &Body,
    blends: &[(Recognised, BlendKind)],
    m: &Meter,
) -> Result<(Body, Journal), Box<Refusal>> {
    let mut edges = Vec::new();
    let mut owner: BTreeMap<Name, Name> = BTreeMap::new();
    for (b, kind) in blends {
        let found = between(body, b.supports);
        if found.is_empty() {
            let lost: Error = TweakError::BlendLost {
                face: b.faces[0],
                reason: BlendLoss::NoEdge,
            }
            .into();
            return Err(Box::new(Refusal {
                raw: lost.clone(),
                lost,
            }));
        }
        for e in found {
            owner.insert(e, b.faces[0]);
            edges.push(EdgeBlend {
                edge: e,
                kind: *kind,
            });
        }
    }
    let (mut out, journal) = crate::blend::blend_in(key, body, &edges, m).map_err(|e| {
        let lost = match &e {
            Error::InvalidBlend(be) => {
                let face = refused_edge(be)
                    .and_then(|n| owner.get(&n).copied())
                    .unwrap_or(blends[0].0.faces[0]);
                TweakError::BlendLost {
                    face,
                    reason: loss(be),
                }
                .into()
            }
            _ => e.clone(),
        };
        Box::new(Refusal { raw: e, lost })
    })?;
    name_blends(orig, &mut out, &journal, blends, &owner);
    Ok((out, journal))
}

/// Gives each blend made again the names of its faces in `orig`: its new
/// faces are those the blend's journal says replaced the edges it was
/// made on; one new face for one old takes its name, and several are
/// paired with the old faces on coincident surfaces, each pairing only
/// where it is one to one.
fn name_blends(
    orig: &Body,
    out: &mut Body,
    journal: &Journal,
    blends: &[(Recognised, BlendKind)],
    owner: &BTreeMap<Name, Name>,
) {
    for (b, _) in blends {
        let mut new: BTreeSet<Name> = BTreeSet::new();
        for e in journal.events() {
            if let Event::Replaced { old, new: faces } = e
                && owner.get(old) == Some(&b.faces[0])
            {
                new.extend(
                    faces
                        .iter()
                        .copied()
                        .filter(|n| matches!(out.by_name(*n), Some(EntityId::Face(_)))),
                );
            }
        }
        // Corner faces replace vertices, not edges: left to `restore`.
        let new: Vec<Name> = new
            .into_iter()
            .filter(|n| match out.by_name(*n) {
                Some(EntityId::Face(f)) => !matches!(
                    out.surfaces[out.faces[f].surface],
                    crate::geom::Surface::Sphere(_)
                ),
                _ => false,
            })
            .collect();
        let surface = |body: &Body, n: Name| match body.by_name(n) {
            Some(EntityId::Face(f)) => Some(body.surfaces[body.faces[f].surface].clone()),
            _ => None,
        };
        if let ([o], [n]) = (b.faces.as_slice(), new.as_slice()) {
            if let Some(EntityId::Face(f)) = out.by_name(*n) {
                rename(orig, out, EntityId::Face(f), *o);
            }
            continue;
        }
        for &o in &b.faces {
            let Some(so) = surface(orig, o) else { continue };
            let on: Vec<Name> = new
                .iter()
                .copied()
                .filter(|&n| {
                    surface(out, n)
                        .is_some_and(|sn| crate::intersect::coincident(&so, &sn) == Some(true))
                })
                .collect();
            let olds = b
                .faces
                .iter()
                .filter(|&&p| {
                    surface(orig, p).is_some_and(|sp| {
                        on.iter().any(|&n| {
                            surface(out, n).is_some_and(|sn| {
                                crate::intersect::coincident(&sp, &sn) == Some(true)
                            })
                        })
                    })
                })
                .count();
            if let ([n], 1) = (on.as_slice(), olds)
                && let Some(x) = out.by_name(*n)
            {
                rename(orig, out, x, o);
            }
        }
    }
}

/// The names of the faces around face `f`.
fn neighbours(body: &Body, f: FaceId) -> BTreeSet<Name> {
    recognise::edges_of(body, f)
        .into_iter()
        .filter_map(|e| recognise::across(body, e, f))
        .map(|g| body.faces[g].name)
        .collect()
}

/// The faces around an edge or a vertex, by name.
fn faces_at(body: &Body, entity: EntityId) -> BTreeSet<Name> {
    let edges = match entity {
        EntityId::Edge(e) => vec![e],
        EntityId::Vertex(v) => body.vertex_edges(v),
        _ => Vec::new(),
    };
    edges
        .into_iter()
        .flat_map(|e| body.edge_fins(e).collect::<Vec<_>>())
        .map(|fin| body.faces[body.loops[body.fins[fin].loop_].face].name)
        .collect()
}

/// Gives the entities of `out` the blend made again the names of the
/// entities of `input` they stand in place of: a new face the name of the
/// one old face whose neighbours (those both bodies have, or already
/// named back) are its own, again until nothing more is named; then a new
/// edge or vertex that of the one old edge or vertex with the same faces
/// round it. Returns the names given back.
fn restore(input: &Body, out: &mut Body) -> BTreeSet<Name> {
    let had: BTreeSet<Name> = input
        .faces()
        .map(|f| input.faces[f].name)
        .chain(input.edges().map(|e| input.edges[e].name))
        .chain(input.vertices().map(|v| input.vertices[v].name))
        .collect();
    let has = |b: &Body, n: Name| b.by_name(n).is_some();
    let mut back: BTreeSet<Name> = BTreeSet::new();
    // Faces, round by round.
    loop {
        let known = |n: &Name, b: &Body| has(input, *n) && has(b, *n);
        let new: Vec<(FaceId, BTreeSet<Name>)> = out
            .faces()
            .filter(|&f| !had.contains(&out.faces[f].name))
            .map(|f| {
                let ns = neighbours(out, f)
                    .into_iter()
                    .filter(|n| known(n, out))
                    .collect();
                (f, ns)
            })
            .collect();
        let old: Vec<(Name, BTreeSet<Name>)> = input
            .faces()
            .filter(|&f| !has(out, input.faces[f].name))
            .map(|f| {
                let ns = neighbours(input, f)
                    .into_iter()
                    .filter(|n| known(n, out))
                    .collect();
                (input.faces[f].name, ns)
            })
            .collect();
        let mut named = false;
        for (f, ns) in &new {
            if ns.is_empty() {
                continue;
            }
            let olds: Vec<&Name> = old
                .iter()
                .filter(|(_, o)| o == ns)
                .map(|(n, _)| n)
                .collect();
            let news = new.iter().filter(|(_, o)| o == ns).count();
            if let ([o], 1) = (olds.as_slice(), news) {
                rename(input, out, EntityId::Face(*f), **o);
                back.insert(**o);
                named = true;
            }
        }
        if !named {
            break;
        }
    }
    // Edges and vertices by the faces round them.
    let entities = |b: &Body| -> Vec<EntityId> {
        b.edges()
            .map(EntityId::Edge)
            .chain(b.vertices().map(EntityId::Vertex))
            .collect()
    };
    let name_of = |b: &Body, x: EntityId| b.name_of(x).expect("a live entity");
    let new: Vec<(EntityId, BTreeSet<Name>)> = entities(out)
        .into_iter()
        .filter(|&x| !had.contains(&name_of(out, x)))
        .map(|x| (x, faces_at(out, x)))
        .collect();
    let old: Vec<(EntityId, Name, BTreeSet<Name>)> = entities(input)
        .into_iter()
        .filter(|&x| !has(out, name_of(input, x)))
        .map(|x| (x, name_of(input, x), faces_at(input, x)))
        .collect();
    let same_kind = |a: EntityId, b: EntityId| {
        matches!(
            (a, b),
            (EntityId::Edge(_), EntityId::Edge(_)) | (EntityId::Vertex(_), EntityId::Vertex(_))
        )
    };
    for (x, fs) in &new {
        let olds: Vec<Name> = old
            .iter()
            .filter(|(y, _, o)| same_kind(*x, *y) && o == fs)
            .map(|(_, n, _)| *n)
            .collect();
        let news = new
            .iter()
            .filter(|(y, o)| same_kind(*x, *y) && o == fs)
            .count();
        if let ([o], 1) = (olds.as_slice(), news)
            && !has(out, *o)
        {
            rename(input, out, *x, *o);
            back.insert(*o);
        }
    }
    back
}

/// Renames `x` of `out` to `name`, with `input`'s recipe for it.
fn rename(input: &Body, out: &mut Body, x: EntityId, name: Name) {
    out.rename(x, name);
    if let (Some(recipes), Some(text)) = (&mut out.recipes, input.recipe(name)) {
        recipes.insert(name, text.to_owned());
    }
}

/// The journal from `input` to `out` across several steps: each step's
/// splits, merges and replacements whose every name either body has;
/// `Modified` for each name both have that a step touched or that was
/// given back; `Deleted` for each `input` name `out` lacks; `Created`
/// for each `out` name `input` lacks.
fn net(input: &Body, out: &Body, steps: &[&Journal], back: &BTreeSet<Name>) -> Journal {
    let names = |b: &Body| -> BTreeSet<Name> {
        b.faces()
            .map(|f| b.faces[f].name)
            .chain(b.edges().map(|e| b.edges[e].name))
            .chain(b.vertices().map(|v| b.vertices[v].name))
            .collect()
    };
    let (was, now) = (names(input), names(out));
    let known = |n: &Name| was.contains(n) || now.contains(n);
    let mut touched: BTreeSet<Name> = back.clone();
    let mut j = Journal::new();
    for step in steps {
        for e in step.events() {
            match e {
                Event::Modified(n) | Event::Created(n) | Event::Deleted(n) => {
                    touched.insert(*n);
                }
                Event::Split { parent, children }
                    if known(parent) && children.iter().all(known) =>
                {
                    j.push(e.clone());
                }
                Event::Merged { parents, child } if parents.iter().all(known) && known(child) => {
                    j.push(e.clone());
                }
                Event::Replaced { old, new } if known(old) && new.iter().all(known) => {
                    j.push(e.clone());
                }
                _ => {}
            }
        }
    }
    for n in &was {
        if !now.contains(n) {
            j.push(Event::Deleted(*n));
        } else if touched.contains(n) {
            j.push(Event::Modified(*n));
        }
    }
    for n in &now {
        if !was.contains(n) {
            j.push(Event::Created(*n));
        }
    }
    j
}

/// The blends `rec` names deleted from `body`, `step` made on the sharp
/// body, and each blend made again with its kind in `kinds`, names given
/// back.
fn remake<X>(
    key: OpKey,
    body: &Body,
    rec: &Recognition,
    kinds: &BTreeMap<usize, BlendKind>,
    m: &Meter,
    step: impl Fn(&Body, &Meter) -> Result<(Body, Journal, X), Error>,
) -> Result<(Body, Journal, X), Error> {
    let (blends, faces) = close(rec, kinds.keys().copied().collect());
    let (sharp, j1) = grow(key, body, &faces, m)?;
    let (stepped, j2, x) = step(&sharp, m)?;
    let todo: Vec<(Recognised, BlendKind)> = blends
        .iter()
        .map(|&i| {
            let b = rec.blends[i].clone();
            let k = kinds.get(&i).copied().unwrap_or(b.kind);
            (b, k)
        })
        .collect();
    let (mut out, j3) = reblend(key, body, &stepped, &todo, m)?;
    let back = restore(body, &mut out);
    let j = net(body, &out, &[&j1, &j2, &j3], &back);
    Ok((out, j, x))
}

pub(crate) fn resize(
    key: OpKey,
    body: &Body,
    sizes: &[(Name, BlendKind)],
    opts: &RecogniseOptions,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    if sizes.is_empty() {
        return Err(TweakError::NoFaces.into());
    }
    let rec = recognise::recognise(body, opts, m)?;
    let mut kinds = BTreeMap::new();
    for (face, kind) in sizes {
        let i = match rec.holding(*face) {
            Some(Ok(i)) => i,
            _ => return Err(TweakError::NotABlend { face: *face }.into()),
        };
        // A chamfer's `first_face` names a support; any other face is the
        // blend's own refusal when it is made.
        if let BlendKind::Chamfer(
            Chamfer::Distances { first_face, .. }
            | Chamfer::DistanceAngle {
                face: first_face, ..
            },
        ) = kind
            && !rec.blends[i].supports.contains(first_face)
        {
            return Err(TweakError::NotABlend { face: *first_face }.into());
        }
        // Every blend of a chain at the one size.
        for (j, b) in rec.blends.iter().enumerate() {
            if b.chain == rec.blends[i].chain {
                kinds.insert(j, *kind);
            }
        }
    }
    let (out, j, ()) = remake(key, body, &rec, &kinds, m, |b, _| {
        Ok((b.clone(), Journal::new(), ()))
    })?;
    Ok((out, j))
}

/// Whether a tweak's refusal is one a blend in the way may cause.
pub(crate) fn breaks(e: &Error) -> bool {
    matches!(
        e,
        Error::InvalidTweak(
            TweakError::TangencyBroken { .. }
                | TweakError::EdgeVanishes { .. }
                | TweakError::VertexSplits { .. }
                | TweakError::FaceCollapses { .. }
                | TweakError::SelfIntersecting { .. }
                | TweakError::GapNotHealed { .. }
                | TweakError::HealAmbiguous { .. }
                | TweakError::RegionTooSmall { .. }
        )
    )
}

/// The faces round the entity a tweak's refusal names.
fn blamed(body: &Body, e: &Error) -> BTreeSet<Name> {
    let Error::InvalidTweak(t) = e else {
        return BTreeSet::new();
    };
    let name = match t {
        TweakError::TangencyBroken { edge } | TweakError::EdgeVanishes { edge } => *edge,
        TweakError::VertexSplits { vertex } => *vertex,
        TweakError::FaceCollapses { face }
        | TweakError::GapNotHealed { face }
        | TweakError::HealAmbiguous { face }
        | TweakError::RegionTooSmall { face } => *face,
        TweakError::SelfIntersecting { faces } => return faces.iter().copied().collect(),
        _ => return BTreeSet::new(),
    };
    match body.by_name(name) {
        Some(EntityId::Face(_)) => BTreeSet::from([name]),
        Some(x) => faces_at(body, x),
        None => BTreeSet::new(),
    }
}

/// The tweak `run` made over the blends in its way: those standing on
/// `faces` or on a face next to one (a hole's wall moved into a fillet's
/// strip stands on the fillet's support), and those holding or standing
/// on the faces round what the tweak's own refusal `refused` names. `None` where no blend is in
/// the way (the tweak's own refusal stands), or where the faces it names
/// are themselves blends' faces.
pub(crate) fn rebuild<X>(
    key: OpKey,
    body: &Body,
    faces: &[Name],
    refused: &Error,
    m: &Meter,
    run: &impl Fn(&Body, &Meter) -> Result<(Body, Journal, X), Error>,
) -> Option<Result<(Body, Journal, X), Error>> {
    let rec = match recognise::recognise(body, &RecogniseOptions::default(), m) {
        Ok(r) => r,
        Err(e) => return Some(Err(e)),
    };
    let named: BTreeSet<Name> = faces.iter().copied().collect();
    let mut blame = blamed(body, refused);
    // The faces next to the changed ones: a hole's wall moved into a
    // fillet's strip stands on the fillet's support.
    for &n in &named {
        if let Some(EntityId::Face(f)) = body.by_name(n) {
            blame.extend(neighbours(body, f));
        }
    }
    let kinds: BTreeMap<usize, BlendKind> = rec
        .blends
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            b.supports
                .iter()
                .chain(&b.faces)
                .any(|s| named.contains(s) || blame.contains(s))
        })
        .filter(|(_, b)| !b.faces.iter().any(|f| named.contains(f)))
        .map(|(i, b)| (i, b.kind))
        .collect();
    if kinds.is_empty() {
        return None;
    }
    let (_, gone) = close(&rec, kinds.keys().copied().collect());
    if gone.iter().any(|f| named.contains(f)) {
        return None;
    }
    Some(remake(key, body, &rec, &kinds, m, run))
}
