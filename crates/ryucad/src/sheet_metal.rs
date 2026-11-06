//! Sheet metal (design 346, plan 16.A): base flanges, which found a sheet
//! metal solid from a sketch, a plate or a bent section ([`section`]);
//! edge flanges on straight edges of one; the kernel's refusals in the
//! agent's terms; and the sheet metal report ([`report`]).

pub mod section;

use crate::doc::id::Id;
use crate::error::{Code, Error};
use crate::rebuild::Built;
use ryukei::name::Name;
use ryukei::{FlangeSize, SheetMetalError as S};
use serde_json::{Value, json};

/// A sheet metal refusal's reason (the kernel's variant in snake case),
/// what it says, and each entity it names with its role in the refusal.
pub fn reason(e: &S) -> (&'static str, &'static str, Vec<(&'static str, Name)>) {
    match e {
        S::BadTolerance => ("bad_tolerance", "a tolerance that isn't finite", vec![]),
        S::NotSheet { face } => ("not_sheet", "the solid isn't sheet metal: a face makes no flange or bend", vec![("face", *face)]),
        S::MixedThickness { faces: [a, b] } => ("mixed_thickness", "two faces disagree about the sheet's thickness", vec![("a", *a), ("b", *b)]),
        S::OutOfSlab { face } => ("out_of_slab", "a face stands out of the sheet between a flange's two sides", vec![("face", *face)]),
        S::NotRadial { face } => ("not_radial", "a face meeting a bend doesn't run across it along radii", vec![("face", *face)]),
        S::SharpBend { edge } => ("sharp_bend", "two flanges meet at a corner with no radius", vec![("edge", *edge)]),
        S::ConicalBend { face } => ("conical_bend", "a bend between cones: only cylindrical bends are sheet metal", vec![("face", *face)]),
        S::BendBlocked { bend, face } => ("bend_blocked", "material beside a bend with no relief", vec![("bend", *bend), ("face", *face)]),
        S::Disconnected => ("disconnected", "the flanges and bends don't all join up", vec![]),
        S::BendLoop { bends, faces } => ("bend_loop", "the bends close a loop", bends.iter().map(|b| ("bend", *b)).chain(faces.iter().map(|f| ("face", *f))).collect()),
        S::NoSuchBend { name } => ("no_such_bend", "no bend of the solid has this face", vec![("bend", *name)]),
        S::NoSuchFace { name } => ("no_such_face", "no flange side of the solid has this face", vec![("face", *name)]),
        S::BadAllowance { bend } => ("bad_allowance", "an allowance out of range for the bend", vec![("bend", *bend)]),
        S::FlatOverlaps { faces: [a, b] } => ("flat_overlaps", "the flat would overlap itself", vec![("a", *a), ("b", *b)]),
        S::FoldCollides { faces: [a, b] } => ("fold_collides", "flanges would fold into each other", vec![("a", *a), ("b", *b)]),
        S::Uncertain { at } => ("uncertain", "a decision rounding left uncertain", vec![("at", *at)]),
        S::NoSuchEdge { name } => ("no_such_edge", "no edge of the solid has this name", vec![("edge", *name)]),
        S::NotAFlangeEdge { edge } => ("not_a_flange_edge", "no flange can be made on this edge", vec![("edge", *edge)]),
        S::DuplicateFlange { edge } => ("duplicate_flange", "two flanges on one edge", vec![("edge", *edge)]),
        S::FlangesMeet { vertex } => ("flanges_meet", "two flanges' bends, flanges, cuts or reliefs would overlap", vec![("vertex", *vertex)]),
        S::BadSize { edge, .. } => ("bad_size", "a size out of range for the flange", vec![("edge", *edge)]),
        S::ReliefNeeded { edge, vertex } => ("relief_needed", "a flange set back from an end cuts into the base there, with no relief", vec![("edge", *edge), ("vertex", *vertex)]),
        S::NotSheetAfter { .. } => ("not_sheet_after", "the flanged solid isn't sheet metal", vec![]),
    }
}

fn size_name(w: FlangeSize) -> &'static str {
    match w {
        FlangeSize::Angle => "angle",
        FlangeSize::Length => "length",
        FlangeSize::Radius => "radius",
        FlangeSize::Setback => "setback",
        FlangeSize::Relief => "relief",
    }
}

