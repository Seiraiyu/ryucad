//! Unfolds and refolds (plan 16.B): one sheet metal solid laid flat from
//! then on in its chain, and folded back by its unfold's flat map. Both
//! act on their target as a flange does, through one kernel call under
//! the feature's key; the request's cancel is checked before and after
//! it (the kernel's sheet metal calls take none).

use super::allowance::{self, Kernel};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sheet_metal::{Allowance, Unfold};
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Flat, FlatBend, Made, Results, Run};
use crate::solid::Solids;
use ryukei::name::Name;
use ryukei::sheet_metal::{Bend, SheetModel, SheetOptions, UnfoldSpec};
use ryukei::topo::EntityId;
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
