//! Requests on features: add, edit, delete, move, list, show, and each type's fields and report.

use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::project::{Part, feature_path};
use crate::error::{Code, Error, Result};
use crate::feature::check_sweep;
use crate::feature::delete_body::check_delete;
use crate::feature::import::import_not_written_here;
use crate::feature::insert::check_insert;
use crate::feature::instance::check_instance;
use crate::feature::report as feature_report;
use crate::feature::{Given, Place, Write};
use crate::mesh;
use crate::param;
use crate::part::{
    check_revision, feature, feature_id, insert_at, no_feature, order, part_id, part_kind, proposed,
};
use crate::proto::Position;
use crate::rebuild;
use crate::save::save;
use crate::session::Session;
use crate::solid::Solids;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Refuses a write that would leave a feature acting on what isn't a
/// solid before it in `q` (the part as the write leaves it): the written
/// features, those that name them, and those that name a written
/// boolean's tools.
fn check_solids(q: &Part, written: &[&Id]) -> Result<()> {
    let solids = Solids::of(q);
    let mut named: Vec<&Id> = written.to_vec();
    for w in written {
        if let Some(Ok(Feature::Boolean(b))) = q.features.get(*w) {
            named.extend(&b.tools);
        }
        if let Some(Ok(f)) = q.features.get(*w) {
            named.extend(f.deletes());
        }
    }
    for id in q.order() {
        let Some(Ok(f)) = q.features.get(id) else {
            continue;
        };
        let targets = f.targets();
        let concerned = written.contains(&id)
            || targets.iter().any(|t| {
                named.contains(t) || written.iter().any(|w| w.as_str() == t.founder_str())
            });
        if targets.is_empty() || !concerned {
            continue;
        }
        crate::graph::check_order(q, q.order(), id, f)?;
        for t in targets {
            if let Some(by) = solids.consumed_by(t, id) {
                return Err(rebuild::consumed(id, t, by));
            }
            if let Some(by) = solids.deleted_by(t, id) {
                return Err(rebuild::deleted(id, t, by));
            }
            if !solids.is_solid(t) {
                return Err(rebuild::not_a_solid(q, &solids, id, "targets", t));
            }
        }
    }
    Ok(())
}

