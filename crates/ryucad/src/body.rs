//! Shells and offsets (design 327, plan 15.A): one solid hollowed to a
//! wall, or grown, shrunk, or its faces pushed and pulled, by one kernel
//! call under the feature's key and the request's cancel. Both act on
//! their target: they join its chain, and what they leave is the target.
//! Mirrors (plan 15.B): a solid's copy reflected in a plane, built as an
//! extrude's body is, a solid of its own or applied to others.

use crate::doc::body::{Direction, Mirror, Offset, Shell};
use crate::doc::files::{Feature, Obj};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Made, Origins, Results, Run};
use crate::solid::Solids;
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::name::{Event, Name};
use ryukei::name::{OpKey, ToolKey};
use ryukei::offset::{OffsetSpec, ShellSpec};
use ryukei::topo::EntityId;
use ryukei::topo::{Body, RegionKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a shell or offset acts on as its view of it holds it, and the
/// faces it names, by name: the open ones, and each grouped one with its
/// thickness or distance in meters.
struct Resolved<'a> {
    target: &'a Id,
    built: &'a Built,
    open: Vec<Name>,
    faces: Vec<(Name, f64)>,
}

/// Resolves a shell's or offset's (feature `id`'s) target and faces:
/// `targets_mixed` for a face of another solid, `reference_wrong_kind`
/// for an edge or vertex, and whatever its references or its target
/// fail with.
fn resolve<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &'a Feature,
    before: &'a dyn Results,
) -> Result<Resolved<'a>> {
    let (target, groups, open) = match f {
        Feature::Shell(s) => (&s.target, s.faces.as_slice(), s.open.as_slice()),
        Feature::Offset(o) => (&o.target, o.faces.as_slice(), &[][..]),
        _ => unreachable!("a shell or an offset"),
    };
    let ty = f.type_name();
    for (role, r) in f.references() {
        if r.feature != *target {
            return Err(Error::new(
                Code::TargetsMixed,
                format!(
                    "{ty} {id}: its {role} {} is a face of {}, not of its target {target}; a {ty} acts on one solid",
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
    let face = |role: &str, r: &Reference| -> Result<Name> {
        let (_, e) = crate::reference::resolve(part, part.order(), id, role, r, before)?;
        match e {
            EntityId::Face(_) => Ok(r.name),
            e => Err(crate::reference::wrong_kind(
                part,
                id,
                role,
                r,
                crate::reference::kind(e),
                "a face",
            )),
        }
    };
    let open = open
        .iter()
        .enumerate()
        .map(|(i, r)| face(&format!("open.{i}"), r))
        .collect::<Result<Vec<_>>>()?;
    let mut faces = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        for (j, r) in g.faces.iter().enumerate() {
            faces.push((
                face(&format!("faces.{i}.faces.{j}"), r)?,
                g.quantity.meters(),
            ));
        }
    }
    Ok(Resolved {
        target,
        built,
        open,
        faces,
    })
}

/// Refuses a shell or offset (feature `id` of `part`, as a write would
/// leave it) whose target isn't a solid before it, or whose faces don't
/// resolve in `results` to faces of that solid. What the kernel makes of
/// them only rebuild can tell.
///
/// A mirror's solid must be a solid before it, not consumed, and its
/// plane a plane; a split's plane a plane (its target is
/// `check_solids`', through `targets()`).
pub fn check(part: &Part, id: &Id, f: &Feature, results: &dyn Results) -> Result<()> {
    let solids = Solids::of(part);
    match f {
        Feature::Mirror(m) => {
            if let Some(by) = solids.consumed_by(&m.solid, id) {
                return Err(crate::rebuild::consumed(id, &m.solid, by));
            }
            if !solids.is_solid(&m.solid) {
                return Err(crate::rebuild::not_a_solid(
                    part, &solids, id, "solid", &m.solid,
                ));
            }
            crate::pattern::mirror_plane(part, id, &m.plane, results).map(drop)
        }
        Feature::Split(s) => {
            let at = crate::geometry::At {
                part,
                order: part.order(),
                owner: id,
                results,
            };
            crate::geometry::one(&at, "plane", &s.plane, crate::geometry::Want::Plane).map(drop)
        }
        _ => resolve(part, &solids, id, f, results).map(drop),
    }
}

/// A shell or offset: its target as it sees it, through one kernel call.
pub fn build(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    before: &dyn Results,
    run: &Run,
) -> Result<Made> {
    let ty = f.type_name();
    let r = resolve(part, solids, id, f, before)?;
    let work = crate::work::work(run.job);
    let key = crate::rebuild::op_key(&part.id, id);
    let made = match f {
        Feature::Shell(s) => {
            let spec = ShellSpec {
                thickness: s.thickness.meters(),
                outward: s.direction == Direction::Outward,
                open: r.open.clone(),
                faces: r.faces.clone(),
            };
            ryukei::offset::shell_with(key, &r.built.body, &spec, &work)
        }
        Feature::Offset(o) => {
            let spec = OffsetSpec {
                distance: o.distance.meters(),
                faces: r.faces.clone(),
            };
            ryukei::offset::offset_body_with(key, &r.built.body, &spec, &work)
        }
        _ => unreachable!("a shell or an offset"),
    }
    .map_err(|e| {
        crate::work::stopped(&e, run.job)
            .unwrap_or_else(|| kernel_error(part, id, ty, (r.target, r.built), e))
    })?;
    let origins = match f {
        Feature::Shell(_) => shell_origins(id, r.built, &made),
        _ => crate::rebuild::replaced_origins(id, r.built, &made),
    };
    let built = crate::rebuild::finish(id, ty, made, origins)?;
    Ok(Made::new(vec![(r.target.clone(), Arc::new(built))]))
}

/// A shell's origins: a face its boolean `Placed` as the tool (the inner
/// faces inwards; outwards, the solid's old surfaces) is
/// `{feature, wall_of}`, the face of the target it is the wall of, by the
/// name the offset kept; a face that keeps its name keeps its origin;
/// anything else (an offset's fill, an edge, a vertex) is the feature's.
/// Splits and merges in the boolean carry a wall on.
fn shell_origins(
    id: &Id,
    target: &Built,
    made: &(ryukei::topo::Body, ryukei::name::Journal),
) -> Origins {
    let mut wall: BTreeMap<Name, Name> = BTreeMap::new();
    for e in made.1.events() {
        match e {
            Event::Placed { from, to, .. } => {
                wall.insert(*to, *from);
            }
            Event::Split { parent, children } => {
                if let Some(w) = wall.get(parent).copied() {
                    for c in children {
                        wall.insert(*c, w);
                    }
                }
            }
            Event::Merged { parents, child } => {
                if let Some(w) = parents.iter().find_map(|p| wall.get(p).copied()) {
                    wall.insert(*child, w);
                }
            }
            _ => {}
        }
    }
    let is_face = |b: &ryukei::topo::Body, n: Name| matches!(b.by_name(n), Some(EntityId::Face(_)));
    crate::rebuild::names(&made.0)
        .into_iter()
        .map(|n| {
            let key = n.to_string();
            let o = match (wall.get(&n), target.origins.get(&key)) {
                (Some(w), _) if is_face(&made.0, n) && is_face(&target.body, *w) => {
                    json!({"feature": id, "wall_of": w.to_string()})
                }
                (Some(_), _) | (None, None) => json!({"feature": id}),
                (None, Some(o)) => o.clone(),
            };
            (key, o)
        })
        .collect()
}

/// The kernel's refusal of a shell or offset (`ty`): `shell_failed` or
/// `offset_failed`, with its `reason` (the refusal's variant in snake
/// case; an unsupported one's `case`), which call refused (`kernel`:
/// `offset`, or `boolean` for a shell's boolean), the kernel's own text
/// (`refusal`), and each entity it names (`entities`, each with its `role`
/// in the refusal): a reference into the target as the feature sees it,
/// with its kind and origin, else its name alone (an entity of a shell's
/// inner offset).
fn kernel_error(
    part: &Part,
    id: &Id,
    ty: &str,
    (target, built): (&Id, &Built),
    e: ryukei::Error,
) -> Error {
    use ryukei::{BooleanError as B, OffsetCase as C, OffsetError as O};
    let entity = |role: &str, n: &Name| -> Value {
        let mut v = json!({"role": role});
        match built.body.by_name(*n) {
            Some(x) => {
                v["reference"] = json!(format!("{}/{target}/{n}", part.id));
                v["kind"] = json!(crate::reference::kind(x));
                if let Some(o) = built.origins.get(&n.to_string()) {
                    v["origin"] = o.clone();
                }
            }
            None => v["name"] = json!(n.to_string()),
        }
        v
    };
    let case_name = |c: &C| match c {
        C::PartlySmooth => "partly_smooth",
        C::MixedVertex => "mixed_vertex",
        C::ClosingVertex => "closing_vertex",
        C::SharedSection => "shared_section",
        C::Fill => "fill",
        C::LoneVertex => "lone_vertex",
        C::LaminarVertex => "laminar_vertex",
    };
    let none = Vec::new;
    let (kernel, reason, case, msg, entities): (&str, &str, Option<&str>, &str, Vec<Value>) =
        match &e {
            ryukei::Error::InvalidOffset(o) => match o {
                O::BadDistance => (
                    "offset",
                    "bad_distance",
                    None,
                    "a thickness or distance is too small for the kernel's resolution",
                    none(),
                ),
                O::NoSuchFace { face } => (
                    "offset",
                    "no_such_face",
                    None,
                    "a face isn't the solid's, or is named twice",
                    vec![entity("face", face)],
                ),
                O::FaceCollapses { face } => (
                    "offset",
                    "face_collapses",
                    None,
                    "a face's offset would fold or collapse: it bends too tightly for the distance",
                    vec![entity("face", face)],
                ),
                O::EdgeVanishes { edge } => (
                    "offset",
                    "edge_vanishes",
                    None,
                    "where an edge's faces close up, their offsets don't meet: the edge would vanish",
                    vec![entity("edge", edge)],
                ),
                O::SelfIntersecting { faces: [a, b] } => (
                    "offset",
                    "self_intersecting",
                    None,
                    "two faces of the result would cross",
                    vec![entity("a", a), entity("b", b)],
                ),
                O::Unsupported { at, case } => (
                    "offset",
                    "unsupported",
                    Some(case_name(case)),
                    "the kernel can't offset this face, edge or vertex yet",
                    vec![entity("at", at)],
                ),
                O::Uncertain { at } => (
                    "offset",
                    "uncertain",
                    None,
                    "a decision rounding left uncertain",
                    vec![entity("at", at)],
                ),
                O::Empty => (
                    "offset",
                    "empty",
                    None,
                    "nothing is left: the body is thinner everywhere than the distance moves its faces",
                    none(),
                ),
                O::ToleranceTooLarge { at, .. } => (
                    "offset",
                    "tolerance_too_large",
                    None,
                    "the result would hold a tolerance past the kernel's cap",
                    vec![entity("at", at)],
                ),
            },
            ryukei::Error::InvalidBoolean(b) => match b {
                B::Unsupported { a, b } => (
                    "boolean",
                    "unsupported",
                    None,
                    "the wall's faces meet on surfaces the kernel can't intersect yet",
                    vec![entity("a", a), entity("b", b)],
                ),
                B::Uncertain { a, b } => (
                    "boolean",
                    "uncertain",
                    None,
                    "where the wall's faces meet stays uncertain",
                    vec![entity("a", a), entity("b", b)],
                ),
                B::NonManifold { faces, .. } => (
                    "boolean",
                    "non_manifold",
                    None,
                    "the wall would touch itself along an edge or at a point",
                    faces.iter().map(|f| entity("face", f)).collect(),
                ),
                B::ToleranceTooLarge { at, .. } => (
                    "boolean",
                    "tolerance_too_large",
                    None,
                    "the result would hold a tolerance past the kernel's cap",
                    vec![entity("at", at)],
                ),
                B::OutsideSizeBox { .. } => (
                    "boolean",
                    "outside_size_box",
                    None,
                    "the wall leaves the ±500 m size box",
                    none(),
                ),
                B::DuplicateToolKey { .. } => (
                    "boolean",
                    "duplicate_tool_key",
                    None,
                    "the shell's boolean refused its tool",
                    none(),
                ),
                // A division's refusal: a shell's boolean never divides.
                B::ToolDoesNotDivide { face } => (
                    "boolean",
                    "tool_does_not_divide",
                    None,
                    "the shell's boolean couldn't put a piece on one side",
                    vec![entity("face", face)],
                ),
            },
            _ => ("offset", "kernel", None, "the kernel refused it", none()),
        };
    let code = if ty == "shell" {
        Code::ShellFailed
    } else {
        Code::OffsetFailed
    };
    let mut err = Error::new(code, format!("{ty} {id}: {msg} ({e})"))
        .with("feature", id)
        .with("solid", target)
        .with("reason", reason)
        .with("kernel", kernel)
        .with("refusal", format!("{e:?}"));
    if let Some(c) = case {
        err = err.with("case", c);
    }
    if ty == "shell" && (reason == "edge_vanishes" || case == Some("mixed_vertex")) {
        err = err.with(
            "hint",
            "the kernel can't yet shell with faces of their own thickness here, an open face beside a fillet its inner wall doesn't reach, or some corners where rounded and sharp edges meet: shell before filleting, and fillet the shell",
        );
    }
    if !entities.is_empty() {
        err = err.with("entities", entities);
    }
    err
}

/// A shell or offset (`ty`) from `current`'s fields (none for a new one)
/// with what was given laid over them: `target` (or one `targets`),
/// `thickness`, `direction`, `distance`, and `open` and `faces` whole
/// (`[]` for none), faces given as `<part>/<solid>/<name>`.
pub fn from_given(
    ty: &str,
    current: Option<&Feature>,
    f: &crate::proto::FeatureFields,
    part: &Id,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Feature> {
    if matches!(ty, "mirror" | "split") {
        return mirror_or_split(ty, current, f, part);
    }
    let mut v = match current {
        Some(Feature::Shell(s)) => s.fields(),
        Some(Feature::Offset(o)) => o.fields(),
        _ => json!({}),
    };
    let o = v.as_object_mut().expect("fields are an object");
    let reference = |t: &str, at: &str| -> Result<Value> {
        Ok(Reference::parse(t, part)
            .map_err(|e| e.with("pointer", at))?
            .to_json())
    };
    let target = match (&f.target, &f.targets) {
        (Some(_), Some(_)) => {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s target once"),
            ));
        }
        (Some(t), None) => Some(t.clone()),
        (None, Some(ts)) => match ts.as_slice() {
            [t] => Some(t.clone()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} acts on one target, not {}", ts.len()),
                )
                .with("param", "targets"));
            }
        },
        (None, None) => None,
    };
    if let Some(t) = target {
        o.insert("target".into(), json!(t));
    }
    if let Some(q) = &f.thickness {
        o.insert("thickness".into(), q.clone());
    }
    if let Some(q) = &f.distance {
        o.insert("distance".into(), q.clone());
    }
    if let Some(d) = &f.direction {
        match Direction::parse(d) {
            Some(Direction::Inward) => {
                o.remove("direction");
            }
            Some(Direction::Outward) => {
                o.insert("direction".into(), json!("outward"));
            }
            None => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty}'s direction is inward or outward, not {d:?}"),
                )
                .with("param", "direction"));
            }
        }
    }
    if let Some(open) = &f.open {
        let refs = open
            .iter()
            .enumerate()
            .map(|(i, t)| reference(t, &format!("/open/{i}")))
            .collect::<Result<Vec<_>>>()?;
        if refs.is_empty() {
            o.remove("open");
        } else {
            o.insert("open".into(), json!(refs));
        }
    }
    if let Some(groups) = &f.faces {
        let Value::Array(gs) = groups else {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a {ty}'s faces are a list of groups, each with its faces"),
            )
            .with("param", "faces"));
        };
        let mut out = Vec::with_capacity(gs.len());
        for (i, g) in gs.iter().enumerate() {
            let mut g = g.clone();
            if let Some(Value::Array(fs)) = g.get_mut("faces") {
                for (j, x) in fs.iter_mut().enumerate() {
                    if let Some(t) = x.as_str().map(str::to_owned) {
                        *x = reference(&t, &format!("/faces/{i}/faces/{j}"))?;
                    }
                }
            }
            out.push(g);
        }
        if out.is_empty() {
            o.remove("faces");
        } else {
            o.insert("faces".into(), Value::Array(out));
        }
    }
    let mut r = Obj::new(&v, "")?;
    let out = match ty {
        "shell" => Feature::Shell(Box::new(Shell::from_fields(&mut r, unit, env)?)),
        _ => Feature::Offset(Box::new(Offset::from_fields(&mut r, unit, env)?)),
    };
    r.finish()?;
    Ok(out)
}

