//! Fillets and chamfers (ryucad 27.C).

use super::Given;
use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::rebuild::{Built, Made, Results, finish, op_key, read, replaced_origins};
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use ryukei::name::Name;
use ryukei::topo::EntityId;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A fillet or chamfer as output reports it: its file's fields, every
/// size as written and in meters (an angle in radians), every edge and
/// face as `<part>/<feature>/<name>`.
fn blend_report(part: &Id, f: &Feature) -> Value {
    use crate::doc::blend::ChamferForm;
    let mut v = match f {
        Feature::Fillet(x) => x.fields(),
        Feature::Chamfer(x) => x.fields(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    let sizes: Vec<Vec<(&str, &Quantity)>> = match f {
        Feature::Fillet(x) => x
            .groups
            .iter()
            .map(|g| vec![("radius", &g.radius)])
            .collect(),
        Feature::Chamfer(x) => x
            .groups
            .iter()
            .map(|g| match &g.form {
                ChamferForm::Equal { d } => vec![("d", d)],
                ChamferForm::Distances { d1, d2, .. } => vec![("d1", d1), ("d2", d2)],
                ChamferForm::DistanceAngle { d, angle, .. } => vec![("d", d), ("angle", angle)],
            })
            .collect(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    for (i, group) in sizes.into_iter().enumerate() {
        for (k, q) in group {
            v["groups"][i][k] = q.report();
        }
    }
    for (role, r) in f.references() {
        let at = format!("/{}", role.replace('.', "/"));
        *v.pointer_mut(&at).expect("its own reference") = json!(r.text(part));
    }
    v
}

pub(crate) const FILLET: [&str; 4] = ["groups", "propagate", "radius", "edges"];

pub(crate) const CHAMFER: [&str; 9] = [
    "groups",
    "propagate",
    "form",
    "d",
    "d1",
    "d2",
    "angle",
    "face",
    "edges",
];

/// A fillet or chamfer (`ty`) from `current`'s fields (none for a new
/// one) with what was given laid over them: its `groups` whole, or its
/// one group's fields (`radius`, or `form`, `d`, `d1`, `d2`, `angle` and
/// `face`; and `edges`), and `propagate`. Edges and faces are given as
/// text and kept as a file holds them; a new form keeps only what it
/// takes.
pub(crate) fn blend_from(
    ty: &str,
    current: Option<&Feature>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::blend::{Chamfer, Fillet};
    let f = &given.fields;
    let reference = |v: &Value, at: &str| -> Result<Value> {
        match v {
            Value::String(t) => Ok(Reference::parse(t, part)
                .map_err(|e| e.with("pointer", at))?
                .to_json()),
            v => Ok(v.clone()),
        }
    };
    let mut v = match current {
        Some(Feature::Fillet(x)) => x.fields(),
        Some(Feature::Chamfer(x)) => x.fields(),
        _ if ty == "chamfer" => json!({"groups": [{"form": "equal"}]}),
        _ => json!({"groups": [{}]}),
    };
    let sizes = [
        ("radius", &f.radius),
        ("d", &f.d),
        ("d1", &f.d1),
        ("d2", &f.d2),
        ("angle", &f.angle),
    ];
    let one = sizes.iter().any(|(_, x)| x.is_some())
        || f.edges.is_some()
        || f.form.is_some()
        || f.face.is_some();
    if let Some(groups) = &f.groups {
        if one {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s groups, or its one group's fields, not both"),
            ));
        }
        let mut groups = groups.clone();
        if let Value::Array(gs) = &mut groups {
            for (i, g) in gs.iter_mut().enumerate() {
                if let Some(Value::Array(es)) = g.get_mut("edges") {
                    for (j, e) in es.iter_mut().enumerate() {
                        *e = reference(e, &format!("/groups/{i}/edges/{j}"))?;
                    }
                }
                if let Some(face) = g.get_mut("face") {
                    *face = reference(face, &format!("/groups/{i}/face"))?;
                }
            }
        }
        v["groups"] = groups;
    } else if one {
        let gs = v["groups"].as_array_mut().expect("groups");
        if gs.len() != 1 {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "this {ty} has {} groups: give them all, as groups",
                    gs.len()
                ),
            )
            .with("param", "groups"));
        }
        let g = gs[0].as_object_mut().expect("a group");
        if let Some(form) = &f.form {
            let takes: &[&str] = match form.as_str() {
                "equal" => &["d", "edges"],
                "distances" => &["d1", "d2", "face", "edges"],
                "distance_angle" => &["d", "angle", "face", "edges"],
                _ => &["edges"],
            };
            g.retain(|k, _| takes.contains(&k.as_str()));
            g.insert("form".into(), json!(form));
        }
        for (k, x) in sizes {
            if let Some(x) = x {
                g.insert(k.into(), x.clone());
            }
        }
        if let Some(face) = &f.face {
            g.insert("face".into(), reference(&json!(face), "/groups/0/face")?);
        }
        if let Some(edges) = &f.edges {
            let edges: Vec<Value> = edges
                .iter()
                .enumerate()
                .map(|(j, e)| reference(&json!(e), &format!("/groups/0/edges/{j}")))
                .collect::<Result<_>>()?;
            g.insert("edges".into(), json!(edges));
        }
    }
    match f.propagate {
        Some(false) => v["propagate"] = json!(false),
        Some(true) => {
            v.as_object_mut().expect("an object").remove("propagate");
        }
        None => {}
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let out = match ty {
        "fillet" => Feature::Fillet(Fillet::from_fields(&mut o, unit, env)?),
        _ => Feature::Chamfer(Chamfer::from_fields(&mut o, unit, env)?),
    };
    o.finish()?;
    Ok(out)
}

