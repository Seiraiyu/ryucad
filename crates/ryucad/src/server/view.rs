//! Requests that look at the model: meshes, inspect, framing a camera, flat patterns.

use super::{noted, versioned};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::entity::{self, EntityRef};
use crate::error::{Code, Error, Result};
use crate::geometry::normal_to_face;
use crate::live;
use crate::live::CadLive;
use crate::live::Live;
use crate::live::SelectMode;
use crate::live::Style;
use crate::mesh;
use crate::part::{feature, feature_id, part_id, part_kind};
use crate::proto::Answer;
use crate::scene::{flat_meshed, meshes, model_points, occurrence_meshes};
use crate::session::Session;
use crate::solid::Solids;
use crate::view::CameraArg;
use crate::view::{Camera, DEFAULT_PROJECTION, StandardView};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The meshes [`mesh_binary`] and [`mesh_get`] would make; `None` when the
/// cache has them all, or the request fails (`handle` says how).
pub(crate) fn mesh_pending(
    session: &mut Session,
    part: &str,
    only: Option<&BTreeSet<String>>,
    quality: Option<&str>,
) -> Option<mesh::Pending> {
    let (quality, items): (_, Vec<(Feature, crate::rebuild::Built)>) = if is_assembly(session, part)
    {
        let (pid, _, quality) = mesh_args(part, None, quality).ok()?;
        let model = session
            .model(&pid, &crate::doc::assembly::Rev::Working)
            .ok()?;
        // Each body once, before anything is copied: a root places
        // 100,000 occurrences of 10,000 bodies.
        let mut seen = BTreeSet::new();
        let items = model
            .occurrences
            .iter()
            .filter(|o| only.is_none_or(|w| w.contains(&crate::assembly::path_text(&o.path))))
            .flat_map(|o| &o.solids)
            .filter(|s| seen.insert(s.built.hash.clone()))
            .filter(|s| !session.meshes.has(&s.built.hash, quality))
            .map(|s| (s.feature.as_ref().clone(), s.built.as_ref().clone()))
            .collect();
        (quality, items)
    } else {
        let one = one_of(only).ok()?;
        let (pid, only, quality) = mesh_args(part, one, quality).ok()?;
        let p = session.project().part(&pid).ok()?;
        let solids = Solids::of(p);
        let features: BTreeMap<Id, Feature> = p
            .features
            .iter()
            .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
            .collect();
        let result = session.rebuilt(&pid).ok()?;
        let items = result
            .shown(&solids)
            .into_iter()
            .filter(|s| only.as_ref().is_none_or(|o| o == s.solid))
            .filter_map(|s| {
                let (f, b) = s.state?;
                Some((features.get(f)?.clone(), b.as_ref().clone()))
            })
            .collect();
        (quality, items)
    };
    let refs: Vec<(&Feature, &crate::rebuild::Built)> = items.iter().map(|(f, b)| (f, b)).collect();
    session.meshes.pending(&refs, quality)
}

/// How a camera is to look: a view's name (standard, else a named view of
/// `part`, else of `focus`, else of the project's model when it is one
/// part), or normal to a face.
pub(crate) struct Look<'a> {
    pub(crate) view: Option<&'a str>,
    pub(crate) normal_to: Option<&'a str>,
    pub(crate) part: Option<&'a str>,
    pub(crate) focus: Option<&'a Id>,
}

/// A camera from a standard view, a named view or one normal to a face,
/// and/or framing entities. With none of those, it keeps `current`'s
/// direction (iso when there is none). A named view is restored as saved
/// unless there are entities to frame, which it frames keeping its
/// direction.
pub(crate) fn framed(
    session: &mut Session,
    current: Option<&Camera>,
    look: Look<'_>,
    fit: &[String],
) -> Result<Camera> {
    let named = match look.view {
        Some(v) if StandardView::named(v).is_none() => {
            let part = crate::views::part_for(session, look.part, look.focus)?;
            Some(crate::views::named(session, part.as_ref(), v)?)
        }
        _ => None,
    };
    if let Some(saved) = &named
        && fit.is_empty()
    {
        return Ok(saved.clone());
    }
    let view = look.view.and_then(StandardView::named);
    let along = look
        .normal_to
        .map(|face| normal_to_face(session, face))
        .transpose()?;
    let refs = entity::parse_all(session, fit)?;
    let points = model_points(session, &refs)?;
    if points.is_empty() {
        return Err(Error::new(Code::InvalidParam, "there is nothing to frame"));
    }
    let projection = current.map_or(DEFAULT_PROJECTION, |c| c.projection);
    Ok(match (named, along, view, current) {
        (Some(saved), ..) => crate::view::refit_points(&saved, &points, 1.0),
        (None, Some((back, up)), ..) => {
            crate::view::frame_points_along(&points, back, up, projection, 1.0)
        }
        // Square: windows that follow the camera may be any shape.
        (None, None, None, Some(c)) => crate::view::refit_points(c, &points, 1.0),
        (None, None, v, _) => {
            crate::view::frame_points(&points, v.unwrap_or(StandardView::Iso), projection, 1.0)
        }
    })
}