/// A shell or offset as output reports it: its file's fields, a shell's
/// direction always, every quantity as written and in meters, every face
/// as `<part>/<solid>/<name>`. A mirror or split: its fields with the
/// plane as text (a split's flip and keep always).
pub fn report(part: &Id, f: &Feature) -> Value {
    match f {
        Feature::Mirror(m) => {
            let mut v = m.fields();
            v["plane"] = json!(m.plane.text(part));
            return v;
        }
        Feature::Split(s) => {
            let mut v = s.fields();
            v["plane"] = json!(s.plane.text(part));
            v["flip"] = json!(s.flip);
            v["keep"] = json!(s.keep.as_str());
            return v;
        }
        _ => {}
    }
    let (mut v, quantity, key, groups) = match f {
        Feature::Shell(s) => (s.fields(), &s.thickness, "thickness", &s.faces),
        Feature::Offset(o) => (o.fields(), &o.distance, "distance", &o.faces),
        _ => unreachable!("a shell or an offset"),
    };
    v[key] = quantity.report();
    for (i, g) in groups.iter().enumerate() {
        v["faces"][i][key] = g.quantity.report();
    }
    if let Feature::Shell(s) = f {
        v["direction"] = json!(s.direction.as_str());
    }
    for (role, r) in f.references() {
        let at = format!("/{}", role.replace('.', "/"));
        *v.pointer_mut(&at).expect("its own reference") = json!(r.text(part));
    }
    v
}