/// The kernel's refusal of a sheet metal feature (`ty`, feature `id` of
/// part `part`) on `solid`, as the feature read it (`built`):
/// `sheet_metal_failed` with its `reason`, what it says, the kernel's own
/// text (`kernel`), each entity it names (`entities`, each with its `role`:
/// a reference into the solid with its kind and origin, else its name)
/// and a `hint` where an agent can fix it. A boolean's refusal is
/// `boolean_failed`, as a cut's.
pub fn refusal(part: &Id, id: &Id, ty: &str, (solid, built): (&Id, &Built), e: ryukei::Error) -> Error {
    let entity = |role: &str, n: &Name| -> Value {
        let mut v = json!({"role": role});
        match built.body.by_name(*n) {
            Some(x) => {
                v["reference"] = json!(format!("{part}/{solid}/{n}"));
                v["kind"] = json!(crate::reference::kind(x));
                if let Some(o) = built.origins.get(&n.to_string()) {
                    v["origin"] = o.clone();
                }
            }
            None => v["name"] = json!(n.to_string()),
        }
        v
    };
    let x = match &e {
        ryukei::Error::InvalidSheetMetal(x) => x,
        ryukei::Error::InvalidBoolean(b) => return boolean(part, id, ty, solid, &entity, b, &e),
        _ => {
            return Error::new(Code::SheetMetalFailed, format!("{ty} {id}: the kernel refused it ({e})"))
                .with("feature", id)
                .with("solid", solid)
                .with("reason", "kernel")
                .with("kernel", format!("{e:?}"));
        }
    };
    let (why, msg, named) = reason(x);
    let mut entities: Vec<Value> = named.iter().map(|(r, n)| entity(r, n)).collect();
    let mut err = Error::new(Code::SheetMetalFailed, format!("{ty} {id}: {msg} ({e})"))
        .with("feature", id)
        .with("solid", solid)
        .with("reason", why)
        .with("kernel", format!("{e:?}"));
    match x {
        S::BadSize { what, .. } => err = err.with("what", size_name(*what)),
        S::NotSheetAfter { fault } => {
            let (inner, _, named) = reason(fault);
            err = err.with("fault", inner);
            // The flanged body never came back: its entities by name.
            entities.extend(named.iter().map(|(r, n)| json!({"role": format!("fault.{r}"), "name": n.to_string()})));
        }
        _ => {}
    }
    let at = |n: &Name| format!("{part}/{solid}/{n}");
    let hint = match x {
        S::BendLoop { .. } => Some("the bends close a loop, which can't unfold: cut a rip (a thin cut through one corner's bend) and unfold again".to_owned()),
        S::ReliefNeeded { vertex, .. } => Some(format!(
            "give that end a relief: --end-relief {}=rectangular,1mm,0.5mm (or obround, tear), or the flange a --relief",
            at(vertex)
        )),
        S::FlangesMeet { vertex } => Some(format!(
            "set back the flanges' ends at that vertex (--end-setback {}=2mm, past their bends' radius and thickness), or flange one of the edges alone",
            at(vertex)
        )),
        S::NotAFlangeEdge { .. } => Some("a flange's edge is a straight edge between a side of the sheet and a planar wall square to it: name an edge of the rim on the side the flange folds towards".to_owned()),
        S::NotSheet { .. } | S::OutOfSlab { .. } | S::NotRadial { .. } | S::SharpBend { .. }
        | S::ConicalBend { .. } | S::MixedThickness { .. } | S::Disconnected => {
            Some("the solid isn't sheet metal: the face named breaks it. Found sheet metal with a base_flange, or remove what stands out of the sheet".to_owned())
        }
        _ => None,
    };
    if let Some(h) = hint {
        err = err.with("hint", h);
    }
    if !entities.is_empty() {
        err = err.with("entities", entities);
    }
    err
}

/// A flange's boolean refused, as `boolean_failed`.
fn boolean(
    part: &Id,
    id: &Id,
    ty: &str,
    solid: &Id,
    entity: &dyn Fn(&str, &Name) -> Value,
    b: &ryukei::BooleanError,
    e: &ryukei::Error,
) -> Error {
    use ryukei::BooleanError as B;
    let _ = part;
    let (msg, faces): (&str, Vec<Value>) = match b {
        B::Unsupported { a, b } => ("the flange's faces meet on surfaces the kernel can't intersect yet", vec![entity("a", a), entity("b", b)]),
        B::Uncertain { a, b } => ("where the flange's faces meet the solid stays uncertain", vec![entity("a", a), entity("b", b)]),
        B::NonManifold { faces, .. } => ("the flange would touch the solid along an edge or at a point", faces.iter().map(|f| entity("face", f)).collect()),
        _ => ("the flange's boolean refused it", Vec::new()),
    };
    let mut err = Error::new(Code::BooleanFailed, format!("{ty} {id}: {msg}"))
        .with("feature", id)
        .with("target", solid)
        .with("kernel", format!("{e:?}"));
    if !faces.is_empty() {
        err = err.with("faces", faces);
    }
    err
}