/// Refuses a feature whose references don't resolve to what they must
/// now, in the part's current rebuild; `order` is the list as the write
/// leaves it. Gives the `reference_followed` warnings of those that
/// follow a face or edge a later feature split or merged (design 447).
fn check_references(
    session: &mut Session,
    part: &Id,
    order: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<Vec<Value>> {
    // Nothing may change a reference import's body, whether or not the
    // feature references anything (a cut, a boolean). Only an import in
    // reference mode makes one, so a part without one isn't rebuilt here.
    if (!f.targets().is_empty() || f.source().is_some())
        && session.project().part(part)?.features.values().any(|g| {
            g.as_ref()
                .ok()
                .and_then(|g| g.import())
                .is_some_and(|i| i.mode() == crate::doc::import::Mode::Reference)
        })
    {
        let p = session.project().part(part)?.clone();
        let results = session.rebuilt(part)?;
        rebuild::check_not_reference(&proposed(&p, order, &[(id, f)]), id, f, results)?;
    }
    // A loft's sections are matched against the rebuild whatever they
    // reference, and what a feature acts on must be a solid (ryucad 18).
    // Only a feature whose file says so makes a sheet, so a part holding
    // none needs no rebuild to judge a feature that wants solids; one that
    // wants a sheet there names a solid, which the rebuild must refuse.
    let may_hold_sheets = session
        .project()
        .part(part)?
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    let read = crate::kind::bodies_read(f);
    let reads_bodies = !read.is_empty()
        && (may_hold_sheets
            || read
                .iter()
                .any(|r| r.want == Some(crate::kind::Want::Sheet)));
    // A kind may judge a feature naming nothing against the rebuild: a
    // loft's sections, an unfold's allowance and a refold's unfold, a
    // planar face's profile, a split by a body.
    if !crate::feature::kind_of(f).always_checked(f)
        && f.references().is_empty()
        && f.geometry().is_empty()
        && !reads_bodies
    {
        return Ok(Vec::new());
    }
    let p = session.project().part(part)?.clone();
    let results = session.rebuilt(part)?;
    let q = proposed(&p, order, &[(id, f)]);
    let checked = crate::kind::check(&q, order, id, f, results)
        .and_then(|()| check_resolved(&p, order, id, f, results));
    let followed = match &checked {
        Ok(()) => crate::reference::followed_by(&q, &Solids::of_order(&q, order), id, f, results),
        Err(_) => Vec::new(),
    };
    checked.map_err(|e| session.explain_in(part, order, e))?;
    Ok(followed)
}

/// [`check_references`] against `results`: the kind's own checks.
fn check_resolved(
    p: &Part,
    order: &[Id],
    id: &Id,
    f: &Feature,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    let cx = crate::feature::Check {
        part: p,
        order,
        id,
        results,
    };
    crate::feature::kind_of(f).check(&cx, f)
}

/// A dry run's answer: the feature as it would be, sizes and all, with
/// nothing written.
fn dry(session: &Session, pid: &Id, fid: &Id, f: &Feature) -> Result<Value> {
    let p = session.project().part(pid)?;
    let unit = session.project().manifest.default_unit;
    Ok(json!({"dry_run": true, "feature": feature_report(p, unit, fid, f)}))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn feature_add(
    session: &mut Session,
    part: &str,
    ty: &str,
    given: Given,
    id: Option<&str>,
    position: &Position,
    expected: Option<&str>,
    dry_run: bool,
) -> Result<Value> {
    let pid = part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let env = p.env.clone();
    let at = insert_at(&list, position)?;
    let kind = crate::feature::kind(ty);
    // A feature its part can't hold is refused before its fields are read.
    if kind.is_some() {
        let fid = match id {
            Some(id) => feature_id(id)?,
            None => {
                let taken: BTreeSet<&str> = p.features.keys().map(Id::as_str).collect();
                Id::generate(ty, |s| taken.contains(s))
            }
        };
        if let Some(e) = crate::assembly::kind_error_of(p, &fid, ty) {
            return Err(e);
        }
    }
    let mut cx = Write {
        session: &mut *session,
        part: &pid,
        unit,
        env: &env,
        given,
        place: Place::New {
            order: &list,
            at,
            id,
        },
        read: None,
        took: None,
    };
    // Each step in the order it has always had: what the kind fills in
    // first, the shared target default, what it reads from other fields
    // and its own target default, then its fields.
    if let Some(k) = kind {
        k.before_targets(ty, &mut cx)?;
    }
    crate::feature::one_target(kind, ty, &mut cx)?;
    if let Some(k) = kind {
        k.take_given(ty, &mut cx.given)?;
        k.default_target(ty, &mut cx)?;
    }
    let Some(k) = kind else {
        return Err(crate::feature::unknown_type(ty));
    };
    k.refuse_others(ty, &cx.given)?;
    let f = k.from_given(ty, None, &mut cx)?;
    let took = cx.took.take();
    // Its borrows of the session and the list end here.
    drop(cx);
    let p = session.project().part(&pid)?;
    let fid = match id {
        Some(id) => feature_id(id)?,
        None => {
            // A set, not a scan per candidate: in a list of thousands the
            // first free number is thousands of candidates in (issue #25).
            let taken: BTreeSet<&str> = p.features.keys().map(Id::as_str).collect();
            Id::generate(f.type_name(), |s| taken.contains(s))
        }
    };
    if p.features.contains_key(&fid) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("part {pid} already has a feature {fid}"),
        )
        .with("feature", &fid));
    }
    list.insert(at, fid.clone());
    if let Some(e) = crate::assembly::kind_error(p, &fid, &f) {
        return Err(e);
    }
    if f.instance().is_some() {
        check_instance(session, &pid, &list, &fid, &f)?;
    }
    if f.insert().is_some() {
        check_insert(session, &pid, &list, &fid, &f)?;
    }
    check_delete(session, &pid, &list, &fid, &f)?;
    // What its references follow: a split face or edge (design 447).
    let mut followed = Vec::new();
    if !f.connectors().is_empty() || f.instance_pattern().is_some() {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
        let r = session.rebuilt(&pid)?;
        followed.extend(crate::mate::followed(&pid, &fid, &f, r));
    }
    let p = session.project().part(&pid)?;
    if !f.profiles().is_empty() {
        check_sweep(p, &list, &fid, &f)?;
    }
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    followed.extend(check_references(session, &pid, &list, &fid, &f)?);
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    if dry_run {
        return dry(session, &pid, &fid, &f);
    }
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, session.project().manifest.default_unit, &fid, &f), "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    if !followed.is_empty() {
        v["warnings"] = json!(followed);
    }
    if let Some(t) = took {
        v["took"] = t;
    }
    Ok(v)
}