/// A mirror or split from `current`'s fields (none for a new one) with
/// what was given laid over them: a mirror's `solid`, `plane`,
/// `operation` and `targets`; a split's `target` (or one `targets`),
/// `plane`, `flip` and `keep`. Planes are given as text (`xy`,
/// `<part>/<datum>`, `<part>/<solid>/<name>`).
fn mirror_or_split(
    ty: &str,
    current: Option<&Feature>,
    f: &crate::proto::FeatureFields,
    part: &Id,
) -> Result<Feature> {
    let mut v = match current {
        Some(Feature::Mirror(m)) => m.fields(),
        Some(Feature::Split(s)) => s.fields(),
        _ => json!({}),
    };
    let o = v.as_object_mut().expect("fields are an object");
    if let Some(p) = &f.plane {
        let g =
            crate::doc::geometry::GeomRef::parse(p, part).map_err(|e| e.with("param", "plane"))?;
        o.insert("plane".into(), g.to_json());
    }
    if ty == "mirror" {
        if let Some(s) = &f.solid {
            o.insert("solid".into(), json!(s));
        }
        if let Some(op) = &f.operation {
            o.insert("operation".into(), json!(op));
            if op == "new" {
                o.remove("targets");
            }
        }
        if let Some(ts) = &f.targets {
            o.insert("targets".into(), json!(ts));
        }
        let mut r = Obj::new(&v, "")?;
        let m = crate::doc::body::Mirror::from_fields(&mut r)?;
        r.finish()?;
        return Ok(Feature::Mirror(Box::new(m)));
    }
    let target = match (&f.target, &f.targets) {
        (Some(_), Some(_)) => return Err(Error::new(Code::Usage, "give a split's target once")),
        (Some(t), None) => Some(t.clone()),
        (None, Some(ts)) => match ts.as_slice() {
            [t] => Some(t.clone()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a split acts on one target, not {}", ts.len()),
                )
                .with("param", "targets"));
            }
        },
        (None, None) => None,
    };
    if let Some(t) = target {
        o.insert("target".into(), json!(t));
    }
    if let Some(flip) = f.flip {
        o.insert("flip".into(), json!(flip));
    }
    if let Some(k) = &f.keep {
        o.insert("keep".into(), json!(k));
    }
    let mut r = Obj::new(&v, "")?;
    let s = crate::doc::body::Split::from_fields(&mut r)?;
    r.finish()?;
    Ok(Feature::Split(Box::new(s)))
}
/// What a mirror or split that built adds to its report: its plane as
/// it resolved, `{origin_m, normal}` (a split's normal as flipped), and a
/// split's sides, each `{side, solid, lumps}`. `None` for anything else.
pub fn resolved(part: &Part, results: &dyn Results, id: &Id, f: &Feature) -> Option<Value> {
    let plane = match f {
        Feature::Mirror(m) => &m.plane,
        Feature::Split(s) => &s.plane,
        _ => return None,
    };
    let at = crate::geometry::At {
        part,
        order: part.order(),
        owner: id,
        results,
    };
    let pl = crate::geometry::one(&at, "plane", plane, crate::geometry::Want::Plane)
        .ok()?
        .plane();
    let flip = matches!(f, Feature::Split(s) if s.flip);
    let n = if flip { -pl.normal } else { pl.normal };
    let mut v = json!({"plane": {"origin_m": [pl.point.x, pl.point.y, pl.point.z],
                                 "normal": [n.x, n.y, n.z]}});
    if let Feature::Split(s) = f
        && let Some(Ok(out)) = results.result(id)
    {
        let side = |name: &str, solid: &Id| {
            out.solid(solid)
                .map(|b| json!({"side": name, "solid": solid, "lumps": lumps(&b.body)}))
        };
        let mut sides = vec![side("behind", &s.target)];
        if s.keep == crate::doc::body::Keep::Both {
            sides.push(side("front", id));
        }
        v["sides"] = json!(sides.into_iter().flatten().collect::<Vec<_>>());
    }
    Some(v)
}
/// A mirror's copy: its solid as the mirror sees it, reflected in its
/// plane (any plane: an origin plane, a datum plane, a sketch's, a planar
/// face, of this solid too). Every entity keeps its source's name, so
/// editing the source renames the copy as it renames the source; its
/// origin is `{feature, mirror_of: <name>}`. The reflection names
/// nothing new, so its journal is empty. A reflection leaving the size
/// box is `mirror_failed`.
pub fn mirror_copy(
    part: &Part,
    solids: &Solids,
    id: &Id,
    m: &Mirror,
    before: &dyn Results,
) -> Result<Built> {
    let source = source(part, solids, id, &m.solid, before)?;
    let plane = crate::pattern::mirror_plane(part, id, &m.plane, before)?;
    let body = source.body.mirrored(&plane).map_err(|e| {
        Error::new(
            Code::MirrorFailed,
            format!(
                "mirror {id}: the mirror image of {} leaves the ±500 m size box",
                m.solid
            ),
        )
        .with("feature", id)
        .with("solid", &m.solid)
        .with("reason", "outside_size_box")
        .with("kernel", format!("{e:?}"))
    })?;
    let origins: Origins = crate::rebuild::names(&body)
        .into_iter()
        .map(|n| {
            let n = n.to_string();
            let o = json!({"feature": id, "mirror_of": n});
            (n, o)
        })
        .collect();
    crate::rebuild::finish(id, "mirror", (body, ryukei::name::Journal::new()), origins)
}

