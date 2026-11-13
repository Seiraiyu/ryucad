//! A project's parts and features as requests name them: IDs parsed,
//! revisions checked, a part's feature list and kind, a write's
//! proposed part, and a part's rebuild report. Moved out of `server`
//! (ryucad 27.B) so the modules that answer requests don't import it.

use crate::doc::files::{Feature, PartFile, PartKind};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sketch::{Plane, Sketch};
use crate::error::{Code, Error, Result};
use crate::proto::Position;
use crate::session::Session;
use crate::solid::Solids;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn part_id(s: &str) -> Result<Id> {
    Id::parse(s, "part")
}

/// Refuses to write when the agent's view of the part is out of date.
pub(crate) fn check_revision(part: &Part, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(e) if e != part.revision => Err(Error::new(
            Code::Conflict,
            format!(
                "part {} changed since revision {e}; read it again and retry",
                part.id
            ),
        )
        .with("part", &part.id)
        .with("expected", e)
        .with("actual", &part.revision)),
        _ => Ok(()),
    }
}

pub(crate) fn feature_id(s: &str) -> Result<Id> {
    Id::parse(s, "feature")
}

/// The part's feature list, or why it can't be used.
pub(crate) fn order(part: &Part) -> Result<Vec<Id>> {
    part.file
        .as_ref()
        .map(|f| f.features.clone())
        .map_err(Clone::clone)
}

/// A feature of `part`, or the error that keeps it from loading.
pub(crate) fn feature<'a>(part: &'a Part, id: &Id) -> Result<&'a Feature> {
    match part.features.get(id) {
        Some(Ok(f)) => Ok(f),
        Some(Err(e)) => Err(e.clone()),
        None => Err(no_feature(part, id)),
    }
}

pub(crate) fn no_feature(part: &Part, id: &Id) -> Error {
    Error::new(
        Code::NotFound,
        format!("no feature {id} in part {}", part.id),
    )
    .with("part", &part.id)
    .with("feature", id)
}

/// Rewrites a sketch's references in `v`, its fields as a file has them,
/// as `<part>/<feature>/<name>`.
pub(crate) fn reference_texts(part: &Id, s: &Sketch, v: &mut Value) {
    if let Plane::Datum(g) = &s.plane {
        v["plane"] = json!(g.text(part));
    }
    if let Plane::Face(on) = &s.plane {
        v["face"] = json!(on.face.text(part));
        if let Some(r) = &on.origin {
            v["origin"] = json!(r.text(part));
        }
        if let Some(r) = &on.x {
            v["x"] = json!(r.text(part));
        }
    }
    for (id, e) in &s.entities {
        if let Some(r) = &e.projected {
            v["entities"][id.as_str()]["projected"] = json!(r.text(part));
        }
    }
}

/// Where `position` puts a feature in `order` (which doesn't hold it).
pub(crate) fn insert_at(order: &[Id], position: &Position) -> Result<usize> {
    match (&position.after, position.first) {
        (Some(_), true) => Err(Error::new(Code::Usage, "give after or first, not both")),
        (None, true) => Ok(0),
        (None, false) => Ok(order.len()),
        (Some(after), false) => {
            let after = feature_id(after)?;
            order
                .iter()
                .position(|f| *f == after)
                .map(|i| i + 1)
                .ok_or_else(|| {
                    Error::new(
                        Code::NotFound,
                        format!("no feature {after} to insert after"),
                    )
                    .with("feature", &after)
                })
        }
    }
}

/// A connector's text, as requests give it.
pub fn connector_text(asm: &Id, c: &crate::doc::mate::Connector) -> String {
    let path = c.path().join("/");
    match &c.at {
        crate::doc::mate::At::Origin => format!("{asm}/{path}"),
        crate::doc::mate::At::Entity(r) => format!("{asm}/{path}/{}/{}", r.feature, r.name),
        crate::doc::mate::At::Datum(d) => format!("{asm}/{path}/{d}"),
    }
}