use crate::doc::project::Part;
use crate::doc::sheet_metal::BaseFlange;
use crate::rebuild::{Results, Run};
use ryukei::sheet_metal::SheetOptions;

/// A base flange's solid: its sketch's closed loops extruded by its
/// thickness (a plate, named as the same extrude's faces are), or its
/// one open chain offset into a bent section and extruded by its depth;
/// then recognized as sheet metal of its thickness.
pub fn base_flange(part: &Part, id: &Id, b: &BaseFlange, before: &dyn Results, run: &Run) -> crate::error::Result<Built> {
    run.job.check()?;
    let solved = crate::rebuild::sketch_input(part, id, &b.sketch, before)?;
    let key = crate::rebuild::op_key(&part.id, id);
    let t = b.thickness.meters();
    let kernel = |e: ryukei::Error| {
        Error::new(Code::RebuildFailed, format!("base_flange {id}: {e}"))
            .with("feature", id)
            .with("kernel", format!("{e:?}"))
    };
    let (made, origins) = match &b.section {
        None => {
            let (valid, keys) = crate::profile::profile(&part.id, &b.sketch, solved, &b.loops)
                .map_err(|e| e.with("feature", id).with("sketch", &b.sketch))?;
            let n = valid.frame().z().get();
            let dir = if b.reverse { -n } else { n };
            let made = ryukei::sweep::extrude(key, &valid, dir, 0.0, t).map_err(kernel)?;
            let mut origins = crate::profile::origins(key, &b.sketch, &valid, &keys);
            for o in origins.values_mut() {
                o["feature"] = json!(id);
            }
            (made, origins)
        }
        Some(sec) => {
            let w = section::walk(&b.sketch, solved).map_err(|e| e.with("feature", id).with("sketch", &b.sketch))?;
            let s = section::profile(&part.id, &b.sketch, solved, &w, t, b.radius.meters(), !b.reverse)
                .map_err(|e| e.with("feature", id).with("sketch", &b.sketch))?;
            let n = s.valid.frame().z().get();
            let d = sec.depth.meters();
            let (lo, hi) = if sec.symmetric { (-d / 2.0, d / 2.0) } else { (0.0, d) };
            let made = ryukei::sweep::extrude(key, &s.valid, n, lo, hi).map_err(kernel)?;
            (made, section::origins(key, id, &b.sketch, &s))
        }
    };
    let built = crate::rebuild::finish(id, "base_flange", made, origins)?;
    let model = ryukei::sheet_metal::recognize(&built.body, &SheetOptions::default())
        .map_err(|e| refusal(&part.id, id, "base_flange", (id, &built), e))?;
    if (model.thickness - t).abs() > model.tolerance {
        return Err(Error::new(
            Code::SheetMetalFailed,
            format!("base_flange {id}: it reads as sheet {} m thick, not its thickness {t} m: the sketch is narrower somewhere than the sheet is thick", model.thickness),
        )
        .with("feature", id)
        .with("solid", id)
        .with("reason", "thickness_mismatch")
        .with("thickness_m", t)
        .with("read_m", model.thickness));
    }
    run.job.check()?;
    Ok(built)
}

use crate::doc::files::Feature;
use crate::doc::reference::Reference;
use crate::doc::sheet_metal::{Flange, Measured, Position, Relief};
use crate::solid::Solids;
use ryukei::sheet_metal::{EdgeFlange, FlangePosition, Measure};
use ryukei::topo::EntityId;

/// What an edge flange acts on as its view of it holds it, and what it
/// asks the kernel for: an [`EdgeFlange`] per edge in the order it lists
/// them, every size in meters and radians, each end's setback and relief
/// at the end of the edge its vertex is.
pub(crate) struct Resolved<'a> {
    pub target: &'a Id,
    pub built: &'a Built,
    pub flanges: Vec<EdgeFlange>,
}

