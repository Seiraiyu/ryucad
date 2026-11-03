//! Sweep features: a profile (a sketch's closed loops, or a planar face's
//! loops) carried along a path (a path or helix feature, an edge, or a
//! sketch's line, arc or circle) by ryukei's `sweep::along`, with a frame
//! law, a twist and a scale, then founding a solid or applied to others
//! as an extrude's body is.
//!
//! Names come from keys only: the profile's segment and loop keys (a
//! sketch's entities, or a face's edges and vertices by name; see
//! [`crate::profile`]) and the path's segment keys (see [`crate::path`]),
//! so editing the path's shape, the twist, the scale or the profile's
//! size renames nothing.

use crate::doc::geometry::GeomRef;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sweep::{Law, Section, Source, Sweep};
use crate::error::{Code, Error, Result};
use crate::geometry::{At, Geom, Want, resolve};
use crate::path::Made as PathMade;
use crate::profile::Keys;
use crate::rebuild::{Built, Results, Run, op_key};
use core::f64::consts::TAU;
use ryukei::geom::FrameLaw;
use ryukei::name::{Name, OpKey, Role, SweepRole as R};
use ryukei::sweep::{AlongOptions, ValidProfile};
use ryukei::tol::ANGULAR_RESOLUTION;
use ryukei::{Error as Kernel, SweepError};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A sweep's profile, resolved: the kernel's profile, which entity each
/// key came from, and how origins name its source (`{"sketch": id}` or
/// `{"face": {feature, name}}`) and its entities (`entity` or `edge`).
pub struct Profile {
    pub valid: ValidProfile,
    pub keys: Keys,
    pub source: Value,
    pub entity: &'static str,
}

/// Resolves sweep `id`'s profile `s`, as `id` sees what it reads.
pub fn profile(part: &Part, id: &Id, s: &Section, before: &dyn Results) -> Result<Profile> {
    match &s.source {
        Source::Sketch(sketch) => {
            let solved = crate::rebuild::sketch_input(part, id, sketch, before)?;
            let (valid, keys) = crate::profile::profile(&part.id, sketch, solved, &s.loops)
                .map_err(|e| e.with("feature", id).with("sketch", sketch))?;
            Ok(Profile {
                valid,
                keys,
                source: json!({"sketch": sketch}),
                entity: "entity",
            })
        }
        Source::Face(g) => {
            let GeomRef::Entity(r) = g else {
                unreachable!("a face section is an entity, as read")
            };
            let (valid, keys) =
                crate::profile::of_face(part, part.order(), id, "profile", r, &s.loops, before)?;
            Ok(Profile {
                valid,
                keys,
                source: json!({"face": g.to_json()}),
                entity: "edge",
            })
        }
    }
}

/// The kernel's frame law for sweep `at.owner`: minimal, or binormal
/// about its direction (an axis's direction, a plane's normal).
pub fn law(at: &At, w: &Sweep) -> Result<FrameLaw> {
    match &w.law {
        Law::Minimal => Ok(FrameLaw::Minimal),
        Law::Binormal(d) => {
            let v = match resolve(at, "direction", d, &[Want::Axis, Want::Plane])? {
                Geom::Axis(l) => l.dir.get(),
                Geom::Plane(p) => p.normal,
                _ => unreachable!("asked for an axis or a plane"),
            };
            let u = v.normalize().ok_or_else(|| {
                at.fail(
                    Code::ReferenceWrongKind,
                    "direction",
                    d,
                    "gives no direction".to_owned(),
                )
            })?;
            Ok(FrameLaw::Binormal(u))
        }
    }
}

/// Refuses a twist that isn't whole turns or a scale other than 1 round a
/// closed path: the profile must come back to itself.
pub fn closing(id: &Id, w: &Sweep, path: &PathMade) -> Result<()> {
    if !path.closed {
        return Ok(());
    }
    let twist = w.twist_radians();
    let turns = (twist / TAU).round();
    if (twist - turns * TAU).abs() > ANGULAR_RESOLUTION {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "sweep {id}: round a closed path its twist is whole turns, not {}",
                w.twist.as_ref().map_or_else(String::new, |q| q.text())
            ),
        )
        .with("feature", id)
        .with("pointer", "/twist"));
    }
    if w.scale_number() != 1.0 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("sweep {id}: round a closed path it can't scale: its end meets its start"),
        )
        .with("feature", id)
        .with("pointer", "/scale"));
    }
    Ok(())
}

