//! Unfolds and refolds (plan 16.B): one sheet metal solid laid flat from
//! then on in its chain, and folded back by its unfold's flat map. Both
//! act on their target as a flange does, through one kernel call under
//! the feature's key; the request's cancel is checked before and after
//! it (the kernel's sheet metal calls take none).

use super::allowance::{self, Kernel};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sheet_metal::{Allowance, Refold, Unfold};
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Flat, FlatBend, Made, Origins, Results, Run};
use crate::solid::Solids;
use ryukei::name::{Event, Journal, Name};
use ryukei::sheet_metal::{Bend, RefoldSpec, SheetModel, SheetOptions, UnfoldSpec};
use ryukei::topo::{Body, EntityId};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The bend of `model` one of whose inner or outer faces is `n`.
fn bend_named(model: &SheetModel, n: Name) -> Option<&Bend> {
    model
        .bends
        .iter()
        .find(|b| b.inner.contains(&n) || b.outer.contains(&n))
}

/// A bend as `sheet.get` names it: its least inner face.
fn bend_text(part: &Id, solid: &Id, b: &Bend) -> String {
    format!("{part}/{solid}/{}", b.inner[0])
}

fn not_a_bend(
    part: &Part,
    id: &Id,
    at: &str,
    r: &Reference,
    model: &SheetModel,
    solid: &Id,
) -> Error {
    Error::new(
        Code::InvalidParam,
        format!(
            "unfold {id}: {} at {at} is no bend's inner or outer face",
            r.text(&part.id)
        ),
    )
    .with("feature", id)
    .with("pointer", at)
    .with(
        "bends",
        model
            .bends
            .iter()
            .map(|b| bend_text(&part.id, solid, b))
            .collect::<Vec<_>>(),
    )
}

/// An unfold resolved against what it reads: its target as it sees it,
/// recognized, the kernel's spec, and the bends it unfolds.
pub(crate) struct Resolved<'a> {
    pub built: &'a Built,
    pub model: SheetModel,
    pub spec: UnfoldSpec,
    pub bends: Vec<FlatBend>,
    pub partial: bool,
}

/// Resolves unfold `id` against `before`: its references faces of its
/// target as it sees it, the target sheet metal, `fixed` a side face,
/// `only` and every override's `bend` a bend's face, and every unfolded
/// bend an allowance (its override's, the unfold's, its founder's).
pub(crate) fn resolve<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    u: &Unfold,
    before: &'a dyn Results,
) -> Result<Resolved<'a>> {
    let target = &u.target;
    for (role, r) in u.references() {
        if r.feature != *target {
            return Err(Error::new(
                Code::TargetsMixed,
                format!(
                    "unfold {id}: its {role} {} is of {}, not of its target {target}; an unfold acts on one solid",
                    r.text(&part.id),
                    r.feature
                ),
            )
            .with("feature", id)
            .with("role", &role)
            .with("solids", [target.as_str(), r.feature.as_str()]));
        }
    }
    let built = crate::rebuild::target(part, solids, id, target, before)?;
    for (role, r) in u.references() {
        let (_, e) = crate::reference::resolve(part, part.order(), id, &role, r, before)?;
        if !matches!(e, EntityId::Face(_)) {
            return Err(crate::reference::wrong_kind(
                part,
                id,
                &role,
                r,
                crate::reference::kind(e),
                "a face",
            ));
        }
    }
    let model = ryukei::sheet_metal::recognize(&built.body, &SheetOptions::default())
        .map_err(|e| super::refusal(&part.id, id, "unfold", (target, built), e))?;
    let fixed = match &u.fixed {
        None => None,
        Some(r) => {
            let side = model
                .flanges
                .iter()
                .any(|f| f.sides.iter().any(|s| s.faces.contains(&r.name)));
            if !side {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "unfold {id}: its fixed face {} isn't a side of a flange: hold still a face on either side of the sheet",
                        r.text(&part.id)
                    ),
                )
                .with("feature", id)
                .with("pointer", "/fixed"));
            }
            Some(r.name)
        }
    };
    let mut only: Vec<Name> = Vec::new();
    for (i, r) in u.only.iter().enumerate() {
        let b = bend_named(&model, r.name)
            .ok_or_else(|| not_a_bend(part, id, &format!("/only/{i}"), r, &model, target))?;
        if !only.contains(&b.name) {
            only.push(b.name);
        }
    }
    let mut overrides: Vec<(Name, &Allowance, String)> = Vec::new();
    for (i, o) in u.bends.iter().enumerate() {
        let at = format!("/bends/{i}/bend");
        let b = bend_named(&model, o.bend.name)
            .ok_or_else(|| not_a_bend(part, id, &at, &o.bend, &model, target))?;
        overrides.push((b.name, &o.allowance, format!("/bends/{i}")));
    }
    let solid = u
        .allowance
        .as_ref()
        .or(super::founder(part, target).map(|f| &f.allowance));
    let t = model.thickness;
    let mut kernel = Vec::new();
    let mut bends = Vec::new();
    for b in model
        .bends
        .iter()
        .filter(|b| only.is_empty() || only.contains(&b.name))
    {
        let named = bend_text(&part.id, target, b);
        let (a, at) = match overrides.iter().find(|(n, _, _)| *n == b.name) {
            Some((_, a, at)) => (*a, at.clone()),
            None => match solid {
                Some(a) => (a, "/allowance".to_owned()),
                None => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("unfold {id}: its target {target} wasn't founded by a base flange, so it has no allowance to take: give the unfold its allowance, or bend {named} its own"),
                    )
                    .with("feature", id)
                    .with("pointer", "/allowance")
                    .with("bend", &named));
                }
            },
        };
        let k = allowance::kernel(a, b.radius, t, b.angle).map_err(|e| {
            Error::new(
                Code::InvalidParam,
                format!("unfold {id}: bend {named}: {}", e.message()),
            )
            .with("feature", id)
            .with("pointer", &at)
            .with("bend", &named)
        })?;
        kernel.push((b.name, k));
        bends.push(FlatBend {
            name: b.name,
            inner: b.inner.clone(),
            outer: b.outer.clone(),
            allowance: allowance::note(&k),
        });
    }
    let partial = !only.is_empty() && only.len() < model.bends.len();
    let spec = UnfoldSpec {
        fixed,
        bends: (!only.is_empty()).then_some(only),
        // Every unfolded bend's own is among the overrides: never read.
        allowance: Kernel::KFactor(0.5),
        overrides: kernel,
    };
    Ok(Resolved {
        built,
        model,
        spec,
        bends,
        partial,
    })
}

