//! Import features: one body of a STEP file the project holds under
//! `imports/`, read and healed by ryustep under the feature's own key, so
//! every name follows the feature and a new version of the file keeps the
//! names of what didn't change (ryukei's import names hash each entity's
//! geometry, never its place in the file).
//!
//! Files are opened once (parsed, decoded, their product tree read) and
//! kept by BLAKE3 for the process, least recently used first, up to
//! [`OPENED_BYTES`] of source: a file a hundred parts import is parsed
//! once, and each part heals only its own body.

use crate::doc::id::Id;
use crate::doc::import::{Import, ProductRef};
use crate::doc::project::{ImportFile, Part};
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, finish, op_key};
use ryukei::HealError;
use ryukei::heal::HealReport;
use ryukei::name::Name;
use ryustep::{ImportError, Opened, ReadOptions, StepError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

pub mod convert;
pub mod external;
pub(crate) mod plan;
mod update;

use plan::Planned;
pub use plan::Writes;
pub(crate) use plan::{product_of, usage_ordinal};
pub use update::{products_diff, usage_warnings};

/// The work units one imported body may heal in by default: at least
/// four times the largest body of ryustep's committed and pinned corpora,
/// rounded up to a power of two (plan 15.A's rule). Plan 15.A set
/// `1 << 23` from the pinned `nist_ctc_02`'s AP242 edition (2,029,065
/// units); its `rc` edition is larger (2,122,190 units, #368), which
/// leaves `1 << 23` 1.2% short of the rule, and #317's finer shell
/// judging took KiCad's two Keystone battery holders to 8.4 and 8.6
/// million units (#398). Hence `1 << 24`. The Vorons' slowest bodies
/// (#239) start healing under it: the Switchwire's threaded insert
/// spends 9,031,258. Plan 15.C weighted enclosure charges (#353) and
/// measured the corpora again (cox-worker-4, 2026-11-04): the largest
/// body is still `nist_ctc_02_asme1_rc.stp`'s, at 1,835,959, which by the
/// rule alone would give `1 << 23`. KiCad's battery holders, measured
/// again on the same box from `Battery.3dshapes` alone, now spend more:
/// `BatteryHolder_Keystone_2479_3xAAA` 10,343,304 and `_2462_2xAA`
/// 6,656,799 (heal-units, at 0fe93515), so `1 << 24` stays, 1.6 times the
/// larger. v0.8.1 raised the default to `1 << 25` for release/0.8, where
/// the Keystone 2468 holder spends 17,326,746 (#520, the owner's
/// decision); main takes the same default, so no release lowers it.
pub const DEFAULT_HEAL_BUDGET: u64 = 1 << 25;

/// The largest body of the corpora `DEFAULT_HEAL_BUDGET` is calibrated
/// against: the pinned `nist_ctc_02_asme1_rc.stp`'s bodies 0 and 2 (#368),
/// as plan 15.C measured them.
const LARGEST_CORPUS_BODY: u64 = 1_835_959;

/// The most KiCad's two Keystone battery holders spend healing since
/// #317's finer shell judging (#398), under plan 15.C's weighted charges.
const BATTERY_HOLDERS: u64 = 10_343_304;

// Plan 15.A's rule, checked when ryucad builds: the default is a power
// of two at least four times the corpora's largest body, and clears the
// battery holders.
const _: () = {
    assert!(DEFAULT_HEAL_BUDGET.is_power_of_two());
    assert!(DEFAULT_HEAL_BUDGET >= 4 * LARGEST_CORPUS_BODY);
    assert!(DEFAULT_HEAL_BUDGET > BATTERY_HOLDERS);
};

/// How many bytes of source the opened files kept may hold together.
pub const OPENED_BYTES: usize = 1 << 30;

/// A file being opened or opened, shared by everyone who asks for it.
type Slot = Arc<OnceLock<std::result::Result<Arc<Opened>, StepError>>>;

/// The files kept, most recently used last, with their sizes.
static OPENED: Mutex<Vec<(String, usize, Slot)>> = Mutex::new(Vec::new());

/// How many bytes of source the opened files kept hold, for the memory
/// budget (see [`crate::budget::Fixed`]).
pub fn opened_bytes() -> u64 {
    let kept = OPENED.lock().unwrap_or_else(|p| p.into_inner());
    kept.iter()
        .filter(|(_, _, slot)| matches!(slot.get(), Some(Ok(_))))
        .map(|(_, n, _)| *n as u64)
        .sum()
}

/// `file` opened, from what is kept when it was opened before. Nobody
/// waits for another thread's opening: `ryustep::open` works on rayon, and
/// a rayon worker waiting in it may run another part's rebuild that asks
/// for the same file, which would then wait on itself (a deadlock the
/// benchmark's 500 imports found). Two threads that ask for a file at
/// once both open it, and the first to finish is kept; parts built side
/// by side open their files first ([`open_ahead`]).
pub fn opened(file: &ImportFile) -> std::result::Result<Arc<Opened>, StepError> {
    let slot = {
        let mut kept = OPENED.lock().unwrap_or_else(|p| p.into_inner());
        let slot = match kept.iter().position(|(h, _, _)| *h == file.blake3) {
            Some(i) => kept.remove(i).2,
            None => Slot::default(),
        };
        kept.push((file.blake3.clone(), file.bytes.len(), Arc::clone(&slot)));
        // The newest always stays, however large.
        let mut total: usize = kept.iter().map(|(_, n, _)| n).sum();
        while total > OPENED_BYTES && kept.len() > 1 {
            total -= kept.remove(0).1;
        }
        slot
    };
    if let Some(done) = slot.get() {
        return done.clone();
    }
    let _ = slot.set(ryustep::open(&file.bytes).map(Arc::new));
    slot.get().expect("set by now").clone()
}

/// Opens every file of `files` that isn't open yet, one after another on
/// this thread, before parts that import them are built side by side.
pub fn open_ahead<'a>(files: impl IntoIterator<Item = &'a ImportFile>) {
    let mut seen = std::collections::BTreeSet::new();
    for f in files {
        if seen.insert(&f.blake3) {
            let _ = opened(f);
        }
    }
}

/// How a solid-mode import heals: one place, which the dry run, `build`
/// and `import convert` all read. Surface models heal as sheets
/// (`Open::Sheet`, design 388), solids exactly as before; a reference
/// reads only its tolerance cap.
pub fn heal_options() -> ryukei::heal::HealOptions {
    ryukei::heal::HealOptions {
        open: ryukei::heal::Open::Sheet,
        ..Default::default()
    }
}

/// What a reference import reports beside its body: its file, product,
/// `mode: reference`, and what the reference path did (counts, and the
/// faces it split, dropped or kept unoriented by name with their STEP
/// instances, for the warnings).
pub fn reference_report(
    i: &Import,
    r: &ryukei::heal::ReferenceReport,
    instances: &BTreeMap<Name, u64>,
) -> Value {
    let named = |names: &[Name]| -> Vec<Value> {
        names
            .iter()
            .map(|n| {
                let mut v = json!({"name": n.to_string()});
                if let Some(k) = instances.get(n) {
                    v["step"] = json!(format!("#{k}"));
                }
                v
            })
            .collect()
    };
    let tolerance = r.tolerances.iter().map(|(_, t)| *t).fold(0.0, f64::max);
    let mut v = json!({
        "file": i.file.path,
        "body": i.body,
        "mode": "reference",
        "reference": {
            "seams": r.seams.len(),
            "poles": r.poles.len(),
            "joined": r.joined.len(),
            "split": r.split.len(),
            "reversed": r.reversed.len(),
            "turned": r.turned.len(),
            "laminar": r.laminar.len(),
            "tolerance_m": tolerance,
            "split_faces": named(&r.split_faces),
            "dropped_faces": named(&r.dropped_faces),
            "unoriented": named(&r.unoriented),
        },
    });
    if let Some(p) = &i.product {
        v["product"] = product_json(p);
    }
    v
}

/// What an import feature reports beside its solid: its file, its
/// product and what healing did, as counts.
pub fn report(i: &Import, healed: &HealReport) -> Value {
    let tolerance = healed
        .tolerances
        .iter()
        .map(|(_, t)| *t)
        .fold(0.0, f64::max);
    let mut v = json!({
        "file": i.file.path,
        "body": i.body,
        "heal": {
            "sewn": healed.sewn.len(),
            "merged": healed.merged.len(),
            "collapsed": healed.collapsed.len(),
            "split": healed.split.len(),
            "seams": healed.seams.len(),
            "poles": healed.poles.len(),
            "joined": healed.joined.len(),
            "reversed": healed.reversed.len(),
            "turned": healed.turned.len(),
            "tightened": healed.tightened.len(),
            "approximated": healed.approximated.len(),
            "tolerance_m": tolerance,
        },
    });
    if let Some(p) = &i.product {
        v["product"] = product_json(p);
    }
    v
}

/// The warnings a reference's report gives, one per kind of fault, each
/// naming its faces by entity reference (`<part>/<feature>/<name>`; a
/// dropped face isn't in the body, but the reference names what it was)
/// and STEP instance. A solid's report gives none.
pub fn reference_warnings(part: &Id, id: &Id, report: &Value) -> Vec<Value> {
    let Some(r) = report.get("reference") else {
        return Vec::new();
    };
    [
        (
            "split_faces",
            "reference_faces_split",
            "were laid out each alone: their piece wouldn't lay out whole",
        ),
        (
            "dropped_faces",
            "reference_faces_dropped",
            "couldn't be laid out even alone and are left out",
        ),
        (
            "unoriented",
            "reference_unoriented",
            "couldn't be oriented and are kept as the file has them",
        ),
    ]
    .into_iter()
    .filter_map(|(key, code, what)| {
        let list = r[key].as_array().filter(|l| !l.is_empty())?;
        let faces: Vec<Value> = list
            .iter()
            .map(|f| {
                let name = f["name"].as_str().unwrap_or_default();
                let mut v = json!({"entity": format!("{part}/{id}/{name}")});
                if let Some(s) = f.get("step") {
                    v["step"] = s.clone();
                }
                v
            })
            .collect();
        Some(json!({
            "code": code,
            "part": part,
            "feature": id,
            "faces": faces,
            "message": format!("import {id}: {} faces of the reference {what}", faces.len()),
        }))
    })
    .collect()
}

/// What an import feature says of itself, whether it built or not: its
/// file and the hash it holds, its product and its body. The rebuild
/// report puts it under `import`, beside [`report`]'s heal counts.
pub fn written(i: &Import) -> Value {
    let mut v = json!({"file": i.file.path, "blake3": i.file.blake3, "body": i.body});
    if let Some(p) = &i.product {
        v["product"] = product_json(p);
    }
    if i.mode() == crate::doc::import::Mode::Reference {
        v["mode"] = json!("reference");
    }
    // A flattened body (its file writes where the assembly put it): the
    // window lists a file's flattened bodies under one row (ryucad 25.B).
    if i.place.is_some() {
        v["flattened"] = json!(true);
    }
    v
}

fn product_json(p: &ProductRef) -> Value {
    let mut v = json!({"id": p.id, "name": p.name});
    if p.ordinal != 0 {
        v["ordinal"] = json!(p.ordinal);
    }
    v
}

/// The index into `opened.held` of the body `i` selects, or why there is
/// none.
pub fn select(id: &Id, i: &Import, opened: &Opened) -> Result<usize> {
    let definition = match &i.product {
        None => None,
        Some(p) => {
            let defs = &opened.products.definitions;
            let found = defs
                .iter()
                .enumerate()
                .filter(|(_, d)| *d.product_id == p.id && *d.name == p.name)
                .nth(p.ordinal as usize)
                .map(|(k, _)| k);
            match found {
                Some(k) => Some(k),
                None => {
                    let products: Vec<Value> = defs
                        .iter()
                        .map(|d| json!({"id": &*d.product_id, "name": &*d.name}))
                        .collect();
                    return Err(Error::new(
                        Code::ImportProductMissing,
                        format!(
                            "import {id}: {} has no product {:?} named {:?}{}",
                            i.file.path,
                            p.id,
                            p.name,
                            if p.ordinal == 0 {
                                String::new()
                            } else {
                                format!(" (number {} of that name)", p.ordinal + 1)
                            }
                        ),
                    )
                    .with("feature", id)
                    .with("file", &i.file.path)
                    .with("product", product_json(p))
                    .with("products", products));
                }
            }
        }
    };
    let bodies: Vec<usize> = opened
        .held
        .iter()
        .enumerate()
        .filter(|(_, h)| h.definition == definition)
        .map(|(k, _)| k)
        .collect();
    bodies.get(i.body as usize).copied().ok_or_else(|| {
        let of = match &i.product {
            Some(p) => format!("product {:?}", p.id),
            None => "the bodies no product reaches".to_owned(),
        };
        Error::new(
            Code::ImportBodyMissing,
            format!(
                "import {id}: {} has {} bod{} in {of}; body {} is past them",
                i.file.path,
                bodies.len(),
                if bodies.len() == 1 { "y" } else { "ies" },
                i.body
            ),
        )
        .with("feature", id)
        .with("file", &i.file.path)
        .with("body", i.body)
        .with("bodies", bodies.len())
    })
}

/// The import's body, healed under the feature's key, with each entity's
/// origin (the STEP instance it came from), and its report.
pub fn build(
    part: &Part,
    id: &Id,
    i: &Import,
    run: &crate::rebuild::Run,
) -> Result<(Built, Value)> {
    let Some(file) = part.imports.get(&i.file.path) else {
        return Err(Error::new(
            Code::ImportFileMissing,
            format!(
                "import {id}: {} is missing; restore it from Git or import the file again",
                i.file.path
            ),
        )
        .with("feature", id)
        .with("path", &i.file.path));
    };
    if file.blake3 != i.file.blake3 {
        return Err(changed(id, i, file));
    }
    let opened = opened(file).map_err(|e| unreadable(&i.file.path, &e).with("feature", id))?;
    let k = select(id, i, &opened)?;
    if i.mode() == crate::doc::import::Mode::Reference {
        return build_reference(part, id, i, &opened, k, run);
    }
    let work = crate::work::work(run.job).with_budget(run.heal_budget);
    let read = opened.body_with(
        k,
        &ReadOptions {
            key: op_key(&part.id, id),
            heal: heal_options(),
        },
        &work,
    );
    let (body, journal, healed) = read.result.map_err(|e| match e {
        ImportError::Work(w) => stopped(id, i, &w, (read.representation, &read.items), run.job),
        e => failed(id, i, &e, &read.instances, &read.solids),
    })?;
    let body = placed(id, i, body)?;
    let step = |n: &Name| read.instances.get(n).map(|k| format!("#{k}"));
    let origins: crate::rebuild::Origins = crate::rebuild::names(&body)
        .into_iter()
        .map(|n| {
            let mut o = json!({"feature": id});
            if let Some(s) = step(&n) {
                o["step"] = json!(s);
            }
            (n.to_string(), o)
        })
        .collect();
    let report = report(i, &healed);
    Ok((finish(id, "import", (body, journal), origins)?, report))
}

/// A reference import's body (design 364): the file's topology as a
/// sheet, never healed, named as healing names it, under `heal.budget`
/// and the request's cancel. Refused only where ryustep can't read the
/// shape, or no face lays out (`import_failed`).
fn build_reference(
    part: &Part,
    id: &Id,
    i: &Import,
    opened: &Opened,
    k: usize,
    run: &crate::rebuild::Run,
) -> Result<(Built, Value)> {
    let work = crate::work::work(run.job).with_budget(run.heal_budget);
    let read = opened.reference_with(
        k,
        &ReadOptions {
            key: op_key(&part.id, id),
            heal: heal_options(),
        },
        &work,
    );
    let (body, report) = read.result.map_err(|e| match e {
        ImportError::Work(w) => stopped(id, i, &w, (read.representation, &read.items), run.job),
        e => failed(id, i, &e, &read.instances, &read.solids),
    })?;
    let body = placed(id, i, body)?;
    let step = |n: &Name| read.instances.get(n).map(|k| format!("#{k}"));
    let origins: crate::rebuild::Origins = crate::rebuild::names(&body)
        .into_iter()
        .map(|n| {
            let mut o = json!({"feature": id});
            if let Some(s) = step(&n) {
                o["step"] = json!(s);
            }
            (n.to_string(), o)
        })
        .collect();
    let v = reference_report(i, &report, &read.instances);
    // A reference changes nothing the file had: its journal is empty, as
    // a mirror's is.
    let journal = ryukei::name::Journal::new();
    Ok((
        crate::rebuild::finish_reference(id, (body, journal), origins)?,
        v,
    ))
}

/// `body` moved where a flattened import's `place` puts it; as it is
/// with none. Names don't depend on where the body stands, so a placed
/// import's names are its file's.
fn placed(id: &Id, i: &Import, body: ryukei::topo::Body) -> Result<ryukei::topo::Body> {
    let Some(p) = &i.place else {
        return Ok(body);
    };
    let r = crate::assembly::pose_rigid(&p.pose).map_err(|e| e.with("feature", id))?;
    body.transformed(&r).map_err(|e| {
        Error::new(
            Code::ImportFailed,
            format!("import {id}: its place puts it where it can't stand: {e:?}"),
        )
        .with("feature", id)
        .with("reason", "place")
    })
}

/// A heal its work stopped: the request's `cancelled`, or
/// `import_budget_exceeded` for a body past `heal.budget`, naming the
/// representation and the B-rep items read, where the agent finds it in
/// the file (import names aren't known for a body that didn't heal).
fn stopped(
    id: &Id,
    i: &Import,
    e: &ryukei::Error,
    (representation, items): (u64, &[u64]),
    job: &ryu_session::job::Job,
) -> Error {
    if let Some(e) = crate::work::stopped(e, job) {
        return e;
    }
    let ryukei::Error::BudgetExceeded { spent, budget } = *e else {
        return Error::new(Code::Internal, format!("import {id}: {e}"));
    };
    let step: Vec<String> = core::iter::once(representation)
        .chain(items.iter().copied())
        .map(|n| format!("#{n}"))
        .collect();
    Error::new(
        Code::ImportBudgetExceeded,
        format!(
            "import {id}: a body of {} didn't heal within heal.budget ({budget} work units; it had spent {spent}); raise heal.budget to heal it",
            i.file.path
        ),
    )
    .with("feature", id)
    .with("file", &i.file.path)
    .with("body", i.body)
    .with("step", step)
    .with("spent", spent)
    .with("budget", budget)
}

/// `import_file_changed`: the file isn't the bytes `i` was written
/// against.
pub fn changed(id: &Id, i: &Import, file: &ImportFile) -> Error {
    Error::new(
        Code::ImportFileChanged,
        format!(
            "import {id}: {} isn't the file it was imported from (its BLAKE3 is {}, not {}); import it again to take the new bytes",
            i.file.path, file.blake3, i.file.blake3
        ),
    )
    .with("feature", id)
    .with("path", &i.file.path)
    .with("expected", &i.file.blake3)
    .with("found", &file.blake3)
}