/// A feature with `expressions` set by JSON pointer, each one of its
/// expression fields, given alone; a sketch's dimensions are solved, so
/// they are set through `sketch.edit`.
fn edited_at(
    old: &Feature,
    given: &Given,
    expressions: &Map<String, Value>,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Feature> {
    if let Some(name) = given.present().into_iter().next() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("give expressions by pointer alone, without {name}"),
        )
        .with("param", name));
    }
    if old.sketch().is_some() {
        return Err(Error::new(
            Code::InvalidParam,
            "a sketch's dimensions are solved: set them with sketch.edit",
        ));
    }
    if let Feature::Import(_) = old {
        return Err(import_not_written_here("edit"));
    }
    let mut v = old.to_json();
    let fields: Vec<String> = crate::doc::files::expression_fields(&v)
        .into_iter()
        .map(|(at, _)| at)
        .collect();
    for (at, x) in expressions {
        if !fields.contains(at) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{at} is no expression field of this feature"),
            )
            .with("pointer", at)
            .with("fields", &fields));
        }
        let e = crate::doc::files::expression(x).map_err(|e| e.with("pointer", at))?;
        *v.pointer_mut(at).expect("a field it has") = Value::String(e.text().to_owned());
    }
    Feature::from_json(&v, unit, env)
}

pub(crate) fn feature_edit(
    session: &mut Session,
    part: &str,
    id: &str,
    given: Given,
    expressions: &Map<String, Value>,
    expected: Option<&str>,
    dry_run: bool,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let unit = session.project().manifest.default_unit;
    let (old, env) = {
        let p = session.project().part(&pid)?;
        (feature(p, &fid)?.clone(), p.env.clone())
    };
    let ty = old.type_name();
    let kind = crate::feature::kind_of(&old);
    let mut cx = Write {
        session: &mut *session,
        part: &pid,
        unit,
        env: &env,
        given,
        place: Place::Edit { id: &fid },
        read: None,
        took: None,
    };
    // An insert's edit may look up its source's newest label: read it
    // before the part is borrowed for the rest.
    if expressions.is_empty() {
        kind.before_revision(ty, &old, &mut cx)?;
    }
    let p = cx.session.project().part(&pid)?;
    check_revision(p, expected)?;
    let list = order(p)?;
    let f = if !expressions.is_empty() {
        edited_at(&old, &cx.given, expressions, unit, &env).map_err(|e| e.with("feature", &fid))?
    } else {
        kind.take_given(ty, &mut cx.given)?;
        kind.refuse_others(ty, &cx.given)?;
        kind.from_given(ty, Some(&old), &mut cx)?
    };
    // Its borrows of the session end here.
    drop(cx);
    let p = session.project().part(&pid)?;
    // A solid's founder that stops founding it leaves what acts on it, or
    // refers to it, with nothing.
    let founded = |f: &Feature| f.founds();
    if founded(&old) && !founded(&f) {
        let users = crate::graph::dependents(p, &fid);
        if !users.is_empty() {
            return Err(Error::new(
                Code::HasDependents,
                format!(
                    "{fid} founds a solid that {} act on or refer to; change those first",
                    users.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
                ),
            )
            .with("feature", &fid)
            .with("features", users));
        }
    }
    if f.instance().is_some() && f != old {
        check_instance(session, &pid, &list, &fid, &f)?;
    }
    if f.insert().is_some() && f != old {
        check_insert(session, &pid, &list, &fid, &f)?;
    }
    check_delete(session, &pid, &list, &fid, &f)?;
    // What its references follow: a split face or edge (design 447).
    let mut followed = Vec::new();
    if (!f.connectors().is_empty() || f.instance_pattern().is_some()) && f != old {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
        let r = session.rebuilt(&pid)?;
        followed.extend(crate::mate::followed(&pid, &fid, &f, r));
    }
    let p = session.project().part(&pid)?;
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    // References already there were checked when they were written; the
    // kind says what else of an edit is judged again (a blend's
    // propagation, a sweep's twist and scale, a loft's sections, a whole
    // body's or tweak's target, any change of sheet metal or a surface).
    if f.references() != old.references()
        || f.geometry() != old.geometry()
        || f.source() != old.source()
        || kind.rechecks(&old, &f)
    {
        followed.extend(check_references(session, &pid, &list, &fid, &f)?);
    }
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    if dry_run {
        return dry(session, &pid, &fid, &f);
    }
    let before = crate::sketch::current(session, &pid)?;
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let sketches = crate::sketch::keep_current(session, &pid, &before)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    // Each assembly the edit reaches whose mates fail now, whether the
    // edit broke them or drives an assembly they already left half
    // solved (issue #155).
    let reached = crate::mate::reached(session.project(), &pid);
    let mut warnings = crate::mate::failing_warnings(session, &reached, "");
    warnings.extend(followed);
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, session.project().manifest.default_unit, &fid, &f), "revision": revision});
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
}

