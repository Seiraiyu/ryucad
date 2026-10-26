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

/// How many bytes of source the opened files kept may hold together.
pub const OPENED_BYTES: usize = 1 << 30;

/// A file being opened or opened, shared by everyone who asks for it.
type Slot = Arc<OnceLock<std::result::Result<Arc<Opened>, StepError>>>;

/// The files kept, most recently used last, with their sizes.
static OPENED: Mutex<Vec<(String, usize, Slot)>> = Mutex::new(Vec::new());

/// `file` opened, from what is kept when it was opened before. Two
/// threads asking for one file at once open it once.
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
    slot.get_or_init(|| ryustep::open(&file.bytes).map(Arc::new))
        .clone()
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
        .map_err(|e| failed(id, i, &e, &read.instances))?;
    let step = |n: &Name| read.instances.get(n).map(|k| format!("#{k}"));
    let origins = crate::rebuild::names(&body)
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
/// healing gave those it made.
pub fn failed(id: &Id, i: &Import, e: &ImportError, instances: &BTreeMap<Name, u64>) -> Error {
    let (reason, numbers, names) = failure(e);
    let mut step: Vec<String> = numbers.iter().map(|n| format!("#{n}")).collect();
    let mut made: Vec<String> = Vec::new();
    for n in names {
        match instances.get(&n) {
            Some(k) => step.push(format!("#{k}")),
            None => made.push(n.to_string()),
        }
    }
    let mut err = Error::new(
        Code::ImportFailed,
        format!(
            "import {id}: a body of {} can't be imported: {e}",
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
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
        );
        assert_eq!(err.code, Code::ImportFailed);
        assert_eq!(err.details["step"], json!(["#12"]));
        assert_eq!(err.details["reason"], "gap_too_large");
    }
}