/// The part as a write would leave it: `writes` in place, listed as
/// `order`.
pub(crate) fn proposed(p: &Part, order: &[Id], writes: &[(&Id, &Feature)]) -> Part {
    let mut q = p.clone();
    for (id, f) in writes {
        q.features.insert((*id).clone(), Ok((*f).clone()));
    }
    q.file = Ok(PartFile {
        kind: part_kind(p),
        features: order.to_vec(),
        items: p.file.as_ref().map(|f| f.items.clone()).unwrap_or_default(),
    });
    q
}

/// Whether `part` is an assembly, as its part file says; a part whose
/// file can't be read is taken for a part.
pub(crate) fn part_kind(part: &Part) -> PartKind {
    part.file.as_ref().map_or(PartKind::Part, |f| f.kind)
}

/// The listed features that sweep `sketch` or drill at its points.
pub(crate) fn sweeps_of(part: &Part, sketch: &Id) -> Vec<Id> {
    part.order()
        .iter()
        .filter(|f| {
            part.features
                .get(*f)
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|f| f.profiles().contains(&sketch))
        })
        .cloned()
        .collect()
}

/// A body's kind in a report: `kind`, and for a sheet how many
/// `boundaries` it has and whether it is `closed`.
fn kinds(v: &mut Value, b: &ryukei::topo::Body) {
    v["kind"] = json!(crate::kind::name(b));
    if b.kind() == ryukei::topo::BodyKind::Sheet {
        let n = crate::kind::boundaries(b);
        v["boundaries"] = json!(n);
        v["closed"] = json!(n == 0);
    }
}