/// Fillets and chamfers.
pub(crate) struct Blends;

/// Whether a fillet or chamfer grows its edges along tangent ones.
fn propagates(f: &Feature) -> bool {
    match f {
        Feature::Fillet(x) => x.propagate,
        Feature::Chamfer(x) => x.propagate,
        _ => false,
    }
}

impl FeatureKind for Blends {
    fn types(&self) -> &'static [&'static str] {
        &["fillet", "chamfer"]
    }

    fn fields(&self, ty: &str) -> &'static [&'static str] {
        if ty == "fillet" { &FILLET } else { &CHAMFER }
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    fn from_given(
        &self,
        ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        blend_from(ty, current, &cx.given, cx.part, cx.unit, cx.env)
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        blend_report(&part.id, f)
    }

    // One that now grows its edges, or stops, may reach an edge twice.
    fn rechecks(&self, old: &Feature, new: &Feature) -> bool {
        propagates(old) != propagates(new)
    }

    // Edges of one solid, a face where a face goes, no edge twice; what
    // the kernel makes of them only rebuild can tell.
    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        crate::rebuild::check_blend(&cx.proposed(f), cx.id, f, cx.results)
    }

    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        super::made(build_blend(cx.part, cx.solids, cx.id, f, cx.before, cx.run))
    }

    fn reads<'f>(&self, f: &'f Feature) -> Vec<crate::kind::Read<'f>> {
        crate::kind::targets(f, "targets", None, None)
    }
}

/// One group of a fillet or chamfer: what it does to each edge, and the
/// edges it lists, each with its role in the feature.
struct Group<'a> {
    kind: ryukei::blend::BlendKind,
    edges: Vec<(String, &'a Reference)>,
}