/// What a pick pass for `me`'s marks leaves out: their participant's
/// hidden set, when the request came from one.
pub(crate) fn pick_from(live: Option<&Live>, me: Option<&str>) -> crate::annotation::PickFrom {
    crate::annotation::PickFrom {
        hidden: live
            .zip(me)
            .and_then(|(l, m)| l.get(m).ok())
            .map(|p| p.view.hidden.clone())
            .unwrap_or_default(),
    }
}

/// The flat `mesh.get` answers of: `solid` (`""`: the part's one sheet
/// metal solid) laid flat as `flat.get` lays it, the commit read.
fn mesh_flat(
    session: &mut Session,
    part: &str,
    solid: &str,
    rev: Option<&str>,
) -> Result<(Value, mesh::Meshed)> {
    let named = (!solid.is_empty()).then_some(solid);
    let (commit, f) = crate::sheet_metal::flat::of(session, part, named, rev, &Default::default())?;
    let mut v = json!({
        "part": f.part, "quality": crate::mesh::Quality::Normal.as_str(), "flat": f.solid,
        "from": match &f.from { crate::sheet_metal::flat::From::Whole => "whole", crate::sheet_metal::flat::From::Unfold(_) => "unfold" },
    });
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    let m = flat_meshed(&f);
    Ok((v, m))
}

/// `mesh.get --flat` with `transfer`: the flat's one body as bytes,
/// labelled by its solid.
pub(crate) fn mesh_flat_binary(
    session: &mut Session,
    part: &str,
    solid: &str,
    rev: Option<&str>,
    compact: bool,
) -> Result<Answer> {
    let (v, m) = mesh_flat(session, part, solid, rev)?;
    let label = v["flat"].as_str().unwrap_or_default().to_owned();
    let bodies = [(label, Ok(m))];
    Ok(match compact {
        true => Answer::bytes(v, mesh::COMPACT, mesh::encode_compact(&bodies)),
        false => Answer::bytes(v, mesh::ENCODING, mesh::encode(&bodies)),
    })
}

/// `mesh.get --flat` meshes one flat body at normal quality, so it takes
/// no feature, occurrences or quality: refused, naming the field, rather
/// than answered as if they were honoured.
pub(crate) fn flat_alone(
    feature: Option<&str>,
    features: &[String],
    quality: Option<&str>,
) -> Result<()> {
    let given = [
        ("feature", feature.is_some()),
        ("features", !features.is_empty()),
        ("quality", quality.is_some()),
    ];
    match given.iter().find(|(_, g)| *g) {
        Some((field, _)) => Err(Error::new(
            Code::InvalidParam,
            format!(
                "a flat body is the solid's one flat, at normal quality: give flat without {field}"
            ),
        )
        .with("param", *field)),
        None => Ok(()),
    }
}

/// `mesh.get --flat` as JSON: the flat's one body, as `mesh_get` gives a
/// part's.
pub(crate) fn mesh_flat_json(
    session: &mut Session,
    part: &str,
    solid: &str,
    rev: Option<&str>,
) -> Result<Value> {
    let (mut v, m) = mesh_flat(session, part, solid, rev)?;
    let mut b = mesh::to_json(&m.body, &m.mesh);
    b["feature"] = v["flat"].clone();
    b["hash"] = json!(m.hash);
    v["bodies"] = json!([b]);
    Ok(v)
}

/// The part, feature and quality a mesh request names.
fn mesh_args(
    part: &str,
    only: Option<&str>,
    quality: Option<&str>,
) -> Result<(Id, Option<Id>, mesh::Quality)> {
    let quality = match quality {
        Some(q) => mesh::Quality::parse(q)?,
        None => mesh::Quality::preferred(),
    };
    Ok((
        part_id(part)?,
        only.map(|s| Id::solid_key(s, "feature")).transpose()?,
        quality,
    ))
}

/// What a `mesh.get` asks for: a solid or occurrence (`feature`), several
/// of an assembly's occurrences (`features`), or (`None`) everything.
pub(crate) fn wanted(feature: Option<&str>, features: &[String]) -> Option<BTreeSet<String>> {
    let all: BTreeSet<String> = feature
        .map(str::to_owned)
        .into_iter()
        .chain(features.iter().cloned())
        .collect();
    (!all.is_empty()).then_some(all)
}

/// The one solid a part's `mesh.get` names, if any: several are an
/// assembly's occurrences only.
fn one_of(only: Option<&BTreeSet<String>>) -> Result<Option<&str>> {
    match only.map(|w| w.iter().collect::<Vec<_>>()).as_deref() {
        None => Ok(None),
        Some([one]) => Ok(Some(one.as_str())),
        Some(_) => Err(Error::new(
            Code::InvalidParam,
            "features names several of an assembly's occurrences; a part's solid is feature",
        )
        .with("pointer", "/features")),
    }
}

/// Display meshes, each with its label.
type Labelled = Vec<(String, Result<mesh::Meshed>)>;