/// The solid a mirror (feature `id`) copies, as it sees it: what
/// [`crate::rebuild::target`] gives, its refusal naming the `solid` role.
fn source<'a>(
    part: &Part,
    solids: &Solids,
    id: &Id,
    s: &Id,
    before: &'a dyn Results,
) -> Result<&'a Built> {
    crate::rebuild::target(part, solids, id, s, before).map_err(|e| {
        if e.code == Code::ReferenceWrongKind
            && e.details.get("role").and_then(Value::as_str) == Some("targets")
        {
            crate::rebuild::not_a_solid(part, solids, id, "solid", s)
        } else {
            crate::rebuild::retag(e, "solid")
        }
    })
}

/// The key of the box a split (feature `id` of `part`) cuts with, from
/// IDs only, so moving the plane or resizing the target renames nothing.
fn split_key(part: &Id, id: &Id) -> OpKey {
    let mut bytes = b"ryucad.split\0".to_vec();
    bytes.extend_from_slice(part.as_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(id.as_str().as_bytes());
    OpKey::from_bytes(&bytes)
}

/// The tool key of one side's boolean (`behind` or `front`): each side's
/// cut face is `Placed` under a key of its own, so each has a name of its
/// own that lasts.
pub(crate) fn side_key(part: &Id, id: &Id, side: &str) -> ToolKey {
    let mut bytes = b"ryucad.tool\0".to_vec();
    for s in [part.as_str(), id.as_str(), side] {
        bytes.extend_from_slice(s.as_bytes());
        bytes.push(0);
    }
    bytes.pop();
    ToolKey(OpKey::from_bytes(&bytes).0)
}

/// How many lumps a body has: its solid regions.
pub fn lumps(b: &Body) -> usize {
    b.regions()
        .filter(|r| b.region(*r).is_some_and(|x| x.kind == RegionKind::Solid))
        .count()
}

/// `operation_no_effect` for a split (feature `id` of `t`) leaving `side`
/// empty: nothing of `t` lies there.
fn empty_side(id: &Id, t: &Id, side: &str) -> Error {
    let place = if side == "front" {
        "in front of"
    } else {
        "behind"
    };
    Error::new(
        Code::OperationNoEffect,
        format!(
            "split {id}: nothing of {t} lies {place} its plane; move the plane into {t}, or flip it"
        ),
    )
    .with("feature", id)
    .with("target", t)
    .with("side", side)
}

/// A split's half-space: a box with one face on the plane through
/// `point`, the box lying in front of it (along `normal`), reaching a
/// hundredth of the target's diagonal past its box every other way, as
/// a through-all extrude reaches. Refuses, naming the side, a plane the
/// target's box lies wholly on one side of.
pub(crate) fn half_space(
    part: &Id,
    id: &Id,
    t: &Id,
    target: &Body,
    point: Point3,
    normal: Vector3,
) -> Result<Body> {
    let n = normal
        .normalize()
        .expect("a plane's normal has a direction")
        .get();
    let (lo, hi) = crate::inspect::body_extent(target)
        .ok_or_else(|| Error::new(Code::Internal, format!("split {id}: {t} has no faces")))?;
    let (lo, hi) = (
        Point3::new(lo[0], lo[1], lo[2]),
        Point3::new(hi[0], hi[1], hi[2]),
    );
    let diagonal = (hi - lo).length();
    let margin = diagonal.max(1e-6) * 0.01;
    let (mut near, mut far) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in [lo.x, hi.x] {
        for y in [lo.y, hi.y] {
            for z in [lo.z, hi.z] {
                let d = (Point3::new(x, y, z) - point).dot(n);
                near = near.min(d);
                far = far.max(d);
            }
        }
    }
    let res = ryukei::tol::LINEAR_RESOLUTION;
    if far <= res {
        return Err(empty_side(id, t, "front"));
    }
    if near >= -res {
        return Err(empty_side(id, t, "behind"));
    }
    // The box's foot: the box's middle brought onto the plane.
    let middle = Point3::new(
        (lo.x + hi.x) / 2.0,
        (lo.y + hi.y) / 2.0,
        (lo.z + hi.z) / 2.0,
    );
    let foot = middle - n * (middle - point).dot(n);
    let hint = if n.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let axes = Frame::new(foot, n, hint).map_err(|e| {
        Error::new(
            Code::Internal,
            format!("split {id}: a plane that isn't one"),
        )
        .with("kernel", format!("{e:?}"))
    })?;
    let w = diagonal / 2.0 + margin;
    let (x, y) = (axes.x().get(), axes.y().get());
    let corner = foot - x * w - y * w;
    let frame = Frame::new(corner, n, x).map_err(|e| {
        Error::new(
            Code::Internal,
            format!("split {id}: a plane that isn't one"),
        )
        .with("kernel", format!("{e:?}"))
    })?;
    let (body, _) = ryukei::primitive::block(
        split_key(part, id),
        2.0 * w,
        2.0 * w,
        far + margin,
        &Rigid::from_frame(&frame),
    )
    .map_err(|e| {
        Error::new(
            Code::RebuildFailed,
            format!("split {id}: its half-space: {e}"),
        )
        .with("feature", id)
        .with("kernel", format!("{e:?}"))
    })?;
    Ok(body)
}

/// One side's tool: the half-space box, its every entity the split's,
/// marked with the side, so the side's cut face says so.
pub(crate) fn side_tool(id: &Id, half: &Body, side: &str) -> Result<Built> {
    let origins: Origins = crate::rebuild::names(half)
        .into_iter()
        .map(|n| (n.to_string(), json!({"feature": id, "side": side})))
        .collect();
    crate::rebuild::finish(
        id,
        "split",
        (half.clone(), ryukei::name::Journal::new()),
        origins,
    )
}

/// Which side a split's boolean (`side`: `behind` is the subtraction,
/// `front` the intersection) shows empty by its refusal: the subtraction
/// leaving nothing (`body_emptied`) or the intersection changing nothing
/// (`operation_no_effect`: the target lies wholly in front) mean nothing
/// lies behind; the other two, nothing in front. The box test in
/// [`half_space`] catches most misses first; these catch a target whose
/// box straddles the plane though the target doesn't (a ring cut through
/// its hole).
pub(crate) fn emptied(side: &str, e: &Error) -> Option<&'static str> {
    match (side, e.code) {
        ("behind", Code::BodyEmptied) | ("front", Code::OperationNoEffect) => Some("behind"),
        ("front", Code::BodyEmptied) | ("behind", Code::OperationNoEffect) => Some("front"),
        _ => None,
    }
}