/// `step_unreadable` for the file at `path`.
pub fn unreadable(path: &str, e: &StepError) -> Error {
    let reason = match e {
        StepError::NotPart21 { .. } => "not_part21",
        StepError::Syntax { .. } => "syntax",
        StepError::DuplicateInstance { .. } => "duplicate_instance",
        StepError::UnknownSchema(_) => "unknown_schema",
        StepError::NoData => "no_data",
        _ => "unreadable",
    };
    let mut err = Error::new(Code::StepUnreadable, format!("{path}: {e}"))
        .with("path", path)
        .with("reason", reason);
    match e {
        StepError::Syntax { line, column, .. } => {
            err = err.with("line", *line).with("column", *column);
        }
        StepError::NotPart21 { offset } => err = err.with("offset", *offset),
        StepError::UnknownSchema(s) => err = err.with("schema", s.as_str()),
        _ => {}
    }
    err
}

/// `import.report`: the transfer report of `path`, a file the project
/// holds, from what its import features and imported instances make of
/// it now (design 448). Each part reading it is rebuilt (side by side);
/// a feature that builds read its body, one that fails names its
/// instances.
pub fn transfer_report(
    session: &mut crate::session::Session,
    path: &str,
    detail: bool,
) -> Result<Value> {
    use crate::doc::files::Feature;
    let Some(file) = session.project().imports.get(path).cloned() else {
        return Err(Error::new(
            Code::NotFound,
            format!("{path} isn't a file the project has imported"),
        )
        .with("path", path));
    };
    let opened = opened(&file).map_err(|e| unreadable(path, &e))?;
    let mut read = ryustep::census::Read::default();
    let mut failed = Vec::new();
    let mut parts: Vec<Id> = Vec::new();
    let (mut bodies, mut made, mut vias) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    for (id, p) in &session.project().parts {
        let reads = p.features.values().any(|f| match f {
            Ok(Feature::Import(i)) => i.file.path == path,
            Ok(Feature::Instance(i)) => i.import.as_ref().is_some_and(|u| u.file == path),
            _ => false,
        });
        if reads {
            parts.push(id.clone());
        }
    }
    session.rebuild_parts(&parts)?;
    for id in &parts {
        read.product_tree = true;
        let r = session.rebuilt(id)?.clone();
        let p = session.project().part(id)?;
        for f in p.features.values() {
            if let Ok(Feature::Instance(inst)) = f
                && let Some(u) = inst.import.as_ref().filter(|u| u.file == path)
                && let Some(k) = crate::file_styles::definition_of(&opened, &u.product)
                && let Some(n) = crate::file_styles::usage_index(&opened, k, u)
            {
                made.insert((k, n));
            }
        }
        for (fid, f) in &p.features {
            let Ok(Feature::Import(i)) = f else {
                continue;
            };
            if i.file.path != path {
                continue;
            }
            match r.features.iter().find(|(x, _)| x == fid).map(|(_, o)| o) {
                Some(Ok(_)) => {
                    if let Ok(k) = select(fid, i, &opened) {
                        read.roots.extend(opened.roots(k));
                        bodies.insert(k);
                        if let Some(place) = &i.place {
                            vias.insert(place.via.clone());
                        }
                    }
                }
                Some(Err(e)) => failed.extend(crate::transfer::failed_instances(&json!(e))),
                None => {}
            }
        }
    }
    let (instances, unplaced) = crate::file_styles::interpreted(&opened, &bodies, &made, &vias);
    read.instances = instances;
    read.instances.extend(crate::file_props::held(
        session.project(),
        &*session,
        &opened,
        path,
        &parts,
    ));
    read.instances.sort_unstable();
    read.instances.dedup();
    failed.extend(crate::file_props::failed(&opened));
    let t = opened.census.transfer(&read, &failed);
    let mut out = json!({
        "file": {"path": path, "blake3": file.blake3, "schema": schema_name(opened.schema)},
        "parts": parts,
        "transfer": crate::transfer::json(&t, path, detail),
    });
    crate::transfer::with_unplaced(&mut out["transfer"], unplaced);
    Ok(out)
}

/// `import_body_missing` for `path`: with `external` (each file the
/// file's externally defined products name, the products naming it by
/// id, else name, and the format the writer gave) and a message saying
/// the parts are in those files, which this version doesn't read yet
/// (design 448; plan 24.D reads them). Without any, as before.
pub fn body_missing(opened: &Opened, path: &str) -> Error {
    let mut files: BTreeMap<String, (Vec<String>, Option<String>)> = BTreeMap::new();
    for d in opened
        .products
        .definitions
        .iter()
        .filter(|d| d.externally_defined())
    {
        let product = if d.product_id.is_empty() {
            d.name.to_string()
        } else {
            d.product_id.to_string()
        };
        for x in &d.external {
            let entry = files.entry(x.name.clone()).or_default();
            if !entry.0.contains(&product) {
                entry.0.push(product.clone());
            }
            if entry.1.is_none() {
                entry.1 = x.format.clone();
            }
        }
    }
    let plain = Error::new(
        Code::ImportBodyMissing,
        format!("{path} holds no solid to import"),
    )
    .with("path", path);
    if files.is_empty() {
        return plain;
    }
    let external: Vec<Value> = files
        .iter()
        .map(|(f, (products, format))| {
            let mut v = json!({"file": f, "products": products});
            if let Some(x) = format {
                v["format"] = json!(x);
            }
            v
        })
        .collect();
    let names: Vec<&str> = files.keys().map(String::as_str).collect();
    Error::new(
        Code::ImportBodyMissing,
        format!(
            "{path} holds no solid to import: its parts are in other files ({}), which ryucad doesn't read yet",
            names.join(", ")
        ),
    )
    .with("path", path)
    .with("external", external)
}

/// `import_failed`: why a body couldn't be read or healed, its reason in
/// snake case and the STEP instances it names (`#n`), with the names
/// healing gave those it made. Faces that overlap or shells that cross in
/// two different solids of the file (`solids`: the B-rep item each face
/// came from) are named as those solids, which touch (#208).
pub fn failed(
    id: &Id,
    i: &Import,
    e: &ImportError,
    instances: &BTreeMap<Name, u64>,
    solids: &BTreeMap<Name, u64>,
) -> Error {
    let (reason, numbers, names) = failure(e);
    let apart = match e {
        ImportError::Heal(HealError::FacesOverlap { a, b } | HealError::ShellsCross { a, b }) => {
            match (solids.get(a), solids.get(b)) {
                (Some(x), Some(y)) if x != y => Some([*x, *y]),
                _ => None,
            }
        }
        _ => None,
    };
    let mut step: Vec<String> = numbers.iter().map(|n| format!("#{n}")).collect();
    let mut made: Vec<String> = Vec::new();
    for n in names {
        match instances.get(&n) {
            Some(k) => step.push(format!("#{k}")),
            None => made.push(n.to_string()),
        }
    }
    let touching = match apart {
        Some([x, y]) => format!(
            "; they are in two solids of the file, #{x} and #{y}, that touch, and ryucad heals a \
             representation's solids as one body: write them to separate parts (or bodies) in \
             the program that made the file"
        ),
        None => String::new(),
    };
    let mut err = Error::new(
        Code::ImportFailed,
        format!(
            "import {id}: a body of {} can't be imported: {e}{touching}",
            i.file.path
        ),
    )
    .with("feature", id)
    .with("file", &i.file.path)
    .with("reason", reason)
    .with("step", step);
    if !made.is_empty() {
        err = err.with("names", made);
    }
    if let Some([x, y]) = apart {
        err = err.with("solids", vec![format!("#{x}"), format!("#{y}")]);
    }
    err
}

/// An import error's reason, the instance numbers it names and the
/// import names it names.
fn failure(e: &ImportError) -> (String, Vec<u64>, Vec<Name>) {
    let snake = |s: &str| {
        let mut out = String::new();
        for (k, c) in s.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if k > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    };
    // The variant's name, from its debug form.
    let variant = |d: String| -> String {
        let end = d.find([' ', '(', '{']).unwrap_or(d.len());
        snake(&d[..end])
    };
    match e {
        ImportError::DanglingReference {
            instance,
            reference,
        } => (
            variant(format!("{e:?}")),
            vec![*instance, *reference],
            vec![],
        ),
        ImportError::WrongEntity { instance, .. }
        | ImportError::ReferenceCycle { instance }
        | ImportError::UnsupportedEntity { instance, .. }
        | ImportError::Malformed { instance, .. }
        | ImportError::InvalidGeometry { instance, .. }
        | ImportError::OutsideSizeBox { instance }
        | ImportError::NotBounded { instance } => {
            (variant(format!("{e:?}")), vec![*instance], vec![])
        }
        ImportError::MissingUnit { context, .. } => {
            (variant(format!("{e:?}")), vec![*context], vec![])
        }
        ImportError::Heal(h) => {
            let names = match h {
                HealError::OpenShell { free_edges } => free_edges.clone(),
                HealError::NonManifold { at }
                | HealError::Misoriented { at }
                | HealError::Uncertain { at } => vec![*at],
                HealError::ShellsCross { a, b }
                | HealError::FacesOverlap { a, b }
                | HealError::PiecesTouch { a, b, .. } => vec![*a, *b],
                HealError::BadSeam { face }
                | HealError::BadDomain { face }
                | HealError::SelfCrossingLoop { face }
                | HealError::SliverTooWide { face } => vec![*face],
                HealError::NoPcurve { edge, face } => vec![*edge, *face],
                HealError::GapTooLarge { entity, .. } => vec![*entity],
                _ => vec![],
            };
            (variant(format!("{h:?}")), vec![], names)
        }
        _ => (variant(format!("{e:?}")), vec![], vec![]),
    }
}

/// What `import` was asked to do.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    /// The file to import, as the daemon can read it: absolute, or
    /// relative to the project's root.
    pub path: String,
    /// The part to make, when the file makes one.
    pub part: Option<String>,
    /// A part to add one product's bodies to, instead of making parts.
    pub into: Option<String>,
    /// With `into`, the product: its id, else its name.
    pub product: Option<String>,
    pub dry_run: bool,
    /// The project's revision, or with `into` that part's.
    pub expected: Option<String>,
    /// A file under `imports/` that `path`'s bytes replace: re-import.
    pub update: Option<String>,
    /// Heal each body into a checked solid, rather than keep it as a
    /// reference (design 364).
    pub solid: bool,
    /// Make one part of every body of the file, placed where its
    /// assemblies put it (#499).
    pub flatten: bool,
    /// Folders to look in for the files an assembly's parts are kept in,
    /// after the folder of the file naming each (design 448, 24.D):
    /// absolute, or relative to the project's root.
    pub search: Vec<String>,
}

impl Ask {
    /// The mode new import features take: a reference unless `solid`.
    pub fn mode(&self) -> Option<crate::doc::import::Mode> {
        (!self.solid).then_some(crate::doc::import::Mode::Reference)
    }
}

/// A schema as files and people name it.
pub fn schema_name(s: ryustep::schema::Schema) -> &'static str {
    match s {
        ryustep::schema::Schema::Ap203 => "AP203",
        ryustep::schema::Schema::Ap214 => "AP214",
        ryustep::schema::Schema::Ap242 => "AP242",
    }
}

/// `<s>` as an ID: lower case, any run of other characters as one `-`,
/// at most 63 long; `None` when nothing (or a name Windows reserves) is
/// left.
pub fn id_from(s: &str) -> Option<Id> {
    let mut out = String::new();
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches(['-', '_']);
    let cut = &trimmed[..trimmed.len().min(63)];
    Id::parse(cut.trim_end_matches(['-', '_']), "part").ok()
}

/// `base`, else `base-2`, `base-3`, … (cut to fit), the first `taken`
/// refuses.
pub fn free(base: &Id, taken: impl Fn(&str) -> bool) -> Id {
    if !taken(base.as_str()) {
        return base.clone();
    }
    (2u64..)
        .find_map(|n| {
            let tail = format!("-{n}");
            let head = &base.as_str()[..base.as_str().len().min(63 - tail.len())];
            let id = Id::parse(
                &format!("{}{tail}", head.trim_end_matches(['-', '_'])),
                "part",
            )
            .ok()?;
            (!taken(id.as_str())).then_some(id)
        })
        .expect("some n is free")
}

/// `--search` folders as the daemon reads them: absolute, or relative to
/// the project's root.
fn search_dirs(root: &std::path::Path, search: &[String]) -> Vec<std::path::PathBuf> {
    search
        .iter()
        .map(|s| {
            let p = std::path::Path::new(s);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        })
        .collect()
}

/// `import`: plans every file change and, unless `dry_run`, makes them.
pub fn import(session: &mut crate::session::Session, ask: Ask) -> Result<Value> {
    // Its rebuilds heal the file's bodies.
    session.job().phase(ryu_session::job::Phase::Heal);
    if ask.update.is_some() {
        return reimport(session, ask);
    }
    let root = session.root().to_path_buf();
    let source = {
        let p = std::path::Path::new(&ask.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };
    let bytes = std::fs::read(&source).map_err(|e| Error::io(&source, &e))?;
    let file = ImportFile::new(bytes);
    let opened = opened(&file).map_err(|e| unreadable(&ask.path, &e))?;
    // Every file the parts are in, found before anything is written.
    let search = search_dirs(&root, &ask.search);
    let tree = external::resolve(&source, file.clone(), opened.clone(), &search, &ask.path)
        .map_err(|e| refused(e, &opened, &ask.path))?;
    if tree.nodes.len() > 1 {
        return import_tree(session, &ask, root, &tree, None)
            .map_err(|e| refused(e, &opened, &ask.path));
    }
    import_opened(session, &ask, root, source, file, &opened)
        .map_err(|e| refused(e, &opened, &ask.path))
}

/// [`import`] once the file is read.
fn import_opened(
    session: &mut crate::session::Session,
    ask: &Ask,
    root: std::path::PathBuf,
    source: std::path::PathBuf,
    file: ImportFile,
    opened: &Opened,
) -> Result<Value> {
    use crate::doc::import::FileRef;
    use crate::doc::project::{IMPORTS, PDM_CONFIG};
    // Where it goes: where the same bytes already are, else a free name.
    let project = session.project();
    let (stored, reused) = match project
        .imports
        .iter()
        .find(|(_, f)| f.blake3 == file.blake3)
    {
        Some((p, _)) => (p.clone(), true),
        None => {
            let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned());
            let base = stem
                .as_deref()
                .and_then(id_from)
                .unwrap_or_else(|| Id::parse("step", "file").expect("an ID"));
            let id = free(&base, |s| {
                project.imports.contains_key(&format!("{IMPORTS}/{s}.step"))
                    || root.join(format!("{IMPORTS}/{s}.step")).exists()
            });
            (format!("{IMPORTS}/{id}.step"), false)
        }
    };
    let file_ref = FileRef {
        path: stored.clone(),
        blake3: file.blake3.clone(),
    };
    if ask.flatten && (ask.into.is_some() || ask.product.is_some()) {
        return Err(Error::new(
            Code::Usage,
            "flatten makes a part of the whole file: leave out into and product",
        ));
    }
    let mut plan: Vec<Planned> = match &ask.into {
        Some(into) => {
            if ask.part.is_some() {
                return Err(Error::new(
                    Code::Usage,
                    "give a part to make or one to add to (into), not both",
                ));
            }
            vec![into_plan(session, opened, &file_ref, into, ask)?]
        }
        None => {
            crate::config::check_project_revision(&root, ask.expected.as_deref())?;
            let retired = |s: &str| {
                Id::parse(s, "part")
                    .is_ok_and(|id| crate::pdm::check_not_retired(&root, &id).is_err())
            };
            let part = match &ask.part {
                Some(p) => {
                    let pid = crate::part::part_id(p)?;
                    if project.parts.contains_key(&pid) {
                        return Err(Error::new(
                            Code::DuplicateId,
                            format!("part {pid} already exists"),
                        )
                        .with("part", &pid));
                    }
                    crate::pdm::check_not_retired(&root, &pid)?;
                    Some(pid)
                }
                None => None,
            };
            let stem = std::path::Path::new(&stored)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let taken = |s: &str| {
                Id::parse(s, "part").is_ok_and(|id| project.parts.contains_key(&id)) || retired(s)
            };
            if ask.flatten {
                vec![
                    plan::Flat {
                        opened,
                        file: &file_ref,
                        stem: &stem,
                        part,
                        taken: &taken,
                        pdm: root.join(PDM_CONFIG).is_file(),
                        mode: ask.mode(),
                        cross: None,
                    }
                    .plan()?,
                ]
            } else {
                plan::Fresh {
                    opened,
                    file: &file_ref,
                    stem: &stem,
                    part,
                    taken: &taken,
                    pdm: root.join(PDM_CONFIG).is_file(),
                    mode: ask.mode(),
                    cross: plan::Cross::default(),
                }
                .plan()?
            }
        }
    };
    let applied = crate::file_props::apply(
        project,
        &*session,
        opened,
        None,
        &stored,
        &mut plan,
        if ask.into.is_some() {
            crate::file_props::Mode::Into
        } else {
            crate::file_props::Mode::Fresh
        },
    );
    let mut warnings = mapped_warnings(opened, &ask.path);
    warnings.extend(applied.warnings.iter().cloned());
    let mut writes = plan::writes(project, &plan)?;
    if !reused {
        writes.insert(stored.clone(), Some(file.bytes.to_vec()));
    }
    let head = json!({"path": stored, "blake3": file.blake3, "reused": reused,
                      "schema": schema_name(opened.schema)});
    if ask.dry_run {
        let mut s = session.proposed(&writes)?;
        let parts = parts_report(&mut s, opened, &plan)?;
        let transfer = transfer_of(opened, &stored, &plan, &parts, &applied.instances);
        return Ok(answer(head, opened, parts, warnings, true, transfer));
    }
    // PDM names need the project's ID; Git's attributes keep the file's
    // bytes once it is there.
    // A cancel stops it up to here; once a file is written it finishes.
    session.job().check()?;
    session.job().writing();
    if plan.iter().any(|pl| !pl.properties.is_empty()) {
        crate::pdm::ensure_id(session)?;
    }
    plan::apply(&root, &writes)?;
    crate::drivers::write_attributes(&root)?;
    session.reload()?;
    let parts = parts_report(session, opened, &plan)?;
    let transfer = transfer_of(opened, &stored, &plan, &parts, &applied.instances);
    Ok(answer(head, opened, parts, warnings, false, transfer))
}