/// An assembly's display meshes: each body its occurrences place, once,
/// labelled by the first occurrence that places it (`<path>/<solid>`), or
/// only the occurrence at path `only`; and the occurrences chosen, which
/// say where each body goes.
fn assembly_meshes(
    session: &mut Session,
    pid: &Id,
    only: Option<&BTreeSet<String>>,
    quality: mesh::Quality,
) -> Result<(Labelled, Vec<Value>)> {
    let model = session.model(pid, &crate::doc::assembly::Rev::Working)?;
    let mut found = BTreeSet::new();
    let chosen: Vec<&crate::assembly::Occurrence> = model
        .occurrences
        .iter()
        .filter(|o| {
            only.is_none_or(|w| {
                let path = crate::assembly::path_text(&o.path);
                w.contains(&path) && found.insert(path)
            })
        })
        .collect();
    if let Some(p) = only.and_then(|w| w.iter().find(|p| !found.contains(*p))) {
        return Err(Error::new(
            Code::NotFound,
            format!("no occurrence {p} in assembly {pid}"),
        )
        .with("part", pid)
        .with("occurrence", p));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut solids: Vec<(String, &Id, &crate::assembly::Solid)> = Vec::new();
    for o in &chosen {
        for s in &o.solids {
            if seen.insert(s.built.hash.clone()) {
                let path = crate::assembly::path_text(&o.path);
                solids.push((format!("{path}/{}", s.founder), &o.part, s));
            }
        }
    }
    let refs: Vec<&crate::assembly::Solid> = solids.iter().map(|(_, _, s)| *s).collect();
    let made = occurrence_meshes(session, &refs, quality)?;
    let bodies = solids
        .into_iter()
        .zip(made)
        .map(|((label, part, s), mesh)| {
            // A reference's holes are warned of, as a part's are (#676).
            let warning = mesh::unmeshed_warning(part, &s.founder, &s.built, &mesh);
            let m = mesh::Meshed {
                body: s.built.body.clone(),
                hash: s.built.hash.clone(),
                mesh,
                warning,
            };
            (label, Ok(m))
        })
        .collect();
    let occurrences = chosen.iter().map(|o| o.to_json()).collect();
    Ok((bodies, occurrences))
}

/// Whether `part` is an assembly.
fn is_assembly(session: &Session, part: &str) -> bool {
    part_id(part)
        .ok()
        .and_then(|id| session.project().parts.get(&id).map(part_kind))
        == Some(PartKind::Assembly)
}

/// The warnings bodies' meshes carry (a reference's unmeshed faces), in
/// body order.
fn mesh_warnings<K>(bodies: &[(K, Result<mesh::Meshed>)]) -> Vec<Value> {
    bodies
        .iter()
        .filter_map(|(_, m)| m.as_ref().ok()?.warning.clone())
        .collect()
}

/// Puts `warnings` into `result` when there are any.
fn with_warnings(result: &mut Value, warnings: Vec<Value>) {
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
}

/// Meshes as bytes: `ryucad.mesh.1`, or `ryucad.mesh.2` when `compact`.
pub(crate) fn mesh_binary(
    session: &mut Session,
    part: &str,
    only: Option<&BTreeSet<String>>,
    quality: Option<&str>,
    compact: bool,
) -> Result<Answer> {
    fn bytes<K: AsRef<str>>(
        bodies: &[(K, Result<mesh::Meshed>)],
        compact: bool,
    ) -> (&'static str, Vec<u8>) {
        match compact {
            true => (mesh::COMPACT, mesh::encode_compact(bodies)),
            false => (mesh::ENCODING, mesh::encode(bodies)),
        }
    }
    if is_assembly(session, part) {
        let (pid, _, quality) = mesh_args(part, None, quality)?;
        let (bodies, occurrences) = assembly_meshes(session, &pid, only, quality)?;
        let mut result =
            json!({"part": pid, "quality": quality.as_str(), "occurrences": occurrences});
        with_warnings(&mut result, mesh_warnings(&bodies));
        let (encoding, b) = bytes(&bodies, compact);
        return Ok(Answer::bytes(result, encoding, b));
    }
    let (pid, only, quality) = mesh_args(part, one_of(only)?, quality)?;
    let bodies = meshes(session, &pid, only.as_ref(), quality)?;
    let mut result = json!({"part": pid, "quality": quality.as_str()});
    with_warnings(&mut result, mesh_warnings(&bodies));
    let (encoding, b) = bytes(&bodies, compact);
    Ok(Answer::bytes(result, encoding, b))
}

/// `mesh.get` with `before`: solid `feature` as feature `before` of the
/// part read it, shown or not (a delete's ghost, ryucad 25.B), with the
/// part's ID and the quality it was meshed at.
fn mesh_before(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    reader: &str,
    quality: Option<&str>,
) -> Result<(Id, Id, mesh::Quality, mesh::Meshed)> {
    let Some(solid) = solid else {
        return Err(Error::new(
            Code::InvalidParam,
            "before names the feature that read the solid named by feature",
        )
        .with("param", "feature"));
    };
    if is_assembly(session, part) {
        return Err(Error::new(
            Code::InvalidParam,
            "before is a part's: an assembly's features don't read solids",
        )
        .with("param", "before"));
    }
    let (pid, solid, quality) = mesh_args(part, Some(solid), quality)?;
    let solid = solid.expect("given");
    let reader = feature_id(reader).map_err(|e| e.with("param", "before"))?;
    let m = crate::scene::mesh_before(session, &pid, &solid, &reader, quality)?;
    Ok((pid, solid, quality, m))
}

/// `mesh_before` answered as JSON.
fn mesh_before_json(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    reader: &str,
    quality: Option<&str>,
) -> Result<Value> {
    let (pid, solid, quality, m) = mesh_before(session, part, solid, reader, quality)?;
    let mut body = mesh::to_json(&m.body, &m.mesh);
    body["feature"] = json!(solid);
    body["hash"] = json!(m.hash);
    let mut v = json!({"part": pid, "quality": quality.as_str(), "bodies": [body]});
    with_warnings(&mut v, m.warning.into_iter().collect());
    Ok(v)
}

/// `mesh_before` answered as bytes: `ryucad.mesh.1`, or `.2` compact.
pub(crate) fn mesh_before_binary(
    session: &mut Session,
    part: &str,
    solid: Option<&str>,
    reader: &str,
    quality: Option<&str>,
    compact: bool,
) -> Result<Answer> {
    let (pid, solid, quality, m) = mesh_before(session, part, solid, reader, quality)?;
    let bodies = [(solid, Ok(m))];
    let mut result = json!({"part": pid, "quality": quality.as_str()});
    with_warnings(&mut result, mesh_warnings(&bodies));
    let (encoding, b) = match compact {
        true => (mesh::COMPACT, mesh::encode_compact(&bodies)),
        false => (mesh::ENCODING, mesh::encode(&bodies)),
    };
    Ok(Answer::bytes(result, encoding, b))
}

pub(crate) fn mesh_get(
    session: &mut Session,
    part: &str,
    only: Option<&BTreeSet<String>>,
    quality: Option<&str>,
) -> Result<Value> {
    if is_assembly(session, part) {
        let (pid, _, quality) = mesh_args(part, None, quality)?;
        let (bodies, occurrences) = assembly_meshes(session, &pid, only, quality)?;
        let warnings = mesh_warnings(&bodies);
        let bodies: Vec<Value> = bodies
            .into_iter()
            .filter_map(|(label, m)| {
                let m = m.ok()?;
                let mut v = mesh::to_json(&m.body, &m.mesh);
                v["feature"] = json!(label);
                v["hash"] = json!(m.hash);
                Some(v)
            })
            .collect();
        let mut v = json!({"part": pid, "quality": quality.as_str(), "bodies": bodies,
                           "occurrences": occurrences});
        with_warnings(&mut v, warnings);
        return Ok(v);
    }
    let (pid, only, quality) = mesh_args(part, one_of(only)?, quality)?;
    let mut warnings = Vec::new();
    let bodies: Vec<Value> = meshes(session, &pid, only.as_ref(), quality)?
        .into_iter()
        .map(|(id, r)| match r {
            Ok(m) => {
                warnings.extend(m.warning.clone());
                let mut v = mesh::to_json(&m.body, &m.mesh);
                v["feature"] = json!(id);
                v["hash"] = json!(m.hash);
                v
            }
            Err(e) => json!({"feature": id, "ok": false, "error": e}),
        })
        .collect();
    // Each body is a solid, by its founder (`feature`, as entity
    // references name it); a reference's holes are warned of.
    let mut v = json!({"part": pid, "quality": quality.as_str(), "bodies": bodies});
    with_warnings(&mut v, warnings);
    Ok(v)
}

/// `inspect` of an assembly: each occurrence (or the one at path `only`)
/// with its part, revision and pose and each solid's body by stable name;
/// with `entity`, a full reference, just that entity.
fn inspect_assembly(
    session: &mut Session,
    pid: &Id,
    only: Option<&str>,
    entity: Option<&str>,
) -> Result<Value> {
    let model = session.model(pid, &crate::doc::assembly::Rev::Working)?;
    let chosen: Vec<&crate::assembly::Occurrence> = model
        .occurrences
        .iter()
        .filter(|o| only.is_none_or(|p| crate::assembly::path_text(&o.path) == p))
        .collect();
    if let Some(p) = only
        && chosen.is_empty()
    {
        return Err(Error::new(
            Code::NotFound,
            format!("no occurrence {p} in assembly {pid}"),
        )
        .with("part", pid)
        .with("occurrence", p));
    }
    if let Some(given) = entity {
        let r = EntityRef::parse(given)?.in_assembly();
        if r.part != *pid || r.is_occurrence() {
            return Err(Error::new(
                Code::NotFound,
                format!("{given} isn't an entity of an occurrence of {pid}"),
            )
            .with("entity", given));
        }
        for o in &chosen {
            if o.path != r.path {
                continue;
            }
            for sd in o.solids.iter().filter(|sd| sd.founder == r.feature) {
                let b = crate::inspect::body(&sd.feature, &sd.built);
                for (list, kind) in [("faces", "face"), ("edges", "edge"), ("vertices", "vertex")] {
                    if let Some(e) = b[list]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|e| e["name"] == r.name.as_str())
                    {
                        return Ok(json!({"occurrence": crate::assembly::path_text(&o.path),
                                         "part": o.part, "solid": sd.founder,
                                         "kind": kind, "entity": e}));
                    }
                }
            }
        }
        return Err(Error::new(
            Code::NotFound,
            format!("no entity {given} in assembly {pid}"),
        )
        .with("entity", given));
    }
    let occurrences: Vec<Value> = chosen
        .iter()
        .map(|o| {
            let bodies: Vec<Value> = o
                .solids
                .iter()
                .map(|sd| {
                    let mut v = crate::inspect::body(&sd.feature, &sd.built);
                    v["solid"] = json!(sd.founder);
                    v
                })
                .collect();
            let mut v = o.to_json();
            v["bodies"] = json!(bodies);
            v
        })
        .collect();
    Ok(json!({"part": pid, "occurrences": occurrences}))
}