/// An unfold: its target as it sees it laid flat, every name and origin
/// kept, its flat beside it.
pub fn unfold(
    part: &Part,
    solids: &Solids,
    id: &Id,
    u: &Unfold,
    before: &dyn Results,
    run: &Run,
) -> Result<Made> {
    let r = resolve(part, solids, id, u, before)?;
    let key = crate::rebuild::op_key(&part.id, id);
    run.job.check()?;
    let out = ryukei::sheet_metal::unfold(key, &r.built.body, &r.model, &r.spec)
        .map_err(|e| super::refusal(&part.id, id, "unfold", (&u.target, r.built), e))?;
    run.job.check()?;
    let mut bends = r.bends;
    bends.sort_by_key(|b| out.map.strips.iter().position(|s| s.bend == b.name));
    let flat = Flat {
        unfold: id.clone(),
        map: out.map,
        pattern: out.pattern,
        bends,
        partial: r.partial,
    };
    let built = crate::rebuild::finish(
        id,
        "unfold",
        (out.body, out.journal),
        (*r.built.origins).clone(),
    )?;
    Ok(Made {
        flat: Some(Arc::new(flat)),
        ..Made::new(vec![(u.target.clone(), Arc::new(built))])
    })
}

fn refold_refused(id: &Id, at: &str, msg: String) -> Error {
    Error::new(Code::InvalidParam, format!("refold {id}: {msg}"))
        .with("feature", id)
        .with("pointer", at)
}

/// A refold's target as it sees it, its unfold's flat and the strips it
/// folds (`None`: every one).
type Folding<'a> = (&'a Built, Arc<Flat>, Option<Vec<Name>>);

/// What refold `id` folds and by what: its target as it sees it and its
/// unfold's flat, with the strips it folds. Its unfold must be an unfold
/// of its target listed before it, with no refold of it between; each of
/// `only` a face of a bend that unfold unfolded.
pub(crate) fn refold_resolve<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    x: &Refold,
    before: &'a dyn Results,
) -> Result<Folding<'a>> {
    let target = &x.target;
    let u = &x.unfold;
    match part.features.get(u) {
        Some(Ok(Feature::Unfold(f))) if f.target == *target => {}
        Some(Ok(Feature::Unfold(f))) => {
            return Err(refold_refused(
                id,
                "/unfold",
                format!(
                    "its unfold {u} lays {} flat, not its target {target}",
                    f.target
                ),
            ));
        }
        _ => {
            return Err(refold_refused(
                id,
                "/unfold",
                format!("{u} isn't an unfold: a refold folds by an earlier unfold of its target"),
            ));
        }
    }
    let order = part.order();
    let pos = |f: &Id| order.iter().position(|o| o == f);
    let chain = solids.chain(target).unwrap_or(&[]);
    let (Some(pu), Some(me)) = (pos(u), pos(id)) else {
        return Err(refold_refused(
            id,
            "/unfold",
            format!("its unfold {u} isn't listed"),
        ));
    };
    if pu >= me || !chain.contains(u) {
        return Err(refold_refused(
            id,
            "/unfold",
            format!("its unfold {u} isn't in {target}'s chain before it"),
        ));
    }
    for f in chain {
        let Some(at) = pos(f) else { continue };
        if at > pu
            && at < me
            && let Some(Ok(Feature::Refold(other))) = part.features.get(f)
            && other.unfold == *u
        {
            return Err(refold_refused(
                id,
                "/unfold",
                format!("{f} already folds {u} back: unfold again to refold"),
            )
            .with("refold", f));
        }
    }
    let built = crate::rebuild::target(part, solids, id, target, before)?;
    let flat = crate::graph::input(part, id, u, before)?
        .flat()
        .cloned()
        .ok_or_else(|| {
            Error::new(
                Code::Internal,
                format!("refold {id}: its unfold {u} left no flat"),
            )
            .with("feature", id)
        })?;
    let mut bends = Vec::new();
    for (i, r) in x.only.iter().enumerate() {
        let Some(b) = flat.bend_of(r.name) else {
            return Err(refold_refused(
                id,
                &format!("/only/{i}"),
                format!("{} is no face of a bend {u} unfolded", r.text(&part.id)),
            )
            .with(
                "bends",
                flat.bends
                    .iter()
                    .map(|b| format!("{}/{target}/{}", part.id, b.inner[0]))
                    .collect::<Vec<_>>(),
            ));
        };
        if !bends.contains(&b.name) {
            bends.push(b.name);
        }
    }
    Ok((built, flat, (!bends.is_empty()).then_some(bends)))
}