pub(crate) fn feature_delete(
    session: &mut Session,
    part: &str,
    id: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    // A broken feature file can still be deleted.
    if !p.features.contains_key(&fid) {
        return Err(no_feature(p, &fid));
    }
    let users = crate::graph::dependents(p, &fid);
    if !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{fid} is read by {}; delete or change those first",
                users.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
            ),
        )
        .with("feature", &fid)
        .with("features", users));
    }
    let mut list = order(p)?;
    list.retain(|f| *f != fid);
    if part_kind(p) == PartKind::Assembly {
        let q = proposed(p, &list, &[]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    let assemblies = crate::mate::current(session, &pid)?;
    // The list first: a crash then leaves an unlisted file, never a listed
    // feature with no file.
    save(session, &pid, &[], list)?;
    let path = feature_path(session.root(), &pid, &fid);
    session.remove_file(&path)?;
    session.reload()?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let revision = &session.project().part(&pid)?.revision;
    let mut v = json!({"deleted": fid, "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

pub(crate) fn feature_move(
    session: &mut Session,
    part: &str,
    id: &str,
    position: &Position,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let from = list
        .iter()
        .position(|f| *f == fid)
        .ok_or_else(|| no_feature(p, &fid))?;
    if position.after.is_none() && !position.first {
        return Err(Error::new(Code::Usage, "give after or first"));
    }
    if position.after.as_deref() == Some(fid.as_str()) {
        return Err(Error::new(Code::Usage, "a feature can't move after itself"));
    }
    list.remove(from);
    list.insert(insert_at(&list, position)?, fid.clone());
    // Everything must still come after what it reads, and act only on
    // solids not yet consumed.
    for f in &list {
        if let Some(Ok(feature)) = p.features.get(f)
            && (*f == fid
                || crate::graph::inputs(feature)
                    .iter()
                    .any(|i| i.founder_str() == fid.as_str()))
        {
            crate::graph::check_order(p, &list, f, feature)?;
        }
    }
    check_solids(&proposed(p, &list, &[]), &[&fid])?;
    let revision = save(session, &pid, &[], list.clone())?;
    Ok(json!({"features": list, "revision": revision}))
}

/// `feature.list`: each feature as listed, `ok` when it builds (issue
/// #123: a file that loads may still fail to build), with the rebuild's
/// error where it doesn't and `blocked` where only a feature it needs
/// failed. A part that can't be rebuilt at all lists what loads.
pub(crate) fn feature_list(session: &mut Session, part: &str) -> Result<Value> {
    let pid = part_id(part)?;
    let built: BTreeMap<Id, std::result::Result<(), Error>> = match session.rebuilt(&pid) {
        Ok(r) => r
            .features
            .iter()
            .map(|(id, o)| (id.clone(), o.as_ref().map(|_| ()).map_err(Error::clone)))
            .collect(),
        Err(_) => BTreeMap::new(),
    };
    let p = session.project().part(&pid)?;
    let features: Vec<Value> = order(p)?
        .iter()
        .map(|id| match feature(p, id) {
            Ok(f) => {
                let mut v = json!({"id": id, "type": f.type_name(), "ok": true});
                if let Some(i) = f.insert() {
                    let r = crate::insert::report(i, None);
                    v["of"] = r["of"].clone();
                    v["rev"] = r["rev"].clone();
                }
                if let Some(Err(e)) = built.get(id) {
                    v["ok"] = json!(false);
                    v["error"] = json!(e);
                    if e.code == Code::DependencyFailed {
                        v["blocked"] = json!(true);
                    }
                }
                // A solid key below an insert is used through the insert.
                let mut uses: Vec<Id> = Vec::new();
                for i in crate::graph::inputs(f) {
                    let i = i.founder();
                    if !uses.contains(&i) {
                        uses.push(i);
                    }
                }
                if !uses.is_empty() {
                    v["uses"] = json!(uses);
                }
                v
            }
            Err(e) => {
                let mut v = json!({"id": id, "ok": false, "error": e});
                if let Some(t) = e.details.get("type").filter(|t| t.is_string()) {
                    v["type"] = t.clone();
                }
                v
            }
        })
        .collect();
    Ok(json!({"part": p.id, "features": features, "orphans": p.orphans(), "revision": p.revision}))
}

/// `param.list`.
pub(crate) fn param_list(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => param::list(session, &p),
        param::Target::Project => param::list_project(session),
    }
}

/// `param.set`.
pub(crate) fn param_set(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    parameters: Map<String, Value>,
    expected_revision: Option<String>,
) -> Result<Value> {
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => param::set(session, &p, parameters, expected_revision.as_deref()),
        param::Target::Project => {
            param::set_project(session, parameters, expected_revision.as_deref())
        }
    }
}

/// `param.delete`.
pub(crate) fn param_delete(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    name: String,
    expected_revision: Option<String>,
) -> Result<Value> {
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => param::delete(session, &p, &name, expected_revision.as_deref()),
        param::Target::Project => {
            param::delete_project(session, &name, expected_revision.as_deref())
        }
    }
}

/// `param.rename`.
pub(crate) fn param_rename(
    session: &mut Session,
    part: Option<String>,
    scope: Option<crate::param::Scope>,
    from: String,
    to: String,
    expected_revision: Option<String>,
) -> Result<Value> {
    match param::Target::of(part, scope)? {
        param::Target::Part(p) => {
            param::rename(session, &p, &from, &to, expected_revision.as_deref())
        }
        param::Target::Project => {
            param::rename_project(session, &from, &to, expected_revision.as_deref())
        }
    }
}

/// `sketch.get`.
pub(crate) fn sketch_get(
    session: &mut Session,
    part: String,
    sketch: String,
    quality: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let quality = match quality {
        Some(q) => mesh::Quality::parse(&q)?,
        None => mesh::Quality::preferred(),
    };
    crate::sketch::get(session, &part, &sketch, quality)
}

/// `path.get`.
pub(crate) fn path_get(
    session: &mut Session,
    part: String,
    path: String,
    quality: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let quality = match quality {
        Some(q) => mesh::Quality::parse(&q)?,
        None => mesh::Quality::preferred(),
    };
    crate::path::get(session, &part, &path, quality)
}

/// `sweep.get`.
pub(crate) fn sweep_get(
    session: &mut Session,
    part: String,
    feature: String,
    quality: Option<String>,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let quality = match quality {
        Some(q) => mesh::Quality::parse(&q)?,
        None => mesh::Quality::preferred(),
    };
    crate::sweep_view::get(session, &part, &feature, quality)
}

/// `feature.show`.
pub(crate) fn feature_show(
    session: &mut Session,
    part: String,
    id: String,
    rev: Option<String>,
) -> Result<Value> {
    let (_, session) = session.at(rev.as_deref())?;
    let (part, fid) = (part_id(&part)?, feature_id(&id)?);
    let p = session.project().part(&part)?;
    let unit = session.project().manifest.default_unit;
    let mut v = feature_report(p, unit, &fid, feature(p, &fid)?);
    // The revision it was read at: an edit made from it says so.
    v["revision"] = json!(p.revision);
    Ok(v)
}