pub(crate) fn inspect(
    session: &mut Session,
    part: &str,
    only: Option<&str>,
    entity: Option<&str>,
) -> Result<Value> {
    if is_assembly(session, part) {
        return inspect_assembly(session, &part_id(part)?, only, entity);
    }
    let pid = part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let only = only.map(feature_id).transpose()?;
    if let Some(f) = &only {
        feature(session.project().part(&pid)?, f)?;
    }
    let features: Vec<(Id, Feature)> = session
        .project()
        .part(&pid)?
        .features
        .iter()
        .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
        .collect();
    let solids = Solids::of(session.project().part(&pid)?);
    // A sweep, loft, mirror, split or tweak asked for says what it built from:
    // its part, held apart from the session while it rebuilds.
    let held = match &only {
        Some(f)
            if features.iter().any(|(i, x)| {
                i == f
                    && (x.sweep().is_some()
                        || x.loft().is_some()
                        || matches!(
                            x,
                            Feature::Mirror(_)
                                | Feature::Split(_)
                                | Feature::MoveFace(_)
                                | Feature::ReplaceFace(_)
                                | Feature::Draft(_)
                                | Feature::DeleteFace(_)
                                | Feature::DeleteBlend(_)
                                | Feature::ResizeBlend(_)
                        ))
            }) =>
        {
            Some(session.project().part(&pid)?.clone())
        }
        _ => None,
    };
    let result = session.rebuilt(&pid)?;
    let of = |id: &Id| {
        &features
            .iter()
            .find(|(i, _)| i == id)
            .expect("built features loaded")
            .1
    };
    // A datum's geometry, or why it didn't build; the model lists every
    // datum (only when it has any).
    let datum_line = |id: &Id| -> Option<Value> {
        let (_, r) = result.features.iter().find(|(f, _)| f == id)?;
        let (_, f) = features.iter().find(|(i, _)| i == id)?;
        let ty = f.type_name();
        if !matches!(f, Feature::Datum(_)) {
            return None;
        }
        Some(match r {
            Ok(o) => json!({"feature": id, "type": ty, "datum": o.datum()?.report()}),
            Err(e) => json!({"feature": id, "type": ty, "error": e}),
        })
    };
    if let Some(f) = &only
        && let Some(d) = datum_line(f)
    {
        return Ok(json!({"part": pid, "datum": d, "bodies": []}));
    }
    // A path's or helix's geometry, or why it didn't build; the model
    // lists every one (only when it has any).
    let path_line = |id: &Id| -> Option<Value> {
        let (_, r) = result.features.iter().find(|(f, _)| f == id)?;
        let (_, f) = features.iter().find(|(i, _)| i == id)?;
        let ty = f.type_name();
        if !matches!(f, Feature::Path(_)) {
            return None;
        }
        Some(match r {
            Ok(o) => json!({"feature": id, "type": ty, ty: o.path()?.report(&pid)}),
            Err(e) => json!({"feature": id, "type": ty, "error": e}),
        })
    };
    if let Some(f) = &only
        && let Some(d) = path_line(f)
    {
        let ty = d["type"].as_str().unwrap_or("path").to_owned();
        return Ok(json!({"part": pid, ty: d, "bodies": []}));
    }
    let (datums, paths): (Vec<Value>, Vec<Value>) = if only.is_none() {
        (
            result
                .features
                .iter()
                .filter_map(|(id, _)| datum_line(id))
                .collect(),
            result
                .features
                .iter()
                .filter_map(|(id, _)| path_line(id))
                .collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut bodies = Vec::new();
    let mut put = |id: &Id, solid: &Id, b: &crate::rebuild::Built| {
        let mut v = crate::inspect::body(of(id), b);
        v["feature"] = json!(id);
        v["solid"] = json!(solid);
        bodies.push(v);
    };
    match &only {
        // The solids a feature leaves, as it leaves them.
        Some(f) => {
            if let Some(out) = result.output(f) {
                for (solid, b) in out.solids() {
                    put(f, solid, b);
                }
            }
        }
        // The model: each solid as it shows. An inserted solid nothing
        // acted on is counted under its insert instead (`inserts`), so a
        // board of hundreds is one line; asked for an entity, every
        // solid counts.
        None => {
            for shown in result.shown(&solids) {
                let untouched =
                    shown.solid.is_path() && shown.tip.as_str() == shown.solid.founder_str();
                if untouched && entity.is_none() {
                    continue;
                }
                if let Some((f, b)) = shown.state {
                    put(f, shown.solid, b);
                }
            }
        }
    }
    if let Some(given) = entity {
        // A bare name, or a full reference as `selection` and Copy
        // reference give it: then only its own part and feature count.
        let (name, in_feature) = if given.contains('/') {
            let r = crate::entity::EntityRef::parse(given)?;
            if r.part != pid {
                return Err(Error::new(
                    Code::NotFound,
                    format!("{given} is in part {}, not {pid}", r.part),
                )
                .with("entity", given));
            }
            (r.name, Some(r.feature))
        } else {
            (given.to_owned(), None)
        };
        // Just the one entity, wherever it is. A reference names its
        // solid. A hole's face says which hole.
        let mut found = None;
        'bodies: for b in &bodies {
            if in_feature
                .as_ref()
                .is_some_and(|f| b["solid"] != f.as_str())
            {
                continue;
            }
            for (list, kind) in [("faces", "face"), ("edges", "edge"), ("vertices", "vertex")] {
                if let Some(e) = b[list]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == name.as_str())
                {
                    found = Some(json!({"feature": b["feature"], "solid": b["solid"],
                                        "kind": kind, "entity": e}));
                    break 'bodies;
                }
            }
        }
        if let Some(mut v) = found {
            if v["kind"] == "face"
                && let (Some(solid), Some(name)) =
                    (v["solid"].as_str(), v["entity"]["name"].as_str())
                && let Some(note) = crate::holes::face_note(session, &pid, solid, name)
            {
                for k in ["hole", "thread"] {
                    if let Some(x) = note.get(k) {
                        v["entity"][k] = x.clone();
                    }
                }
            }
            return Ok(v);
        }
        return Err(
            Error::new(Code::NotFound, format!("no entity {given} in part {pid}"))
                .with("entity", given),
        );
    }
    let mut v = json!({"part": pid, "bodies": bodies});
    // Each insert: what it inserted, by count for the model, whole for
    // the insert asked for (design 521).
    let inserts: Vec<Value> = features
        .iter()
        .filter_map(|(id, f)| Some((id, f.insert()?)))
        .filter(|(id, _)| only.as_ref().is_none_or(|o| o == *id))
        .map(|(id, i)| {
            let made = match result.output(id) {
                Some(crate::rebuild::Output::Solids(m)) => Some(&**m),
                _ => None,
            };
            if only.is_some() {
                return crate::insert::report(i, made);
            }
            let r = crate::insert::report(i, None);
            json!({"feature": id, "of": r["of"], "rev": r["rev"],
                   "solids": made.map_or(0, |m| m.solids.len())})
        })
        .collect();
    match (&only, inserts.as_slice()) {
        (Some(_), [one]) => v["insert"] = one.clone(),
        (None, [_, ..]) => v["inserts"] = json!(inserts),
        _ => {}
    }
    if let (Some(p), Some(f)) = (&held, &only)
        && let Some(Ok(Feature::Sweep(w))) = p.features.get(f)
        && let Some(d) = crate::along::described(p, f, w, result)
    {
        v["sweep"] = d;
    }
    if let (Some(p), Some(f)) = (&held, &only)
        && let Some(Ok(Feature::Loft(l))) = p.features.get(f)
        && let Some(d) = crate::loft::described(p, f, l, result)
    {
        v["loft"] = d;
    }
    if !datums.is_empty() {
        v["datums"] = json!(datums);
    }
    if !paths.is_empty() {
        v["paths"] = json!(paths);
    }
    // A shell, offset, mirror or split says what it was given, as
    // `feature.show` does; a mirror or split that built, its plane as
    // resolved too.
    if let Some(f) = &only
        && let Some((
            _,
            x @ (Feature::Shell(_)
            | Feature::Offset(_)
            | Feature::Mirror(_)
            | Feature::Split(_)
            | Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_)),
        )) = features.iter().find(|(i, _)| i == f)
    {
        let k = x.type_name();
        let tweak = crate::tweak::takes_sheets(x);
        v[k] = if tweak {
            crate::tweak::report(&pid, x)
        } else {
            crate::body::report(&pid, x)
        };
        let built = matches!(crate::rebuild::Results::result(result, f), Some(Ok(_)));
        if let Some(Value::Object(extra)) = held.as_ref().filter(|_| built).and_then(|q| {
            if tweak {
                crate::tweak::resolved(q, result, f, x, true, unit)
            } else {
                crate::body::resolved(q, result, f, x)
            }
        }) {
            for (key, e) in extra {
                v[k][key] = e;
            }
        }
    }
    if let Some(f) = &only
        && let Some((_, x @ (Feature::Fill(_) | Feature::Extract(_)))) =
            features.iter().find(|(i, _)| i == f)
    {
        v[x.type_name()] = crate::surface::report(&pid, x);
    }
    Ok(v)
}