/// A refold: its target as the chain left it folded back by its unfold's
/// flat map.
pub fn refold(
    part: &Part,
    solids: &Solids,
    id: &Id,
    x: &Refold,
    before: &dyn Results,
    run: &Run,
) -> Result<Made> {
    let (built, flat, bends) = refold_resolve(part, solids, id, x, before)?;
    let key = crate::rebuild::op_key(&part.id, id);
    run.job.check()?;
    let made = ryukei::sheet_metal::refold(key, &built.body, &flat.map, &RefoldSpec { bends })
        .map_err(|e| super::refusal(&part.id, id, "refold", (&x.target, built), e))?;
    run.job.check()?;
    let origins = refold_origins(id, built, &flat, &made);
    let b = crate::rebuild::finish(id, "refold", made, origins)?;
    Ok(Made::new(vec![(x.target.clone(), Arc::new(b))]))
}

/// The bend whose folded end plane a new edge of `body` named `n` lies
/// in: its middle in the plane through the strip's axis at angle 0 or
/// the strip's angle, between its radii.
fn bend_line_of<'f>(body: &Body, n: Name, flat: &'f Flat) -> Option<&'f FlatBend> {
    let Some(EntityId::Edge(e)) = body.by_name(n) else {
        return None;
    };
    let edge = body.edge(e)?;
    let curve = body.curve(edge.curve)?;
    let mid = curve.point(0.5 * (edge.interval[0] + edge.interval[1]));
    let t = flat.map.thickness;
    flat.map.strips.iter().find_map(|s| {
        let l = s.axis.to_local(mid);
        let rho = ryukei::math::sqrt(l.x * l.x + l.y * l.y);
        let phi = ryukei::math::atan2(l.y, l.x);
        let tol = 10.0 * ryukei::tol::LINEAR_RESOLUTION;
        let between = rho > s.radius - tol && rho < s.radius + t + tol;
        let at_end = [0.0, s.angle].iter().any(|a| {
            let d = phi - a;
            ryukei::math::sin(d).abs() < 1e-6 && ryukei::math::cos(d) > 0.0
        });
        (between && at_end)
            .then(|| flat.bends.iter().find(|b| b.name == s.bend))
            .flatten()
    })
}

/// A refold's origins: a name its target had keeps its origin; a face it
/// split keeps its parent's; a new edge on a folded bend's end plane is
/// that bend's line; anything else is the refold's.
fn refold_origins(id: &Id, target: &Built, flat: &Flat, made: &(Body, Journal)) -> Origins {
    let mut parent: BTreeMap<Name, Name> = BTreeMap::new();
    for e in made.1.events() {
        if let Event::Split {
            parent: p,
            children,
        } = e
        {
            for c in children {
                parent.insert(*c, *p);
            }
        }
    }
    let inherited = |n: Name| -> Option<serde_json::Value> {
        let mut at = n;
        for _ in 0..64 {
            if let Some(o) = target.origins.get(&at.to_string()) {
                return Some(o.clone());
            }
            at = *parent.get(&at)?;
        }
        None
    };
    crate::rebuild::names(&made.0)
        .into_iter()
        .map(|n| {
            let o = match inherited(n) {
                Some(o) => o,
                None => match bend_line_of(&made.0, n, flat) {
                    Some(b) => json!({"feature": id, "bend_line_of": b.inner[0].to_string()}),
                    None => json!({"feature": id}),
                },
            };
            (n.to_string(), o)
        })
        .collect()
}

/// An unfold's flat as the rebuild report gives it: how many bends it
/// unfolded, whether some were left folded, and its pattern's box in its
/// frame, in metres.
pub fn summary(f: &Flat) -> serde_json::Value {
    let (lo, hi) = (f.pattern.bounds.min(), f.pattern.bounds.max());
    json!({
        "bends": f.bends.len(),
        "partial": f.partial,
        "box_m": [[lo.x, lo.y], [hi.x, hi.y]],
    })
}