/// A sweep's own body: its profile along its path, under the request's
/// cancel.
pub fn build(part: &Part, id: &Id, w: &Sweep, before: &dyn Results, run: &Run) -> Result<Built> {
    let at = At {
        part,
        order: part.order(),
        owner: id,
        results: before,
    };
    let p = profile(part, id, &w.profile, before)?;
    let path = crate::path::of_curve(&at, "path", &w.path)?;
    let law = law(&at, w)?;
    closing(id, w, &path)?;
    let key = op_key(&part.id, id);
    let opts = AlongOptions {
        law,
        twist: w.twist_radians(),
        scale: w.scale_number(),
    };
    let work = crate::work::work(run.job);
    let origins = origins(key, &part.id, &p, &path);
    let made =
        ryukei::sweep::along_with(key, &p.valid, &path.kernel(), &opts, &work).map_err(|e| {
            match crate::work::stopped(&e, run.job) {
                Some(stop) => stop,
                None => failed(&part.id, id, e, &p, &path, &origins),
            }
        })?;
    let mut origins = origins;
    for o in origins.values_mut() {
        o["feature"] = json!(id);
    }
    crate::rebuild::finish(id, "sweep", made, origins)
}

/// A kernel refusal's `reason`, in snake case.
fn reason(e: &SweepError) -> &'static str {
    match e {
        SweepError::ProfileAlongPath => "profile_along_path",
        SweepError::PathTooCurved { .. } => "path_too_curved",
        SweepError::FrameUndefined { .. } => "frame_undefined",
        SweepError::NotClosing => "not_closing",
        SweepError::BadScale => "bad_scale",
        SweepError::SelfIntersecting { .. } => "self_intersecting",
        SweepError::Uncertain { .. } => "uncertain",
        _ => "kernel",
    }
}

/// The kernel's refusal of sweep `id`, in its own terms: `sweep_failed`
/// with its `reason` and what it names (the path segment of a leg that
/// folds or whose frame is undefined, the two faces that meet, the
/// profile's entities).
fn failed(
    part: &Id,
    id: &Id,
    e: Kernel,
    p: &Profile,
    path: &PathMade,
    origins: &BTreeMap<String, Value>,
) -> Error {
    let kernel = format!("{e:?}");
    let Kernel::InvalidSweep(s) = e else {
        return Error::new(Code::RebuildFailed, format!("sweep {id}: {e}"))
            .with("feature", id)
            .with("kernel", kernel);
    };
    let segment = |k: ryukei::sweep::SegmentKey| {
        path.pieces
            .iter()
            .find(|q| q.key == k)
            .map(|q| q.from.text(part))
    };
    let mut err = Error::new(Code::SweepFailed, format!("sweep {id}: {}", describe(&s)))
        .with("feature", id)
        .with("reason", reason(&s))
        .with("kernel", kernel);
    match s {
        SweepError::PathTooCurved { segment: k } | SweepError::FrameUndefined { segment: k } => {
            if let Some(t) = segment(k) {
                err = err.with("path_segment", t);
            }
        }
        SweepError::SelfIntersecting { faces } | SweepError::Uncertain { faces } => {
            let faces: Vec<Value> = faces
                .iter()
                .map(|n| {
                    let mut v = json!({"reference": format!("{part}/{id}/{n}")});
                    if let Some(o) = origins.get(&n.to_string()) {
                        let mut o = o.clone();
                        o["feature"] = json!(id);
                        v["origin"] = o;
                    }
                    v
                })
                .collect();
            err = err.with("faces", faces);
        }
        SweepError::CrossesAxis { segment: k }
        | SweepError::TouchesAxisInside { segment: k }
        | SweepError::PinchedOnAxis { segment: k } => {
            if let Some(e) = p.keys.segments.get(&k.0) {
                err = err.with("entities", [e]);
            }
        }
        _ => {}
    }
    err
}

/// What a refusal means, for people.
fn describe(e: &SweepError) -> &'static str {
    match e {
        SweepError::ProfileAlongPath => {
            "the profile's plane holds the path's start tangent: put the profile across the path, on a datum plane normal to it at its start"
        }
        SweepError::PathTooCurved { .. } => {
            "the path bends too tightly for the profile: it would fold where the path's radius of curvature is smaller than the profile's reach"
        }
        SweepError::FrameUndefined { .. } => {
            "its binormal direction runs along the path's tangent there"
        }
        SweepError::NotClosing => "round a closed path the profile must come back to itself",
        SweepError::BadScale => "its scale isn't more than nothing",
        SweepError::SelfIntersecting { .. } => "the swept faces meet each other",
        SweepError::Uncertain { .. } => "where two swept faces meet couldn't be certified",
        _ => "ryukei refuses it",
    }
}