/// `appearance.get`.
pub(crate) fn appearance_get(
    session: &mut Session,
    live: Option<&mut Live>,
    part: Option<String>,
    participant: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    // A participant's appearance is of what its window shows.
    let part = part.or_else(|| {
        let l = live.as_deref()?;
        Some(
            l.get(participant.as_deref()?)
                .ok()?
                .view
                .focus
                .as_ref()?
                .to_string(),
        )
    });
    let (commit, s) = session.at(rev.as_deref())?;
    noted(crate::appearance::get(s, part.as_deref()), commit)
}

/// `appearance.set`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn appearance_set(
    session: &mut Session,
    entities: Vec<String>,
    colour: Option<String>,
    material: bool,
    opacity: Option<f64>,
    unset: Vec<String>,
    file_styles: Option<bool>,
    expected_revision: Option<String>,
) -> Result<Value> {
    let change = crate::appearance::Change {
        colour,
        material,
        opacity,
        unset,
        file_styles,
    };
    crate::appearance::set(session, &entities, &change, expected_revision.as_deref())
}

/// `inspect`.
pub(crate) fn inspect_request(
    session: &mut Session,
    part: String,
    feature: Option<String>,
    entity: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (commit, s) = session.at(rev.as_deref())?;
    versioned(noted(
        inspect(s, &part, feature.as_deref(), entity.as_deref()),
        commit,
    ))
}