/// A fillet's or chamfer's groups, each chamfer face resolved in `solid`
/// as `id` sees it.
fn groups<'a>(
    part: &Part,
    id: &Id,
    f: &'a Feature,
    before: &dyn Results,
) -> Result<Vec<Group<'a>>> {
    use crate::doc::blend::ChamferForm;
    use ryukei::blend::{BlendKind, Chamfer};
    let roles = |i: usize, edges: &'a [Reference]| {
        edges
            .iter()
            .enumerate()
            .map(|(j, e)| (format!("groups.{i}.edges.{j}"), e))
            .collect::<Vec<_>>()
    };
    match f {
        Feature::Fillet(fl) => Ok(fl
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| Group {
                kind: BlendKind::Fillet {
                    radius: g.radius.meters(),
                },
                edges: roles(i, &g.edges),
            })
            .collect()),
        Feature::Chamfer(c) => c
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let face = |r: &Reference| -> Result<Name> {
                    let role = format!("groups.{i}.face");
                    let found = crate::reference::resolve(
                        part,
                        part.order(),
                        id,
                        &role,
                        crate::reference::Follow::One,
                        r,
                        before,
                    )?;
                    match found.1 {
                        EntityId::Face(_) => Ok(r.name),
                        e => Err(crate::reference::wrong_kind(
                            part,
                            id,
                            &role,
                            r,
                            crate::reference::kind(e),
                            "a face",
                        )),
                    }
                };
                let chamfer = match &g.form {
                    ChamferForm::Equal { d } => Chamfer::Equal {
                        distance: d.meters(),
                    },
                    ChamferForm::Distances { d1, d2, face: on } => Chamfer::Distances {
                        first: d1.meters(),
                        second: d2.meters(),
                        first_face: face(on)?,
                    },
                    ChamferForm::DistanceAngle { d, angle, face: on } => Chamfer::DistanceAngle {
                        distance: d.meters(),
                        angle: angle.radians(),
                        face: face(on)?,
                    },
                };
                Ok(Group {
                    kind: BlendKind::Chamfer(chamfer),
                    edges: roles(i, &g.edges),
                })
            })
            .collect(),
        _ => unreachable!("a fillet or a chamfer"),
    }
}