/// A part's rebuild as agents and subscribers see it: each feature's
/// status, hash and counts. With `occurrences`, an assembly's occurrences
/// come apart, as the JSON text of the report's `occurrences` (#533: the
/// root's 100,000 took about 450 MiB as a `Value` tree), for
/// [`object_text`] to write into the report's text.
pub(crate) fn part_report(
    session: &mut Session,
    id: &Id,
    occurrences: bool,
) -> Result<(Value, Option<String>)> {
    let unit = session.project().manifest.default_unit;
    let part = session.project().part(id)?;
    let revision = part.revision.clone();
    let assembly = part_kind(part) == PartKind::Assembly;
    let solids = Solids::of(part);
    // A feature that doesn't load may still say what it is: a conflict
    // record's type where the merge left it alone.
    let types: BTreeMap<Id, Value> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let ty = match f {
                Ok(f) => json!(f.type_name()),
                Err(e) => e.details.get("type").filter(|t| t.is_string())?.clone(),
            };
            Some((fid.clone(), ty))
        })
        .collect();
    // What each feature reads, for showing dependencies.
    let uses: BTreeMap<Id, Vec<Id>> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let mut ins: Vec<Id> = Vec::new();
            for i in crate::graph::inputs(f.as_ref().ok()?) {
                let i = i.founder();
                if !ins.contains(&i) {
                    ins.push(i);
                }
            }
            (!ins.is_empty()).then(|| (fid.clone(), ins))
        })
        .collect();
    // What each feature does to solids, and the bodies it consumes (a
    // boolean's tools, a sew's sheets, a thicken's sheet): for showing the
    // chains.
    let ops: BTreeMap<Id, (&'static str, Vec<Id>)> = part
        .features
        .iter()
        .filter_map(|(fid, f)| match f.as_ref().ok()? {
            Feature::Boolean(b) => Some((fid.clone(), (b.op.as_str(), b.tools.clone()))),
            Feature::Hole(_) => Some((fid.clone(), ("cut", Vec::new()))),
            // A pattern does what its seeds do.
            Feature::Pattern(x) => {
                let seed = part.features.get(x.features.first()?)?.as_ref().ok()?;
                let op = match seed {
                    Feature::Hole(_) => "cut",
                    f => f.combine()?.operation.as_str(),
                };
                Some((fid.clone(), (op, Vec::new())))
            }
            f => {
                let consumes: Vec<Id> = f.consumes().into_iter().cloned().collect();
                match f.combine() {
                    Some(c) if !c.is_new() => Some((fid.clone(), (c.operation.as_str(), consumes))),
                    _ if !consumes.is_empty() => Some((fid.clone(), ("new", consumes))),
                    _ => None,
                }
            }
        })
        .collect();
    // A pattern's form (`linear`, `circular` or `mirror`), for the
    // window's icons, whatever it rebuilt to.
    let forms: BTreeMap<Id, &'static str> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            use crate::doc::mate::PatternForm;
            use crate::doc::pattern::Kind;
            let form = match f.as_ref().ok()? {
                Feature::Pattern(p) => match p.kind {
                    Kind::Linear { .. } => "linear",
                    Kind::Circular { .. } => "circular",
                    Kind::Mirror { .. } => "mirror",
                },
                Feature::InstancePattern(p) => match p.form {
                    PatternForm::Linear { .. } => "linear",
                    PatternForm::Circular { .. } => "circular",
                },
                _ => return None,
            };
            Some((fid.clone(), form))
        })
        .collect();
    // A mate's form and connectors, as requests give them, for showing it
    // whatever it rebuilt to.
    let mates: BTreeMap<Id, Value> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let m = f.as_ref().ok()?.mate()?;
            Some((
                fid.clone(),
                json!({"form": m.form.as_str(), "a": connector_text(id, &m.a),
                       "b": connector_text(id, &m.b)}),
            ))
        })
        .collect();
    let counts = |b: &crate::rebuild::Built| {
        let c = b.body.counts();
        json!({"faces": c.faces, "edges": c.edges, "vertices": c.vertices})
    };
    // A shell's or offset's fields as resolved, shown whether it built or
    // not.
    let bodied: BTreeMap<Id, (&'static str, Value)> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let f = f.as_ref().ok()?;
            let report = match f {
                Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
                    crate::body::report(id, f)
                }
                Feature::BaseFlange(_)
                | Feature::Flange(_)
                | Feature::Unfold(_)
                | Feature::Refold(_) => crate::sheet_metal::report(part, f),
                // A fill's rebuild report is what it measured (see
                // `Made::report`), an extract's its faces.
                Feature::Extract(_) => crate::surface::report(id, f),
                Feature::MoveFace(_)
                | Feature::ReplaceFace(_)
                | Feature::Draft(_)
                | Feature::DeleteFace(_)
                | Feature::DeleteBlend(_)
                | Feature::ResizeBlend(_) => crate::tweak::report(id, f),
                _ => return None,
            };
            Some((fid.clone(), (f.type_name(), report)))
        })
        .collect();
    // An import's file, hash, product and body, as written: shown
    // whether it built or not.
    let imports: BTreeMap<Id, Value> = part
        .features
        .iter()
        .filter_map(|(fid, f)| match f.as_ref().ok()? {
            Feature::Import(i) => Some((fid.clone(), crate::import::written(i))),
            _ => None,
        })
        .collect();
    // An insert's source, pin and solids (design 521): shown whether it
    // built or not.
    let inserts: BTreeMap<Id, crate::doc::insert::Insert> = part
        .features
        .iter()
        .filter_map(|(fid, f)| Some((fid.clone(), f.as_ref().ok()?.insert()?.clone())))
        .collect();
    let deletes: BTreeMap<Id, crate::doc::body::DeleteBody> = part
        .features
        .iter()
        .filter_map(|(fid, f)| match f {
            Ok(Feature::DeleteBody(d)) => Some((fid.clone(), (**d).clone())),
            _ => None,
        })
        .collect();
    // A sweep's profile and path, a loft's sections, a mirror's or
    // split's plane, and a tweak's motion, are resolved again from the rebuild for its report: the part, held apart
    // from the session while it rebuilds.
    let held = part
        .features
        .values()
        .any(|f| {
            f.as_ref().is_ok_and(|f| {
                f.sweep().is_some()
                    || f.loft().is_some()
                    || matches!(
                        f,
                        Feature::Mirror(_)
                            | Feature::Split(_)
                            | Feature::MoveFace(_)
                            | Feature::ReplaceFace(_)
                            | Feature::Draft(_)
                            | Feature::DeleteFace(_)
                            | Feature::DeleteBlend(_)
                            | Feature::ResizeBlend(_)
                    )
            })
        })
        .then(|| part.clone());
    // Instances an update kept though their usage left the file.
    let kept = if assembly {
        crate::import::usage_warnings(session.project(), id)
    } else {
        Vec::new()
    };
    // References that follow a face or edge a later feature split or
    // merged (design 447): under their features and the part's warnings.
    session.rebuilt(id)?;
    let followed = match (session.project().parts.get(id), session.cached(id)) {
        (Some(p), Some(r)) => crate::reference::followed(p, r),
        _ => Vec::new(),
    };
    let r = session.rebuilt(id)?;
    let features: Vec<Value> = r
        .features
        .iter()
        .map(|(fid, b)| {
            let mut v = match b {
                Ok(o @ crate::rebuild::Output::Solids(_)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": o.hash()});
                    if let crate::rebuild::Output::Solids(m) = o
                        && let Some(i) = &m.report
                    {
                        // A fill's report under its own type, else an
                        // import's; a tweak's report comes with its own
                        // fields (`tweak::resolved`).
                        let key = match types.get(fid).and_then(Value::as_str) {
                            Some("fill") => Some("fill"),
                            Some(
                                "draft" | "move_face" | "replace_face" | "delete_face"
                                | "delete_blend" | "resize_blend",
                            ) => None,
                            _ => Some("import"),
                        };
                        if let Some(key) = key {
                            v[key] = i.as_ref().clone();
                        }
                    }
                    if let crate::rebuild::Output::Solids(m) = o
                        && let Some(f) = &m.flat
                    {
                        v["flat"] = crate::sheet_metal::unfold::summary(f);
                    }
                    if let Some(p) = &held
                        && let Some(Ok(Feature::Sweep(w))) = p.features.get(fid)
                        && let Some(d) = crate::along::described(p, fid, w, r)
                    {
                        v["sweep"] = d;
                    }
                    if let Some(p) = &held
                        && let Some(Ok(Feature::Loft(l))) = p.features.get(fid)
                        && let Some(d) = crate::loft::described(p, fid, l, r)
                    {
                        v["loft"] = d;
                    }
                    match o.body() {
                        Some(b) => v["counts"] = counts(b),
                        None => {
                            v["bodies"] = o
                                .solids()
                                .iter()
                                .map(|(s, b)| {
                                    let mut e = json!({"solid": s, "hash": b.hash,
                                                       "counts": counts(b)});
                                    kinds(&mut e, &b.body);
                                    e
                                })
                                .collect();
                        }
                    }
                    v
                }
                Ok(crate::rebuild::Output::Instance(p)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": p.hash, "instance": p.to_json()});
                    // What its mates leave it, when the assembly has any.
                    if let Some(b) = r.solve.as_ref().and_then(|s| s.instance(fid)) {
                        v["instance"]["solve"] = crate::mate::body_json(b);
                    }
                    v
                }
                Ok(crate::rebuild::Output::Mate(m)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": m.hash, "mate": m.to_json(unit)});
                    // A mate with stops says them, for the window's arcs.
                    if let Some(l) = r.solve.as_ref().and_then(|s| s.limits(fid)) {
                        v["mate"]["limits"] = crate::mate::limits_json(&l);
                    }
                    v
                }
                Ok(crate::rebuild::Output::Copies(c)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true, "hash": c.hash,
                           "copies": c.to_json(fid)})
                }
                Ok(crate::rebuild::Output::Datum(d)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true, "hash": d.hash,
                           "datum": d.report()})
                }
                Ok(crate::rebuild::Output::Path(m)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": m.hash});
                    v[m.kind()] = m.report(id);
                    v
                }
                // A sketch makes no body: its state, not a body hash.
                Ok(crate::rebuild::Output::Sketch(s)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true,
                           "sketch": {"state": s.report.state_str(), "dof": s.report.dof,
                                      "stale": s.stale}})
                }
                Err(e) => json!({"id": fid, "type": types.get(fid), "ok": false, "error": e}),
            };
            if let Some(u) = uses.get(fid) {
                v["uses"] = json!(u);
            }
            if let Some(form) = forms.get(fid) {
                v["form"] = json!(form);
            }
            if let Some((k, x)) = bodied.get(fid) {
                v[*k] = x.clone();
                if let Some(Value::Object(extra)) =
                    held.as_ref().filter(|_| v["ok"] == true).and_then(|q| {
                        let f = q.features.get(fid)?.as_ref().ok()?;
                        crate::body::resolved(q, r, fid, f)
                            .or_else(|| crate::tweak::resolved(q, r, fid, f, false, unit))
                    })
                {
                    for (key, x) in extra {
                        v[*k][key] = x;
                    }
                }
            }
            if let Some(d) = deletes.get(fid) {
                v["delete_body"] = crate::body::delete_report(d, r);
            }
            if let Some(i) = inserts.get(fid) {
                let made = match b {
                    Ok(crate::rebuild::Output::Solids(m)) => Some(&**m),
                    _ => None,
                };
                v["insert"] = crate::insert::report(i, made);
            }
            if let Some(Value::Object(m)) = imports.get(fid) {
                if !v["import"].is_object() {
                    v["import"] = json!({});
                }
                for (k, x) in m {
                    v["import"][k] = x.clone();
                }
            }
            if let Some(Value::Object(m)) = mates.get(fid) {
                if !v["mate"].is_object() {
                    v["mate"] = json!({});
                }
                for (k, x) in m {
                    v["mate"][k] = x.clone();
                }
            }
            let of = solids.solids_of(fid);
            if !of.is_empty() {
                v["solids"] = json!(of);
            }
            if let Some((op, tools)) = ops.get(fid) {
                v["operation"] = json!(op);
                if !tools.is_empty() {
                    v["consumes"] = json!(tools);
                }
            }
            // An insert's own (`insert_source_failed`, `insert_obsolete`)
            // go under it too, as a followed reference's do.
            let inserted = match b {
                Ok(o) => o.inserted().map(|i| i.warnings.as_slice()),
                Err(_) => None,
            };
            let own: Vec<&Value> = inserted
                .unwrap_or_default()
                .iter()
                .chain(followed.iter().filter(|w| w["feature"] == fid.as_str()))
                .collect();
            if !own.is_empty() {
                v["warnings"] = json!(own);
            }
            v
        })
        .collect();
    // The model: each solid with its chain, and its body as it shows.
    let shown: Vec<Value> = r
        .shown(&solids)
        .into_iter()
        .map(|s| {
            let mut v = json!({"solid": s.solid, "ok": s.error.is_none(),
                               "features": solids.chain(s.solid)});
            if let Some((f, b)) = s.state {
                v["shown"] = json!(f);
                v["hash"] = json!(b.hash);
                v["counts"] = counts(b);
                kinds(&mut v, &b.body);
                if b.reference {
                    v["reference"] = json!(true);
                }
            }
            if let Some(e) = s.error {
                v["error"] = json!(e);
            }
            v
        })
        .collect();
    let mut v = json!({"part": id, "revision": revision, "hash": r.hash,
                       "failed": r.failed(), "features": features, "solids": shown});
    let mut all = None;
    let mut warnings = r.warnings(id);
    warnings.extend(followed);
    // An assembly's model is what its instances place.
    warnings.extend(kept);
    if assembly {
        v["kind"] = json!("assembly");
        v["occurrence_count"] = json!(crate::assembly::Model::count_of_assembly(r));
        if occurrences {
            let model = crate::assembly::Model::of_assembly(r);
            let mut text = String::from("[");
            for (i, o) in model.occurrences.iter().enumerate() {
                if i > 0 {
                    text.push(',');
                }
                text.push_str(&o.to_json().to_string());
            }
            text.push(']');
            all = Some(text);
        }
        if let Some(solve) = &r.solve {
            v["solve"] = solve.to_json();
        }
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok((v, all))
}
