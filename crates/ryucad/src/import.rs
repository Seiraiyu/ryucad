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

mod plan;
mod update;

use plan::Planned;
pub use plan::Writes;
pub use update::{products_diff, usage_warnings};

/// How many bytes of source the opened files kept may hold together.
pub const OPENED_BYTES: usize = 1 << 30;

/// A file being opened or opened, shared by everyone who asks for it.
type Slot = Arc<OnceLock<std::result::Result<Arc<Opened>, StepError>>>;

/// The files kept, most recently used last, with their sizes.
static OPENED: Mutex<Vec<(String, usize, Slot)>> = Mutex::new(Vec::new());

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

/// What an import feature says of itself, whether it built or not: its
/// file and the hash it holds, its product and its body. The rebuild
/// report puts it under `import`, beside [`report`]'s heal counts.
pub fn written(i: &Import) -> Value {
    let mut v = json!({"file": i.file.path, "blake3": i.file.blake3, "body": i.body});
    if let Some(p) = &i.product {
        v["product"] = product_json(p);
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
pub fn build(part: &Part, id: &Id, i: &Import) -> Result<(Built, Value)> {
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
    let read = opened.body(
        k,
        &ReadOptions {
            key: op_key(&part.id, id),
            heal: Default::default(),
        },
    );
    let (body, journal, healed) = read
        .result
        .map_err(|e| failed(id, i, &e, &read.instances, &read.solids))?;
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
                HealError::ShellsCross { a, b } | HealError::FacesOverlap { a, b } => {
                    vec![*a, *b]
                }
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

/// `import`: plans every file change and, unless `dry_run`, makes them.
pub fn import(session: &mut crate::session::Session, ask: Ask) -> Result<Value> {
    use crate::doc::import::FileRef;
    use crate::doc::project::{IMPORTS, PDM_CONFIG};
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
    let plan: Vec<Planned> = match &ask.into {
        Some(into) => {
            if ask.part.is_some() {
                return Err(Error::new(
                    Code::Usage,
                    "give a part to make or one to add to (into), not both",
                ));
            }
            vec![into_plan(session, &opened, &file_ref, into, &ask)?]
        }
        None => {
            crate::config::check_project_revision(&root, ask.expected.as_deref())?;
            let retired = |s: &str| {
                Id::parse(s, "part")
                    .is_ok_and(|id| crate::pdm::check_not_retired(&root, &id).is_err())
            };
            let part = match &ask.part {
                Some(p) => {
                    let pid = crate::server::part_id(p)?;
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
            plan::Fresh {
                opened: &opened,
                file: &file_ref,
                stem: &stem,
                part,
                taken: &taken,
                pdm: root.join(PDM_CONFIG).is_file(),
            }
            .plan()?
        }
    };
    let warnings = mapped_warnings(&opened, &ask.path);
    let mut writes = plan::writes(project, &plan)?;
    if !reused {
        writes.insert(stored.clone(), Some(file.bytes.to_vec()));
    }
    let head = json!({"path": stored, "blake3": file.blake3, "reused": reused,
                      "schema": schema_name(opened.schema)});
    if ask.dry_run {
        let mut s = session.proposed(&writes)?;
        let parts = parts_report(&mut s, &opened, &plan)?;
        return Ok(answer(head, &opened, parts, warnings, true));
    }
    // PDM names need the project's ID; Git's attributes keep the file's
    // bytes once it is there.
    if plan.iter().any(|pl| !pl.properties.is_empty()) {
        crate::pdm::ensure_id(session)?;
    }
    plan::apply(&root, &writes)?;
    crate::drivers::write_attributes(&root)?;
    session.reload()?;
    let parts = parts_report(session, &opened, &plan)?;
    Ok(answer(head, &opened, parts, warnings, false))
}

/// `import --update <stored> <file>`: `file`'s bytes replace the stored
/// file, and the parts and assemblies made of it follow, in one change
/// (see [`update`]). The answer lists what changed, the products the file
/// no longer has, instances kept because something reads them, and the
/// features that would build now and won't after, found by rebuilding
/// the planned files before anything is written.
pub fn reimport(session: &mut crate::session::Session, ask: Ask) -> Result<Value> {
    use crate::doc::import::FileRef;
    use crate::doc::project::PDM_CONFIG;
    let target = ask.update.clone().expect("an update");
    if ask.part.is_some() || ask.into.is_some() || ask.product.is_some() {
        return Err(Error::new(
            Code::Usage,
            "an update replaces a file the project holds: part, into and product don't apply",
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
        opened: &opened,
        file: &file_ref,
        stem: &stem,
        part: None,
        taken: &taken,
        pdm: root.join(PDM_CONFIG).is_file(),
    };
    let u = update::plan(project, &opened, &file_ref, &fresh)?;
    let mut writes = plan::writes(project, &u.planned)?;
    if old.blake3 != file.blake3 {
        writes.insert(target.clone(), Some(file.bytes.to_vec()));
    }
    let mut warnings = mapped_warnings(&opened, &ask.path);
    warnings.extend(u.warnings.iter().cloned());
    for (a, i) in &u.kept {
        warnings.push(format!(
            "import_usage_missing: {a}/{i}'s usage is gone from {target}, but a mate or pattern reads it, so it stays"
        ));
    }
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
        if !u.kept.is_empty() {
            let kept: Vec<Value> = u
                .kept
                .iter()
                .map(|(a, i)| json!({"part": a, "instance": i}))
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
        let parts = parts_report(&mut proposed, &opened, &u.planned)?;
        return Ok(extra(answer(head, &opened, parts, warnings, true), None));
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
    if u.planned.iter().any(|pl| !pl.properties.is_empty()) {
        crate::pdm::ensure_id(session)?;
    }
    plan::apply(&root, &writes)?;
    session.reload()?;
    let assemblies = crate::mate::keep_current(session, &current)?;
    let parts = parts_report(session, &opened, &u.planned)?;
    Ok(extra(
        answer(head, &opened, parts, warnings, false),
        Some(assemblies),
    ))
}

/// What `import --update` writes when the bytes already at `path` (a
/// file replaced by hand) are the new version: `doctor`'s `import_update`
/// repair. `None` when they aren't a STEP file this build reads.
pub fn update_in_place(root: &std::path::Path, path: &str) -> Result<Option<Writes>> {
    use crate::doc::import::FileRef;
    use crate::doc::project::{PDM_CONFIG, Project};
    let project = Project::load(root)?;
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
        return Err(Error::new(
            Code::ImportBodyMissing,
            format!("{} holds no solid to import", ask.path),
        )
        .with("path", &ask.path));
    }
    let pid = crate::server::part_id(into)?;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, ask.expected.as_deref())?;
    // A list that doesn't load (a conflict record, a newer format) would
    // be written over with only the import's features.
    crate::server::order(p)?;
    if crate::server::part_kind(p) == PartKind::Assembly {
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
    let features = plan::import_features(file, product, 0..plan::bodies(opened, k), &mut taken);
    Ok(Planned {
        part: pid,
        new: false,
        kind: PartKind::Part,
        definition: k,
        features,
        removed: Vec::new(),
        properties: BTreeMap::new(),
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
                        if let Some(x) = &m.import {
                            v["heal"] = x["heal"].clone();
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

/// The answer: the stored file (`head`), the parts, the file's product
/// tree faults, and warnings: those given, then each import feature that
/// doesn't build (`import_failed`).
fn answer(
    head: Value,
    opened: &Opened,
    parts: Vec<Value>,
    warnings: Vec<String>,
    dry_run: bool,
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
                    format!(
                        "import_failed: {}/{} doesn't build: {}",
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

    /// OCCT's 10 × 20 × 30 mm box with a copy of its solid 10 mm along x,
    /// both items of its one representation: two solids touching on the
    /// plane x = 10 mm, as multi-body files from other programs hold them
    /// (#208).
    pub(crate) fn touching_boxes() -> Vec<u8> {
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
                    && let Param::Real(x) = &mut xyz[0]
                {
                    *x += 10.0;
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
    fn faces_overlapping_in_two_solids_name_the_solids() {
        let (_dir, mut s) = with_import(&touching_boxes(), Some(("part 1", "part 1")));
        let v = rebuild(&mut s);
        let f = &v["features"][0];
        assert_eq!(f["ok"], false, "{v}");
        let e = &f["error"];
        assert_eq!(e["code"], "import_failed", "{e}");
        assert_eq!(e["details"]["reason"], "faces_overlap", "{e}");
        let solids = e["details"]["solids"].as_array().expect("solids named");
        assert_eq!(solids.len(), 2, "{e}");
        assert_ne!(solids[0], solids[1]);
        let message = e["message"].as_str().unwrap();
        assert!(message.contains("in two solids of the file"), "{message}");
        assert!(message.contains(solids[0].as_str().unwrap()), "{message}");
    }

    /// A project whose part `p` lists one import `i` of `imports/f.step`
    /// holding `bytes`, written against them.
    pub(crate) fn with_import(
        bytes: &[u8],
        product: Option<(&str, &str)>,
    ) -> (tempfile::TempDir, crate::session::Session) {
        use crate::doc::files::{Feature, PartFile};
        use crate::doc::project::{feature_path, part_path, write_atomic, write_json};
        let (dir, mut s) = crate::server::tests::fresh();
        let root = dir.path();
        let f = ImportFile::new(bytes.to_vec());
        write_atomic(&root.join("imports/f.step"), bytes).unwrap();
        let p = Id::parse("p", "part").unwrap();
        write_json(
            &part_path(root, &p),
            &PartFile::new(vec![id("i")]).to_json(),
        )
        .unwrap();
        let i = Feature::Import(import(&f, product, 0));
        write_json(&feature_path(root, &p, &id("i")), &i.to_json()).unwrap();
        s.reload().unwrap();
        (dir, s)
    }

    /// `p`'s rebuild report, whether or not a feature failed.
    fn rebuild(s: &mut crate::session::Session) -> Value {
        match crate::server::tests::call(s, "rebuild", json!({"part": "p"})) {
            Ok(v) => v["parts"][0].clone(),
            Err(e) => e.details["parts"][0].clone(),
        }
    }

    /// The names of the faces of `p`'s one solid, with their plane's
    /// normal's z and origin's z, for those on planes.
    fn faces(s: &mut crate::session::Session) -> Vec<(String, Value)> {
        let v = crate::server::tests::call(s, "inspect", json!({"part": "p"})).unwrap();
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
        let v = crate::server::tests::call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        for list in ["faces", "edges", "vertices"] {
            for e in v["bodies"][0][list].as_array().unwrap() {
                assert_eq!(e["origin"]["feature"], "i", "{e}");
                let step = e["origin"]["step"].as_str().unwrap();
                assert!(step.starts_with('#'), "{step}");
            }
        }
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

    fn import_call(s: &mut crate::session::Session, params: Value) -> Result<Value> {
        crate::server::tests::call(s, "import", params)
    }

    #[test]
    fn importing_stores_the_file_and_makes_a_part_of_each_product() {
        let (_src, path) = outside("box_ap214.stp");
        let (dir, mut s) = crate::server::tests::fresh();
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
        let r = crate::server::tests::call(&mut s, "rebuild", json!({"part": "part-2"})).unwrap();
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
        let (dir, mut s) = crate::server::tests::fresh();
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

    #[test]
    fn into_adds_one_products_bodies_to_a_part() {
        let (_src, path) = outside("assembly.stp");
        let (_box_src, box_path) = outside("box_ap214.stp");
        let (_dir, mut s) = crate::server::tests::fresh();
        crate::server::tests::call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        crate::server::tests::call(
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
        let list =
            crate::server::tests::call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        let ids: Vec<&str> = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["body", "body-2"]);
        // An assembly takes no bodies; neither part and into together.
        crate::server::tests::call(
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
        let (dir, mut s) = crate::server::tests::fresh();
        crate::server::tests::call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        crate::server::tests::call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "body", "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
        let list = crate::doc::project::part_path(dir.path(), &Id::parse("p", "part").unwrap());
        let broken = b"{\"features\": [\"body\"\n";
        std::fs::write(&list, broken).unwrap();
        let add = crate::server::tests::call(
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
        let (_dir, mut s) = crate::server::tests::fresh();
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
        let (dir, mut s) = crate::server::tests::fresh();
        crate::server::tests::call(&mut s, "pdm.init", json!({})).unwrap();
        let v = import_call(&mut s, json!({"path": path})).unwrap();
        assert_eq!(v["parts"][0]["properties"], json!({"name": "part 2"}));
        let props =
            crate::server::tests::call(&mut s, "props.get", json!({"part": "part-2"})).unwrap();
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
        use crate::server::tests::call_live;
        let (_src, path) = outside("box_ap214.stp");
        let (dir, mut s) = crate::server::tests::fresh();
        let root = dir.path();
        let before = crate::session::files(root).unwrap();
        let mut live = Live::new();
        call_live(&mut s, &mut live, None, "import", json!({"path": path})).unwrap();
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
        use crate::server::tests::call;
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
        use crate::server::tests::call;
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
        let (dir, mut s) = crate::server::tests::fresh();
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
        let r = crate::server::tests::call(&mut s, "rebuild", json!({"part": "assembly"})).unwrap();
        assert_eq!(
            r["parts"][0]["occurrences"].as_array().unwrap().len(),
            3,
            "{r}"
        );
        assert_eq!(crate::doctor::diagnose(root), []);
        // Its BOM: the block twice, the pin once.
        let bom = crate::server::tests::call(&mut s, "bom.get", json!({})).unwrap();
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
        let (dir, mut s) = crate::server::tests::fresh();
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
        let r = crate::server::tests::call(&mut s, "rebuild", json!({})).unwrap();
        assert!(r.get("warnings").is_none(), "{r}");
        let tree =
            crate::server::tests::call(&mut s, "assembly.tree", json!({"part": "f-1"})).unwrap();
        assert_eq!(tree["instances"].as_array().unwrap().len(), 3, "{tree}");
    }

    #[test]
    fn a_label_places_the_imported_part_as_its_commit_had_the_file() {
        use crate::git::tests::git;
        use crate::server::tests::call;
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