/// The kernel's relief for a file's.
pub(crate) fn kernel_relief(r: &Relief) -> ryukei::sheet_metal::Relief {
    use ryukei::sheet_metal::Relief as K;
    match r {
        Relief::None => K::None,
        Relief::Rectangular { width, depth } => K::Rectangular { width: width.meters(), depth: depth.meters() },
        Relief::Obround { width, depth } => K::Obround { width: width.meters(), depth: depth.meters() },
        Relief::Tear { width } => K::Tear { width: width.meters() },
    }
}

/// The base flange that founded `target`, if one did: its settings are
/// what the solid's features inherit.
pub fn founder<'a>(part: &'a Part, target: &Id) -> Option<&'a crate::doc::sheet_metal::BaseFlange> {
    part.features.get(target)?.as_ref().ok()?.base_flange()
}

/// Resolves an edge flange (feature `id`) against `before`: every
/// reference a face, edge or vertex of its target as it sees it
/// (`targets_mixed` for another solid's, `reference_wrong_kind` for the
/// wrong kind), every end vertex an end of an edge it lists
/// (`invalid_param`), and a radius, its own or its founder's
/// (`invalid_param` at `/radius` when its target has no base flange).
pub(crate) fn resolve<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &'a Flange,
    before: &'a dyn Results,
) -> crate::error::Result<Resolved<'a>> {
    let target = &f.target;
    for (role, r) in f.references() {
        if r.feature != *target {
            return Err(Error::new(
                Code::TargetsMixed,
                format!(
                    "flange {id}: its {role} {} is of {}, not of its target {target}; a flange acts on one solid",
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
    let kind = |role: &str, r: &Reference, want: &str| -> crate::error::Result<EntityId> {
        let (_, e) = crate::reference::resolve(part, part.order(), id, role, r, before)?;
        let ok = matches!(
            (want, e),
            ("an edge", EntityId::Edge(_)) | ("a face", EntityId::Face(_)) | ("a vertex", EntityId::Vertex(_))
        );
        if ok {
            Ok(e)
        } else {
            Err(crate::reference::wrong_kind(part, id, role, r, crate::reference::kind(e), want))
        }
    };
    let founder = founder(part, target);
    let radius = match (&f.radius, founder) {
        (Some(q), _) => q.meters(),
        (None, Some(b)) => b.radius.meters(),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("flange {id}: its target {target} wasn't founded by a base flange, so it has no inner bend radius to take: give the flange its radius"),
            )
            .with("feature", id)
            .with("pointer", "/radius"));
        }
    };
    let relief = f
        .relief
        .as_ref()
        .or(founder.map(|b| &b.relief))
        .map_or(ryukei::sheet_metal::Relief::None, kernel_relief);
    let setback = f.setback.as_ref().map_or(0.0, |q| q.meters());
    // Each end's vertex by name.
    let mut ends = Vec::with_capacity(f.ends.len());
    for (i, e) in f.ends.iter().enumerate() {
        kind(&format!("ends.{i}.vertex"), &e.vertex, "a vertex")?;
        ends.push((i, e.vertex.name, e));
    }
    let mut used = vec![false; ends.len()];
    let mut flanges = Vec::with_capacity(f.edges.len());
    for (i, x) in f.edges.iter().enumerate() {
        let role = if x.side.is_some() { format!("edges.{i}.edge") } else { format!("edges.{i}") };
        let EntityId::Edge(eid) = kind(&role, &x.edge, "an edge")? else {
            unreachable!("asked for an edge")
        };
        let side = match &x.side {
            Some(s) => {
                kind(&format!("edges.{i}.side"), s, "a face")?;
                Some(s.name)
            }
            None => None,
        };
        let edge = built.body.edge(eid).expect("a live edge");
        let at = |v| built.body.vertex(v).expect("a live vertex").name;
        let mut setbacks = [setback; 2];
        let mut reliefs = [relief; 2];
        for (k, v) in [at(edge.start), at(edge.end)].into_iter().enumerate() {
            for (j, (_, name, e)) in ends.iter().enumerate() {
                if *name == v {
                    used[j] = true;
                    if let Some(q) = &e.setback {
                        setbacks[k] = q.meters();
                    }
                    if let Some(r) = &e.relief {
                        reliefs[k] = kernel_relief(r);
                    }
                }
            }
        }
        flanges.push(EdgeFlange {
            edge: x.edge.name,
            side,
            angle: f.angle.radians(),
            length: f.length.meters(),
            measured: match f.measured {
                Measured::OuterMould => Measure::OuterMould,
                Measured::InnerMould => Measure::InnerMould,
                Measured::Tangent => Measure::Tangent,
            },
            radius,
            position: match f.position {
                Position::MaterialInside => FlangePosition::MaterialInside,
                Position::MaterialOutside => FlangePosition::MaterialOutside,
                Position::BendOutside => FlangePosition::BendOutside,
            },
            setbacks,
            reliefs,
        });
    }
    if let Some(j) = used.iter().position(|u| !u) {
        let (i, _, e) = &ends[j];
        return Err(Error::new(
            Code::InvalidParam,
            format!("flange {id}: its end {} isn't an end of any edge it flanges", e.vertex.text(&part.id)),
        )
        .with("feature", id)
        .with("pointer", format!("/ends/{i}/vertex")));
    }
    Ok(Resolved { target, built, flanges })
}

use crate::rebuild::{Made, Origins};
use ryukei::name::{Event, OpKey, Role, SheetMetalRole, ToolKey};
use std::collections::BTreeMap;
use std::sync::Arc;

/// An edge flange: its target as it sees it, through one kernel call
/// under the feature's key. The request's cancel is checked before and
/// after it: the kernel's sheet metal calls take none.
pub fn flange(part: &Part, solids: &Solids, id: &Id, f: &Flange, before: &dyn Results, run: &Run) -> crate::error::Result<Made> {
    let r = resolve(part, solids, id, f, before)?;
    let key = crate::rebuild::op_key(&part.id, id);
    run.job.check()?;
    let made = ryukei::sheet_metal::flange(key, &r.built.body, &r.flanges, &SheetOptions::default())
        .map_err(|e| refusal(&part.id, id, "flange", (r.target, r.built), e))?;
    run.job.check()?;
    let origins = flange_origins(id, key, r.built, &r.flanges, &made);
    let built = crate::rebuild::finish(id, "flange", made, origins)?;
    Ok(Made::new(vec![(r.target.clone(), Arc::new(built))]))
}

/// A flange's origins: what its booleans `Placed` from one edge's bend,
/// flange or cut is `{feature, flange_of: <edge>, part: bend | flange |
/// cut}`, carried through splits and merges; a name the target had keeps
/// its origin; anything else is the feature's.
fn flange_origins(id: &Id, key: OpKey, target: &Built, flanges: &[EdgeFlange], made: &(ryukei::topo::Body, ryukei::name::Journal)) -> Origins {
    let mut tools: BTreeMap<ToolKey, (Name, &'static str)> = BTreeMap::new();
    for x in flanges {
        for (role, word) in [(SheetMetalRole::Bend, "bend"), (SheetMetalRole::Flange, "flange"), (SheetMetalRole::Cut, "cut")] {
            let k = Name::derive(key, Role::SheetMetal(role), &[x.edge]).to_u128();
            tools.insert(ToolKey(k), (x.edge, word));
        }
    }
    let mut from: BTreeMap<Name, (Name, &'static str)> = BTreeMap::new();
    for e in made.1.events() {
        match e {
            Event::Placed { tool, to, .. } => {
                if let Some(t) = tools.get(tool) {
                    from.insert(*to, *t);
                }
            }
            Event::Split { parent, children } => {
                if let Some(t) = from.get(parent).copied() {
                    for c in children {
                        from.insert(*c, t);
                    }
                }
            }
            Event::Merged { parents, child } => {
                if let Some(t) = parents.iter().find_map(|p| from.get(p).copied()) {
                    from.insert(*child, t);
                }
            }
            _ => {}
        }
    }
    crate::rebuild::names(&made.0)
        .into_iter()
        .map(|n| {
            let k = n.to_string();
            let o = match (from.get(&n), target.origins.get(&k)) {
                (Some((edge, part)), _) => json!({"feature": id, "flange_of": edge.to_string(), "part": part}),
                (None, Some(o)) => o.clone(),
                (None, None) => json!({"feature": id}),
            };
            (k, o)
        })
        .collect()
}

/// What a flange reads of its founder that shapes no body of the
/// founder's own: its radius and relief as written. Its input hash adds
/// them. `None` for anything else.
pub fn founder_settings(part: &Part, f: &Feature) -> Option<Value> {
    let b = founder(part, &f.flange()?.target)?;
    Some(json!({"radius": b.radius.text(), "relief": b.relief.to_json()}))
}

#[cfg(test)]
mod tests;