/// `selection.get`.
pub(crate) fn selection_get(live: Option<&mut Live>, participant: Option<String>) -> Result<Value> {
    let target = match &live {
        Some(l) => l.target(participant.as_deref())?,
        None => None,
    };
    Ok(match target {
        Some(p) => json!({"participant": p.id, "selection": p.selection}),
        None => json!({"participant": null, "selection": []}),
    })
}

/// `selection.set`.
pub(crate) fn selection_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    entities: Vec<String>,
    mode: SelectMode,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("selection.set"))?;
    // What is being removed may no longer exist.
    let refs = match mode {
        SelectMode::Remove => entities
            .iter()
            .map(|s| EntityRef::parse(s))
            .collect::<Result<Vec<_>>>()?,
        _ => entity::parse_all(session, &entities)?,
    };
    let id = live.caller(me);
    let selection = live.select(&id, refs, mode)?;
    Ok(json!({"participant": id, "selection": selection}))
}

/// `camera.get`.
pub(crate) fn camera_get(live: Option<&mut Live>, participant: Option<String>) -> Result<Value> {
    let target = match &live {
        Some(l) => l.target(participant.as_deref())?,
        None => None,
    };
    Ok(match target {
        Some(p) => json!({"participant": p.id, "camera": p.view.camera}),
        None => json!({"participant": null, "camera": null}),
    })
}

