//! Requests on features: add, edit, delete, move, list, show, and each type's fields and report.

use crate::doc::files::{Feature, PartKind, Primitive, PrimitiveType};
use crate::doc::id::Id;
use crate::doc::project::{Part, feature_path};
use crate::doc::sketch::Sketch;
use crate::doc::solid::Combine;
use crate::doc::sweep::{Angle, Extent, RevolveAxis};
use crate::error::{Code, Error, Result};
use crate::feature::{
    apply_placement, check_sweep, check_up_to, new_placement, solids_before, source_is_sheet,
};
use crate::mesh;
use crate::param;
use crate::part::{
    check_revision, connector_text, feature, feature_id, insert_at, no_feature, order, part_id,
    part_kind, proposed,
};
use crate::proto::Position;
use crate::rebuild;
use crate::save::save;
use crate::session::Session;
use crate::solid::Solids;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
// The old path, for `route.rs` (design 541, "Paths kept").
pub(crate) use crate::feature::Given;
use crate::feature::blend::{CHAMFER, FILLET, blend_from, blend_report};
use crate::feature::boolean::{BOOLEAN, boolean_from};
use crate::feature::delete_body::{DELETE_BODY, check_delete, delete_from};
use crate::feature::extrude::{EXTRUDE, REVOLVE, sweep_from};
use crate::feature::hole::{HOLE, hole_from, hole_report};
use crate::feature::import::import_not_written_here;
use crate::feature::insert::{INSERT, check_insert, insert_from};
use crate::feature::instance::{INSTANCE, check_instance, instance_from};
use crate::feature::instance_pattern::{INSTANCE_PATTERN, instance_pattern_from};
use crate::feature::mate::{MATE, mate_from};
use crate::feature::pattern::{PATTERN, pattern_from, pattern_report};
use crate::feature::planar::planar_from;
use crate::feature::primitive::{combine_from, primitive_report, radius_param, refuse_sizes};
use crate::feature::sketch::{SKETCH, sketch_plane, sketch_report};

/// How a feature appears in output: every quantity as written and in
/// meters, every reference as `<part>/<feature>/<name>`.
pub fn feature_report(p: &Part, id: &Id, f: &Feature) -> Value {
    let part = &p.id;
    let mut v = match f {
        Feature::Primitive(p) => primitive_report(p),
        Feature::Sketch(s) => sketch_report(part, s),
        Feature::Extrude(e) => {
            let mut v = e.fields();
            match &e.extent {
                Extent::Blind(q) | Extent::Symmetric(q) => v["extent"]["length"] = q.report(),
                Extent::ThroughAll { .. } => {}
                Extent::UpTo { target, offset } => {
                    v["extent"]["target"] = json!(target.text(part));
                    if let Some(q) = offset {
                        v["extent"]["offset"] = q.report();
                    }
                }
            }
            v
        }
        Feature::Boolean(b) => b.fields(),
        Feature::MoveFace(_)
        | Feature::ReplaceFace(_)
        | Feature::Draft(_)
        | Feature::DeleteFace(_)
        | Feature::DeleteBlend(_)
        | Feature::ResizeBlend(_) => crate::tweak::report(part, f),
        Feature::Datum(d) => crate::datum::report(part, d),
        Feature::Path(x) => crate::path::report(part, x),
        Feature::Sweep(w) => crate::along::feature_report(part, w),
        Feature::Loft(l) => crate::loft::feature_report(part, l),
        Feature::Planar(x) => {
            let mut v = x.fields();
            if let crate::doc::sweep::Source::Face(g) = &x.profile.source {
                v["profile"]["face"] = json!(g.text(part));
            }
            v
        }
        Feature::Instance(i) => i.fields(),
        Feature::Insert(i) => i.fields(),
        Feature::DeleteBody(d) => d.fields(),
        Feature::Import(i) => i.fields(),
        Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_) => {
            crate::surface::report(part, f)
        }
        Feature::Mate(m) => {
            let mut v = m.fields();
            for (side, c) in [("a", &m.a), ("b", &m.b)] {
                v[side]["text"] = json!(connector_text(part, c));
            }
            if let Some(q) = &m.angle {
                v["angle"] = q.report();
            }
            if let Some(q) = &m.distance {
                v["distance"] = q.report();
            }
            v
        }
        Feature::InstancePattern(x) => {
            let mut v = x.fields();
            for (role, c) in x.connectors() {
                v[role]["text"] = json!(connector_text(part, c));
            }
            v
        }
        Feature::Fillet(_) | Feature::Chamfer(_) => blend_report(part, f),
        Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
            crate::body::report(part, f)
        }
        Feature::BaseFlange(_) | Feature::Flange(_) | Feature::Unfold(_) | Feature::Refold(_) => {
            crate::sheet_metal::report(p, f)
        }
        Feature::Hole(h) => hole_report(part, h),
        Feature::Pattern(x) => pattern_report(part, x),
        Feature::Revolve(r) => {
            let mut v = r.fields();
            if let Angle::Of(q) = &r.angle {
                v["angle"] = q.report();
            }
            if let RevolveAxis::Ref(e) = &r.axis {
                v["axis"] = json!(e.text(part));
            }
            v
        }
    };
    v["id"] = json!(id);
    v["type"] = json!(f.type_name());
    v
}

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
    let planar = matches!(f, Feature::Planar(_));
    // An unfold's allowance and a refold's unfold are judged against the
    // rebuild whatever they reference.
    if f.loft().is_none()
        && f.unfold().is_none()
        && f.refold().is_none()
        && f.references().is_empty()
        && f.geometry().is_empty()
        && !reads_bodies
        && !planar
        && !matches!(f, Feature::Split(s) if s.plane().is_none())
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