/// `import --update <stored> <file>`: `file`'s bytes replace the stored
/// file, and the parts and assemblies made of it follow, in one change
/// (see [`update`]). The answer lists what changed, the products the file
/// no longer has, instances kept because something reads them, and the
/// features that would build now and won't after, found by rebuilding
/// the planned files before anything is written.
pub fn reimport(session: &mut crate::session::Session, ask: Ask) -> Result<Value> {
    let target = ask.update.clone().expect("an update");
    if ask.part.is_some() || ask.into.is_some() || ask.product.is_some() || ask.flatten {
        return Err(Error::new(
            Code::Usage,
            "an update replaces a file the project holds: part, into, product and flatten don't apply (a flattened part follows its file as it was imported)",
        ));
    }
    let root = session.root().to_path_buf();
    crate::config::check_project_revision(&root, ask.expected.as_deref())?;
    let Some(old) = session.project().imports.get(&target).cloned() else {
        return Err(Error::new(
            Code::NotFound,
            format!("{target} isn't a file the project has imported"),
        )
        .with("path", &target));
    };
    if let Some((part, files)) = update::cross_flattened(session.project(), &target) {
        return Err(Error::new(
            Code::Usage,
            format!(
                "part {part} is flattened across files ({}): an update can't walk it again yet; delete it and import the assembly again with --flatten",
                files.join(", ")
            ),
        )
        .with("part", &part)
        .with("files", files));
    }
    let source = {
        let p = std::path::Path::new(&ask.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };
    let bytes = std::fs::read(&source).map_err(|e| Error::io(&source, &e))?;
    let file = ImportFile::new(bytes);
    let opened = opened(&file).map_err(|e| unreadable(&ask.path, &e))?;
    let search = search_dirs(&root, &ask.search);
    let tree = external::resolve(&source, file.clone(), opened.clone(), &search, &ask.path)
        .map_err(|e| refused(e, &opened, &ask.path))?;
    if tree.nodes.len() > 1 {
        return import_tree(session, &ask, root, &tree, Some(&target))
            .map_err(|e| refused(e, &opened, &ask.path));
    }
    reimport_opened(session, &ask, root, target, old, file, &opened)
        .map_err(|e| refused(e, &opened, &ask.path))
}

/// [`reimport`] once the file is read.
fn reimport_opened(
    session: &mut crate::session::Session,
    ask: &Ask,
    root: std::path::PathBuf,
    target: String,
    old: ImportFile,
    file: ImportFile,
    opened: &Opened,
) -> Result<Value> {
    use crate::doc::import::FileRef;
    use crate::doc::project::PDM_CONFIG;
    let file_ref = FileRef {
        path: target.clone(),
        blake3: file.blake3.clone(),
    };
    let project = session.project();
    let stem = std::path::Path::new(&target)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let taken = |s: &str| {
        Id::parse(s, "part").is_ok_and(|id| {
            project.parts.contains_key(&id) || crate::pdm::check_not_retired(&root, &id).is_err()
        })
    };
    let fresh = plan::Fresh {
        opened,
        file: &file_ref,
        stem: &stem,
        part: None,
        taken: &taken,
        pdm: root.join(PDM_CONFIG).is_file(),
        mode: ask.mode(),
        cross: plan::Cross::default(),
    };
    let mut u = update::plan(project, opened, &file_ref, &fresh)?;
    let old_opened = self::opened(&old).ok();
    let applied = crate::file_props::apply(
        project,
        &*session,
        opened,
        old_opened.as_deref(),
        &target,
        &mut u.planned,
        crate::file_props::Mode::Update,
    );
    let mut writes = plan::writes(project, &u.planned)?;
    if old.blake3 != file.blake3 {
        writes.insert(target.clone(), Some(file.bytes.to_vec()));
    }
    let mut warnings = mapped_warnings(opened, &ask.path);
    warnings.extend(u.warnings.iter().cloned());
    for (a, i) in &u.kept {
        warnings.push(format!(
            "import_usage_missing: {a}/{i}'s usage is gone from {target}, but a mate or pattern reads it, so it stays"
        ));
    }
    for (p, f) in &u.kept_bodies {
        warnings.push(format!(
            "import_usage_missing: {p}/{f}'s body is gone from {target}, but another feature reads it, so it stays"
        ));
    }
    warnings.extend(applied.warnings.iter().cloned());
    // What builds now and wouldn't after, from the files as they would be.
    let now = failing(session)?;
    let mut proposed = session.proposed(&writes)?;
    let after = failing(&mut proposed)?;
    let newly: Vec<Value> = after
        .into_iter()
        .filter(|(k, _)| !now.contains_key(k))
        .map(|((part, feature), e)| json!({"part": part, "feature": feature, "error": e}))
        .collect();
    let head = json!({"path": target, "blake3": file.blake3, "from": old.blake3,
                      "schema": schema_name(opened.schema)});
    let extra = |mut v: Value, assemblies: Option<Vec<Value>>| {
        v["update"] = json!(true);
        let vanished: Vec<Value> = u
            .vanished
            .iter()
            .map(|(r, parts)| json!({"product": product_json(r), "parts": parts}))
            .collect();
        if !vanished.is_empty() {
            v["vanished"] = json!(vanished);
        }
        if !u.kept.is_empty() || !u.kept_bodies.is_empty() {
            let kept: Vec<Value> = u
                .kept
                .iter()
                .map(|(a, i)| json!({"part": a, "instance": i}))
                .chain(
                    u.kept_bodies
                        .iter()
                        .map(|(p, f)| json!({"part": p, "feature": f})),
                )
                .collect();
            v["kept"] = json!(kept);
        }
        if !newly.is_empty() {
            v["failing"] = json!(newly);
        }
        if let Some(a) = assemblies.filter(|a| !a.is_empty()) {
            v["assemblies"] = json!(a);
        }
        v
    };
    if ask.dry_run {
        let parts = parts_report(&mut proposed, opened, &u.planned)?;
        let transfer = transfer_of(opened, &target, &u.planned, &parts, &applied.instances);
        return Ok(extra(
            answer(head, opened, parts, warnings, true, transfer),
            None,
        ));
    }
    // Assemblies kept current until now stay so: the moves this writes
    // are solved again in the same change.
    let mut current = Vec::new();
    for pl in u.planned.iter().filter(|pl| !pl.new) {
        for a in crate::mate::current(session, &pl.part)? {
            if !current.contains(&a) {
                current.push(a);
            }
        }
    }
    // A cancel stops it up to here; once a file is written it finishes.
    session.job().check()?;
    session.job().writing();
    if u.planned.iter().any(|pl| !pl.properties.is_empty()) {
        crate::pdm::ensure_id(session)?;
    }
    plan::apply(&root, &writes)?;
    session.reload()?;
    let assemblies = crate::mate::keep_current(session, &current)?;
    let parts = parts_report(session, opened, &u.planned)?;
    let transfer = transfer_of(opened, &target, &u.planned, &parts, &applied.instances);
    Ok(extra(
        answer(head, opened, parts, warnings, false, transfer),
        Some(assemblies),
    ))
}

/// The parts every group plans as a session rebuilds them, the first
/// file's transfer report, and each other file's line in `external`:
/// `{path, blake3, reused | from, source, documents, stored, transfer}`.
fn tree_report(
    s: &mut crate::session::Session,
    tree: &external::Tree,
    groups: &[external::Group],
    interpreted: &[Vec<u64>],
) -> Result<(Vec<Value>, Value, Vec<Value>)> {
    let mut parts = Vec::new();
    let mut all: Vec<Planned> = Vec::new();
    for g in groups {
        parts.extend(parts_report(s, &tree.nodes[g.node].opened, &g.planned)?);
        all.extend(g.planned.iter().cloned());
    }
    let mut root = Value::Null;
    let mut listed = Vec::new();
    for g in groups {
        let node = &tree.nodes[g.node];
        let t = transfer_of(&node.opened, &g.path, &all, &parts, &interpreted[g.node]);
        if g.node == 0 {
            root = t;
            continue;
        }
        let mut v = json!({
            "path": g.path,
            "blake3": node.file.blake3,
            "source": node.source.display().to_string(),
            "documents": node.documents,
            "stored": external::names(&all, &g.path),
            "transfer": t,
        });
        match &g.from {
            Some(f) => v["from"] = json!(f),
            None => v["reused"] = json!(g.reused),
        }
        listed.push(v);
    }
    Ok((parts, root, listed))
}

/// [`import`] and [`reimport`] of a file whose products' shapes are in
/// other files (design 448, plan 24.D, #443): every file of `tree`
/// planned as an import of its own, leaves first
/// ([`external::Planner`]), and written as one change. `update` is the
/// stored file an update replaces.
fn import_tree(
    session: &mut crate::session::Session,
    ask: &Ask,
    root: std::path::PathBuf,
    tree: &external::Tree,
    update: Option<&str>,
) -> Result<Value> {
    use crate::doc::project::PDM_CONFIG;
    if ask.into.is_some() || ask.product.is_some() {
        return Err(Error::new(
            Code::Usage,
            "into and product take one product's bodies from one file, and this file's products are in other files: import the file holding the product --into the part",
        ));
    }
    if update.is_none() {
        crate::config::check_project_revision(&root, ask.expected.as_deref())?;
    }
    let (groups, writes, warnings, interpreted, kept, vanished) = {
        let project = session.project();
        let part = match (&ask.part, update) {
            (Some(p), None) => {
                let pid = crate::part::part_id(p)?;
                if project.parts.contains_key(&pid) {
                    return Err(Error::new(
                        Code::DuplicateId,
                        format!("part {pid} already exists"),
                    )
                    .with("part", &pid));
                }
                crate::pdm::check_not_retired(&root, &pid)?;
                Some(pid)
            }
            _ => None,
        };
        let taken = |s: &str| {
            Id::parse(s, "part").is_ok_and(|id| {
                project.parts.contains_key(&id)
                    || crate::pdm::check_not_retired(&root, &id).is_err()
            })
        };
        let mut planner = external::Planner::new(
            project,
            tree,
            ask.mode(),
            root.join(PDM_CONFIG).is_file(),
            part,
            &taken,
        );
        if ask.flatten {
            planner.flatten()?;
        } else {
            planner.visit(0, update.map(str::to_owned))?;
        }
        let mut groups = std::mem::take(&mut planner.groups);
        groups.sort_by_key(|g| g.node);
        let mut warnings = Vec::new();
        let mut interpreted: Vec<Vec<u64>> =
            tree.nodes.iter().map(|n| n.interpreted.clone()).collect();
        let (mut kept, mut vanished) = (Vec::new(), Vec::new());
        for g in &mut groups {
            let node = &tree.nodes[g.node];
            let old = g
                .from
                .as_ref()
                .and_then(|_| project.imports.get(&g.path))
                .and_then(|f| opened(f).ok());
            let applied = crate::file_props::apply(
                project,
                &*session,
                &node.opened,
                old.as_deref(),
                &g.path,
                &mut g.planned,
                if g.from.is_some() {
                    crate::file_props::Mode::Update
                } else {
                    crate::file_props::Mode::Fresh
                },
            );
            warnings.extend(mapped_warnings(&node.opened, &g.path));
            if let Some(u) = &g.update {
                warnings.extend(u.warnings.iter().cloned());
                for (a, i) in &u.kept {
                    warnings.push(format!(
                        "import_usage_missing: {a}/{i}'s usage is gone from {}, but a mate or pattern reads it, so it stays",
                        g.path
                    ));
                    kept.push(json!({"part": a, "instance": i}));
                }
                for (p, f) in &u.kept_bodies {
                    warnings.push(format!(
                        "import_usage_missing: {p}/{f}'s body is gone from {}, but another feature reads it, so it stays",
                        g.path
                    ));
                    kept.push(json!({"part": p, "feature": f}));
                }
                for (r, parts) in &u.vanished {
                    vanished
                        .push(json!({"file": g.path, "product": product_json(r), "parts": parts}));
                }
            }
            warnings.extend(applied.warnings.iter().cloned());
            interpreted[g.node].extend(applied.instances.iter().copied());
        }
        let all = external::merge(
            groups
                .iter()
                .flat_map(|g| g.planned.iter().cloned())
                .collect(),
        );
        let mut writes = plan::writes(project, &all)?;
        for g in &groups {
            let node = &tree.nodes[g.node];
            if g.writes_bytes(&all, &node.file.blake3) {
                writes.insert(g.path.clone(), Some(node.file.bytes.to_vec()));
            }
        }
        (groups, writes, warnings, interpreted, kept, vanished)
    };
    let opened0 = &tree.nodes[0].opened;
    let g0 = &groups[0];
    let all: Vec<Planned> = groups
        .iter()
        .flat_map(|g| g.planned.iter().cloned())
        .collect();
    let mut head = json!({"path": g0.path, "blake3": tree.nodes[0].file.blake3,
                          "schema": schema_name(opened0.schema)});
    match &g0.from {
        Some(f) => head["from"] = json!(f),
        None => head["reused"] = json!(g0.reused),
    }
    if !external::names(&all, &g0.path) {
        head["stored"] = json!(false);
    }
    // What builds now and wouldn't after, for an update.
    let newly: Vec<Value> = match update {
        Some(_) => {
            let now = failing(session)?;
            let mut proposed = session.proposed(&writes)?;
            let after = failing(&mut proposed)?;
            after
                .into_iter()
                .filter(|(k, _)| !now.contains_key(k))
                .map(|((part, feature), e)| json!({"part": part, "feature": feature, "error": e}))
                .collect()
        }
        None => Vec::new(),
    };
    let extra = |mut v: Value, listed: Vec<Value>, assemblies: Option<Vec<Value>>| {
        v["external"] = json!(listed);
        if update.is_some() {
            v["update"] = json!(true);
        }
        if !vanished.is_empty() {
            v["vanished"] = json!(vanished);
        }
        if !kept.is_empty() {
            v["kept"] = json!(kept);
        }
        if !newly.is_empty() {
            v["failing"] = json!(newly);
        }
        if let Some(a) = assemblies.filter(|a| !a.is_empty()) {
            v["assemblies"] = json!(a);
        }
        v
    };
    if ask.dry_run {
        let mut s = session.proposed(&writes)?;
        let (parts, transfer, listed) = tree_report(&mut s, tree, &groups, &interpreted)?;
        return Ok(extra(
            answer(head, opened0, parts, warnings, true, transfer),
            listed,
            None,
        ));
    }
    // Assemblies kept current until now stay so (an update).
    let mut current = Vec::new();
    if update.is_some() {
        for pl in all.iter().filter(|pl| !pl.new) {
            for a in crate::mate::current(session, &pl.part)? {
                if !current.contains(&a) {
                    current.push(a);
                }
            }
        }
    }
    // A cancel stops it up to here; once a file is written it finishes.
    session.job().check()?;
    session.job().writing();
    if all.iter().any(|pl| !pl.properties.is_empty()) {
        crate::pdm::ensure_id(session)?;
    }
    plan::apply(&root, &writes)?;
    crate::drivers::write_attributes(&root)?;
    session.reload()?;
    let assemblies = match update {
        Some(_) => Some(crate::mate::keep_current(session, &current)?),
        None => None,
    };
    let (parts, transfer, listed) = tree_report(session, tree, &groups, &interpreted)?;
    Ok(extra(
        answer(head, opened0, parts, warnings, false, transfer),
        listed,
        assemblies,
    ))
}

/// A refusal once the file is read gains the file's transfer report,
/// nothing interpreted (design 448). Its code and other details don't
/// change, so callers that read codes see no change.
fn refused(e: Error, opened: &Opened, path: &str) -> Error {
    if e.details.get("transfer").is_some() {
        return e;
    }
    let t = opened
        .census
        .transfer(&ryustep::census::Read::default(), &[]);
    e.with("transfer", crate::transfer::json(&t, path, false))
}

/// What `import --update` writes when the bytes already at `path` (a
/// file replaced by hand) are the new version: `doctor`'s `import_update`
/// repair. `None` when they aren't a STEP file this build reads.
pub fn update_in_place(root: &std::path::Path, path: &str) -> Result<Option<Writes>> {
    use crate::doc::import::FileRef;
    use crate::doc::project::{PDM_CONFIG, Project};
    let project = Project::load(root)?;
    if update::cross_flattened(&project, path).is_some() {
        return Ok(None);
    }
    let Some(file) = project.imports.get(path) else {
        return Ok(None);
    };
    let Ok(opened) = opened(file) else {
        return Ok(None);
    };
    let file_ref = FileRef {
        path: path.to_owned(),
        blake3: file.blake3.clone(),
    };
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let taken = |s: &str| {
        Id::parse(s, "part").is_ok_and(|id| {
            project.parts.contains_key(&id) || crate::pdm::check_not_retired(root, &id).is_err()
        })
    };
    let fresh = plan::Fresh {
        opened: &opened,
        file: &file_ref,
        stem: &stem,
        part: None,
        taken: &taken,
        pdm: root.join(PDM_CONFIG).is_file(),
        // Doctor's repair asks for nothing: new products are references,
        // as an import's are by default.
        mode: Some(crate::doc::import::Mode::Reference),
        cross: plan::Cross {
            external: update::external_parts(&project, path, &opened, &opened),
            ..Default::default()
        },
    };
    let u = update::plan(&project, &opened, &file_ref, &fresh)?;
    Ok(Some(plan::writes(&project, &u.planned)?))
}

/// A fixed instance an import made that isn't where the bytes now in its
/// file place its usage: what `import --update` with those bytes would
/// move (`import_misplaced`). A merge leaves one when it takes the file's
/// bytes from one side and the instance's pose, solver state, from the
/// other.
#[derive(Clone, Debug, PartialEq)]
pub struct Misplaced {
    /// The imported file, under `imports/`.
    pub file: String,
    pub assembly: Id,
    pub instance: Id,
    /// The instance as it is, with the pose its file gives it.
    pub placed: crate::doc::assembly::Instance,
}

/// Every fixed instance of `project` an import made whose pose isn't the
/// one the bytes in its file give its usage, planned as an update with
/// those bytes in place plans it (doctor's `import_update`), by assembly
/// and instance. Files are opened ([`opened`]: parsed and decoded, never
/// healed, kept by BLAKE3), and only those a fixed imported instance
/// names. A file whose import features name other bytes is
/// `import_file_changed`'s, and one this build can't read or plan is left
/// out.
pub fn misplaced(project: &crate::doc::project::Project) -> Vec<Misplaced> {
    use crate::doc::files::Feature;
    use crate::doc::import::FileRef;
    let features = || project.parts.values().flat_map(|p| p.features.values());
    let files: std::collections::BTreeSet<&str> = features()
        .filter_map(|f| match f {
            Ok(Feature::Instance(i)) if i.fixed => i.import.as_ref().map(|u| u.file.as_str()),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for path in files {
        let Some(file) = project.imports.get(path) else {
            continue;
        };
        let other_bytes = features().any(|f| {
            matches!(f, Ok(Feature::Import(i)) if i.file.path == path && i.file.blake3 != file.blake3)
        });
        if other_bytes {
            continue;
        }
        let Ok(opened) = opened(file) else {
            continue;
        };
        let file_ref = FileRef {
            path: path.to_owned(),
            blake3: file.blake3.clone(),
        };
        let taken = |s: &str| Id::parse(s, "part").is_ok_and(|id| project.parts.contains_key(&id));
        let fresh = plan::Fresh {
            opened: &opened,
            file: &file_ref,
            stem: "",
            part: None,
            taken: &taken,
            pdm: false,
            mode: None,
            cross: plan::Cross {
                external: update::external_parts(project, path, &opened, &opened),
                ..Default::default()
            },
        };
        let Ok(u) = update::plan(project, &opened, &file_ref, &fresh) else {
            continue;
        };
        for pl in u.planned.iter().filter(|pl| !pl.new) {
            let Ok(part) = project.part(&pl.part) else {
                continue;
            };
            for (fid, f) in &pl.features {
                let (Feature::Instance(new), Some(Ok(Feature::Instance(old)))) =
                    (f, part.features.get(fid))
                else {
                    continue;
                };
                if old.fixed && old.pose != new.pose {
                    out.push(Misplaced {
                        file: path.to_owned(),
                        assembly: pl.part.clone(),
                        instance: fid.clone(),
                        placed: crate::doc::assembly::Instance {
                            pose: new.pose,
                            ..old.clone()
                        },
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.assembly, &a.instance).cmp(&(&b.assembly, &b.instance)));
    out
}

/// Every feature of every part that fails in `s`, with its error, by part
/// and feature.
pub(crate) fn failing(s: &mut crate::session::Session) -> Result<BTreeMap<(Id, Id), Value>> {
    let ids: Vec<Id> = s.project().parts.keys().cloned().collect();
    s.rebuild_parts(&ids)?;
    let mut out = BTreeMap::new();
    for id in ids {
        for (f, r) in &s.rebuilt(&id)?.features {
            if let Err(e) = r {
                out.insert((id.clone(), f.clone()), json!(e));
            }
        }
    }
    Ok(out)
}

/// The plan of `import --into PART`: one product's bodies (`product` when
/// the file has several) as import features after the part's last.
fn into_plan(
    session: &crate::session::Session,
    opened: &Opened,
    file: &crate::doc::import::FileRef,
    into: &str,
    ask: &Ask,
) -> Result<Planned> {
    use crate::doc::files::PartKind;
    let defs = &opened.products.definitions;
    // Each group of bodies: a product's, then those no product reaches.
    let mut groups: Vec<Option<usize>> = (0..defs.len())
        .filter(|k| opened.held.iter().any(|h| h.definition == Some(*k)))
        .map(Some)
        .collect();
    if opened.held.iter().any(|h| h.definition.is_none()) {
        groups.push(None);
    }
    if groups.is_empty() {
        return Err(body_missing(opened, &ask.path));
    }
    let pid = crate::part::part_id(into)?;
    let p = session.project().part(&pid)?;
    crate::part::check_revision(p, ask.expected.as_deref())?;
    // A list that doesn't load (a conflict record, a newer format) would
    // be written over with only the import's features.
    crate::part::order(p)?;
    if crate::part::part_kind(p) == PartKind::Assembly {
        return Err(Error::new(
            Code::WrongPartKind,
            format!("{pid} is an assembly: imported bodies go into a part"),
        )
        .with("part", &pid));
    }
    let listed = |k: &Option<usize>| match k {
        Some(k) => json!({"id": &*defs[*k].product_id, "name": &*defs[*k].name}),
        None => json!(null),
    };
    let k = match &ask.product {
        Some(want) => {
            let by_id: Vec<Option<usize>> = groups
                .iter()
                .copied()
                .filter(|k| k.is_some_and(|k| *defs[k].product_id == **want))
                .collect();
            let by_name: Vec<Option<usize>> = groups
                .iter()
                .copied()
                .filter(|k| k.is_some_and(|k| *defs[k].name == **want))
                .collect();
            match (by_id.as_slice(), by_name.as_slice()) {
                ([k], _) | ([], [k]) => *k,
                _ => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!(
                            "{} has {} products {want:?} names; name one by its id",
                            ask.path,
                            by_id.len().max(by_name.len())
                        ),
                    )
                    .with("param", "product")
                    .with("products", groups.iter().map(listed).collect::<Vec<_>>()));
                }
            }
        }
        None => match groups.as_slice() {
            [k] => *k,
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "{} holds bodies of {} products: name the one to add with product",
                        ask.path,
                        groups.len()
                    ),
                )
                .with("param", "product")
                .with("products", groups.iter().map(listed).collect::<Vec<_>>()));
            }
        },
    };
    let mut taken: Vec<String> = p.features.keys().map(|f| f.to_string()).collect();
    let product = k.map(|k| plan::product_of(opened, k));
    let features = plan::import_features(
        file,
        product,
        ask.mode(),
        &plan::body_names(opened, k),
        0..plan::bodies(opened, k),
        &mut taken,
    );
    Ok(Planned {
        part: pid,
        new: false,
        kind: PartKind::Part,
        definition: k,
        features,
        removed: Vec::new(),
        properties: BTreeMap::new(),
        props_of: k,
    })
}