/// Where each entity of a sweep came from, by the hex of its stable name,
/// derived from the keys as the kernel names them: a side from its
/// profile entity and its path segment (`path_segment`, as the path's
/// `inspect` gives it); a lateral from the profile entities either side
/// of its vertex and its path segment; a section between legs and a
/// joint's vertex from both path segments (`path_segments`); caps from
/// their loop and end, cap edges and end vertices as an extrude's. Each
/// carries the profile's source (`sketch`, or `face`). Entities a sweep
/// doesn't make simply have no origin.
pub fn origins(key: OpKey, part: &Id, p: &Profile, path: &PathMade) -> BTreeMap<String, Value> {
    let derive = |role: R, parents: &[Name]| Name::derive(key, Role::Sweep(role), parents);
    let mut out = BTreeMap::new();
    let mut put = |n: Name, extra: Value| {
        let mut v = p.source.clone();
        for (k, x) in extra.as_object().expect("an object") {
            v[k] = x.clone();
        }
        out.insert(n.to_string(), v);
    };
    let paths: Vec<(Name, String)> = path
        .pieces
        .iter()
        .map(|q| {
            (
                derive(R::PathSegment, &[Name::from_u128(q.key.0)]),
                q.from.text(part),
            )
        })
        .collect();
    let k = paths.len();
    let ent = p.entity;
    for lump in p.valid.lumps() {
        let outer = &lump.outer;
        let lid = p.keys.loops[&outer.key.0].clone();
        let stand = derive(R::Loop, &[Name::from_u128(outer.key.0)]);
        let caps = [
            ("start", derive(R::StartCap, &[stand])),
            ("end", derive(R::EndCap, &[stand])),
        ];
        for (end, cap) in caps {
            put(cap, json!({"cap": end, "loop": lid}));
        }
        for l in std::iter::once(outer).chain(&lump.holes) {
            let n = l.segments.len();
            let ids: Vec<Id> = l
                .segments
                .iter()
                .map(|g| p.keys.segments[&g.key.0].clone())
                .collect();
            let stands: Vec<Name> = l
                .segments
                .iter()
                .map(|g| derive(R::Segment, &[Name::from_u128(g.key.0)]))
                .collect();
            for (i, e) in ids.iter().enumerate() {
                let sides: Vec<Name> = paths
                    .iter()
                    .map(|(q, _)| derive(R::Side, &[stands[i], *q]))
                    .collect();
                for (r, side) in sides.iter().enumerate() {
                    put(*side, json!({ent: e, "path_segment": paths[r].1}));
                    let next = (r + 1) % k;
                    put(
                        derive(R::Section, &[*side, sides[next]]),
                        json!({ent: e, "path_segments": [paths[r].1, paths[next].1]}),
                    );
                }
                put(
                    derive(R::CapEdge, &[caps[0].1, sides[0]]),
                    json!({ent: e, "cap": "start"}),
                );
                put(
                    derive(R::CapEdge, &[caps[1].1, sides[k - 1]]),
                    json!({ent: e, "cap": "end"}),
                );
                // The profile vertex between this curve and the one before.
                let pair = [stands[(i + n - 1) % n], stands[i]];
                let between = [&ids[(i + n - 1) % n], e];
                put(
                    derive(R::StartVertex, &pair),
                    json!({"entities": between, "cap": "start"}),
                );
                put(
                    derive(R::EndVertex, &pair),
                    json!({"entities": between, "cap": "end"}),
                );
                for r in 0..k {
                    let next = (r + 1) % k;
                    put(
                        derive(R::Lateral, &[pair[0], pair[1], paths[r].0]),
                        json!({"entities": between, "path_segment": paths[r].1}),
                    );
                    put(
                        derive(
                            R::JointVertex,
                            &[pair[0], pair[1], paths[r].0, paths[next].0],
                        ),
                        json!({"entities": between, "path_segments": [paths[r].1, paths[next].1]}),
                    );
                }
            }
        }
    }
    out
}