/// `camera.set`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn camera_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    camera: Option<CameraArg>,
    view: Option<String>,
    fit: Vec<String>,
    normal_to: Option<String>,
    part: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("camera.set"))?;
    let id = live.caller(me);
    // A named view restores the planes and the explode it saved,
    // as well as its camera.
    let mut restore = None;
    let camera: Camera = match camera {
        Some(_) if view.is_some() || !fit.is_empty() || normal_to.is_some() => {
            return Err(Error::new(
                Code::Usage,
                "give a camera, or a view and/or entities to fit, not both",
            ));
        }
        Some(c) => c.resolve(session.project().manifest.default_unit)?,
        None if view.is_some() && normal_to.is_some() => {
            return Err(Error::new(
                Code::Usage,
                "give a view or a face to look normal to, not both",
            ));
        }
        None => {
            let me = live.get(&id)?;
            let (current, focus) = (me.view.camera.clone(), me.view.focus.clone());
            if let Some(v) = view.as_deref().filter(|v| StandardView::named(v).is_none()) {
                let at = crate::views::part_for(session, part.as_deref(), focus.as_ref())?;
                restore = Some(crate::views::named_saved(session, at.as_ref(), v)?);
            }
            framed(
                session,
                current.as_ref(),
                Look {
                    view: view.as_deref(),
                    normal_to: normal_to.as_deref(),
                    part: part.as_deref(),
                    focus: focus.as_ref(),
                },
                &fit,
            )?
        }
    };
    live.get_mut(&id)?.view.camera = Some(camera.clone());
    if let Some(saved) = restore {
        live.set_sections(&id, saved.sections)?;
        live.set_explode(&id, saved.explode)?;
    }
    let v = &live.get(&id)?.view;
    Ok(json!({"participant": id, "camera": camera, "sections": v.sections, "explode": v.explode}))
}

/// `view.save`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn view_save(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    part: String,
    view: String,
    camera: Option<CameraArg>,
    participant: Option<String>,
    expected_revision: Option<String>,
) -> Result<Value> {
    // A participant's camera comes with its planes and explode.
    let (camera, sections, explode) = match camera {
        Some(c) => (
            c.resolve(session.project().manifest.default_unit)?,
            Vec::new(),
            None,
        ),
        None => {
            let live = live.ok_or_else(|| live::no_live_session("view.save"))?;
            let who = live.compare_target(participant.as_deref(), me)?;
            let v = &live.get(&who)?.view;
            let camera = v.camera.clone().ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!("participant {who} has no camera to save"),
                )
                .with("participant", &who)
            })?;
            (camera, v.sections.clone(), v.explode.clone())
        }
    };
    crate::views::save(
        session,
        &part,
        &view,
        &camera,
        &sections,
        explode.as_ref(),
        expected_revision.as_deref(),
    )
}