/// [`check_references`] against `results`.
fn check_resolved(
    p: &Part,
    order: &[Id],
    id: &Id,
    f: &Feature,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    match f {
        Feature::Sketch(s) => crate::sketch::context(p, order, id, s, results).map(|_| ()),
        // What the target is; how far it is only rebuild can tell.
        Feature::Extrude(e) => match &e.extent {
            Extent::UpTo { target, .. } => check_up_to(p, order, id, "target", target, results),
            _ => Ok(()),
        },
        Feature::Hole(h) => match &h.depth {
            crate::doc::hole::Depth::UpTo { target, .. } => {
                check_up_to(p, order, id, "depth", target, results)
            }
            _ => Ok(()),
        },
        // What its directions, axis and plane are; where the instances
        // land only rebuild can tell.
        Feature::Pattern(x) => {
            let q = proposed(p, order, &[(id, f)]);
            crate::pattern::motions(&q, id, x, results).map(|_| ())
        }
        // Edges of one solid, a face where a face goes, no edge twice; what
        // the kernel makes of them only rebuild can tell.
        Feature::Fillet(_) | Feature::Chamfer(_) => {
            rebuild::check_blend(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // That it is an axis; whether it lies in the sketch's plane only
        // rebuild can tell.
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Ref(g) => {
                let at = crate::geometry::At {
                    part: p,
                    order,
                    owner: id,
                    results,
                };
                crate::geometry::one(&at, "axis", g, crate::geometry::Want::Axis).map(|_| ())
            }
            RevolveAxis::Line(_) => Ok(()),
        },
        // That each reference is the kind its field wants; what they make
        // of each other (parallel, collinear) only rebuild can tell.
        Feature::Datum(d) => crate::datum::check(p, order, id, d, results),
        Feature::Path(x) => crate::path::check(p, order, id, x, results),
        Feature::Sweep(w) => crate::along::check(p, order, id, w, results),
        Feature::Loft(l) => crate::loft::check(p, order, id, l, results),
        // A planar face's profile is closed loops: its open curves are
        // refused when written (what else fails is rebuild's).
        Feature::Planar(x) => match crate::profile::planar(p, id, x, results) {
            Err(e) if e.code == Code::ProfileInvalid => Err(e),
            _ => Ok(()),
        },
        // Faces of its target, each once; what the kernel makes of them
        // only rebuild can tell.
        Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
            crate::body::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // Faces of its target, its direction, axis or surfaces their
        // kinds; what the kernel makes of them only rebuild can tell.
        Feature::MoveFace(_)
        | Feature::ReplaceFace(_)
        | Feature::Draft(_)
        | Feature::DeleteFace(_)
        | Feature::DeleteBlend(_)
        | Feature::ResizeBlend(_) => {
            crate::tweak::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        Feature::BaseFlange(_) | Feature::Flange(_) | Feature::Unfold(_) | Feature::Refold(_) => {
            crate::sheet_metal::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // Its sides edges and curves, its faces of one body; whether they
        // close up, or extract cleanly, only rebuild can tell.
        Feature::Fill(x) => {
            crate::surface::check_fill(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        Feature::Extract(x) => {
            crate::surface::check_extract(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        // Its sheet or faces; what the kernel makes of them only rebuild
        // can tell. A sew's sheets are judged by kind and order already.
        Feature::Thicken(x) => {
            crate::surface::check_thicken(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        _ => Ok(()),
    }
}

/// Every feature type `feature.add` makes.
const FEATURE_TYPES: [&str; 28] = [
    "box",
    "cylinder",
    "cone",
    "sphere",
    "torus",
    "sketch",
    "extrude",
    "revolve",
    "sweep",
    "loft",
    "planar",
    "fill",
    "extract",
    "sew",
    "thicken",
    "boolean",
    "fillet",
    "chamfer",
    "hole",
    "pattern",
    "instance",
    "mate",
    "instance_pattern",
    "insert",
    "delete_body",
    "plane",
    "axis",
    "point",
];

pub(crate) fn feature_add(
    session: &mut Session,
    part: &str,
    ty: &str,
    mut given: Given,
    id: Option<&str>,
    position: &Position,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let env = p.env.clone();
    let at = insert_at(&list, position)?;
    // A feature its part can't hold is refused before its fields are read.
    if FEATURE_TYPES.contains(&ty) {
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
    // A pattern with no targets acts on its first seed's.
    if ty == "pattern" && given.fields.targets.is_none() {
        let seed = given.fields.features.as_ref().and_then(|s| s.first());
        let theirs = seed
            .and_then(|s| Id::parse(s, "feature").ok())
            .and_then(|s| p.features.get(&s).cloned())
            .and_then(|f| f.ok());
        match (seed, theirs) {
            (_, Some(f)) if !f.targets().is_empty() => {
                given.fields.targets = Some(f.targets().iter().map(|t| t.to_string()).collect());
            }
            // Its seed's own fault says what's wrong with it.
            (Some(seed), Some(f)) => {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!(
                        "{}its seed {seed} is a {} that joins or cuts nothing; a pattern repeats joins, cuts and holes",
                        id.map(|i| format!("pattern {i}: ")).unwrap_or_default(),
                        f.type_name()
                    ),
                )
                .with("seed", seed)
                .with("pointer", "/features/0"));
            }
            (Some(seed), None)
                if Id::parse(seed, "feature").is_ok_and(|s| !p.features.contains_key(&s)) =>
            {
                return Err(Error::new(
                    Code::NotFound,
                    format!("a pattern's seed {seed} isn't a feature of part {pid}"),
                )
                .with("seed", seed)
                .with("pointer", "/features/0"));
            }
            _ => {}
        }
    }
    // A mirror whose operation is left out joins its own source (design
    // 327, question 20); one with no solid copies the part's one solid.
    // The file names both all the same.
    if ty == "mirror" && given.fields.solid.is_none() {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.solid = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a mirror names the solid it copies (solid): the part has {} before it",
                        before.len()
                    ),
                )
                .with("param", "solid")
                .with("solids", before));
            }
        }
    }
    // A sheet's copy can't be joined: left out, its operation is new
    // (design 388).
    if ty == "mirror" && given.fields.operation.is_none() {
        let sheet = given
            .fields
            .solid
            .as_deref()
            .and_then(|x| Id::parse(x, "feature").ok())
            .is_some_and(|x| source_is_sheet(session, &pid, &list, at, &x));
        if sheet {
            given.fields.operation = Some("new".into());
        } else {
            given.fields.operation = Some("join".into());
            if given.fields.targets.is_none() {
                given.fields.targets = given.fields.solid.clone().map(|s| vec![s]);
            }
        }
    }
    // An insert may look up its source's newest label: read it before
    // the part is borrowed for the rest.
    let mut took = None;
    let mut inserted = None;
    if ty == "insert" {
        given.only(ty, &INSERT)?;
        let (ins, t) = insert_from(session, None, &given, unit, &env)?;
        (inserted, took) = (Some(ins), t);
    }
    let p = session.project().part(&pid)?;
    // A join, cut, intersect or hole with no targets acts on the part's
    // one solid; the file names it all the same.
    let acts = ty == "hole"
        || given
            .fields
            .operation
            .as_deref()
            .is_some_and(|o| o != "new");
    if acts && given.fields.targets.is_none() {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.targets = Some(vec![one.to_string()]),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {} names the solids it acts on (targets): the part has {} before it",
                        if ty == "hole" {
                            "hole"
                        } else {
                            given.fields.operation.as_deref().unwrap_or_default()
                        },
                        before.len()
                    ),
                )
                .with("param", "targets")
                .with("solids", before));
            }
        }
    }
    // A tweak sorts what it was given; with no target it acts on the part's
    // one solid, else the one its faces name. The file names it all the same.
    if crate::tweak::TYPES.contains(&ty) {
        crate::tweak::take_given(ty, &mut given.fields)?;
        if given.fields.target.is_none() && given.fields.targets.is_none() {
            let before = solids_before(p, &list, at);
            given.fields.target = Some(crate::tweak::default_target(
                ty,
                &given.fields,
                &before,
                &pid,
            )?);
        }
    }
    // A shell or offset with no target acts on the part's one solid; the
    // file names it all the same.
    if matches!(ty, "shell" | "offset" | "split")
        && given.fields.target.is_none()
        && given.fields.targets.is_none()
    {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.target = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {ty} names the solid it acts on (target): the part has {} before it",
                        before.len()
                    ),
                )
                .with("param", "target")
                .with("solids", before));
            }
        }
    }
    if matches!(ty, "flange" | "unfold" | "refold")
        && given.fields.target.is_none()
        && given.fields.targets.is_none()
    {
        let before = solids_before(p, &list, at);
        let sheet: Vec<Id> = before
            .iter()
            .filter(|s| crate::sheet_metal::founder(p, s).is_some())
            .cloned()
            .collect();
        match (before.as_slice(), sheet.as_slice()) {
            ([one], _) | (_, [one]) => given.fields.target = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} names the solid it acts on (target): the part has {} solids before it, {} of them founded by a base flange", before.len(), sheet.len()),
                )
                .with("param", "target")
                .with("solids", before));
            }
        }
    }
    let f = match ty {
        "sketch" => {
            given.only(ty, &SKETCH)?;
            Feature::Sketch(Sketch::new(sketch_plane(None, &given, &pid, unit, &env)?))
        }
        "extrude" | "revolve" => {
            given.only(ty, if ty == "extrude" { &EXTRUDE } else { &REVOLVE })?;
            sweep_from(ty, None, &given, &pid, unit, &env)?
        }
        "sweep" => {
            given.only(ty, &crate::doc::sweep::SWEEP_FIELDS)?;
            crate::along::from_given(None, &given.fields, p, unit, &env)?
        }
        "loft" => {
            given.only(ty, &crate::doc::sweep::LOFT_FIELDS)?;
            crate::loft::from_given(None, &given.fields, p, unit, &env)?
        }
        "planar" => {
            given.only(ty, &crate::doc::sweep::PLANAR_FIELDS)?;
            planar_from(None, &given, &pid)?
        }
        "fill" | "extract" | "sew" | "thicken" => {
            given.only(ty, crate::doc::surface::request_fields(ty))?;
            crate::surface::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "boolean" => {
            given.only(ty, &BOOLEAN)?;
            Feature::Boolean(boolean_from(None, &given)?)
        }
        "instance" => {
            given.only(ty, &INSTANCE)?;
            let deps = session.project().manifest.dependencies.clone();
            Feature::Instance(instance_from(None, &given, unit, &env, &deps)?)
        }
        "insert" => Feature::Insert(Box::new(
            inserted.take().expect("read before the part was borrowed"),
        )),
        "delete_body" => {
            given.only(ty, &DELETE_BODY)?;
            Feature::DeleteBody(Box::new(delete_from(&given)?))
        }
        "mate" => {
            given.only(ty, &MATE)?;
            mate_from(None, &given, (session.project(), &pid), unit, &env)?
        }
        "instance_pattern" => {
            given.only(ty, &INSTANCE_PATTERN)?;
            instance_pattern_from(None, &given, &pid, unit, &env)?
        }
        "fillet" | "chamfer" => {
            given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
            blend_from(ty, None, &given, &pid, unit, &env)?
        }
        "shell" | "offset" | "mirror" | "split" => {
            given.only(ty, crate::doc::body::request_fields(ty))?;
            crate::body::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "move_face" | "replace_face" | "draft" | "delete_face" | "delete_blend"
        | "resize_blend" => {
            given.only(ty, crate::tweak::request_fields(ty))?;
            crate::tweak::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "base_flange" | "flange" | "unfold" | "refold" => {
            crate::sheet_metal::take_given(ty, &mut given.fields);
            given.only(ty, crate::doc::sheet_metal::request_fields(ty))?;
            crate::sheet_metal::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "hole" => {
            given.only(ty, &HOLE)?;
            hole_from(None, &given, &pid, unit, &env)?
        }
        "pattern" => {
            given.only(ty, &PATTERN)?;
            pattern_from(None, &given, &pid, unit, &env)?
        }
        "import" => return Err(import_not_written_here("add")),
        "plane" | "axis" | "point" => {
            given.only(ty, crate::datum::request_fields(ty))?;
            crate::datum::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "path" | "helix" => {
            given.only(ty, crate::doc::path::request_fields(ty))?;
            crate::path::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        _ => {
            let ty = PrimitiveType::parse(ty)?;
            radius_param(ty, &mut given)?;
            refuse_sizes(ty, &given)?;
            let combine = combine_from(&Combine::NEW, &given)?;
            Feature::Primitive(Primitive {
                ty,
                params: Primitive::params_from_json(ty, &Value::Object(given.params), unit, &env)?,
                placement: new_placement(given.placement.unwrap_or_default(), unit, &env)?,
                combine,
            })
        }
    };
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
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, &fid, &f), "revision": revision});
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
    mut given: Given,
    expressions: &Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let unit = session.project().manifest.default_unit;
    // An insert's edit may look up its source's newest label: read it
    // before the part is borrowed for the rest.
    let inserted = {
        let p = session.project().part(&pid)?;
        match feature(p, &fid)?.insert() {
            Some(i) if expressions.is_empty() => {
                let (i, env) = (i.clone(), p.env.clone());
                given.only("insert", &INSERT)?;
                Some(insert_from(session, Some(&i), &given, unit, &env)?.0)
            }
            _ => None,
        }
    };
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let list = order(p)?;
    let old = feature(p, &fid)?.clone();
    let f = if !expressions.is_empty() {
        edited_at(&old, &given, expressions, unit, &p.env).map_err(|e| e.with("feature", &fid))?
    } else {
        match old.clone() {
            Feature::Primitive(mut f) => {
                radius_param(f.ty, &mut given)?;
                refuse_sizes(f.ty, &given)?;
                f.combine = combine_from(&f.combine, &given)?;
                let mut merged: Map<String, Value> = f
                    .params
                    .iter()
                    .map(|(k, q)| (k.clone(), Value::String(q.text())))
                    .collect();
                for (k, v) in given.params {
                    if v.is_null() {
                        merged.remove(&k);
                    } else {
                        merged.insert(k, v);
                    }
                }
                f.params = Primitive::params_from_json(f.ty, &Value::Object(merged), unit, &p.env)?;
                apply_placement(
                    &mut f.placement,
                    given.placement.unwrap_or_default(),
                    unit,
                    &p.env,
                )?;
                Feature::Primitive(f)
            }
            // Its geometry changes through sketch.edit; the plane moves the
            // sketch without changing its solve.
            Feature::Sketch(mut s) => {
                given.only("sketch", &SKETCH)?;
                s.plane = sketch_plane(Some(&s.plane), &given, &pid, unit, &p.env)?;
                Feature::Sketch(s)
            }
            Feature::Instance(i) => {
                given.only("instance", &INSTANCE)?;
                let deps = session.project().manifest.dependencies.clone();
                Feature::Instance(instance_from(Some(&i), &given, unit, &p.env, &deps)?)
            }
            Feature::Insert(_) => Feature::Insert(Box::new(
                inserted.clone().expect("read before the part was borrowed"),
            )),
            Feature::DeleteBody(d) => {
                given.only("delete_body", &DELETE_BODY)?;
                if given.fields.solids.is_none() && given.fields.solid.is_none() {
                    Feature::DeleteBody(d)
                } else {
                    Feature::DeleteBody(Box::new(delete_from(&given)?))
                }
            }
            Feature::Mate(m) => {
                given.only("mate", &MATE)?;
                mate_from(Some(&m), &given, (session.project(), &pid), unit, &p.env)?
            }
            Feature::InstancePattern(x) => {
                given.only("instance_pattern", &INSTANCE_PATTERN)?;
                instance_pattern_from(Some(&x), &given, &pid, unit, &p.env)?
            }
            Feature::Boolean(b) => {
                given.only("boolean", &BOOLEAN)?;
                Feature::Boolean(boolean_from(Some(&b), &given)?)
            }
            f @ (Feature::Fillet(_) | Feature::Chamfer(_)) => {
                let ty = f.type_name();
                given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
                blend_from(ty, Some(&f), &given, &pid, unit, &p.env)?
            }
            f @ (Feature::Shell(_)
            | Feature::Offset(_)
            | Feature::Mirror(_)
            | Feature::Split(_)) => {
                let ty = f.type_name();
                given.only(ty, crate::doc::body::request_fields(ty))?;
                crate::body::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            f @ (Feature::BaseFlange(_)
            | Feature::Flange(_)
            | Feature::Unfold(_)
            | Feature::Refold(_)) => {
                let ty = f.type_name();
                crate::sheet_metal::take_given(ty, &mut given.fields);
                given.only(ty, crate::doc::sheet_metal::request_fields(ty))?;
                crate::sheet_metal::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Pattern(x) => {
                given.only("pattern", &PATTERN)?;
                pattern_from(Some(&x), &given, &pid, unit, &p.env)?
            }
            Feature::Hole(h) => {
                given.only("hole", &HOLE)?;
                let f = hole_from(Some(&h), &given, &pid, unit, &p.env)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
            Feature::Import(_) => return Err(import_not_written_here("edit").with("feature", &fid)),
            f
            @ (Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)) => {
                let ty = f.type_name();
                given.only(ty, crate::doc::surface::request_fields(ty))?;
                crate::surface::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            f @ (Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_)) => {
                let ty = f.type_name();
                crate::tweak::take_given(ty, &mut given.fields)?;
                given.only(ty, crate::tweak::request_fields(ty))?;
                crate::tweak::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Datum(d) => {
                let ty = d.type_name();
                given.only(ty, crate::datum::request_fields(ty))?;
                crate::datum::from_given(ty, Some(&d), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Path(x) => {
                let ty = x.type_name();
                given.only(ty, crate::doc::path::request_fields(ty))?;
                crate::path::from_given(ty, Some(&x), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Sweep(w) => {
                given.only("sweep", &crate::doc::sweep::SWEEP_FIELDS)?;
                let f = crate::along::from_given(Some(&w), &given.fields, p, unit, &p.env)?;
                if !f.profiles().is_empty() {
                    check_sweep(p, &list, &fid, &f)?;
                }
                f
            }
            Feature::Loft(l) => {
                given.only("loft", &crate::doc::sweep::LOFT_FIELDS)?;
                let f = crate::loft::from_given(Some(&l), &given.fields, p, unit, &p.env)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
            Feature::Planar(x) => {
                given.only("planar", &crate::doc::sweep::PLANAR_FIELDS)?;
                let f = planar_from(Some(x.fields()), &given, &pid)?;
                if !f.profiles().is_empty() {
                    check_sweep(p, &list, &fid, &f)?;
                }
                f
            }
            f @ (Feature::Extrude(_) | Feature::Revolve(_)) => {
                let ty = f.type_name();
                given.only(ty, if ty == "extrude" { &EXTRUDE } else { &REVOLVE })?;
                let current = match &f {
                    Feature::Extrude(e) => e.fields(),
                    Feature::Revolve(r) => r.fields(),
                    _ => unreachable!("a sweep"),
                };
                let f = sweep_from(ty, Some(current), &given, &pid, unit, &p.env)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
        }
    };
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
    // References already there were checked when they were written; a
    // fillet or chamfer that now grows its edges, or stops, may reach an
    // edge twice.
    let propagates = |f: &Feature| match f {
        Feature::Fillet(x) => x.propagate,
        Feature::Chamfer(x) => x.propagate,
        _ => false,
    };
    // A sweep's twist and scale are judged against its path's closing, a
    // loft's sections against each other.
    let sweep_changed = (f.sweep().is_some() || f.loft().is_some()) && f != old;
    if f.references() != old.references()
        || f.geometry() != old.geometry()
        || propagates(&f) != propagates(&old)
        || sweep_changed
        || (matches!(
            f,
            Feature::Shell(_)
                | Feature::Offset(_)
                | Feature::Mirror(_)
                | Feature::Split(_)
                | Feature::MoveFace(_)
                | Feature::ReplaceFace(_)
                | Feature::Draft(_)
                | Feature::DeleteFace(_)
                | Feature::DeleteBlend(_)
                | Feature::ResizeBlend(_)
        ) && f.targets() != old.targets())
        || ((f.base_flange().is_some()
            || f.flange().is_some()
            || f.unfold().is_some()
            || f.refold().is_some())
            && f != old)
        || (matches!(
            f,
            Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)
        ) && f != old)
        || f.source() != old.source()
    {
        followed.extend(check_references(session, &pid, &list, &fid, &f)?);
    }
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
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
    let mut v = json!({"feature": feature_report(saved, &fid, &f), "revision": revision});
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
    let mut v = feature_report(p, &fid, feature(p, &fid)?);
    // The revision it was read at: an edit made from it says so.
    v["revision"] = json!(p.revision);
    Ok(v)
}