/// A side's boolean refused because a side is empty: [`empty_side`],
/// naming it. Anything else as it was.
pub(crate) fn side_error(id: &Id, t: &Id, side: &str, e: Error) -> Error {
    match emptied(side, &e) {
        Some(s) => empty_side(id, t, s),
        None => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::solid::Operation::New;
    use crate::rebuild::rebuild_part;
    use crate::solid::tests::{boxed, part};
    use crate::units::Unit;
    use std::sync::atomic::AtomicBool;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// A shell or offset from its fields, in millimetres.
    fn feature(v: Value) -> Feature {
        let mut v = v;
        v["format"] = json!(9);
        v["kind"] = json!("ryucad.feature");
        Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap()
    }

    fn face(solid: &str, n: Name) -> Value {
        json!({"feature": solid, "name": n.to_string()})
    }

    /// The face of `b` lying flat at `at` mm along `axis`.
    fn plane(b: &Body, axis: usize, at: f64) -> Name {
        let f = b
            .faces()
            .find(|f| {
                let bx = ryukei::intersect::face_box(b, *f);
                let c = |p: ryukei::geom::Point3| [p.x, p.y, p.z][axis];
                (c(bx.min()) - at * 1e-3).abs() < 1e-9 && (c(bx.max()) - at * 1e-3).abs() < 1e-9
            })
            .unwrap_or_else(|| panic!("no face at {at} mm along axis {axis}"));
        b.face(f).unwrap().name
    }

    fn volume(b: &Body) -> f64 {
        let m = ryukei::mesh::tessellate(b, &ryukei::mesh::MeshParams::new(1e-7, 0.1)).unwrap();
        assert!(m.faults.is_empty(), "{:?}", m.faults);
        ryukei_testkit::mesh::volume_and_area(&m).0
    }

    /// The 1 mm cube `a` of `crate::solid::tests::boxed`.
    fn cube() -> Body {
        let r = rebuild_part(&part(vec![("a", boxed("0", New, &[]))]));
        r.output(&id("a"))
            .unwrap()
            .solid(&id("a"))
            .unwrap()
            .body
            .clone()
    }

    #[test]
    fn a_shell_hollows_its_target_and_its_walls_say_what_they_are_the_walls_of() {
        let b = cube();
        let (top, bottom) = (plane(&b, 2, 1.0), plane(&b, 2, 0.0));
        let s = feature(
            json!({"type": "shell", "target": "a", "thickness": "0.2 mm",
                               "open": [face("a", top)]}),
        );
        let p = part(vec![("a", boxed("0", New, &[])), ("s", s)]);
        assert_eq!(Solids::of(&p).chain(&id("a")).unwrap(), [id("a"), id("s")]);
        let r = rebuild_part(&p);
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let built = r.output(&id("s")).unwrap().solid(&id("a")).unwrap();
        let want = 1.0 - 0.6 * 0.6 * 0.8;
        assert!(
            (volume(&built.body) * 1e9 - want).abs() < 1e-9,
            "{}",
            volume(&built.body)
        );
        // The outside keeps its names and origins, the open face as the
        // walls' rim; the inner floor is the wall of the bottom.
        assert_eq!(
            built.origins.get(&top.to_string()),
            Some(&json!({"feature": "a"}))
        );
        let floor = plane(&built.body, 2, 0.2);
        assert_eq!(
            built.origins.get(&floor.to_string()),
            Some(&json!({"feature": "s", "wall_of": bottom.to_string()}))
        );
        // Outwards: the inner faces are the solid's old surfaces, `Placed`,
        // so they are the walls of the faces whose names the outside keeps.
        let s = feature(
            json!({"type": "shell", "target": "a", "thickness": "0.2 mm",
                               "direction": "outward", "open": [face("a", top)]}),
        );
        let r = rebuild_part(&part(vec![("a", boxed("0", New, &[])), ("s", s)]));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let built = r.output(&id("s")).unwrap().solid(&id("a")).unwrap();
        let floor = plane(&built.body, 2, 0.0);
        assert_eq!(
            built.origins.get(&floor.to_string()),
            Some(&json!({"feature": "s", "wall_of": bottom.to_string()}))
        );
        assert_eq!(
            built.origins.get(&bottom.to_string()),
            Some(&json!({"feature": "a"}))
        );
    }

    #[test]
    fn a_shell_refuses_what_isnt_its_targets_face_and_says_what_the_kernel_refused() {
        let b = cube();
        let other = feature(
            json!({"type": "shell", "target": "a", "thickness": "0.2 mm",
                                   "open": [face("c", plane(&b, 2, 1.0))]}),
        );
        let edge = b.edge(b.edges().next().unwrap()).unwrap().name;
        let wrong = feature(
            json!({"type": "shell", "target": "a", "thickness": "0.2 mm",
                                   "open": [face("a", edge)]}),
        );
        let deep = feature(json!({"type": "shell", "target": "a", "thickness": "0.6 mm"}));
        let err = |f: Feature| {
            let p = part(vec![
                ("a", boxed("0", New, &[])),
                ("c", boxed("5", New, &[])),
                ("s", f),
            ]);
            let r = rebuild_part(&p);
            r.features[2].1.as_ref().unwrap_err().clone()
        };
        let e = err(other);
        assert_eq!(e.code, Code::TargetsMixed, "{e}");
        assert_eq!(e.details["role"], "open.0");
        let e = err(wrong);
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::ReferenceWrongKind, Some("open.0")),
            "{e}"
        );
        // A wall past half the cube: nothing is left inside it.
        let e = err(deep);
        assert_eq!(e.code, Code::ShellFailed, "{e}");
        assert_eq!(
            (e.details["reason"].as_str(), e.details["kernel"].as_str()),
            (Some("empty"), Some("offset")),
            "{e}"
        );
        assert!(e.details.get("hint").is_none(), "{e}");
    }

    #[test]
    fn an_offset_grows_a_solid_and_its_fills_say_what_they_replace() {
        let o = feature(json!({"type": "offset", "target": "a", "distance": "0.1 mm"}));
        let r = rebuild_part(&part(vec![("a", boxed("0", New, &[])), ("o", o)]));
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let built = r.output(&id("o")).unwrap().solid(&id("a")).unwrap();
        let d = 0.1;
        let pi = core::f64::consts::PI;
        let want = 1.0 + 6.0 * d + pi * d * d * 3.0 + 4.0 / 3.0 * pi * d * d * d;
        assert!((volume(&built.body) * 1e9 - want).abs() < 1e-3 * want);
        // A fill for each of the 12 edges and 8 vertices.
        let replaced = built
            .origins
            .values()
            .filter(|o| o["feature"] == "o" && o.get("replaces").is_some())
            .count();
        assert_eq!(replaced, 20);
        // Its top pulled down alone: a box half as tall.
        let top = plane(&cube(), 2, 1.0);
        let pushed = |d: &str| {
            let o = feature(json!({"type": "offset", "target": "a", "distance": 0,
                                   "faces": [{"distance": d, "faces": [face("a", top)]}]}));
            rebuild_part(&part(vec![("a", boxed("0", New, &[])), ("o", o)]))
        };
        let r = pushed("-0.5 mm");
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let built = r.output(&id("o")).unwrap().solid(&id("a")).unwrap();
        assert!((volume(&built.body) * 1e9 - 0.5).abs() < 1e-9);
        // Pushed up, its sides' edges run on past their ends (#396): a box
        // half as tall again.
        let r = pushed("0.5 mm");
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let built = r.output(&id("o")).unwrap().solid(&id("a")).unwrap();
        assert!((volume(&built.body) * 1e9 - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_cancel_reaches_the_shells_kernel_call() {
        let top = plane(&cube(), 2, 1.0);
        let s = feature(
            json!({"type": "shell", "target": "a", "thickness": "0.2 mm",
                               "open": [face("a", top)]}),
        );
        let p = part(vec![("a", boxed("0", New, &[])), ("s", s.clone())]);
        let r = rebuild_part(&p);
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        // The same shell built again from the same results, its request
        // already cancelled: the kernel stops at its first charge.
        let flag = Arc::new(AtomicBool::new(true));
        let job = ryu_session::job::Job::new("t", "p", "rebuild", flag);
        let run = Run::with_job(&job);
        let e = build(&p, &Solids::of(&p), &id("s"), &s, &r, &run).unwrap_err();
        assert_eq!(e.code, Code::Cancelled, "{e}");
    }

    #[test]
    fn requests_become_files() {
        let (part, unit, env) = (id("p"), Unit::Mm, crate::expr::Env::empty());
        let n = "0123456789abcdef0123456789abcdef";
        let f = crate::proto::FeatureFields {
            targets: Some(vec!["a".into()]),
            thickness: Some(json!("2 mm")),
            open: Some(vec![format!("p/a/{n}")]),
            faces: Some(
                json!([{"thickness": "4 mm", "faces": [format!("p/a/{}", "f".repeat(32))]}]),
            ),
            ..Default::default()
        };
        let s = from_given("shell", None, &f, &part, unit, &env).unwrap();
        let v = s.to_json();
        assert_eq!(v["open"], json!([{"feature": "a", "name": n}]));
        assert_eq!(v["thickness"], "2 mm");
        // Edited: outward, its open faces and groups gone.
        let f = crate::proto::FeatureFields {
            direction: Some("outward".into()),
            open: Some(vec![]),
            faces: Some(json!([])),
            ..Default::default()
        };
        let s = from_given("shell", Some(&s), &f, &part, unit, &env).unwrap();
        assert_eq!(
            s.to_json(),
            json!({"format": 9, "kind": "ryucad.feature", "type": "shell", "target": "a",
                   "thickness": "2 mm", "direction": "outward"})
        );
        // Two targets, or a direction that isn't one, are refused.
        let two = crate::proto::FeatureFields {
            targets: Some(vec!["a".into(), "b".into()]),
            thickness: Some(json!(1)),
            ..Default::default()
        };
        let e = from_given("shell", None, &two, &part, unit, &env).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let up = crate::proto::FeatureFields {
            target: Some("a".into()),
            thickness: Some(json!(1)),
            direction: Some("up".into()),
            ..Default::default()
        };
        let e = from_given("shell", None, &up, &part, unit, &env).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        // An offset's groups take a distance.
        let f = crate::proto::FeatureFields {
            target: Some("a".into()),
            distance: Some(json!(0)),
            faces: Some(json!([{"distance": "-1 mm", "faces": [format!("p/a/{n}")]}])),
            ..Default::default()
        };
        let o = from_given("offset", None, &f, &part, unit, &env).unwrap();
        assert_eq!(o.to_json()["faces"][0]["distance"], "-1 mm");
    }

    /// A mirror from its fields.
    fn mirror_of(v: Value) -> Feature {
        let mut v = v;
        v["type"] = json!("mirror");
        feature_at(v, 10)
    }

    /// [`feature`] at format `n`.
    fn feature_at(mut v: Value, n: u64) -> Feature {
        v["format"] = json!(n);
        v["kind"] = json!("ryucad.feature");
        Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap()
    }

    #[test]
    fn a_mirror_copies_its_solid_keeping_its_names() {
        // The 1 mm cube at the origin, mirrored about yz: a cube at x < 0.
        let m = mirror_of(json!({"solid": "a", "plane": "yz", "operation": "new"}));
        let p = part(vec![("a", boxed("0", New, &[])), ("m", m)]);
        let s = Solids::of(&p);
        assert!(s.is_solid(&id("m")));
        assert_eq!(
            crate::graph::inputs(p.features[&id("m")].as_ref().unwrap()),
            [&id("a")]
        );
        let r = rebuild_part(&p);
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let copy = r.output(&id("m")).unwrap().solid(&id("m")).unwrap();
        let source = r.output(&id("a")).unwrap().solid(&id("a")).unwrap();
        assert!((volume(&copy.body) * 1e9 - 1.0).abs() < 1e-9);
        let (lo, hi) = crate::inspect::body_extent(&copy.body).unwrap();
        assert!((lo[0] + 1e-3).abs() < 1e-12 && hi[0].abs() < 1e-12);
        // Its names are its source's; its origins say what each reflects.
        assert_eq!(
            crate::rebuild::names(&copy.body),
            crate::rebuild::names(&source.body)
        );
        let n = crate::rebuild::names(&copy.body)
            .into_iter()
            .next()
            .unwrap()
            .to_string();
        assert_eq!(
            copy.origins.get(&n),
            Some(&json!({"feature": "m", "mirror_of": n}))
        );
        assert!(copy.journal.is_empty());
    }

    #[test]
    fn a_joined_mirror_unites_with_its_source_into_one_solid() {
        // Mirrored about its own +x face, the cube's copy touches it there.
        let b = cube();
        let px = plane(&b, 0, 1.0);
        let m = mirror_of(
            json!({"solid": "a", "plane": face("a", px), "operation": "join",
                                 "targets": ["a"]}),
        );
        let p = part(vec![("a", boxed("0", New, &[])), ("m", m)]);
        assert_eq!(Solids::of(&p).chain(&id("a")).unwrap(), [id("a"), id("m")]);
        let r = rebuild_part(&p);
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let out = r.output(&id("m")).unwrap();
        let built = out.solid(&id("a")).unwrap();
        assert!((volume(&built.body) * 1e9 - 2.0).abs() < 1e-9);
        // One block: the faces either side of the plane on one surface merged.
        assert_eq!(built.body.faces().count(), 6);
        assert!(out.applied().is_some());
        // Mirrored in a plane of its own symmetry, the copy is the cube:
        // the join changes nothing.
        let m = mirror_of(json!({"solid": "a", "plane": "xy", "operation": "join",
                                 "targets": ["a"]}));
        let mid = |v: f64| {
            let mut f = boxed("0", New, &[]);
            if let Feature::Primitive(p) = &mut f {
                p.placement.origin[2] =
                    crate::units::Quantity::parse(&v.to_string(), Unit::Mm).unwrap();
            }
            f
        };
        let p = part(vec![("a", mid(-0.5)), ("m", m)]);
        let r = rebuild_part(&p);
        let e = r.features[1].1.as_ref().unwrap_err();
        assert_eq!(e.code, Code::OperationNoEffect, "{e}");
    }

    #[test]
    fn a_mirror_says_what_it_cant_mirror() {
        // A plane that is an edge, and a solid that isn't one.
        let b = cube();
        let edge = b.edge(b.edges().next().unwrap()).unwrap().name;
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            (
                "m",
                mirror_of(json!({"solid": "a", "plane": face("a", edge), "operation": "new"})),
            ),
            (
                "j",
                boxed("0.5", crate::doc::solid::Operation::Join, &["a"]),
            ),
            (
                "n",
                mirror_of(json!({"solid": "j", "plane": "yz", "operation": "new"})),
            ),
        ]);
        let r = rebuild_part(&p);
        let e = r.features[1].1.as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::ReferenceWrongKind, Some("plane")),
            "{e}"
        );
        let e = r.features[3].1.as_ref().unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        // Mirrored about a plane 400 m off, the copy leaves the size box.
        let far = feature_at(
            json!({"type": "plane", "by": "offset", "from": "yz", "distance": "400 m"}),
            7,
        );
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("d", far),
            (
                "m",
                mirror_of(json!({"solid": "a", "plane": {"feature": "d"}, "operation": "new"})),
            ),
        ]);
        let r = rebuild_part(&p);
        let e = r.features[2].1.as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::MirrorFailed, Some("outside_size_box")),
            "{e}"
        );
    }

    fn split_of(target: &str, plane: Value, extra: Value) -> Feature {
        let mut v = json!({"type": "split", "target": target, "plane": plane});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        feature_at(v, 10)
    }

    /// The xy plane moved up `z` mm, as an offset datum.
    fn level(z: &str) -> Feature {
        feature_at(
            json!({"type": "plane", "by": "offset", "from": "xy", "distance": z}),
            7,
        )
    }

    #[test]
    fn a_split_keeps_the_back_and_founds_the_front() {
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("d", level("0.25 mm")),
            ("s", split_of("a", json!({"feature": "d"}), json!({}))),
        ]);
        let r = rebuild_part(&p);
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        let out = r.output(&id("s")).unwrap();
        let ids: Vec<&str> = out.solids().iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(ids, ["a", "s"]);
        let (behind, front) = (out.solid(&id("a")).unwrap(), out.solid(&id("s")).unwrap());
        assert!((volume(&behind.body) * 1e9 - 0.25).abs() < 1e-9);
        assert!((volume(&front.body) * 1e9 - 0.75).abs() < 1e-9);
        assert_eq!((lumps(&behind.body), lumps(&front.body)), (1, 1));
        // Each side's cut face says it is the split's, and which side.
        let cut = |b: &Built, side: &str| {
            b.origins
                .iter()
                .filter(|(_, o)| **o == json!({"feature": "s", "side": side}))
                .count()
        };
        assert!(cut(behind, "behind") >= 1 && cut(front, "front") >= 1);
        // Flipped, the sides swap.
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("d", level("0.25 mm")),
            (
                "s",
                split_of("a", json!({"feature": "d"}), json!({"flip": true})),
            ),
        ]);
        let r = rebuild_part(&p);
        let out = r.output(&id("s")).unwrap();
        assert!((volume(&out.solid(&id("a")).unwrap().body) * 1e9 - 0.75).abs() < 1e-9);
        // Keeping one side, the split is a trim: one solid.
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("d", level("0.25 mm")),
            (
                "s",
                split_of("a", json!({"feature": "d"}), json!({"keep": "behind"})),
            ),
        ]);
        let r = rebuild_part(&p);
        let out = r.output(&id("s")).unwrap();
        assert_eq!(out.solids().len(), 1);
        assert!((volume(&out.solid(&id("a")).unwrap().body) * 1e9 - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_split_that_misses_says_which_side_is_empty_and_fails_both_chains() {
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("d", level("3 mm")),
            ("s", split_of("a", json!({"feature": "d"}), json!({}))),
            (
                "j",
                boxed("0.5", crate::doc::solid::Operation::Join, &["s"]),
            ),
        ]);
        let r = rebuild_part(&p);
        let e = r.features[2].1.as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["side"].as_str()),
            (Code::OperationNoEffect, Some("front")),
            "{e}"
        );
        let e = r.features[3].1.as_ref().unwrap_err();
        assert_eq!(e.code, Code::DependencyFailed, "{e}");
        // Below the cube, nothing lies behind.
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("d", level("-3 mm")),
            ("s", split_of("a", json!({"feature": "d"}), json!({}))),
        ]);
        let e = rebuild_part(&p).features[2].1.as_ref().unwrap_err().clone();
        assert_eq!(e.details["side"], "behind", "{e}");
        // An axis isn't a plane.
        let p = part(vec![
            ("a", boxed("0", New, &[])),
            ("s", split_of("a", json!("z"), json!({}))),
        ]);
        let e = rebuild_part(&p).features[1].1.as_ref().unwrap_err().clone();
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::ReferenceWrongKind, Some("plane")),
            "{e}"
        );
    }
}