/// `view.focus`.
pub(crate) fn view_focus(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    part: Option<String>,
    project: bool,
    participant: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("view.focus"))?;
    let part = match (part, project) {
        (Some(p), false) => {
            let id = part_id(&p)?;
            session.project().part(&id)?;
            Some(Some(id))
        }
        (None, true) => Some(None),
        (None, false) => None,
        (Some(_), true) => {
            return Err(Error::new(
                Code::Usage,
                "give a part to show, or project, not both",
            ));
        }
    };
    let id = live.compare_target(participant.as_deref(), me)?;
    if let Some(focus) = part {
        live.set_focus(&id, focus)?;
    }
    let focus = live.get(&id)?.view.focus.clone();
    let shows = match &focus {
        Some(p) => vec![p.clone()],
        None => crate::assembly::shown_parts(session.project()),
    };
    Ok(json!({"participant": id, "focus": focus, "shows": shows}))
}

/// `visibility.set`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn visibility_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    hide: Vec<String>,
    show: Vec<String>,
    isolate: Vec<String>,
    all: bool,
    participant: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("visibility.set"))?;
    let id = live.compare_target(participant.as_deref(), me)?;
    let change = crate::visibility::Change {
        all,
        isolate,
        show,
        hide,
    };
    let hidden = crate::visibility::apply(session, &live.get(&id)?.view.hidden, &change)?;
    live.set_hidden(&id, hidden)?;
    Ok(json!({"participant": id, "hidden": live.get(&id)?.view.hidden}))
}

/// `section.get`.
pub(crate) fn section_get(
    live: Option<&mut Live>,
    me: Option<&str>,
    participant: Option<String>,
) -> Result<Value> {
    match live {
        Some(l) => {
            let id = l.view_target(participant.as_deref(), me)?;
            Ok(json!({"participant": id, "sections": l.get(&id)?.view.sections}))
        }
        None => Ok(json!({"participant": null, "sections": []})),
    }
}

/// `section.set`.
pub(crate) fn section_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    planes: Vec<crate::section::PlaneArg>,
    add: bool,
    off: bool,
    participant: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("section.set"))?;
    if off && (add || !planes.is_empty()) {
        return Err(Error::new(
            Code::Usage,
            "give planes to cut with, or off, not both",
        ));
    }
    if !off && planes.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "give a plane to cut with, or off to cut with none",
        ));
    }
    let id = live.compare_target(participant.as_deref(), me)?;
    let new = crate::section::resolve_all(session, &planes, "/planes")?;
    let sections = crate::section::apply(&live.get(&id)?.view.sections, new, add)?;
    live.set_sections(&id, sections)?;
    Ok(json!({"participant": id, "sections": live.get(&id)?.view.sections}))
}

/// `highlight.set`.
pub(crate) fn highlight_set(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    entities: Vec<String>,
    style: Style,
    label: Option<String>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("highlight.set"))?;
    let refs = entity::parse_all(session, &entities)?;
    let id = live.caller(me);
    let highlights = live.highlight(&id, style, label, refs)?;
    Ok(json!({"participant": id, "highlights": highlights}))
}

/// `highlight.clear`.
pub(crate) fn highlight_clear(
    live: Option<&mut Live>,
    me: Option<&str>,
    style: Option<Style>,
) -> Result<Value> {
    let live = live.ok_or_else(|| live::no_live_session("highlight.clear"))?;
    let id = live.caller(me);
    let highlights = live.clear_highlights(&id, style)?;
    Ok(json!({"participant": id, "highlights": highlights}))
}

/// `mesh.get` with `flat`: a sheet metal solid's flat pattern.
pub(crate) fn mesh_get_flat(
    session: &mut Session,
    part: String,
    feature: Option<String>,
    features: Vec<String>,
    quality: Option<String>,
    rev: Option<String>,
    solid: String,
) -> Result<Value> {
    flat_alone(feature.as_deref(), &features, quality.as_deref())?;
    versioned(mesh_flat_json(session, &part, &solid, rev.as_deref()))
}

/// `mesh.get`: a part's or an assembly's display meshes.
pub(crate) fn mesh_get_request(
    session: &mut Session,
    part: String,
    feature: Option<String>,
    features: Vec<String>,
    quality: Option<String>,
    rev: Option<String>,
    before: Option<String>,
) -> Result<Value> {
    let (commit, s) = session.at(rev.as_deref())?;
    if let Some(reader) = before {
        return versioned(noted(
            mesh_before_json(s, &part, feature.as_deref(), &reader, quality.as_deref()),
            commit,
        ));
    }
    let only = wanted(feature.as_deref(), &features);
    versioned(noted(
        mesh_get(s, &part, only.as_ref(), quality.as_deref()),
        commit,
    ))
}