/// Refuses sweep `id` whose references don't resolve to the kinds its
/// fields want now: a sketch profile's sketch listed before it with the
/// entities its loops name, a face profile's planar face, its path a
/// curve and its direction an axis or a plane; round a closed path, a
/// twist of whole turns and no scale. Whether the kernel can sweep it only
/// rebuild tells.
pub fn check(part: &Part, order: &[Id], id: &Id, w: &Sweep, results: &dyn Results) -> Result<()> {
    let at = At {
        part,
        order,
        owner: id,
        results,
    };
    match &w.profile.source {
        Source::Sketch(_) => {}
        Source::Face(GeomRef::Entity(r)) => {
            let found = crate::reference::resolve(part, order, id, "profile", r, results)?;
            crate::reference::plane(part, id, "profile", r, found)?;
        }
        Source::Face(_) => unreachable!("a face section is an entity, as read"),
    }
    let path = crate::path::of_curve(&at, "path", &w.path)?;
    law(&at, w)?;
    closing(id, w, &path)
}

/// The law a new sweep along `path` (in `part`, listed in `order` before
/// it) gets when the request leaves it out: binormal about the helix's
/// axis along a path that is exactly one helix (a helix feature, or a path
/// of that helix alone), so its profile turns with the helix as a thread
/// or spring needs; else minimal.
pub fn default_law(part: &Part, path: &GeomRef) -> Law {
    use crate::doc::files::Feature;
    use crate::doc::path::PathFeature;
    let helix = |g: &GeomRef| -> Option<Id> {
        let GeomRef::Feature(f) = g else {
            return None;
        };
        match part.features.get(f)?.as_ref().ok()? {
            Feature::Path(p) => match p.as_ref() {
                PathFeature::Helix(_) => Some(f.clone()),
                _ => None,
            },
            _ => None,
        }
    };
    let found = helix(path).or_else(|| {
        let GeomRef::Feature(f) = path else {
            return None;
        };
        match part.features.get(f)?.as_ref().ok()? {
            Feature::Path(p) => match p.as_ref() {
                PathFeature::Path(x) if x.segments.len() == 1 => helix(&x.segments[0].curve),
                _ => None,
            },
            _ => None,
        }
    });
    match found {
        Some(h) => Law::Binormal(GeomRef::Feature(h)),
        None => Law::Minimal,
    }
}

/// A sweep as rebuild reports and `inspect --feature` give it: its
/// profile (`{sketch}` or `{face}`, with the loops built: each loop's
/// anchor and entities), the path it followed (as the path's report
/// gives it: `{segments, closed, length_m}`), its law, twist and scale.
pub fn report(part: &Id, p: &Profile, path: &PathMade, w: &Sweep) -> Value {
    let loops: Vec<Value> = p
        .valid
        .lumps()
        .iter()
        .flat_map(|lump| std::iter::once(&lump.outer).chain(&lump.holes))
        .map(|l| {
            let ids: Vec<&Id> = l
                .segments
                .iter()
                .map(|g| &p.keys.segments[&g.key.0])
                .collect();
            json!({"anchor": p.keys.loops[&l.key.0], p.entity: ids})
        })
        .collect();
    let mut profile = p.source.clone();
    profile["loops"] = json!(loops);
    let mut v = json!({
        "profile": profile,
        "path": path.report(part),
        "law": w.law.as_str(),
        "twist_rad": w.twist_radians(),
        "scale": w.scale_number(),
    });
    if let Law::Binormal(d) = &w.law {
        v["direction"] = json!(d.text(part));
    }
    v
}