/// `import_mapped_skipped`, when products of the file place shapes by
/// mapped items.
fn mapped_warnings(opened: &Opened, path: &str) -> Vec<String> {
    let mapped = opened
        .products
        .definitions
        .iter()
        .filter(|d| d.shapes.iter().any(|s| !s.mapped.is_empty()))
        .count();
    if mapped == 0 {
        return Vec::new();
    }
    vec![format!(
        "import_mapped_skipped: {mapped} product(s) of {path} place shapes by mapped items, which aren't imported yet"
    )]
}

/// Each planned part as `s` rebuilds it: its kind, its product, the
/// features written (each import feature with its counts and heal
/// counts, each instance with its part and pose, or the error), those
/// removed, and its properties. Parts heal side by side first.
fn parts_report(
    s: &mut crate::session::Session,
    opened: &Opened,
    plan: &[Planned],
) -> Result<Vec<Value>> {
    use crate::doc::files::{Feature, PartKind};
    let plain: Vec<Id> = plan
        .iter()
        .filter(|pl| pl.kind == PartKind::Part)
        .map(|pl| pl.part.clone())
        .collect();
    s.rebuild_parts(&plain)?;
    let mut out = Vec::new();
    for pl in plan {
        let r = s.rebuilt(&pl.part)?.clone();
        let features: Vec<Value> = pl
            .features
            .iter()
            .map(|(fid, f)| {
                let mut v = json!({"id": fid, "type": f.type_name()});
                match f {
                    Feature::Import(i) => v["body"] = json!(i.body),
                    Feature::Instance(i) => {
                        v["of"] = json!(i.part);
                        if let Some(u) = i.import.as_ref().and_then(|u| u.usage.as_ref()) {
                            v["usage"] = json!(u);
                        }
                        v["pose"] = i.pose.to_json();
                    }
                    _ => {}
                }
                match r.features.iter().find(|(f, _)| f == fid).map(|(_, o)| o) {
                    Some(Ok(crate::rebuild::Output::Solids(m))) => {
                        v["ok"] = json!(true);
                        v["hash"] = json!(m.hash);
                        if let Some(b) = m.solids.first() {
                            let c = b.1.body.counts();
                            v["counts"] = json!({"faces": c.faces, "edges": c.edges,
                                                 "vertices": c.vertices});
                        }
                        if let Some(x) = &m.report {
                            if x.get("reference").is_some() {
                                v["mode"] = json!("reference");
                                v["reference"] = x["reference"].clone();
                            } else {
                                v["heal"] = x["heal"].clone();
                            }
                        }
                    }
                    Some(Ok(_)) => v["ok"] = json!(true),
                    Some(Err(e)) => {
                        v["ok"] = json!(false);
                        v["error"] = json!(e);
                    }
                    None => {}
                }
                v
            })
            .collect();
        let mut v = json!({"part": pl.part, "kind": pl.kind.as_str(), "created": pl.new,
                           "features": features});
        if let Some(k) = pl.definition {
            v["product"] = product_json(&plan::product_of(opened, k));
        }
        if !pl.removed.is_empty() {
            v["removed"] = json!(pl.removed);
        }
        if !pl.properties.is_empty() {
            v["properties"] = json!(pl.properties);
        }
        out.push(v);
    }
    Ok(out)
}

/// The transfer report of `plan` over `opened` (design 448): each import
/// feature that built read its body's representation; the product tree
/// was read when anything was made; each failing feature's error names
/// its instances (`step`) with its code. `parts` is `parts_report`'s, in
/// `plan`'s order.
fn transfer_of(
    opened: &Opened,
    path: &str,
    plan: &[Planned],
    parts: &[Value],
    props: &[u64],
) -> Value {
    use crate::doc::files::Feature;
    use std::collections::BTreeSet;
    let mut read = ryustep::census::Read {
        product_tree: !plan.is_empty(),
        ..Default::default()
    };
    let mut failed = Vec::new();
    let (mut bodies, mut made, mut vias) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for (pl, p) in plan.iter().zip(parts) {
        for (fid, f) in &pl.features {
            let i = match f {
                Feature::Import(i) if i.file.path == path => i,
                Feature::Instance(inst) => {
                    if let (Some(k), Some(u)) = (pl.definition, &inst.import)
                        && u.file == path
                        && let Some(n) = crate::file_styles::usage_index(opened, k, u)
                    {
                        made.insert((k, n));
                    }
                    continue;
                }
                _ => continue,
            };
            let Some(row) = p["features"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["id"] == fid.as_str())
            else {
                continue;
            };
            if row["ok"] == true {
                if let Ok(k) = select(fid, i, opened) {
                    read.roots.extend(opened.roots(k));
                    bodies.insert(k);
                    if let Some(place) = &i.place {
                        vias.insert(place.via.clone());
                    }
                }
            } else {
                failed.extend(crate::transfer::failed_instances(&row["error"]));
            }
        }
    }
    let (instances, unplaced) = crate::file_styles::interpreted(opened, &bodies, &made, &vias);
    read.instances = instances;
    read.instances.extend(props);
    read.instances.sort_unstable();
    read.instances.dedup();
    failed.extend(crate::file_props::failed(opened));
    let mut v = crate::transfer::json(&opened.census.transfer(&read, &failed), path, false);
    crate::transfer::with_unplaced(&mut v, unplaced);
    v
}