/// The edges a fillet or chamfer (feature `id`) blends in `solid`, as
/// its view of it holds them: each listed edge resolved and, when it
/// propagates, grown to its tangent chain, with the role of the listed
/// edge that reached it. An edge in two groups is `edge_listed_twice`;
/// one reached twice in one group is blended once.
fn blend_edges<'g>(
    part: &Part,
    id: &Id,
    ty: &str,
    (solid, built): (&Id, &Built),
    groups: &'g [Group],
    propagate: bool,
    before: &dyn Results,
) -> Result<Vec<(ryukei::blend::EdgeBlend, &'g str)>> {
    use ryukei::topo::EdgeId;
    let body = &built.body;
    // Each edge blended so far: its group and the role that reached it.
    let mut seen: BTreeMap<EdgeId, (usize, &str)> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        for (role, r) in &g.edges {
            // An edge a later feature split is every piece (design 447).
            let (_, found) =
                crate::reference::resolve_all(part, part.order(), id, role, r, before)?;
            for e in found {
                let EntityId::Edge(e) = e else {
                    return Err(crate::reference::wrong_kind(
                        part,
                        id,
                        role,
                        r,
                        crate::reference::kind(e),
                        "an edge",
                    ));
                };
                // The kernel rounds only edges with a face either side; a
                // tangent chain from an inner edge never reaches a laminar one.
                if body.is_laminar(e) {
                    let name = body.edge(e).expect("a live edge").name;
                    return Err(laminar_blend(part, id, ty, solid, name));
                }
                let chain = if propagate {
                    ryukei::blend::tangent_chain(body, e).map_err(|k| {
                        Error::new(Code::Internal, format!("{id}: {k}")).with("feature", id)
                    })?
                } else {
                    vec![e]
                };
                for x in chain {
                    match seen.get(&x) {
                        Some((j, _)) if *j == i => {}
                        Some((j, other)) => {
                            let name = body.edge(x).expect("a live edge").name;
                            return Err(Error::new(
                            Code::EdgeListedTwice,
                            format!(
                                "{id}: edge {}/{solid}/{name} is in group {j} (by its {other}) and group {i} (by its {role})",
                                part.id
                            ),
                        )
                        .with("feature", id)
                        .with("edge", format!("{}/{solid}/{name}", part.id))
                        .with("groups", [*j, i])
                        .with("roles", [*other, role.as_str()]));
                        }
                        None => {
                            seen.insert(x, (i, role.as_str()));
                            let edge = ryukei::blend::EdgeBlend {
                                edge: body.edge(x).expect("a live edge").name,
                                kind: g.kind,
                            };
                            out.push((edge, role.as_str()));
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The one solid a fillet or chamfer (feature `id`) acts on, and whether
/// it propagates: `targets_mixed` when its references name several.
fn blend_solid(id: &Id, f: &Feature) -> Result<(Id, bool)> {
    let ty = f.type_name();
    match (f.targets().as_slice(), f) {
        ([one], Feature::Fillet(x)) => Ok(((*one).clone(), x.propagate)),
        ([one], Feature::Chamfer(x)) => Ok(((*one).clone(), x.propagate)),
        (many, _) => Err(Error::new(
            Code::TargetsMixed,
            format!(
                "{ty} {id}: its edges belong to {}; a {ty} acts on one solid, so split it in two",
                many.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .with("feature", id)
        .with("solids", many)),
    }
}

/// Refuses a fillet or chamfer (feature `id` of `part`, as a write would
/// leave it) whose references don't resolve in `results` to what it
/// needs: edges of one solid, a face where a face goes, no edge in two
/// groups. What the kernel makes of them only rebuild can tell.
pub fn check_blend(part: &Part, id: &Id, f: &Feature, results: &dyn Results) -> Result<()> {
    let solids = Solids::of(part);
    let (solid, propagate) = blend_solid(id, f)?;
    let built = read(part, &solids, id, f, "targets", &solid, results)?;
    let groups = groups(part, id, f, results)?;
    blend_edges(
        part,
        id,
        f.type_name(),
        (&solid, built),
        &groups,
        propagate,
        results,
    )
    .map(drop)
}

/// The kernel's refusal of a fillet or chamfer: `blend_failed`, with its
/// `reason` and each entity it names (`entities`, with its `role` in the
/// refusal): as a reference into the solid as the feature sees it, its
/// kind, its origin, and for an edge the listed edge whose chain reached
/// it (`listed`).
fn blend_error(
    part: &Part,
    id: &Id,
    ty: &str,
    (solid, built): (&Id, &Built),
    listed: &[(ryukei::blend::EdgeBlend, &str)],
    e: ryukei::Error,
) -> Error {
    use ryukei::BlendError as B;
    let entity = |role: &str, n: &Name| -> Value {
        let mut v = json!({"role": role});
        match built.body.by_name(*n) {
            Some(x) => {
                v["reference"] = json!(format!("{}/{solid}/{n}", part.id));
                v["kind"] = json!(crate::reference::kind(x));
                if let Some(o) = built.origins.get(&n.to_string()) {
                    v["origin"] = o.clone();
                }
                if let Some((_, by)) = listed.iter().find(|(b, _)| b.edge == *n) {
                    v["listed"] = json!(by);
                }
            }
            // A name a staged corner made between its stages.
            None => v["name"] = json!(n.to_string()),
        }
        v
    };
    let (reason, msg, entities) = match &e {
        ryukei::Error::InvalidBlend(b) => match b {
            B::NoSuchEdge { edge } => (
                "no_such_edge",
                "an edge isn't there",
                vec![entity("edge", edge)],
            ),
            B::DuplicateEdge { edge } => (
                "duplicate_edge",
                "an edge is named twice",
                vec![entity("edge", edge)],
            ),
            B::BadSize { edge } => (
                "bad_size",
                "a size is too small for the kernel's resolution, or an angle out of range",
                vec![entity("edge", edge)],
            ),
            B::NotAFace { edge, face } => (
                "not_a_face",
                "the face a chamfer's first distance runs along doesn't bound one of its edges",
                vec![entity("edge", edge), entity("face", face)],
            ),
            B::SmoothEdge { edge } => (
                "smooth_edge",
                "an edge's faces meet tangentially: there is nothing to round",
                vec![entity("edge", edge)],
            ),
            B::ConvexityChanges { edge } => (
                "convexity_changes",
                "an edge is convex in one place and concave in another",
                vec![entity("edge", edge)],
            ),
            B::OpenTangentChain { blended, open } => (
                "open_tangent_chain",
                "a blended edge runs on tangentially into one that isn't: blend the whole chain (propagate)",
                vec![entity("blended", blended), entity("open", open)],
            ),
            B::SizeChangeAlongChain { a, b } => (
                "size_change_along_chain",
                "edges that meet tangentially are blended with different sizes",
                vec![entity("a", a), entity("b", b)],
            ),
            B::Overflow { edge, face } => (
                "overflow",
                "the blend would run off a face it lies on, or cross another there",
                vec![entity("edge", edge), entity("face", face)],
            ),
            B::RadiusTooLarge { edge } => (
                "radius_too_large",
                "the size is too large for the faces or the edge",
                vec![entity("edge", edge)],
            ),
            B::Unsupported { at } => (
                "unsupported",
                "a face, edge or vertex the kernel can't blend yet",
                vec![entity("at", at)],
            ),
            B::Uncertain { at } => (
                "uncertain",
                "a decision rounding left uncertain",
                vec![entity("at", at)],
            ),
            B::NonManifold { at } => (
                "non_manifold",
                "blends would meet only at an edge or a point",
                vec![entity("at", at)],
            ),
        },
        _ => ("kernel", "the kernel refused it", Vec::new()),
    };
    let mut err = Error::new(Code::BlendFailed, format!("{ty} {id}: {msg} ({e})"))
        .with("feature", id)
        .with("solid", solid)
        .with("reason", reason)
        .with("kernel", format!("{e:?}"));
    if !entities.is_empty() {
        err = err.with("entities", entities);
    }
    if let ryukei::Error::InvalidBlend(B::Unsupported { at }) = &e
        && let Some(EntityId::Edge(x)) = built.body.by_name(*at)
        && built.body.is_laminar(x)
    {
        err = err.with(
            "hint",
            laminar_hint(&format!("{}/{solid}/{at}", part.id), solid),
        );
    }
    err
}

/// What to say of boundary (laminar) edge `reference` of sheet `solid`
/// that a fillet or chamfer names.
fn laminar_hint(reference: &str, solid: &Id) -> String {
    format!(
        "{reference} is a boundary of sheet {solid} (laminar): only a sheet's inner edges are rounded"
    )
}

/// `blend_failed` for a fillet or chamfer of a sheet's boundary: the
/// kernel rounds only edges with a face either side.
fn laminar_blend(part: &Part, id: &Id, ty: &str, solid: &Id, edge: Name) -> Error {
    let reference = format!("{}/{solid}/{edge}", part.id);
    let hint = laminar_hint(&reference, solid);
    Error::new(Code::BlendFailed, format!("{ty} {id}: {hint}"))
        .with("feature", id)
        .with("solid", solid)
        .with("reason", "unsupported")
        .with(
            "entities",
            json!([{"role": "edge", "reference": reference, "kind": "edge"}]),
        )
        .with("hint", hint)
}

/// A fillet or chamfer: its solid as it sees it, with every edge its
/// groups name, grown along tangent chains, blended in one kernel call.
pub(crate) fn build_blend(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    before: &dyn Results,
    run: &crate::rebuild::Run,
) -> Result<Made> {
    let ty = f.type_name();
    let (solid, propagate) = blend_solid(id, f)?;
    let built = read(part, solids, id, f, "targets", &solid, before)?;
    let groups = groups(part, id, f, before)?;
    let listed = blend_edges(part, id, ty, (&solid, built), &groups, propagate, before)?;
    let edges: Vec<ryukei::blend::EdgeBlend> = listed.iter().map(|(e, _)| *e).collect();
    let work = crate::work::work(run.job);
    let made = ryukei::blend::blend_with(op_key(&part.id, id), &built.body, &edges, &work)
        .map_err(|e| {
            crate::work::stopped(&e, run.job)
                .unwrap_or_else(|| blend_error(part, id, ty, (&solid, built), &listed, e))
        })?;
    // A blend face says which edges (and corner vertices) it took the
    // place of; what else is new is the feature's; the rest keeps its
    // origin.
    let origins = replaced_origins(id, built, &made);
    let built = finish(id, ty, made, origins)?;
    Ok(Made::new(vec![(solid, Arc::new(built))]))
}