/// A sweep from `current` (none for a new one) with the request's fields
/// laid over it: references as text in `part`, quantities as
/// expressions. `sketch` or `face` replaces the profile (its loops with
/// it, unless `loops` is given too); `law` `minimal` drops a direction,
/// and a `direction` alone makes the law binormal; `twist` and `scale` of
/// `""` are written as nothing; `operation` `new` drops `targets`. A new
/// sweep that names no law gets [`default_law`]'s, written into its file.
pub fn from_given(
    current: Option<&Sweep>,
    f: &crate::proto::FeatureFields,
    part: &Part,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<crate::doc::files::Feature> {
    let pid = &part.id;
    let geom = |t: &str, at: &str| crate::server::geom_json(t, pid, at);
    let bad = |param: &str, msg: String| Error::new(Code::InvalidParam, msg).with("param", param);
    let mut v = current.map_or_else(|| json!({}), Sweep::fields);
    let o = v.as_object_mut().expect("fields are an object");
    match (&f.sketch, &f.face) {
        (Some(_), Some(_)) => {
            return Err(bad(
                "face",
                "a sweep's profile is a sketch or a planar face, not both".into(),
            ));
        }
        (Some(s), None) => {
            o.insert("profile".into(), json!({"sketch": s}));
        }
        (None, Some(t)) => {
            o.insert("profile".into(), json!({"face": geom(t, "/profile/face")?}));
        }
        (None, None) => {}
    }
    if let Some(l) = &f.loops {
        let Some(p) = o.get_mut("profile").and_then(Value::as_object_mut) else {
            return Err(bad(
                "loops",
                "loops narrow a profile: name its sketch or face too".into(),
            ));
        };
        if l.is_empty() {
            p.remove("loops");
        } else {
            p.insert("loops".into(), json!(l));
        }
    }
    if !o.contains_key("profile") {
        return Err(bad(
            "sketch",
            "a sweep carries a profile: a sketch's closed loops (sketch) or a planar face's (face)"
                .into(),
        ));
    }
    if let Some(t) = &f.path {
        o.insert("path".into(), geom(t, "/path")?);
    }
    let Some(path) = o.get("path").cloned() else {
        return Err(bad(
            "path",
            "a sweep names its path: a path or helix, an edge, or a sketch's line, arc or circle"
                .into(),
        ));
    };
    match (f.law.as_deref(), &f.direction) {
        (Some("minimal"), Some(_)) => {
            return Err(bad(
                "direction",
                "a minimal law takes no direction; binormal does".into(),
            ));
        }
        (Some("minimal"), None) => {
            o.insert("law".into(), json!("minimal"));
            o.remove("direction");
        }
        (Some("binormal") | None, Some(d)) => {
            o.insert("law".into(), json!("binormal"));
            o.insert("direction".into(), geom(d, "/direction")?);
        }
        (Some("binormal"), None) => {
            o.insert("law".into(), json!("binormal"));
            if !o.contains_key("direction") {
                return Err(bad(
                    "direction",
                    "a binormal law keeps a direction in the profile's plane: name it (an axis, or a plane's normal)".into(),
                ));
            }
        }
        (Some(l), _) => {
            return Err(bad(
                "law",
                format!("a sweep's law is minimal or binormal, not {l}"),
            ));
        }
        (None, None) => {}
    }
    if !o.contains_key("law") {
        let g = crate::doc::geometry::GeomRef::from_json(&path, "/path")?;
        match default_law(part, &g) {
            Law::Minimal => {
                o.insert("law".into(), json!("minimal"));
            }
            Law::Binormal(d) => {
                o.insert("law".into(), json!("binormal"));
                o.insert("direction".into(), d.to_json());
            }
        }
    }
    for (k, x) in [("twist", &f.twist), ("scale", &f.scale)] {
        match x {
            None => {}
            Some(Value::String(t)) if t.is_empty() => {
                o.remove(k);
            }
            Some(x) => {
                o.insert(k.into(), x.clone());
            }
        }
    }
    if let Some(op) = &f.operation {
        if op == "new" {
            o.remove("operation");
            o.remove("targets");
        } else {
            o.insert("operation".into(), json!(op));
        }
    }
    if let Some(t) = &f.targets {
        o.insert("targets".into(), json!(t));
    }
    v["format"] = json!(crate::doc::sweep::FORMAT);
    v["kind"] = json!(crate::doc::files::FEATURE_KIND);
    v["type"] = json!("sweep");
    crate::doc::files::Feature::from_json(&v, unit, env)
}

/// A sweep as output reports it: its file's fields, every reference as
/// text, every quantity as written and as a value.
pub fn feature_report(part: &Id, w: &Sweep) -> Value {
    let mut v = w.fields();
    if let Source::Face(g) = &w.profile.source {
        v["profile"]["face"] = json!(g.text(part));
    }
    v["path"] = json!(w.path.text(part));
    if let Law::Binormal(d) = &w.law {
        v["direction"] = json!(d.text(part));
    }
    for (k, q) in w.quantities() {
        v[k] = q.report();
    }
    v
}

/// What sweep `id` built from, for its rebuild report: [`report`] over
/// its profile and path resolved again from `results` (both cheap once
/// built), or `None` where they no longer resolve.
pub fn described(part: &Part, id: &Id, w: &Sweep, results: &dyn Results) -> Option<Value> {
    let at = At {
        part,
        order: part.order(),
        owner: id,
        results,
    };
    let p = profile(part, id, &w.profile, results).ok()?;
    let path = crate::path::of_curve(&at, "path", &w.path).ok()?;
    Some(report(&part.id, &p, &path, w))
}
