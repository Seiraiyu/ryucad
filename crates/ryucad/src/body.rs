//! Shells and offsets (design 327, plan 15.A): one solid hollowed to a
//! wall, or grown, shrunk, or its faces pushed and pulled, by one kernel
//! call under the feature's key and the request's cancel. Both act on
//! their target: they join its chain, and what they leave is the target.

use crate::doc::body::{Direction, Offset, Shell};
use crate::doc::files::{Feature, Obj};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Made, Origins, Results, Run};
use crate::solid::Solids;
use ryukei::name::{Event, Name};
use ryukei::offset::{OffsetSpec, ShellSpec};
use ryukei::topo::EntityId;
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
pub fn check(part: &Part, id: &Id, f: &Feature, results: &dyn Results) -> Result<()> {
    resolve(part, &Solids::of(part), id, f, results).map(drop)
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
            "the kernel can't yet shell past half the part, an open face beside a fillet its inner wall doesn't reach, or some corners where rounded and sharp edges meet: shell before filleting, and fillet the shell",
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
/// as `<part>/<solid>/<name>`.
pub fn report(part: &Id, f: &Feature) -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::solid::Operation::New;
    use crate::rebuild::rebuild_part;
    use crate::solid::tests::{boxed, part};
    use crate::units::Unit;
    use ryukei::topo::Body;
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
        // A wall past half the cube: the top's and bottom's offsets cross.
        let e = err(deep);
        assert_eq!(e.code, Code::ShellFailed, "{e}");
        assert_eq!(
            (e.details["reason"].as_str(), e.details["kernel"].as_str()),
            (Some("edge_vanishes"), Some("offset")),
            "{e}"
        );
        assert!(e.details["hint"].is_string(), "{e}");
        let first = &e.details["entities"][0];
        assert_eq!(first["kind"], "edge", "{e}");
        assert!(
            first["reference"].as_str().unwrap().starts_with("p/a/"),
            "{e}"
        );
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
        // Pushed up, its sides' edges would have to run on past their
        // ends, which the kernel doesn't carry kept edges past yet.
        let r = pushed("0.5 mm");
        let e = r.features[1].1.as_ref().unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            (Code::OffsetFailed, Some("edge_vanishes")),
            "{e}"
        );
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
}