/// The answer: the stored file (`head`), the parts, the file's product
/// tree faults, warnings (those given, then each import feature that
/// doesn't build: `import_failed`) and the transfer report.
fn answer(
    head: Value,
    opened: &Opened,
    parts: Vec<Value>,
    warnings: Vec<String>,
    dry_run: bool,
    transfer: Value,
) -> Value {
    let failed: Vec<String> = parts
        .iter()
        .flat_map(|p| {
            p["features"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| f["ok"] == false && f["type"] == "import")
                .map(move |f| {
                    // A body past its heal budget warns with its own code.
                    let code = if f["error"]["code"] == "import_budget_exceeded" {
                        "import_budget_exceeded"
                    } else {
                        "import_failed"
                    };
                    format!(
                        "{code}: {}/{} doesn't build: {}",
                        p["part"].as_str().unwrap_or_default(),
                        f["id"].as_str().unwrap_or_default(),
                        f["error"]["message"].as_str().unwrap_or_default()
                    )
                })
        })
        .collect();
    let mut v = json!({"file": head, "parts": parts, "dry_run": dry_run});
    let faults: Vec<String> = opened
        .products
        .faults
        .iter()
        .map(|f| f.to_string())
        .collect();
    if !faults.is_empty() {
        v["faults"] = json!(faults);
    }
    let warnings: Vec<String> = warnings.into_iter().chain(failed).collect();
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    v["transfer"] = transfer;
    v
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::assembly::Pose;
    use crate::doc::import::FileRef;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    pub(crate) fn occt(name: &str) -> Vec<u8> {
        let path = format!(
            "{}/../ryustep/tests/data/occt/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(path).unwrap()
    }

    fn import(file: &ImportFile, product: Option<(&str, &str)>, body: u32) -> Import {
        Import {
            file: FileRef {
                path: "imports/f.step".into(),
                blake3: file.blake3.clone(),
            },
            product: product.map(|(i, n)| ProductRef {
                id: i.into(),
                name: n.into(),
                ordinal: 0,
            }),
            body,
            mode: None,
            place: None,
        }
    }

    #[test]
    fn files_are_opened_once() {
        let f = ImportFile::new(occt("plate_hole.stp"));
        let a = opened(&f).unwrap();
        let b = opened(&f).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let bad = ImportFile::new(b"not step".to_vec());
        let e = opened(&bad).err().unwrap();
        let e = unreadable("imports/bad.step", &e);
        assert_eq!(e.code, Code::StepUnreadable);
        assert_eq!(e.details["reason"], "not_part21");
    }

    #[test]
    fn products_and_bodies_are_selected_or_refused() {
        let f = ImportFile::new(occt("assembly.stp"));
        let o = opened(&f).unwrap();
        let d = &o.products.definitions;
        let k = o.held.iter().find_map(|h| h.definition).unwrap();
        let first = &d[k];
        let p = Some((&*first.product_id, &*first.name));
        let k = select(&id("i"), &import(&f, p, 0), &o).unwrap();
        assert!(o.held[k].definition.is_some());
        let e = select(&id("i"), &import(&f, p, 9), &o).unwrap_err();
        assert_eq!(e.code, Code::ImportBodyMissing);
        assert_eq!(e.details["body"], 9);
        let e = select(&id("i"), &import(&f, Some(("nope", "Nope")), 0), &o).unwrap_err();
        assert_eq!(e.code, Code::ImportProductMissing);
        assert_eq!(e.details["products"].as_array().unwrap().len(), d.len());
        let e = select(&id("i"), &import(&f, None, 0), &o).unwrap_err();
        assert_eq!(e.code, Code::ImportBodyMissing);
    }

    /// OCCT's 10 × 20 × 30 mm box with a copy of its solid moved by `d` mm,
    /// both items of its one representation: `[10, 0, 0]` makes two solids
    /// touching on the plane x = 10 mm, as multi-body files from other
    /// programs hold them (#208).
    pub(crate) fn shifted_boxes(d: [f64; 3]) -> Vec<u8> {
        use ryustep::part21::{Instance, Param, Record, parse, write};
        let mut file = parse(&occt("box_ap242.stp")).unwrap();
        let rep = file
            .instances
            .iter()
            .find(|i| &*i.record.parts()[0].name == "ADVANCED_BREP_SHAPE_REPRESENTATION")
            .unwrap()
            .id;
        let solid = file
            .instances
            .iter()
            .find(|i| &*i.record.parts()[0].name == "MANIFOLD_SOLID_BREP")
            .unwrap()
            .id;
        // Every instance the solid reaches, copied under new numbers.
        let offset = file.instances.last().unwrap().id;
        let mut todo = vec![solid];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            fn refs(p: &Param, out: &mut Vec<u64>) {
                match p {
                    Param::Ref(r) => out.push(*r),
                    Param::List(l) => l.iter().for_each(|q| refs(q, out)),
                    Param::Typed(_, q) => refs(q, out),
                    _ => {}
                }
            }
            for part in file.get(id).unwrap().record.parts() {
                part.params.iter().for_each(|p| refs(p, &mut todo));
            }
        }
        fn moved(p: &Param, offset: u64) -> Param {
            match p {
                Param::Ref(r) => Param::Ref(r + offset),
                Param::List(l) => Param::List(l.iter().map(|q| moved(q, offset)).collect()),
                Param::Typed(n, q) => Param::Typed(n.clone(), Box::new(moved(q, offset))),
                q => q.clone(),
            }
        }
        let mut copies = Vec::new();
        for &id in &seen {
            let mut record = file.get(id).unwrap().record.clone();
            let parts: Vec<&mut ryustep::part21::Part> = match &mut record {
                Record::Simple(p) => vec![p],
                Record::Complex(ps) => ps.iter_mut().collect(),
            };
            for part in parts {
                part.params = part.params.iter().map(|p| moved(p, offset)).collect();
                if &*part.name == "CARTESIAN_POINT"
                    && let Param::List(xyz) = &mut part.params[1]
                {
                    for (c, d) in xyz.iter_mut().zip(d) {
                        if let Param::Real(x) = c {
                            *x += d;
                        }
                    }
                }
            }
            copies.push(Instance {
                id: id + offset,
                record,
            });
        }
        file.instances.extend(copies);
        let k = file.instances.iter().position(|i| i.id == rep).unwrap();
        let Record::Simple(part) = &mut file.instances[k].record else {
            panic!("a simple representation");
        };
        let Param::List(items) = &mut part.params[1] else {
            panic!("a list of items");
        };
        items.push(Param::Ref(solid + offset));
        write(&file).unwrap()
    }

    #[test]
    fn touching_and_crossing_solids_import_as_their_union() {
        // #315 and #208: solids of one representation that touch on a
        // face are healed alone and united: one solid. #319: so are
        // solids whose volumes cross, sharing no face (plan 14.C; v0.7.x
        // still refuses them, naming both solids).
        for d in [[10.0, 0.0, 0.0], [5.0, 5.0, 5.0]] {
            let (_dir, mut s) = with_import(&shifted_boxes(d), Some(("part 1", "part 1")));
            let v = rebuild(&mut s);
            let f = &v["features"][0];
            assert_eq!(f["ok"], true, "{d:?}: {v}");
        }
    }

    /// A project whose part `p` lists one import `i` of `imports/f.step`
    /// holding `bytes`, written against them.
    pub(crate) fn with_import(
        bytes: &[u8],
        product: Option<(&str, &str)>,
    ) -> (tempfile::TempDir, crate::session::Session) {
        with_import_as(bytes, product, None)
    }

    /// [`with_import`], the feature in `mode`.
    pub(crate) fn with_import_as(
        bytes: &[u8],
        product: Option<(&str, &str)>,
        mode: Option<crate::doc::import::Mode>,
    ) -> (tempfile::TempDir, crate::session::Session) {
        use crate::doc::files::{Feature, PartFile};
        use crate::doc::project::{feature_path, part_path, write_atomic, write_json};
        let (dir, mut s) = crate::testing::fresh();
        let root = dir.path();
        let f = ImportFile::new(bytes.to_vec());
        write_atomic(&root.join("imports/f.step"), bytes).unwrap();
        let p = Id::parse("p", "part").unwrap();
        write_json(
            &part_path(root, &p),
            &PartFile::new(vec![id("i")]).to_json(),
        )
        .unwrap();
        let i = Feature::Import(Import {
            mode,
            ..import(&f, product, 0)
        });
        write_json(&feature_path(root, &p, &id("i")), &i.to_json()).unwrap();
        s.reload().unwrap();
        (dir, s)
    }

    /// `p`'s rebuild report, whether or not a feature failed.
    fn rebuild(s: &mut crate::session::Session) -> Value {
        match crate::testing::call(s, "rebuild", json!({"part": "p"})) {
            Ok(v) => v["parts"][0].clone(),
            Err(e) => e.details["parts"][0].clone(),
        }
    }

    #[test]
    fn a_placed_import_stands_where_its_place_puts_it() {
        use crate::doc::files::Feature;
        use crate::doc::import::{Mode, Place, Via};
        use crate::doc::project::{feature_path, write_json};
        let bytes = occt("box_ap214.stp");
        for mode in [None, Some(Mode::Reference)] {
            let (dir, mut s) = with_import_as(&bytes, Some(("part 2", "part 2")), mode);
            let bounds = |s: &mut crate::session::Session| {
                let v = crate::testing::call(s, "inspect", json!({"part": "p"})).unwrap();
                let b = &v["bodies"][0]["bounds"];
                [0, 1, 2].map(|k| b["min"][k].as_f64().unwrap())
            };
            let before = bounds(&mut s);
            let f = ImportFile::new(bytes.clone());
            let i = Feature::Import(Import {
                mode,
                place: Some(Place {
                    pose: crate::doc::assembly::Pose {
                        origin_m: [0.1, 0.0, 0.0],
                        ..crate::doc::assembly::Pose::IDENTITY
                    },
                    via: vec![Via {
                        usage: "u1".into(),
                        product: crate::doc::import::ProductRef {
                            id: "part 2".into(),
                            name: "part 2".into(),
                            ordinal: 0,
                        },
                        ordinal: 0,
                    }],
                }),
                ..import(&f, Some(("part 2", "part 2")), 0)
            });
            let p = Id::parse("p", "part").unwrap();
            write_json(&feature_path(dir.path(), &p, &id("i")), &i.to_json()).unwrap();
            s.reload().unwrap();
            let after = bounds(&mut s);
            assert!(
                (after[0] - before[0] - 0.1).abs() < 1e-12,
                "{mode:?}: {before:?} {after:?}"
            );
            assert!((after[1] - before[1]).abs() < 1e-12);
            assert!((after[2] - before[2]).abs() < 1e-12);
        }
    }

    #[test]
    fn a_reference_import_builds_the_files_topology_with_healings_face_names() {
        use crate::doc::import::Mode;
        let bytes = occt("box_ap214.stp");
        let (_dr, mut r) =
            with_import_as(&bytes, Some(("part 2", "part 2")), Some(Mode::Reference));
        let (_ds, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let fr = rebuild(&mut r)["features"][0].clone();
        let fs = rebuild(&mut s)["features"][0].clone();
        assert_eq!(fr["ok"], true, "{fr}");
        assert_eq!(fr["import"]["mode"], "reference", "{fr}");
        assert!(fr["import"].get("heal").is_none());
        assert_eq!(fr["import"]["reference"]["dropped_faces"], json!([]));
        assert!(fs["import"].get("mode").is_none(), "{fs}");
        assert_eq!(fr["counts"], fs["counts"]);
        // Face names are healing's: the same in both.
        let names = |s: &mut crate::session::Session| -> Vec<String> {
            let mut n: Vec<String> = faces(s).into_iter().map(|(n, _)| n).collect();
            n.sort();
            n
        };
        assert_eq!(names(&mut r), names(&mut s));
        // Every entity still names its STEP instance.
        for (_, f) in faces(&mut r) {
            assert!(
                f["origin"]["step"].as_str().unwrap().starts_with('#'),
                "{f}"
            );
        }
    }

    #[test]
    fn reference_faults_are_warnings_naming_faces_and_instances() {
        let report = json!({"mode": "reference", "reference": {
            "split_faces": [{"name": "aa", "step": "#12"}],
            "dropped_faces": [{"name": "bb", "step": "#30"}, {"name": "cc"}],
            "unoriented": []}});
        let p = Id::parse("p", "part").unwrap();
        let w = reference_warnings(&p, &id("i"), &report);
        let codes: Vec<&str> = w.iter().map(|w| w["code"].as_str().unwrap()).collect();
        assert_eq!(codes, ["reference_faces_split", "reference_faces_dropped"]);
        assert_eq!(w[0]["faces"], json!([{"entity": "p/i/aa", "step": "#12"}]));
        assert_eq!(w[1]["faces"][1], json!({"entity": "p/i/cc"}));
        assert!(w[1]["message"].as_str().unwrap().contains("2 faces"));
        // A clean reference warns of nothing; a solid's report is never read.
        let clean = json!({"reference": {
            "split_faces": [], "dropped_faces": [], "unoriented": []}});
        assert!(reference_warnings(&p, &id("i"), &clean).is_empty());
        assert!(reference_warnings(&p, &id("i"), &json!({"heal": {}})).is_empty());
    }

    #[test]
    fn a_references_solid_is_marked_in_the_rebuild_report() {
        use crate::doc::import::Mode;
        let bytes = occt("box_ap214.stp");
        let (_dr, mut r) =
            with_import_as(&bytes, Some(("part 2", "part 2")), Some(Mode::Reference));
        let (_ds, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let rr = rebuild(&mut r);
        assert_eq!(rr["solids"][0]["reference"], true, "{rr}");
        assert!(
            rr.get("warnings")
                .is_none_or(|w| w.as_array().unwrap().is_empty()),
            "{rr}"
        );
        let ss = rebuild(&mut s);
        assert!(ss["solids"][0].get("reference").is_none(), "{ss}");
    }

    /// The absolute path of OCCT's file `name`, as text.
    pub(crate) fn occt_path(name: &str) -> String {
        format!(
            "{}/../ryustep/tests/data/occt/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "a large file: tests/step.rs reads it in release"
    )]
    fn a_surface_model_imports_as_a_sheet() {
        // NIST CTC 04's PMI support geometry, representation #14185: one
        // face, two laminar edges (ryustep's corpus manifest).
        let (_dir, mut s) = crate::testing::fresh();
        let call = crate::testing::call;
        let file = format!(
            "{}/../ryustep/tests/corpus/nist_ctc_04_asme1_rd.stp",
            env!("CARGO_MANIFEST_DIR")
        );
        let v = call(&mut s, "import", json!({"path": file, "solid": true})).unwrap();
        let mut kinds = Vec::new();
        for p in v["parts"].as_array().unwrap() {
            let r = call(&mut s, "rebuild", json!({"part": p["part"]})).unwrap();
            for x in r["parts"][0]["solids"].as_array().unwrap() {
                kinds.push((x["kind"].clone(), x["closed"].clone()));
            }
        }
        assert!(kinds.contains(&(json!("sheet"), json!(false))), "{kinds:?}");
        // Solids heal as before: the file's solid is still a solid.
        assert!(kinds.iter().any(|(k, _)| k == "solid"), "{kinds:?}");
        assert_eq!(heal_options().open, ryukei::heal::Open::Sheet);
    }

    #[test]
    fn a_products_lone_surface_model_imports_as_a_sheet_both_ways() {
        // Issue #442: a product whose shape is a manifold surface
        // representation of one open shell, a planar face bounded by a
        // circle (a board outline or a marking beside an MCAD export's
        // solids). Written by us in that shape; it was `open_shell`.
        let file = format!(
            "{}/tests/data/surface_disc.step",
            env!("CARGO_MANIFEST_DIR")
        );
        let call = crate::testing::call;
        for (part, solid) in [("r", false), ("s", true)] {
            let (_dir, mut s) = crate::testing::fresh();
            let v = call(
                &mut s,
                "import",
                json!({"path": file, "part": part, "solid": solid}),
            )
            .unwrap();
            let f = &v["parts"][0]["features"][0];
            assert_eq!(f["ok"], true, "{v}");
            let r = call(&mut s, "rebuild", json!({"part": part})).unwrap();
            let x = &r["parts"][0]["solids"][0];
            assert_eq!(x["kind"], "sheet", "{r}");
            assert_eq!(x["closed"], false, "{r}");
            assert_eq!(x["boundaries"], 1, "{r}");
        }
    }

    #[test]
    fn imports_are_references_unless_asked_for_solids() {
        let (dir, mut s) = crate::testing::fresh();
        let file = occt_path("box_ap214.stp");
        let call = crate::testing::call;
        let v = call(&mut s, "import", json!({"path": file, "part": "r"})).unwrap();
        let f = &v["parts"][0]["features"][0];
        assert_eq!(f["ok"], true, "{v}");
        assert_eq!(f["mode"], "reference", "{v}");
        assert!(f.get("heal").is_none(), "{v}");
        let fid = f["id"].as_str().unwrap();
        let text = std::fs::read_to_string(dir.path().join(format!("parts/r/features/{fid}.json")))
            .unwrap();
        assert!(text.contains("\"mode\": \"reference\""), "{text}");
        assert!(text.contains("\"format\": 13"), "{text}");
        // Asked for a solid: healed, and the file is as 0.8.x writes it.
        let v = call(
            &mut s,
            "import",
            json!({"path": file, "part": "h", "solid": true}),
        )
        .unwrap();
        let f = &v["parts"][0]["features"][0];
        assert!(f["heal"].is_object(), "{v}");
        let fid = f["id"].as_str().unwrap();
        let text = std::fs::read_to_string(dir.path().join(format!("parts/h/features/{fid}.json")))
            .unwrap();
        assert!(!text.contains("\"mode\""), "{text}");
        assert!(text.contains("\"format\": 5"), "{text}");
        // --into takes the same flag; one part may hold both modes.
        call(&mut s, "import", json!({"path": file, "into": "h"})).unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "h"})).unwrap();
        let modes: Vec<Value> = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["import"]["mode"].clone())
            .collect();
        assert_eq!(modes, [Value::Null, json!("reference")]);
    }

    #[test]
    fn a_dry_run_says_what_each_mode_would_do() {
        let (dir, mut s) = crate::testing::fresh();
        let file = occt_path("box_ap214.stp");
        let call = crate::testing::call;
        let r = call(
            &mut s,
            "import",
            json!({"path": file, "part": "x", "dry_run": true}),
        )
        .unwrap();
        assert_eq!(r["parts"][0]["features"][0]["mode"], "reference", "{r}");
        let h = call(
            &mut s,
            "import",
            json!({"path": file, "part": "x", "dry_run": true, "solid": true}),
        )
        .unwrap();
        assert!(h["parts"][0]["features"][0]["heal"].is_object(), "{h}");
        assert!(!dir.path().join("parts/x").exists());
    }

    #[test]
    fn a_reference_import_stops_at_a_cancel() {
        use crate::doc::import::Mode;
        use std::sync::atomic::AtomicBool;
        let bytes = occt("box_ap214.stp");
        let (_d, s) = with_import_as(&bytes, Some(("part 2", "part 2")), Some(Mode::Reference));
        let part = s
            .project()
            .part(&Id::parse("p", "part").unwrap())
            .unwrap()
            .clone();
        let fid = id("i");
        let Some(Ok(crate::doc::files::Feature::Import(i))) = part.features.get(&fid).cloned()
        else {
            unreachable!()
        };
        let job = ryu_session::job::Job::new("t", "p", "rebuild", Arc::new(AtomicBool::new(true)));
        let e = build(&part, &fid, &i, &crate::rebuild::Run::with_job(&job)).unwrap_err();
        assert_eq!(e.code, Code::Cancelled, "{e:?}");
    }

    /// The names of the faces of `p`'s one solid, with their plane's
    /// normal's z and origin's z, for those on planes.
    fn faces(s: &mut crate::session::Session) -> Vec<(String, Value)> {
        let v = crate::testing::call(s, "inspect", json!({"part": "p"})).unwrap();
        v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["name"].as_str().unwrap().to_owned(), f.clone()))
            .collect()
    }

    #[test]
    fn an_import_founds_a_solid_from_its_body() {
        let bytes = occt("box_ap214.stp");
        let (_dir, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let r = rebuild(&mut s);
        let f = &r["features"][0];
        assert_eq!(f["ok"], true, "{r}");
        assert_eq!(f["type"], "import");
        assert_eq!(f["solids"], json!(["i"]));
        assert_eq!(f["counts"], json!({"faces": 6, "edges": 12, "vertices": 8}));
        assert_eq!(f["import"]["file"], "imports/f.step");
        assert_eq!(
            f["import"]["product"],
            json!({"id": "part 2", "name": "part 2"})
        );
        assert_eq!(f["import"]["heal"]["sewn"], 0);
        assert_eq!(r["solids"][0]["solid"], "i");
        // The body is ryustep's own, read under the feature's key.
        let p = s
            .project()
            .part(&Id::parse("p", "part").unwrap())
            .unwrap()
            .clone();
        let o = opened(&ImportFile::new(bytes.clone())).unwrap();
        let read = o.body(
            0,
            &ReadOptions {
                key: op_key(&p.id, &id("i")),
                heal: Default::default(),
            },
        );
        let want = blake3::hash(read.result.unwrap().0.dump().as_bytes()).to_hex()[..32].to_owned();
        assert_eq!(f["hash"], want);
        // Every face, edge and vertex says which STEP instance it came
        // from.
        let v = crate::testing::call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        for list in ["faces", "edges", "vertices"] {
            for e in v["bodies"][0][list].as_array().unwrap() {
                assert_eq!(e["origin"]["feature"], "i", "{e}");
                let step = e["origin"]["step"].as_str().unwrap();
                assert!(step.starts_with('#'), "{step}");
            }
        }
    }

    #[test]
    fn a_body_past_its_heal_budget_fails_alone() {
        let bytes = occt("box_ap214.stp");
        let (_dir, s) = with_import(&bytes, Some(("part 2", "part 2")));
        let part = s
            .project()
            .part(&Id::parse("p", "part").unwrap())
            .unwrap()
            .clone();
        let fid = id("i");
        let f = part.features[&fid].as_ref().unwrap().clone();
        let crate::doc::files::Feature::Import(i) = &f else {
            unreachable!()
        };
        let job = ryu_session::job::Job::default();
        let run = crate::rebuild::Run {
            job: &job,
            heal_budget: 1,
            edit_budget: crate::tweak::DEFAULT_EDIT_BUDGET,
        };
        let e = build(&part, &fid, i, &run).unwrap_err();
        assert_eq!(e.code, Code::ImportBudgetExceeded, "{e:?}");
        assert_eq!(e.details["budget"], 1);
        assert!(e.details["spent"].as_u64().unwrap() > 1);
        assert!(!e.details["step"].as_array().unwrap().is_empty());
        // The default heals it.
        assert!(build(&part, &fid, i, &crate::rebuild::Run::none()).is_ok());
        // The budget is in the input hash only when it isn't the default.
        let solids = crate::solid::Solids::of(&part);
        let before = BTreeMap::new();
        let hash = |r: &crate::rebuild::Run| {
            crate::rebuild::input_hash(&part, &solids, &fid, &f, &before, r)
        };
        let plain = hash(&crate::rebuild::Run::none());
        assert_ne!(plain, hash(&run));
        assert_eq!(plain, hash(&crate::rebuild::Run::with_job(&job)));
    }

    #[test]
    fn an_import_says_what_it_reads_whether_or_not_it_builds() {
        let bytes = occt("box_ap214.stp");
        let blake3 = ImportFile::new(bytes.clone()).blake3;
        let (_dir, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let f = rebuild(&mut s)["features"][0].clone();
        assert_eq!(f["ok"], true);
        assert_eq!(f["import"]["blake3"], blake3);
        assert_eq!(f["import"]["body"], 0);
        assert!(f["import"]["heal"].is_object(), "{f}");
        // No body no product reaches: it fails, and still says so.
        let (_dir, mut s) = with_import(&bytes, None);
        let f = rebuild(&mut s)["features"][0].clone();
        assert_eq!(f["ok"], false);
        assert_eq!(
            f["import"],
            json!({"file": "imports/f.step", "blake3": blake3, "body": 0})
        );
    }

    #[test]
    fn an_import_whose_file_isnt_there_or_isnt_its_fails() {
        let bytes = occt("box_ap214.stp");
        let (dir, mut s) = with_import(&bytes, None);
        // No body no product reaches: the box is a product's.
        let r = rebuild(&mut s);
        assert_eq!(
            r["features"][0]["error"]["code"], "import_body_missing",
            "{r}"
        );
        let (dir2, mut s2) = with_import(&bytes, Some(("part 2", "part 2")));
        drop(dir);
        let root = dir2.path();
        std::fs::write(root.join("imports/f.step"), b"ISO-10303-21;").unwrap();
        let r = rebuild(&mut s2);
        let e = &r["features"][0]["error"];
        assert_eq!(e["code"], "import_file_changed", "{r}");
        assert_eq!(
            e["details"]["found"],
            ImportFile::new(b"ISO-10303-21;".to_vec()).blake3
        );
        assert_eq!(r["failed"], 1);
        std::fs::remove_file(root.join("imports/f.step")).unwrap();
        let r = rebuild(&mut s2);
        assert_eq!(
            r["features"][0]["error"]["code"], "import_file_missing",
            "{r}"
        );
        // The bytes come back: so does the solid.
        std::fs::write(root.join("imports/f.step"), &bytes).unwrap();
        let r = rebuild(&mut s2);
        assert_eq!(r["features"][0]["ok"], true, "{r}");
        // A file that isn't STEP, imported as such.
        let (_dir3, mut s3) = with_import(b"not step at all", None);
        let r = rebuild(&mut s3);
        let e = &r["features"][0]["error"];
        assert_eq!(e["code"], "step_unreadable", "{r}");
        assert_eq!(e["details"]["reason"], "not_part21", "{r}");
    }

    #[test]
    fn names_follow_geometry_not_the_file() {
        // The box raised from 30 mm to 40 mm: its bottom and the edges
        // round it keep their names; what moved is new.
        let bytes = occt("box_ap214.stp");
        let (dir, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let before = faces(&mut s);
        let text = String::from_utf8(bytes).unwrap();
        let taller = text
            .replace(",30.))", ",40.))")
            .replace("(30.,0.)", "(40.,0.)");
        std::fs::write(dir.path().join("imports/f.step"), &taller).unwrap();
        let path = dir.path().join("parts/p/features/i.json");
        let feature = std::fs::read_to_string(&path).unwrap();
        let old = ImportFile::new(text.into_bytes()).blake3;
        let new = ImportFile::new(taller.into_bytes()).blake3;
        std::fs::write(&path, feature.replace(&old, &new)).unwrap();
        let after = faces(&mut s);
        // The faces across z, at their planes' heights.
        let across = |l: &[(String, Value)], height: f64| {
            let frame = |f: &Value| f["surface"]["frame"].clone();
            l.iter()
                .find(|(_, f)| {
                    let fr = frame(f);
                    fr["z"][2].as_f64().is_some_and(|z| z.abs() == 1.0)
                        && fr["origin"][2].as_f64() == Some(height)
                })
                .unwrap()
                .0
                .clone()
        };
        let bottom = |l: &[(String, Value)]| across(l, 0.0);
        let top = |l: &[(String, Value)], h: f64| across(l, h);
        assert_eq!(bottom(&before), bottom(&after));
        assert_ne!(top(&before, 0.03), top(&after, 0.04));
        let kept = before
            .iter()
            .filter(|(n, _)| after.iter().any(|(m, _)| m == n))
            .count();
        assert_eq!(kept, 1);
    }

    /// `file` from ryustep's OCCT files, copied where an agent would have
    /// it: outside the project.
    fn outside(name: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, occt(name)).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    /// An import asking for solids, as every test written before design
    /// 364 meant, unless `params` says otherwise.
    fn import_call(s: &mut crate::session::Session, mut params: Value) -> Result<Value> {
        if params.get("solid").is_none() {
            params["solid"] = json!(true);
        }
        crate::testing::call(s, "import", params)
    }

    /// Every import answer says what it made of the file (design 448):
    /// a dry run and an import alike, the box's geometry, topology, units
    /// and product tree interpreted and nothing failed.
    #[test]
    fn an_import_reports_its_transfer() {
        let (_src, path) = outside("box_ap214.stp");
        let (_dir, mut s) = crate::testing::fresh();
        for dry in [true, false] {
            let v = import_call(&mut s, json!({"path": path, "dry_run": dry})).unwrap();
            let t = &v["transfer"];
            assert_eq!(t["file"], "imports/box_ap214.step", "{t}");
            let cats: Vec<&str> = t["categories"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["category"].as_str().unwrap())
                .collect();
            for c in ["geometry", "topology", "product_structure", "units"] {
                assert!(cats.contains(&c), "{c}: {cats:?}");
                let line = t["categories"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|x| x["category"] == c)
                    .unwrap();
                assert!(line["interpreted"]["count"].as_u64().unwrap() > 0, "{line}");
                assert_eq!(line["failed"]["count"], 0);
                assert!(line["interpreted"]["names"].as_array().unwrap().len() <= 8);
            }
            let total: u64 = t["categories"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    ["interpreted", "retained", "unreferenced", "failed"]
                        .iter()
                        .map(|b| c[*b]["count"].as_u64().unwrap())
                        .sum::<u64>()
                })
                .sum();
            assert_eq!(t["instances"], total);
            assert_eq!(t["summary"].as_array().unwrap().len(), cats.len());
        }
    }

    /// A body that fails names its instances under `failed`, with its
    /// code; what else its representation reaches is retained.
    #[test]
    fn a_failing_body_counts_its_instances_as_failed() {
        let failing = crate::transfer::failed_instances(&json!({
            "code": "import_failed",
            "details": {"step": ["#12", "#40", "x"]}
        }));
        assert_eq!(
            failing,
            [
                ryustep::census::Failed {
                    instance: 12,
                    code: "import_failed".into()
                },
                ryustep::census::Failed {
                    instance: 40,
                    code: "import_failed".into()
                },
            ]
        );
    }

    /// A refusal once the file is read keeps its code and gains the
    /// file's transfer report, nothing interpreted (design 448).
    #[test]
    fn a_refusal_after_reading_carries_the_transfer_report() {
        let (_src, path) = outside("assembly.stp");
        let (_dir, mut s) = crate::testing::fresh();
        // Adding the file's bodies to a part that doesn't exist.
        let e = import_call(&mut s, json!({"path": path, "into": "nope"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound, "{e:?}");
        let t = &e.details["transfer"];
        assert!(
            t["categories"].as_array().is_some_and(|c| !c.is_empty()),
            "{t}"
        );
        for c in t["categories"].as_array().unwrap() {
            assert_eq!(c["interpreted"]["count"], 0, "{c}");
        }
        // Before the file is read there is nothing to report.
        let e = import_call(&mut s, json!({"path": "/nowhere/at/all.step"})).unwrap_err();
        assert!(e.details.get("transfer").is_none());
    }

    /// An assembly whose part files aren't there (OCCT's form, design
    /// 448) is refused `import_external_missing`, naming each file, the
    /// products naming it and where it was looked for, before anything
    /// is written; the report counts the documents retained.
    #[test]
    fn an_assembly_of_missing_part_files_names_every_file() {
        let path = format!(
            "{}/../ryustep/tests/data/external/occt_style.stp",
            env!("CARGO_MANIFEST_DIR")
        );
        let (dir, mut s) = crate::testing::fresh();
        let before = crate::session::files(dir.path()).unwrap();
        let e = import_call(&mut s, json!({"path": path})).unwrap_err();
        assert_eq!(e.code, Code::ImportExternalMissing);
        let files: Vec<(&str, &Value, &str)> = e.details["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["file"].as_str().unwrap(),
                    &m["products"],
                    m["reason"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            files,
            [
                ("bracket.stp", &json!(["P1"]), "not_found"),
                ("parts/plate.stp", &json!(["P2"]), "not_found")
            ]
        );
        assert!(
            e.message.contains("bracket.stp") && e.message.contains("parts/plate.stp"),
            "{}",
            e.message
        );
        let docs = e.details["transfer"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == "external_references")
            .unwrap()
            .clone();
        assert!(docs["retained"]["count"].as_u64().unwrap() > 0, "{docs}");
        assert_eq!(
            crate::session::files(dir.path()).unwrap(),
            before,
            "nothing written"
        );
    }

    /// An assembly of a sub-assembly file of part files: an assembly per
    /// assembly file, the sub-assembly's instance naming its own file.
    #[test]
    fn a_nested_external_assembly_imports_file_by_file() {
        let d = folder(&[
            (
                "top.stp",
                external_assembly(
                    "TOP",
                    &[
                        ("SUB", "sub/sub.stp", [0.0; 3]),
                        ("SUB", "sub/sub.stp", [0.0, 40.0, 0.0]),
                    ],
                ),
            ),
            (
                "sub/sub.stp",
                external_assembly("SUB", &[("PIN", "pin.stp", [5.0, 0.0, 0.0])]),
            ),
            ("sub/pin.stp", part_file("PIN", 2.0)),
        ]);
        let (_dir, mut s) = crate::testing::fresh();
        let v = import_call(&mut s, json!({"path": d.path().join("top.stp")})).unwrap();
        let parts: Vec<(&str, &str)> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["part"].as_str().unwrap(), p["kind"].as_str().unwrap()))
            .collect();
        assert_eq!(
            parts,
            [("top", "assembly"), ("sub", "assembly"), ("pin", "part")]
        );
        let m = s
            .model(
                &Id::parse("top", "part").unwrap(),
                &crate::doc::assembly::Rev::Working,
            )
            .unwrap();
        let pins = m.occurrences.iter().filter(|o| o.path.len() == 2).count();
        assert_eq!(pins, 2, "one pin in each sub-assembly");
        let sub = listed(_dir.path(), "sub");
        assert!(
            sub.iter()
                .all(|(_, f)| f["import"]["file"] == "imports/sub.step"),
            "{sub:?}"
        );
    }

    /// --part names the first file's root; into and product don't take
    /// a file whose products are elsewhere.
    #[test]
    fn part_names_the_assembly_and_into_is_refused() {
        let (_dir, mut s) = crate::testing::fresh();
        let v = import_call(&mut s, json!({"path": set_path("asm.stp"), "part": "rig"})).unwrap();
        assert_eq!(v["parts"][0]["part"], "rig");
        crate::testing::call(&mut s, "part.create", json!({"part": "holder"})).unwrap();
        let e = import_call(
            &mut s,
            json!({"path": set_path("asm.stp"), "into": "holder"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
    }

    /// Two part files of one stem in different folders are stored under
    /// two names.
    #[test]
    fn files_of_one_stem_are_stored_apart() {
        let d = folder(&[
            (
                "asm.stp",
                external_assembly(
                    "ASM",
                    &[
                        ("A", "a/part.stp", [0.0; 3]),
                        ("B", "b/part.stp", [20.0, 0.0, 0.0]),
                    ],
                ),
            ),
            ("a/part.stp", part_file("A", 2.0)),
            ("b/part.stp", part_file("B", 3.0)),
        ]);
        let (_dir, mut s) = crate::testing::fresh();
        let v = import_call(&mut s, json!({"path": d.path().join("asm.stp")})).unwrap();
        let paths: Vec<&str> = v["external"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["imports/part.step", "imports/part-2.step"]);
    }

    /// `import.report` reads a committed file again: what the project's
    /// import features made of it, `detail` listing every name with its
    /// first instances; a path the project hasn't imported is not_found.
    #[test]
    fn a_committed_file_is_reported_again() {
        let (_src, path) = outside("assembly.stp");
        let (_dir, mut s) = crate::testing::fresh();
        let made = import_call(&mut s, json!({"path": path})).unwrap();
        let stored = made["file"]["path"].as_str().unwrap().to_owned();
        let v = crate::testing::call(&mut s, "import.report", json!({"file": stored})).unwrap();
        assert_eq!(v["file"]["path"], stored.as_str());
        assert_eq!(
            v["transfer"]["categories"], made["transfer"]["categories"],
            "the same as the import said"
        );
        let d = crate::testing::call(
            &mut s,
            "import.report",
            json!({"file": stored, "detail": true}),
        )
        .unwrap();
        assert_eq!(d["transfer"]["detail"], true);
        let geometry = &d["transfer"]["categories"][0];
        assert_eq!(geometry["category"], "geometry");
        let first = &geometry["interpreted"]["names"][0];
        assert!(
            first["instances"]
                .as_array()
                .is_some_and(|i| !i.is_empty() && i.len() <= 64),
            "{first}"
        );
        let e = crate::testing::call(
            &mut s,
            "import.report",
            json!({"file": "imports/none.step"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn importing_stores_the_file_and_makes_a_part_of_each_product() {
        let (_src, path) = outside("box_ap214.stp");
        let (dir, mut s) = crate::testing::fresh();
        let root = dir.path();
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        assert_eq!(v["file"]["path"], "imports/box_ap214.step");
        assert_eq!(v["file"]["reused"], false);
        assert_eq!(v["dry_run"], false);
        let part = &v["parts"][0];
        assert_eq!(part["part"], "part-2");
        assert_eq!(part["created"], true);
        assert_eq!(part["product"], json!({"id": "part 2", "name": "part 2"}));
        assert!(part.get("properties").is_none());
        let f = &part["features"][0];
        assert_eq!((&f["id"], &f["ok"]), (&json!("body"), &json!(true)), "{v}");
        assert_eq!(f["counts"]["faces"], 6);
        assert_eq!(f["heal"]["sewn"], 0);
        assert_eq!(
            std::fs::read(root.join("imports/box_ap214.step")).unwrap(),
            occt("box_ap214.stp")
        );
        let attributes = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
        assert!(
            attributes.contains(crate::drivers::IMPORT_LINE),
            "{attributes}"
        );
        let r = crate::testing::call(&mut s, "rebuild", json!({"part": "part-2"})).unwrap();
        assert_eq!(r["parts"][0]["features"][0]["type"], "import");
        // The same bytes again: stored once, another part.
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        assert_eq!(
            v["file"],
            json!({"path": "imports/box_ap214.step", "reused": true,
                                     "blake3": v["file"]["blake3"], "schema": "AP214"})
        );
        assert_eq!(v["parts"][0]["part"], "part-2-2");
        // Named, and refused where the name is taken.
        let v = import_call(&mut s, json!({"path": path, "part": "block"})).unwrap();
        assert_eq!(v["parts"][0]["part"], "block");
        let e = import_call(&mut s, json!({"path": path, "part": "block"})).unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        // Nothing to say about the project being healthy afterwards.
        assert_eq!(crate::doctor::diagnose(root), []);
    }

    #[test]
    fn a_dry_run_writes_nothing_and_says_how_each_body_heals() {
        let (_src, path) = outside("plate_hole.stp");
        let (dir, mut s) = crate::testing::fresh();
        let root = dir.path();
        let before = crate::session::files(root).unwrap();
        let v = import_call(&mut s, json!({"path": path, "dry_run": true})).unwrap();
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["parts"][0]["features"][0]["ok"], true, "{v}");
        assert_eq!(crate::session::files(root).unwrap(), before);
        assert!(!root.join("imports").exists());
        // What isn't STEP is refused before anything is written.
        let junk = root.join("../junk.step");
        std::fs::write(&junk, "hello").unwrap();
        let e = import_call(&mut s, json!({"path": junk})).unwrap_err();
        assert_eq!(e.code, Code::StepUnreadable);
        assert_eq!(crate::session::files(root).unwrap(), before);
        let e = import_call(&mut s, json!({"path": "/nowhere/at/all.step"})).unwrap_err();
        assert_eq!(e.code, Code::Io);
        // A stale project revision is a conflict.
        let e = import_call(&mut s, json!({"path": path, "expected_revision": "0"})).unwrap_err();
        assert_eq!(e.code, Code::Conflict);
    }

    /// A dry run cancelled stops before the body it would heal and leaves
    /// nothing that changes the next; a real import cancelled before it
    /// writes writes nothing, and one cancelled after its first file
    /// finishes and says `cancel_ignored`.
    #[test]
    fn a_cancelled_import_writes_nothing_or_finishes() {
        use ryu_session::job::Job;
        let (_src, path) = outside("plate_hole.stp");
        let (dir, s) = crate::testing::fresh();
        let root = dir.path().to_path_buf();
        let before = crate::session::files(&root).unwrap();
        let mut slot = Some(s);
        let ask = |dry: bool| {
            crate::proto::Request::from_rpc(
                "import",
                Some(json!({"solid": true, "path": path, "dry_run": dry})),
            )
            .unwrap()
        };
        let mut run = |dry: bool, job: &Job| {
            crate::server::handle_job(&mut slot, None, None, &root, ask(dry), job)
        };
        let e = run(true, &Job::cancelled_at(1)).unwrap_err();
        assert_eq!(e.code, Code::Cancelled, "{e:?}");
        let plain = run(true, &Job::default()).unwrap();
        assert_eq!(plain.result["parts"][0]["features"][0]["ok"], true);
        let e = run(false, &Job::cancelled_at(1)).unwrap_err();
        assert_eq!(e.code, Code::Cancelled, "{e:?}");
        assert_eq!(crate::session::files(&root).unwrap(), before);
        // Its first boundary is before it writes; the second, after.
        let late = run(false, &Job::cancelled_at(2)).unwrap();
        assert_eq!(late.result["cancel_ignored"], true, "{}", late.result);
        assert_eq!(late.result["parts"][0]["features"][0]["ok"], true);
        assert!(root.join("imports/plate_hole.step").is_file());
    }

    #[test]
    fn into_adds_one_products_bodies_to_a_part() {
        let (_src, path) = outside("assembly.stp");
        let (_box_src, box_path) = outside("box_ap214.stp");
        let (_dir, mut s) = crate::testing::fresh();
        crate::testing::call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        crate::testing::call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "body", "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
        // The assembly file holds several products' bodies: name one.
        let e = import_call(&mut s, json!({"path": path, "into": "p"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let products = e.details["products"].as_array().unwrap().clone();
        assert!(products.len() > 1, "{e:?}");
        let one = products[0]["id"].as_str().unwrap().to_owned();
        let v = import_call(&mut s, json!({"path": path, "into": "p", "product": one})).unwrap();
        let part = &v["parts"][0];
        assert_eq!(
            (&part["part"], &part["created"]),
            (&json!("p"), &json!(false))
        );
        // `body` is the box's, so the import's is the next free.
        assert_eq!(part["features"][0]["id"], "body-2", "{v}");
        let list = crate::testing::call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        let ids: Vec<&str> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["body", "body-2"]);
        // An assembly takes no bodies; neither part and into together.
        crate::testing::call(
            &mut s,
            "part.create",
            json!({"part": "a", "kind": "assembly"}),
        )
        .unwrap();
        let e = import_call(&mut s, json!({"path": box_path, "into": "a"})).unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
        let e =
            import_call(&mut s, json!({"path": box_path, "into": "p", "part": "q"})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // A file of one product needs no product named.
        let v = import_call(&mut s, json!({"path": box_path, "into": "p"})).unwrap();
        assert_eq!(v["parts"][0]["features"][0]["id"], "body-3");
        // part names the file's one root: here, its assembly.
        let v = import_call(&mut s, json!({"path": path, "part": "x"})).unwrap();
        assert_eq!(
            (&v["parts"][0]["part"], &v["parts"][0]["kind"]),
            (&json!("x"), &json!("assembly"))
        );
    }

    #[test]
    fn into_a_part_whose_list_doesnt_load_is_refused_with_its_error() {
        let (_src, path) = outside("box_ap214.stp");
        let (dir, mut s) = crate::testing::fresh();
        crate::testing::call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        crate::testing::call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "body", "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
        let list = crate::doc::project::part_path(dir.path(), &Id::parse("p", "part").unwrap());
        let broken = b"{\"features\": [\"body\"\n";
        std::fs::write(&list, broken).unwrap();
        let add = crate::testing::call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "other", "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap_err();
        let e = import_call(&mut s, json!({"path": path, "into": "p"})).unwrap_err();
        assert_eq!(e.code, add.code, "{e:?}");
        assert_eq!(std::fs::read(&list).unwrap(), broken);
        assert!(!dir.path().join("imports").exists());
    }

    #[test]
    fn a_body_that_cant_be_healed_still_gets_its_part() {
        // OCCT's sheet offset from a spline: ryustep refuses it.
        let (_src, path) = outside("offset_sheet.stp");
        let (_dir, mut s) = crate::testing::fresh();
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        let f = &v["parts"][0]["features"][0];
        assert_eq!(f["ok"], false, "{v}");
        assert_eq!(f["error"]["code"], "import_failed");
        let reason = f["error"]["details"]["reason"].as_str().unwrap();
        assert!(!reason.is_empty());
        let warnings = v["warnings"].as_array().unwrap();
        assert!(
            warnings[0].as_str().unwrap().starts_with("import_failed: "),
            "{v}"
        );
    }

    #[test]
    fn a_project_using_pdm_gets_names_and_descriptions() {
        let (_src, path) = outside("box_ap214.stp");
        let (dir, mut s) = crate::testing::fresh();
        crate::testing::call(&mut s, "pdm.init", json!({})).unwrap();
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        assert_eq!(v["parts"][0]["properties"], json!({"name": "part 2"}));
        let props = crate::testing::call(&mut s, "props.get", json!({"part": "part-2"})).unwrap();
        let name = props["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "name")
            .unwrap()["value"]
            .clone();
        assert_eq!(name, "part 2");
        assert!(dir.path().join("parts/part-2/properties.json").is_file());
    }

    #[test]
    fn an_import_is_one_change_undone_whole() {
        use crate::live::Live;
        use crate::testing::call_live;
        let (_src, path) = outside("box_ap214.stp");
        let (dir, mut s) = crate::testing::fresh();
        let root = dir.path();
        let before = crate::session::files(root).unwrap();
        let mut live = Live::new();
        call_live(
            &mut s,
            &mut live,
            None,
            "import",
            json!({"solid": true, "path": path}),
        )
        .unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let entry = &log["entries"][0];
        assert_eq!(entry["method"], "import", "{log}");
        let files: Vec<&str> = entry["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            [
                ".gitattributes",
                "imports/box_ap214.step",
                "parts/part-2/features/body.json",
                "parts/part-2/part.json"
            ]
        );
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert!(!root.join("imports/box_ap214.step").exists());
        assert!(!root.join("parts/part-2/part.json").exists());
        let mut after = crate::session::files(root).unwrap();
        after.retain(|p| before.contains(p));
        assert_eq!(after, before);
    }

    #[test]
    fn revisions_read_their_own_imported_bytes_and_diff_them() {
        use crate::git::tests::git;
        use crate::testing::call;
        let (dir, mut s) = with_import(&occt("box_ap214.stp"), Some(("part 2", "part 2")));
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "a box"]);
        let head = rebuild(&mut s)["hash"].clone();
        // The file replaced by a taller box, and the feature with it, as
        // importing it again will (plan 6.B).
        let text = String::from_utf8(occt("box_ap214.stp")).unwrap();
        let taller = text
            .replace(",30.))", ",40.))")
            .replace("(30.,0.)", "(40.,0.)");
        std::fs::write(root.join("imports/f.step"), &taller).unwrap();
        let path = root.join("parts/p/features/i.json");
        let (old, new) = (
            ImportFile::new(text.into_bytes()).blake3,
            ImportFile::new(taller.into_bytes()).blake3,
        );
        let feature = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, feature.replace(&old, &new)).unwrap();
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert_eq!(
            v["imports"],
            json!([{"path": "imports/f.step", "status": "changed", "from": old, "to": new}])
        );
        assert_eq!(
            v["parts"]["p"]["features"]["i"]["changes"],
            json!([{"at": "/file/blake3", "from": old, "to": new}])
        );
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        // What moved is new; the bottom, its four edges and its corners
        // keep their names.
        let body = &v["parts"]["p"]["features"]["i"]["model"]["body"];
        let count = |kind: &str, how: &str| body[kind][how].as_array().map_or(0, Vec::len);
        assert_eq!(
            (count("faces", "new"), count("faces", "gone")),
            (5, 5),
            "{body}"
        );
        assert_eq!(
            (count("edges", "new"), count("edges", "gone")),
            (8, 8),
            "{body}"
        );
        assert_eq!(
            (count("vertices", "new"), count("vertices", "gone")),
            (4, 4),
            "{body}"
        );
        // HEAD still builds from HEAD's bytes.
        let at = call(&mut s, "rebuild", json!({"part": "p", "rev": "HEAD"})).unwrap();
        assert_eq!(at["parts"][0]["hash"], head);
        assert_ne!(rebuild(&mut s)["hash"], head);
    }

    #[test]
    fn check_finds_what_doctor_and_rebuild_find_of_imports() {
        use crate::testing::call;
        let (dir, mut s) = with_import(&occt("box_ap214.stp"), Some(("part 2", "part 2")));
        let root = dir.path();
        crate::drivers::write_attributes(root).unwrap();
        s.reload().unwrap();
        let v = call(&mut s, "check", json!({})).unwrap();
        assert_eq!(
            (&v["passed"], &v["findings"]),
            (&json!(true), &json!([])),
            "{v}"
        );
        std::fs::write(root.join("imports/f.step"), b"ISO-10303-21;").unwrap();
        let e = call(&mut s, "check", json!({})).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        let codes: Vec<(&str, &str)> = e.details["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["code"].as_str().unwrap(),
                    f["source"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert!(
            codes.contains(&("import_file_changed", "doctor")),
            "{codes:?}"
        );
        assert!(codes.contains(&("feature_failed", "")), "{codes:?}");
    }

    /// A STEP file ryustep writes, in meters, of `products`.
    pub(crate) fn written(products: Vec<ryustep::Product>) -> Vec<u8> {
        let opts = ryustep::WriteOptions {
            unit: ryustep::LengthUnit::Meter,
            file_name: "frame.step".into(),
            time_stamp: "2026-10-27T00:00:00".into(),
            ..Default::default()
        };
        ryustep::write(&ryustep::WriteModel { products }, &opts).unwrap()
    }

    /// The context, units and origin every external-assembly file below
    /// shares: `#1` to `#13`.
    const EXTERNAL_HEAD: &str = "#1=APPLICATION_CONTEXT('automotive design');
#2=APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#1);
#3=PRODUCT_CONTEXT('',#1,'mechanical');
#4=PRODUCT_DEFINITION_CONTEXT('part definition',#1,'design');
#5=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#9))GLOBAL_UNIT_ASSIGNED_CONTEXT((#6,#7,#8))REPRESENTATION_CONTEXT('',''));
#6=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));
#7=(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.));
#8=(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT());
#9=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#6,'distance_accuracy_value','');
#10=CARTESIAN_POINT('',(0.,0.,0.));
#11=DIRECTION('',(0.,0.,1.));
#12=DIRECTION('',(1.,0.,0.));
#13=AXIS2_PLACEMENT_3D('',#10,#11,#12);
";

    /// An assembly file (AP242, millimetres) whose root `root` places
    /// products kept in other files, in CAx-IF's form (design 448): each
    /// of `uses` is `(product id, its file, where it goes in mm)`, its
    /// usage numbered from 1; a product named twice is one definition
    /// placed twice. A product's name is its id.
    pub(crate) fn external_assembly(root: &str, uses: &[(&str, &str, [f64; 3])]) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut products: Vec<(&str, &str)> = Vec::new();
        for &(id, file, _) in uses {
            if !products.iter().any(|(p, _)| *p == id) {
                products.push((id, file));
            }
        }
        let mut s = String::from(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ryucad test: external references'),'2;1');\n\
             FILE_NAME('asm.stp','2026-11-12T00:00:00',(''),(''),'ryucad tests','','');\n\
             FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }'));\n\
             ENDSEC;\nDATA;\n",
        );
        s.push_str(EXTERNAL_HEAD);
        let targets: Vec<String> = (0..uses.len())
            .map(|j| format!("#{}", 1001 + 10 * j))
            .collect();
        writeln!(s, "#20=PRODUCT('{root}','{root}','',(#3));").unwrap();
        s.push_str("#21=PRODUCT_DEFINITION_FORMATION('','',#20);\n");
        s.push_str("#22=PRODUCT_DEFINITION('design','',#21,#4);\n");
        s.push_str("#23=PRODUCT_DEFINITION_SHAPE('','',#22);\n");
        writeln!(
            s,
            "#24=SHAPE_REPRESENTATION('',(#13,{}),#5);",
            targets.join(",")
        )
        .unwrap();
        s.push_str("#25=SHAPE_DEFINITION_REPRESENTATION(#23,#24);\n");
        for (i, (id, file)) in products.iter().enumerate() {
            let b = 200 + 20 * i;
            writeln!(s, "#{b}=PRODUCT('{id}','{id}','',(#3));").unwrap();
            writeln!(s, "#{}=PRODUCT_DEFINITION_FORMATION('','',#{b});", b + 1).unwrap();
            writeln!(
                s,
                "#{}=PRODUCT_DEFINITION('design','',#{},#4);",
                b + 2,
                b + 1
            )
            .unwrap();
            writeln!(s, "#{}=PRODUCT_DEFINITION_SHAPE('','',#{});", b + 3, b + 2).unwrap();
            writeln!(s, "#{}=SHAPE_REPRESENTATION('',(#13),#5);", b + 4).unwrap();
            writeln!(
                s,
                "#{}=SHAPE_DEFINITION_REPRESENTATION(#{},#{});",
                b + 5,
                b + 3,
                b + 4
            )
            .unwrap();
            writeln!(
                s,
                "#{}=DOCUMENT_FILE('{file}','',$,#{},'',$);",
                b + 6,
                b + 7
            )
            .unwrap();
            writeln!(s, "#{}=DOCUMENT_TYPE('');", b + 7).unwrap();
            writeln!(
                s,
                "#{}=DOCUMENT_REPRESENTATION_TYPE('digital',#{});",
                b + 8,
                b + 6
            )
            .unwrap();
            writeln!(
                s,
                "#{}=APPLIED_DOCUMENT_REFERENCE(#{},'',(#{}));",
                b + 9,
                b + 6,
                b + 2
            )
            .unwrap();
        }
        for (j, &(id, _, at)) in uses.iter().enumerate() {
            let u = 1000 + 10 * j;
            let i = products.iter().position(|(p, _)| *p == id).unwrap();
            let (pd, rep) = (200 + 20 * i + 2, 200 + 20 * i + 4);
            writeln!(
                s,
                "#{u}=CARTESIAN_POINT('',({:?},{:?},{:?}));",
                at[0], at[1], at[2]
            )
            .unwrap();
            writeln!(s, "#{}=AXIS2_PLACEMENT_3D('',#{u},#11,#12);", u + 1).unwrap();
            writeln!(
                s,
                "#{}=NEXT_ASSEMBLY_USAGE_OCCURRENCE('{}','','',#22,#{pd},$);",
                u + 2,
                j + 1
            )
            .unwrap();
            writeln!(s, "#{}=PRODUCT_DEFINITION_SHAPE('','',#{});", u + 3, u + 2).unwrap();
            writeln!(
                s,
                "#{}=ITEM_DEFINED_TRANSFORMATION('','',#13,#{});",
                u + 4,
                u + 1
            )
            .unwrap();
            writeln!(
                s,
                "#{}=(REPRESENTATION_RELATIONSHIP('','',#{rep},#24)REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{})SHAPE_REPRESENTATION_RELATIONSHIP());",
                u + 5,
                u + 4
            )
            .unwrap();
            writeln!(
                s,
                "#{}=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{},#{});",
                u + 6,
                u + 5,
                u + 3
            )
            .unwrap();
        }
        s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
        s.into_bytes()
    }

    /// A STEP file ryustep writes (meters) of one product `id` (its name
    /// too) holding one block `x_mm` long, 10 mm wide and high.
    pub(crate) fn part_file(id: &str, x_mm: f64) -> Vec<u8> {
        use ryukei::geom::Rigid;
        let block = ryukei::primitive::block(
            ryukei::name::OpKey(1),
            x_mm / 1000.0,
            0.01,
            0.01,
            &Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        written(vec![ryustep::Product {
            id: id.into(),
            name: id.into(),
            bodies: vec![block],
            ..Default::default()
        }])
    }

    /// `files` written under a new folder, each `(path relative to it,
    /// bytes)`, folders made as needed.
    pub(crate) fn folder(files: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, b) in files {
            let path = d.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b).unwrap();
        }
        d
    }

    /// The committed set: `asm.stp` placing `BOX` (`box.stp`, OCCT's
    /// box) at the origin and `PIN` (`parts/cylinder.stp`, OCCT's
    /// cylinder) at 50 mm along x and along y.
    pub(crate) fn set_path(name: &str) -> String {
        format!(
            "{}/../ryustep/tests/data/external/set/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// The helpers' and the committed set's assemblies read as 24.A reads
    /// external references: each placed product externally defined, with
    /// its file.
    #[test]
    fn external_assembly_files_name_their_parts() {
        let docs = |bytes: Vec<u8>| -> Vec<(String, String, bool)> {
            let o = ryustep::open(&bytes).unwrap();
            o.products
                .definitions
                .iter()
                .filter(|d| !d.external.is_empty())
                .map(|d| {
                    (
                        d.product_id.to_string(),
                        d.external[0].name.clone(),
                        d.externally_defined(),
                    )
                })
                .collect()
        };
        let made = external_assembly(
            "ASM",
            &[
                ("BOX", "box.stp", [0.0; 3]),
                ("PIN", "pin.stp", [50.0, 0.0, 0.0]),
            ],
        );
        assert_eq!(
            docs(made),
            [
                ("BOX".into(), "box.stp".into(), true),
                ("PIN".into(), "pin.stp".into(), true)
            ]
        );
        let set = std::fs::read(set_path("asm.stp")).unwrap();
        assert_eq!(
            docs(set),
            [
                ("BOX".into(), "box.stp".into(), true),
                ("PIN".into(), "cylinder.stp".into(), true)
            ]
        );
        let p = ryustep::open(&part_file("BOX", 10.0)).unwrap();
        assert_eq!(p.products.roots.len(), 1);
        assert_eq!(p.held.len(), 1);
    }

    /// An import feature of `imports/<name>.step`, body 0, no product.
    pub(crate) fn import_of(name: &str) -> Import {
        Import {
            file: FileRef {
                path: format!("imports/{name}.step"),
                blake3: "0".repeat(64),
            },
            product: None,
            body: 0,
            mode: None,
            place: None,
        }
    }

    /// The committed set (design 448, 24.D): the assembly and both part
    /// files stored; a part per file (named as the assembly names them),
    /// the pin one part placed twice; the answer lists the other files,
    /// each with its own report; the documents are interpreted.
    #[test]
    fn an_assembly_of_part_files_imports_as_an_assembly_over_their_parts() {
        let (dir, mut s) = crate::testing::fresh();
        let v = import_call(&mut s, json!({"path": set_path("asm.stp")})).unwrap();
        assert_eq!(v["file"]["path"], "imports/asm.step");
        let parts: Vec<(&str, &str)> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["part"].as_str().unwrap(), p["kind"].as_str().unwrap()))
            .collect();
        assert_eq!(
            parts,
            [("asm", "assembly"), ("box", "part"), ("pin", "part")]
        );
        let asm = &v["parts"][0]["features"];
        let of: Vec<&str> = asm
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["of"].as_str().unwrap())
            .collect();
        assert_eq!(of, ["box", "pin", "pin"]);
        let ext: Vec<(&str, &Value)> = v["external"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["path"].as_str().unwrap(), &x["documents"]))
            .collect();
        assert_eq!(
            ext,
            [
                ("imports/box.step", &json!(["box.stp"])),
                ("imports/cylinder.step", &json!(["cylinder.stp"]))
            ]
        );
        for x in v["external"].as_array().unwrap() {
            assert_eq!(x["stored"], true);
            assert_eq!(x["reused"], false);
            assert!(x["transfer"]["categories"].is_array());
        }
        let refs = v["transfer"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == "external_references")
            .unwrap();
        // #100 to #107 and #110 to #117: every one, nothing retained.
        assert_eq!(refs["interpreted"]["count"], 16, "{refs}");
        assert_eq!(refs["retained"]["count"], 0, "{refs}");
        for f in ["asm", "box", "cylinder"] {
            assert!(
                dir.path().join(format!("imports/{f}.step")).is_file(),
                "{f}"
            );
        }
        // Every part builds, and the assembly places the pin twice.
        let r = crate::testing::call(&mut s, "rebuild", json!({"part": "asm"})).unwrap();
        assert!(
            r.get("warnings")
                .is_none_or(|w| w.as_array().unwrap().is_empty()),
            "{r}"
        );
        let m = s
            .model(
                &Id::parse("asm", "part").unwrap(),
                &crate::doc::assembly::Rev::Working,
            )
            .unwrap();
        assert_eq!(m.occurrences.len(), 3);
    }

    #[test]
    fn a_dry_run_across_files_writes_nothing() {
        let (dir, mut s) = crate::testing::fresh();
        let before = crate::session::files(dir.path()).unwrap();
        let v = import_call(
            &mut s,
            json!({"path": set_path("asm.stp"), "dry_run": true}),
        )
        .unwrap();
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["external"].as_array().unwrap().len(), 2);
        assert_eq!(crate::session::files(dir.path()).unwrap(), before);
    }

    #[test]
    fn the_same_set_again_reuses_the_files_and_makes_parts_anew() {
        let (_dir, mut s) = crate::testing::fresh();
        import_call(&mut s, json!({"path": set_path("asm.stp")})).unwrap();
        let v = import_call(&mut s, json!({"path": set_path("asm.stp")})).unwrap();
        assert_eq!(v["file"]["reused"], true);
        assert!(
            v["external"]
                .as_array()
                .unwrap()
                .iter()
                .all(|x| x["reused"] == true)
        );
        assert_eq!(v["parts"][0]["part"], "asm-2");
    }

    /// Instances of parts made from other files are their usages: a
    /// rebuild warns nothing, doctor finds nothing misplaced, and its
    /// in-place repair of the assembly file deletes nothing.
    #[test]
    fn instances_of_external_parts_are_their_usages() {
        let (dir, mut s) = crate::testing::fresh();
        import_call(&mut s, json!({"path": set_path("asm.stp")})).unwrap();
        let asm = Id::parse("asm", "part").unwrap();
        assert_eq!(
            update::usage_warnings(s.project(), &asm),
            Vec::<Value>::new()
        );
        assert!(misplaced(s.project()).is_empty());
        let w = update_in_place(dir.path(), "imports/asm.step")
            .unwrap()
            .unwrap();
        assert!(
            w.values().all(Option::is_some),
            "{:?}",
            w.keys().collect::<Vec<_>>()
        );
        assert!(
            !w.keys().any(|k| k.starts_with("parts/asm/features/")),
            "the instances stay as they are: {:?}",
            w.keys().collect::<Vec<_>>()
        );
    }

    /// `--flatten` across files (design 448): one part of every body,
    /// each import feature naming its own file and placed through the
    /// root's usages; the root file, which no feature reads, isn't stored.
    #[test]
    fn a_set_flattens_to_one_part_whose_features_name_their_files() {
        let (dir, mut s) = crate::testing::fresh();
        let v = import_call(
            &mut s,
            json!({"path": set_path("asm.stp"), "flatten": true, "part": "rig"}),
        )
        .unwrap();
        assert_eq!(v["parts"].as_array().unwrap().len(), 1);
        assert_eq!(v["file"]["stored"], false);
        let l = listed(dir.path(), "rig");
        let files: Vec<&str> = l
            .iter()
            .map(|(_, f)| f["file"]["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            [
                "imports/box.step",
                "imports/cylinder.step",
                "imports/cylinder.step"
            ]
        );
        assert!(
            l.iter()
                .all(|(_, f)| f["place"]["via"].as_array().unwrap().len() == 1)
        );
        assert!(!dir.path().join("imports/asm.step").exists());
        let r = crate::testing::call(&mut s, "rebuild", json!({"part": "rig"})).unwrap();
        assert!(
            r["parts"][0]["solids"]
                .as_array()
                .unwrap()
                .iter()
                .all(|x| x["ok"] == true),
            "{r}"
        );
    }

    /// The first version: BOX (10 mm) at the origin and PIN at 50 mm.
    fn version_one() -> tempfile::TempDir {
        folder(&[
            (
                "asm.stp",
                external_assembly(
                    "ASM",
                    &[
                        ("BOX", "box.stp", [0.0; 3]),
                        ("PIN", "pin.stp", [50.0, 0.0, 0.0]),
                    ],
                ),
            ),
            ("box.stp", part_file("BOX", 10.0)),
            ("pin.stp", part_file("PIN", 2.0)),
        ])
    }

    /// A new version of the assembly and one of its part files (design
    /// 448, 24.D): the box's features take its new bytes, the pin's file
    /// (the same bytes) is left alone, the pin moves, a new part file
    /// makes a part, and the answer lists only the files that changed.
    #[test]
    fn an_update_follows_the_assembly_and_its_changed_part_files() {
        let one = version_one();
        let (dir, mut s) = crate::testing::fresh();
        import_call(&mut s, json!({"path": one.path().join("asm.stp")})).unwrap();
        let pin_before = std::fs::read(dir.path().join("imports/pin.step")).unwrap();
        let two = folder(&[
            (
                "asm.stp",
                external_assembly(
                    "ASM",
                    &[
                        ("BOX", "box.stp", [0.0; 3]),
                        ("PIN", "pin.stp", [60.0, 0.0, 0.0]),
                        ("NUT", "nut.stp", [0.0, 30.0, 0.0]),
                    ],
                ),
            ),
            ("box.stp", part_file("BOX", 12.0)),
            ("pin.stp", part_file("PIN", 2.0)),
            ("nut.stp", part_file("NUT", 4.0)),
        ]);
        let v = import_call(
            &mut s,
            json!({"path": two.path().join("asm.stp"), "update": "imports/asm.step"}),
        )
        .unwrap();
        assert_eq!(v["update"], true);
        let ext: Vec<(&str, bool)> = v["external"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["path"].as_str().unwrap(), x.get("from").is_some()))
            .collect();
        assert_eq!(
            ext,
            [("imports/box.step", true), ("imports/nut.step", false)]
        );
        let new_box = ImportFile::new(part_file("BOX", 12.0)).blake3;
        assert!(
            listed(dir.path(), "box")
                .iter()
                .all(|(_, f)| f["file"]["blake3"] == new_box.as_str())
        );
        assert_eq!(
            std::fs::read(dir.path().join("imports/pin.step")).unwrap(),
            pin_before
        );
        let asm = listed(dir.path(), "asm");
        let of: Vec<&str> = asm
            .iter()
            .map(|(_, f)| f["part"].as_str().unwrap())
            .collect();
        assert_eq!(of, ["box", "pin", "nut"]);
        let x = asm[1].1["pose"]["origin_m"][0].as_f64().unwrap();
        assert!((x - 0.06).abs() < 1e-12, "the pin moved: {x}");
    }

    /// A part file updated alone, as any imported file is: its part
    /// follows, the assembly is untouched.
    #[test]
    fn a_part_file_updates_alone() {
        let one = version_one();
        let (dir, mut s) = crate::testing::fresh();
        import_call(&mut s, json!({"path": one.path().join("asm.stp")})).unwrap();
        let asm_before = listed(dir.path(), "asm");
        let three = folder(&[("box.stp", part_file("BOX", 14.0))]);
        import_call(
            &mut s,
            json!({"path": three.path().join("box.stp"), "update": "imports/box.step"}),
        )
        .unwrap();
        let new_box = ImportFile::new(part_file("BOX", 14.0)).blake3;
        assert!(
            listed(dir.path(), "box")
                .iter()
                .all(|(_, f)| f["file"]["blake3"] == new_box.as_str())
        );
        assert_eq!(listed(dir.path(), "asm"), asm_before);
    }

    /// An update whose new version names a file that isn't there is
    /// refused before anything is written.
    #[test]
    fn an_update_missing_a_part_file_writes_nothing() {
        let one = version_one();
        let (dir, mut s) = crate::testing::fresh();
        import_call(&mut s, json!({"path": one.path().join("asm.stp")})).unwrap();
        let before = crate::session::files(dir.path()).unwrap();
        let two = folder(&[(
            "asm.stp",
            external_assembly("ASM", &[("BOX", "box.stp", [0.0; 3])]),
        )]);
        let e = import_call(
            &mut s,
            json!({"path": two.path().join("asm.stp"), "update": "imports/asm.step"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ImportExternalMissing);
        assert_eq!(crate::session::files(dir.path()).unwrap(), before);
    }

    /// A part flattened across files can't be walked again by an update
    /// (design 448 adds no field naming its root file): an update of any
    /// file it reads is refused naming it, and doctor leaves the file
    /// alone; a single-file flattened part updates as before.
    #[test]
    fn a_part_flattened_across_files_refuses_updates() {
        let one = version_one();
        let (dir, mut s) = crate::testing::fresh();
        import_call(
            &mut s,
            json!({"path": one.path().join("asm.stp"), "flatten": true, "part": "rig"}),
        )
        .unwrap();
        let three = folder(&[("box.stp", part_file("BOX", 14.0))]);
        let e = import_call(
            &mut s,
            json!({"path": three.path().join("box.stp"), "update": "imports/box.step"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        assert_eq!(e.details["part"], "rig");
        assert!(
            update_in_place(dir.path(), "imports/box.step")
                .unwrap()
                .is_none()
        );
    }

    /// OCCT's multi-file assembly imports as its one-file twin does: the
    /// same instances at the same poses, the same bodies.
    #[test]
    fn occts_multi_file_assembly_matches_its_one_file_twin() {
        use crate::doc::assembly::Rev;
        let poses = |path: String| {
            let (_d, mut s) = crate::testing::fresh();
            let v = import_call(&mut s, json!({"path": path})).unwrap();
            let asm = Id::parse(v["parts"][0]["part"].as_str().unwrap(), "part").unwrap();
            let m = s.model(&asm, &Rev::Working).unwrap();
            let mut p: Vec<String> = m
                .occurrences
                .iter()
                .map(|o| format!("{:?}", crate::assembly::rigid_pose(&o.pose)))
                .collect();
            p.sort();
            p
        };
        assert_eq!(
            poses(occt_path("multi/assembly.stp")),
            poses(occt_path("assembly.stp"))
        );
    }

    /// A frame (`F-1`) with a plate of its own, a sub-assembly (`S-1`) of
    /// two pins (`P-1`) and one pin more; and a loose block (`L-1`) no
    /// product uses: two roots. Every usage of the sub-assembly's and the
    /// frame's has the id `1`.
    pub(crate) fn frame_file() -> Vec<u8> {
        frame_version(1)
    }

    /// [`frame_file`] (`version` 1), or its second version: the plate
    /// 2 mm thicker and everything on it raised with it, the pin on the
    /// frame moved 10 mm along x, the sub-assembly's second pin gone and a
    /// third (`3`) added, the loose block gone and a new block (`N-1`).
    pub(crate) fn frame_version(version: u32) -> Vec<u8> {
        use ryukei::geom::{Rigid, Vector3};
        use ryukei::name::OpKey;
        use ryukei::primitive;
        use ryustep::{Child, Product};
        let at = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
        let k = OpKey(1);
        let (thick, pin_x) = if version == 1 {
            (0.01, 0.05)
        } else {
            (0.012, 0.06)
        };
        let plate = primitive::block(k, 0.1, 0.1, thick, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let pin = primitive::cylinder(k, 0.002, 0.02, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let block = primitive::block(k, 0.01, 0.01, 0.01, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let product = |id: &str, name: &str, bodies, children| Product {
            id: id.into(),
            name: name.into(),
            bodies,
            children,
            ..Default::default()
        };
        let sub = if version == 1 {
            vec![
                Child {
                    product: 2,
                    place: at(0.01, 0.0, 0.0),
                    id: String::new(),
                },
                Child {
                    product: 2,
                    place: at(0.02, 0.0, 0.0),
                    id: String::new(),
                },
            ]
        } else {
            vec![
                Child {
                    product: 2,
                    place: at(0.01, 0.0, 0.0),
                    id: String::new(),
                },
                Child {
                    product: 2,
                    place: at(0.03, 0.0, 0.0),
                    id: String::new(),
                },
            ]
        };
        let last = if version == 1 {
            ("L-1", "Loose")
        } else {
            ("N-1", "New")
        };
        let bytes = written(vec![
            product(
                "F-1",
                "Frame",
                vec![plate],
                vec![
                    Child {
                        product: 1,
                        place: at(0.0, 0.0, thick),
                        id: String::new(),
                    },
                    Child {
                        product: 2,
                        place: at(pin_x, 0.05, thick),
                        id: String::new(),
                    },
                ],
            ),
            product("S-1", "Sub", vec![], sub),
            product("P-1", "Pin", vec![pin], vec![]),
            product(last.0, last.1, vec![block], vec![]),
        ]);
        let text = String::from_utf8(bytes).unwrap();
        // Usage ids: the first version's all `1`; the second's sub-assembly
        // uses `1` and then `3`.
        let same = if version == 1 {
            text.replace(
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE('2'",
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE('1'",
            )
        } else {
            let sub = text.rfind("NEXT_ASSEMBLY_USAGE_OCCURRENCE('2'").unwrap();
            let mut t = text.clone();
            t.replace_range(sub..sub + 34, "NEXT_ASSEMBLY_USAGE_OCCURRENCE('3'");
            t.replacen(
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE('2'",
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE('1'",
                1,
            )
        };
        assert_ne!(same, text);
        same.into_bytes()
    }

    /// `part`'s features as its files have them, in order.
    pub(crate) fn listed(root: &std::path::Path, part: &str) -> Vec<(String, Value)> {
        let read = |p: std::path::PathBuf| -> Value {
            serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
        };
        let list = read(root.join(format!("parts/{part}/part.json")));
        list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let f = f.as_str().unwrap().to_owned();
                let v = read(root.join(format!("parts/{part}/features/{f}.json")));
                (f, v)
            })
            .collect()
    }

    #[test]
    fn an_assembly_file_makes_parts_and_assemblies() {
        // XDE's: a block placed twice and a pin turned a quarter.
        let (_src, path) = outside("assembly.stp");
        let (dir, mut s) = crate::testing::fresh();
        let root = dir.path();
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        let made: Vec<(&str, &str)> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["part"].as_str().unwrap(), p["kind"].as_str().unwrap()))
            .collect();
        assert_eq!(
            made,
            [("assembly", "assembly"), ("block", "part"), ("pin", "part")]
        );
        let asm = listed(root, "assembly");
        let ids: Vec<&str> = asm.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
        let (_, pin) = &asm[2];
        assert_eq!(pin["type"], "instance");
        assert_eq!(
            (&pin["part"], &pin["fixed"], &pin["format"]),
            (&json!("pin"), &json!(true), &json!(5))
        );
        assert_eq!(
            pin["pose"],
            json!({"origin_m": [0, 0.05, 0], "z": [0, 0, 1], "x": [0, 1, 0]})
        );
        assert_eq!(
            pin["import"],
            json!({"file": "imports/assembly.step",
                   "product": {"id": "assembly", "name": "assembly"}, "usage": "3"})
        );
        assert_eq!(asm[1].1["pose"]["origin_m"], json!([0.1, 0, 0]));
        // Its parts numbered in the order its usages place them.
        let list: Value =
            serde_json::from_slice(&std::fs::read(root.join("parts/assembly/part.json")).unwrap())
                .unwrap();
        assert_eq!(list["items"], json!({"block": 1, "pin": 2}));
        let instances = &v["parts"][0]["features"];
        assert_eq!(instances[2]["of"], "pin", "{v}");
        assert_eq!(instances[2]["ok"], true, "{v}");
        let r = crate::testing::call(&mut s, "rebuild", json!({"part": "assembly"})).unwrap();
        assert_eq!(
            r["parts"][0]["occurrences"].as_array().unwrap().len(),
            3,
            "{r}"
        );
        assert_eq!(crate::doctor::diagnose(root), []);
        // Its BOM: the block twice, the pin once.
        let bom = crate::testing::call(&mut s, "bom.get", json!({})).unwrap();
        let rows: Vec<(&Value, &Value)> = bom["tree"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (&r["part"], &r["quantity"]["count"]))
            .collect();
        assert_eq!(
            rows,
            [(&json!("block"), &json!(2)), (&json!("pin"), &json!(1))]
        );
        // Imported again, it is parts of its own: an import shares only
        // within itself.
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        assert_eq!(v["parts"][0]["part"], "assembly-2");
        let again = listed(root, "assembly-2");
        assert_eq!(again[0].1["part"], "block-2");
    }

    #[test]
    fn products_with_bodies_and_children_shared_and_unplaced() {
        let (dir, mut s) = crate::testing::fresh();
        let root = dir.path();
        let src = tempfile::tempdir().unwrap();
        let path = src.path().join("frame.step");
        std::fs::write(&path, frame_file()).unwrap();
        // Two roots: part names neither.
        let e = import_call(&mut s, json!({"path": path, "part": "x"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let v = import_call(&mut s, json!({"path": path, "dry_run": true})).unwrap();
        assert!(!root.join("parts").exists());
        let made: Vec<(&str, &str)> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["part"].as_str().unwrap(), p["kind"].as_str().unwrap()))
            .collect();
        assert_eq!(
            made,
            [
                ("f-1", "assembly"),
                ("f-1-body", "part"),
                ("s-1", "assembly"),
                ("p-1", "part"),
                ("l-1", "part")
            ]
        );
        let all_ok = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["features"].as_array().unwrap())
            .all(|f| f["ok"] == true);
        assert!(all_ok, "{v}");
        import_call(&mut s, json!({"path": path})).unwrap();
        // The frame places its own plate at its origin, then the
        // sub-assembly and a pin, both used as `1`.
        let frame = listed(root, "f-1");
        let placed: Vec<(&str, &str, &Value)> = frame
            .iter()
            .map(|(f, v)| (f.as_str(), v["part"].as_str().unwrap(), &v["import"]))
            .collect();
        let product = json!({"id": "F-1", "name": "Frame"});
        let file = "imports/frame.step";
        assert_eq!(
            placed,
            [
                (
                    "f-1-body",
                    "f-1-body",
                    &json!({"file": file, "product": product})
                ),
                (
                    "1",
                    "s-1",
                    &json!({"file": file, "product": product, "usage": "1"})
                ),
                (
                    "1-2",
                    "p-1",
                    &json!({"file": file, "product": product, "usage": "1"})
                ),
            ]
        );
        assert_eq!(frame[0].1["pose"], Pose::IDENTITY.to_json());
        // The sub-assembly uses the pin twice as `1`: the second is the
        // second of that name.
        let sub = listed(root, "s-1");
        assert_eq!(sub[1].1["import"]["ordinal"], 1);
        assert_eq!(sub[1].1["pose"]["origin_m"], json!([0.02, 0, 0]));
        assert!(sub[0].1["import"].get("ordinal").is_none());
        // The frame's own body is the plate.
        let body = listed(root, "f-1-body");
        assert_eq!(body[0].1["product"], product);
        let r = crate::testing::call(&mut s, "rebuild", json!({})).unwrap();
        assert!(r.get("warnings").is_none(), "{r}");
        let tree = crate::testing::call(&mut s, "assembly.tree", json!({"part": "f-1"})).unwrap();
        assert_eq!(tree["instances"].as_array().unwrap().len(), 3, "{tree}");
    }

    #[test]
    fn a_label_places_the_imported_part_as_its_commit_had_the_file() {
        use crate::git::tests::git;
        use crate::testing::call;
        let bytes = occt("box_ap214.stp");
        let (dir, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "100"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "p A"]);
        let released = rebuild(&mut s)["solids"][0]["hash"].clone();
        // A taller box since, the feature written against it.
        let text = String::from_utf8(bytes).unwrap();
        let taller = text
            .replace(",30.))", ",40.))")
            .replace("(30.,0.)", "(40.,0.)");
        std::fs::write(root.join("imports/f.step"), &taller).unwrap();
        let path = root.join("parts/p/features/i.json");
        let (old, new) = (
            ImportFile::new(text.into_bytes()).blake3,
            ImportFile::new(taller.into_bytes()).blake3,
        );
        let feature = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, feature.replace(&old, &new)).unwrap();
        call(
            &mut s,
            "part.create",
            json!({"part": "frame", "kind": "assembly"}),
        )
        .unwrap();
        crate::assembly::tests::place_in(root, "frame", "a", "p", json!({"label": "A"}), [0.0; 3]);
        s.refresh().unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "frame"})).unwrap();
        assert_eq!(r["parts"][0]["failed"], 0, "{r}");
        let m = s
            .model(
                &Id::parse("frame", "part").unwrap(),
                &crate::doc::assembly::Rev::Working,
            )
            .unwrap();
        assert_eq!(json!(m.occurrences[0].solids[0].built.hash), released);
        assert_ne!(rebuild(&mut s)["solids"][0]["hash"], released);
    }

    #[test]
    fn a_label_heals_its_imported_part_under_the_sessions_budget() {
        use crate::git::tests::git;
        use crate::testing::call;
        let bytes = occt("box_ap214.stp");
        let (dir, mut s) = with_import(&bytes, Some(("part 2", "part 2")));
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "100"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "p"})).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "p A"]);
        call(
            &mut s,
            "part.create",
            json!({"part": "frame", "kind": "assembly"}),
        )
        .unwrap();
        crate::assembly::tests::place_in(root, "frame", "a", "p", json!({"label": "A"}), [0.0; 3]);
        s.refresh().unwrap();
        // Under a budget too small for the box, the part at its label fails
        // as the working part does: both heal under the session's budget.
        // An occurrence shows the solids its part built.
        let solids = |s: &mut crate::session::Session| {
            let r = call(s, "rebuild", json!({"part": "frame"})).unwrap();
            r["parts"][0]["occurrences"][0]["solids"]
                .as_array()
                .unwrap()
                .len()
        };
        s.set_heal_budget(1);
        assert_eq!(solids(&mut s), 0);
        // The default heals it again.
        s.set_heal_budget(DEFAULT_HEAL_BUDGET);
        assert_eq!(solids(&mut s), 1);
    }

    #[test]
    fn failures_say_why_and_where() {
        let e = ImportError::DanglingReference {
            instance: 4,
            reference: 9,
        };
        assert_eq!(
            failure(&e),
            ("dangling_reference".into(), vec![4, 9], vec![])
        );
        let e = ImportError::Heal(HealError::Malformed { what: "x" });
        assert_eq!(failure(&e).0, "malformed");
        let n = Name::from_u128(7);
        let e = ImportError::Heal(HealError::GapTooLarge {
            entity: n,
            gap: 1.0,
        });
        assert_eq!(failure(&e), ("gap_too_large".into(), vec![], vec![n]));
        let f = ImportFile::new(b"x".to_vec());
        let err = failed(
            &id("i"),
            &import(&f, None, 0),
            &e,
            &[(n, 12u64)].into_iter().collect(),
            &BTreeMap::new(),
        );
        assert_eq!(err.code, Code::ImportFailed);
        assert_eq!(err.details["step"], json!(["#12"]));
        assert_eq!(err.details["reason"], "gap_too_large");
    }
}
