//! `import.kicad`: KiCad libraries into this project's library, or with
//! `into` a library project's it names. Every file is read, every ID
//! worked out and every object checked before anything is written; then
//! the files that differ are written in one logged, undoable change,
//! guarded by the library's revision. IDs come from KiCad's names (the
//! library's and the item's), so importing again finds the same objects:
//! unchanged ones are left alone, changed ones written again, and nothing
//! is deleted.

use super::Notes;
use super::footprint::{self as kfp, ModelRef};
use super::symbol::{self as ksym, Item};
use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::{self, Fault};
use crate::footprint::{self, Footprint, Model, Placed, StepModel};
use crate::id::Id;
use crate::libcomp::{self, LibComp, LibRef, Offer, Use};
use crate::library::{self, Library, ObjKind};
use crate::name;
use crate::project::{Project, SELF, ref_text, resolve};
use crate::server::check_revision;
use crate::session::{Session, path_of};
use crate::source::{join, write_atomic};
use crate::symbol::{self, Symbol};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The line that keeps model files byte for byte in Git.
pub const MODELS_LINE: &str = "library/models/** binary";
/// Where that line goes, beside the library project's manifest.
pub const GITATTRIBUTES: &str = ".gitattributes";

/// What an import is asked to read, every path absolute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// `.kicad_sym` files, `.pretty` folders (or `.kicad_mod` files in
    /// one) and `.3dshapes` folders.
    pub paths: Vec<PathBuf>,
    /// Where to look for footprints symbols name: folders holding
    /// `<library>.pretty` folders, or a `.pretty` folder itself.
    pub footprints: Vec<PathBuf>,
    /// What KiCad's model variables (`${KICAD9_3DMODEL_DIR}`,
    /// `${KIPRJMOD}`, …) stand for, tried in turn.
    pub models: Vec<PathBuf>,
    /// The library to write: this project's (`None`) or an alias's.
    pub into: Option<String>,
    pub dry_run: bool,
}

/// A source file's path as the answer shows it.
fn shown(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, &e))?;
    String::from_utf8(bytes).map_err(|_| {
        Error::new(
            Code::KicadUnreadable,
            format!("{} isn't UTF-8", path.display()),
        )
        .with("path", path)
    })
}

/// The stem of `p`'s name with `ext` taken off, if it ends so.
fn stem<'a>(p: &'a Path, ext: &str) -> Option<&'a str> {
    p.file_name()?.to_str()?.strip_suffix(ext)
}

/// A footprint to read: its KiCad library (the `.pretty` folder's stem)
/// and name, and its file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FpSource {
    lib: String,
    name: String,
    file: PathBuf,
}

/// The `.kicad_mod` files of a `.pretty` folder, by name.
fn pretty(dir: &Path) -> Result<Vec<FpSource>> {
    let lib = stem(dir, ".pretty").unwrap_or_default().to_owned();
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| Error::io(dir, &e))? {
        let e = e.map_err(|e| Error::io(dir, &e))?;
        let file = e.path();
        if let Some(name) = stem(&file, ".kicad_mod") {
            out.push(FpSource {
                lib: lib.clone(),
                name: name.to_owned(),
                file: file.clone(),
            });
        }
    }
    out.sort();
    Ok(out)
}

/// What the positional paths are.
#[derive(Default)]
struct Sources {
    symbols: Vec<PathBuf>,
    footprints: Vec<FpSource>,
    pretties: Vec<PathBuf>,
    shapes: Vec<PathBuf>,
}

fn classify(paths: &[PathBuf]) -> Result<Sources> {
    let mut s = Sources::default();
    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        if !p.exists() {
            return Err(
                Error::new(Code::NotFound, format!("{} isn't there", p.display())).with("path", p),
            );
        }
        match (ext.as_deref(), p.is_dir()) {
            (Some("kicad_sym"), false) => s.symbols.push(p.clone()),
            (Some("pretty"), true) => {
                s.footprints.extend(pretty(p)?);
                s.pretties.push(p.clone());
            }
            (Some("kicad_mod"), false) => s.footprints.push(FpSource {
                lib: p
                    .parent()
                    .and_then(|d| stem(d, ".pretty"))
                    .unwrap_or_default()
                    .to_owned(),
                name: stem(p, ".kicad_mod").unwrap_or_default().to_owned(),
                file: p.clone(),
            }),
            (Some("3dshapes"), true) => s.shapes.push(p.clone()),
            (Some("lib" | "dcm" | "mod"), false) => {
                return Err(Error::new(
                    Code::KicadLegacyUnsupported,
                    format!(
                        "{} is a KiCad 5 library; open and save it in KiCad 6 or later",
                        p.display()
                    ),
                )
                .with("path", p));
            }
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "{} isn't a .kicad_sym file, a .pretty or .3dshapes folder or a .kicad_mod file",
                        p.display()
                    ),
                )
                .with("path", p));
            }
        }
    }
    Ok(s)
}

/// `Lib:Name` as a symbol's `Footprint` property names it, found in the
/// positional `.pretty` folders, then in each `--footprints` folder.
fn find_footprint(lib: &str, name: &str, s: &Sources, dirs: &[PathBuf]) -> Option<FpSource> {
    let file = format!("{name}.kicad_mod");
    let pretty = format!("{lib}.pretty");
    let candidates = s
        .pretties
        .iter()
        .chain(dirs.iter())
        .flat_map(|d| {
            let own = d.file_name().is_some_and(|n| n == pretty.as_str());
            [
                own.then(|| d.join(&file)),
                Some(d.join(&pretty).join(&file)),
            ]
        })
        .flatten();
    for c in candidates {
        if c.is_file() {
            return Some(FpSource {
                lib: lib.to_owned(),
                name: name.to_owned(),
                file: c,
            });
        }
    }
    None
}

/// The ID an item takes: its readable form of `<library> <item>`,
/// numbered on a collision within this import.
fn id_for(lib: &str, item: &str, taken: &mut BTreeSet<String>) -> Id {
    let text = if lib.is_empty() {
        item.to_owned()
    } else {
        format!("{lib} {item}")
    };
    let id = name::free(&text, |s| taken.contains(s));
    taken.insert(id.to_string());
    id
}

/// What a symbol's properties give a library component; the rest are
/// parameters, as written.
fn component(props: &[(String, String)], symbol: LibRef, footprints: Vec<Use>) -> LibComp {
    let squash = |k: &str| -> String {
        k.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    const MAKER: [&str; 5] = ["manufacturer", "mfr", "mf", "manufacturername", "mfrname"];
    const MPN: [&str; 7] = [
        "mpn",
        "manufacturerpartnumber",
        "mfrpn",
        "mfrpartnumber",
        "mfpn",
        "manufacturerpn",
        "partnumber",
    ];
    const OFFERS: [(&str, &[&str]); 3] = [
        (
            "LCSC",
            &[
                "lcsc",
                "lcscpart",
                "lcscpartnumber",
                "lcscpn",
                "jlcpcbpart",
                "jlcpcbpartnumber",
            ],
        ),
        (
            "DigiKey",
            &["digikey", "digikeypn", "digikeypartnumber", "dkpn"],
        ),
        ("Mouser", &["mouser", "mouserpn", "mouserpartnumber"]),
    ];
    let mut c = LibComp {
        symbol,
        prefix: "U".to_owned(),
        value: String::new(),
        description: String::new(),
        datasheet: String::new(),
        parameters: BTreeMap::new(),
        manufacturer: String::new(),
        mpn: String::new(),
        offers: Vec::new(),
        footprints,
    };
    for (k, v) in props {
        let key = squash(k);
        match k.as_str() {
            "Reference" => {
                let letters: String = v
                    .trim_start_matches('#')
                    .chars()
                    .take_while(char::is_ascii_alphabetic)
                    .collect();
                if !letters.is_empty() {
                    c.prefix = letters;
                }
            }
            "Value" => c.value = v.clone(),
            "Datasheet" => c.datasheet = if v == "~" { String::new() } else { v.clone() },
            "Description" | "ki_description" => c.description = v.clone(),
            "Footprint" => {}
            _ if v.is_empty() && MAKER.iter().chain(MPN.iter()).any(|m| *m == key) => {}
            _ if MAKER.contains(&key.as_str()) && c.manufacturer.is_empty() => {
                c.manufacturer = v.clone()
            }
            _ if MPN.contains(&key.as_str()) && c.mpn.is_empty() => c.mpn = v.clone(),
            _ => match OFFERS.iter().find(|(_, keys)| keys.contains(&key.as_str())) {
                Some((supplier, _)) if !v.is_empty() => {
                    if !c.offers.iter().any(|o| o.supplier == *supplier) {
                        c.offers.push(Offer {
                            supplier: (*supplier).to_owned(),
                            sku: v.clone(),
                        });
                    }
                }
                Some(_) => {}
                None => {
                    c.parameters.insert(k.clone(), v.clone());
                }
            },
        }
    }
    c
}

/// Where a model's file is, by KiCad's path, or why it isn't there:
/// `Err(true)` when it isn't STEP and has no STEP file beside it.
fn find_model(
    m: &ModelRef,
    pretty_dir: Option<&Path>,
    s: &Sources,
    dirs: &[PathBuf],
) -> std::result::Result<PathBuf, bool> {
    let path = m.path.replace('\\', "/");
    let rest = ["${", "$("].iter().find_map(|open| {
        let inner = path.strip_prefix(open)?;
        let close = if *open == "${" { '}' } else { ')' };
        let (_, rest) = inner.split_once(close)?;
        Some(rest.trim_start_matches('/').to_owned())
    });
    let mut bases: Vec<PathBuf> = Vec::new();
    let rel = match rest {
        Some(r) => r,
        None if Path::new(&path).is_absolute() => {
            bases.push(PathBuf::new());
            path.clone()
        }
        None => {
            if let Some(d) = pretty_dir.and_then(Path::parent) {
                bases.push(d.to_path_buf());
            }
            path.clone()
        }
    };
    bases.extend(dirs.iter().cloned());
    let mut candidates: Vec<PathBuf> = bases
        .iter()
        .map(|b| rel.split('/').fold(b.clone(), |p, x| p.join(x)))
        .collect();
    // `<Lib>.3dshapes/<file>` in a `.3dshapes` folder given itself.
    if let Some((folder, file)) = rel.rsplit_once('/') {
        let folder = folder.rsplit('/').next().unwrap_or(folder);
        for sh in &s.shapes {
            if sh.file_name().is_some_and(|n| n == folder) {
                candidates.push(sh.join(file));
            }
        }
    }
    let is_step = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("step") || e.eq_ignore_ascii_case("stp"))
    };
    let step = is_step(Path::new(&rel));
    for c in &candidates {
        if is_step(c) {
            if c.is_file() {
                return Ok(c.clone());
            }
            continue;
        }
        for ext in ["step", "stp", "STEP", "STP"] {
            let sibling = c.with_extension(ext);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    Err(!step)
}

/// One object an import writes.
struct Planned {
    kind: ObjKind,
    id: Id,
    text: String,
}

/// The answer's lists for one kind of object.
#[derive(Default)]
struct Tally {
    added: Vec<String>,
    updated: Vec<String>,
    unchanged: usize,
}

impl Tally {
    fn json(&self) -> Value {
        let sorted = |v: &[String]| {
            let mut v = v.to_vec();
            v.sort();
            v
        };
        json!({"added": sorted(&self.added), "updated": sorted(&self.updated), "unchanged": self.unchanged})
    }
}

/// Every fault an import found reading, for one file.
fn notes_for(path: &Path, notes: &Notes, power: &[String], demorgan: &[String]) -> Vec<Fault> {
    let p = shown(path);
    let mut out = notes.faults(&p);
    if !power.is_empty() {
        out.push(
            Fault::warning(
                Code::ImportPowerSkipped,
                format!(
                    "{p}: power symbols {} skipped: a rail is a net with intent, not a component",
                    power.join(", ")
                ),
            )
            .with("path", &p)
            .with("symbols", power),
        );
    }
    if !demorgan.is_empty() {
        out.push(
            Fault::warning(
                Code::ImportDemorganDropped,
                format!(
                    "{p}: the De Morgan body styles of {} dropped",
                    demorgan.join(", ")
                ),
            )
            .with("path", &p)
            .with("symbols", demorgan),
        );
    }
    out
}

/// A refusal of one item: where it came from, what it was, and why.
fn refusal(source: &Path, item: &str, kind: ObjKind, e: &Error) -> Value {
    json!({"source": shown(source), "item": item, "kind": kind.as_str(), "error": e})
}

/// `import.kicad`.
pub fn import(session: &mut Session, opts: &Options, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    let alias = opts.into.clone().unwrap_or_else(|| SELF.to_owned());
    let lib = p.library(&alias)?;
    check_revision(&format!("library {alias}"), &lib.revision, expected)?;
    let src = classify(&opts.paths)?;
    let mut notices: Vec<Fault> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();

    // Symbol libraries, each read whole before anything else.
    let symbol_libs: Vec<(PathBuf, ksym::Library)> = src
        .symbols
        .par_iter()
        .map(|path| {
            let text = read_text(path)?;
            ksym::read(&text)
                .map(|l| (path.clone(), l))
                .map_err(|e| e.with("path", path))
        })
        .collect::<Result<_>>()?;

    // Footprints: those given, and those the symbols name and the
    // folders hold.
    let mut fp_sources: BTreeSet<FpSource> = src.footprints.iter().cloned().collect();
    let mut unresolved_fp: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut named: BTreeMap<(String, String), Option<FpSource>> = BTreeMap::new();
    for (path, l) in &symbol_libs {
        for (_, it) in &l.items {
            let Ok(it) = it else { continue };
            let Some((_, fp)) = it.properties.iter().find(|(k, _)| k == "Footprint") else {
                continue;
            };
            let Some((flib, fname)) = fp.split_once(':') else {
                if !fp.is_empty() {
                    unresolved_fp
                        .entry(path.clone())
                        .or_default()
                        .insert(fp.clone());
                }
                continue;
            };
            let key = (flib.to_owned(), fname.to_owned());
            if named.contains_key(&key) {
                continue;
            }
            let found = fp_sources
                .iter()
                .find(|s| s.lib == flib && s.name == fname)
                .cloned()
                .or_else(|| find_footprint(flib, fname, &src, &opts.footprints));
            if let Some(f) = &found {
                fp_sources.insert(f.clone());
            }
            named.insert(key, found);
        }
    }

    // IDs, in the order of KiCad's names.
    let mut taken = BTreeSet::new();
    let fp_ids: BTreeMap<(String, String), Id> = fp_sources
        .iter()
        .map(|s| {
            (
                (s.lib.clone(), s.name.clone()),
                id_for(&s.lib, &s.name, &mut taken),
            )
        })
        .collect();
    let fp_list: Vec<FpSource> = fp_sources.into_iter().collect();
    let read_fps: Vec<(FpSource, Notes, Result<kfp::Read>)> = fp_list
        .into_par_iter()
        .map(|s| {
            let mut notes = Notes::default();
            let r = read_text(&s.file).and_then(|t| kfp::read(&t, &mut notes));
            (s, notes, r)
        })
        .collect();

    // Models, found and read; stored once by their bytes.
    let existing_models: BTreeMap<String, String> = lib
        .models
        .iter()
        .map(|(file, m)| (m.blake3.clone(), file.clone()))
        .collect();
    let mut model_files: BTreeMap<String, (String, Arc<[u8]>)> = BTreeMap::new();
    let mut model_names: BTreeSet<String> = lib.models.keys().cloned().collect();
    let mut models_added: Vec<String> = Vec::new();
    let mut models_reused = 0usize;
    let mut planned: Vec<Planned> = Vec::new();
    let mut footprint_ok: BTreeSet<Id> = BTreeSet::new();
    for (s, notes, r) in read_fps {
        let id = fp_ids[&(s.lib.clone(), s.name.clone())].clone();
        let mut skipped = Vec::new();
        let mut unresolved = Vec::new();
        let read = match r {
            Ok(r) => r,
            Err(e) => {
                refused.push(refusal(&s.file, &s.name, ObjKind::Footprint, &e));
                notices.extend(notes.faults(&shown(&s.file)));
                continue;
            }
        };
        let mut f: Footprint = read.footprint;
        notices.extend(notes.faults(&shown(&s.file)));
        // Checked before its models are stored, so a footprint refused
        // leaves no model file behind.
        let errors: Vec<Fault> = f.check().into_iter().filter(Fault::is_error).collect();
        if let Some(first) = errors.first() {
            let e = first.error.clone().with("faults", fault::list(&errors));
            refused.push(refusal(&s.file, &s.name, ObjKind::Footprint, &e));
            continue;
        }
        for m in &read.models {
            let found = find_model(m, s.file.parent(), &src, &opts.models);
            let file = match found {
                Ok(file) => file,
                Err(true) => {
                    skipped.push(m.path.clone());
                    continue;
                }
                Err(false) => {
                    unresolved.push(m.path.clone());
                    continue;
                }
            };
            let bytes: Arc<[u8]> = std::fs::read(&file)
                .map_err(|e| Error::io(&file, &e))?
                .into();
            let blake3 = blake3::hash(&bytes).to_hex().to_string();
            let stored = match existing_models.get(&blake3) {
                Some(name) => {
                    models_reused += 1;
                    name.clone()
                }
                None => match model_files.get(&blake3) {
                    Some((name, _)) => name.clone(),
                    None => {
                        let stem = file.file_stem().and_then(|x| x.to_str()).unwrap_or("model");
                        let base = name::readable(stem);
                        let fname = std::iter::once(format!("{base}.step"))
                            .chain((2u64..).map(|n| format!("{base}-{n}.step")))
                            .find(|n| !model_names.contains(n))
                            .expect("some name is free");
                        model_names.insert(fname.clone());
                        models_added.push(fname.clone());
                        model_files.insert(blake3.clone(), (fname.clone(), bytes));
                        fname
                    }
                },
            };
            f.models.push(Placed {
                model: Model::Step(StepModel {
                    path: format!("library/models/{stored}"),
                    blake3,
                }),
                offset_nm: m.offset_nm,
                rotation_mdeg: m.rotation_mdeg,
                scale_ppm: m.scale_ppm,
            });
        }
        let p_str = shown(&s.file);
        if !skipped.is_empty() {
            notices.push(
                Fault::warning(
                    Code::ImportModelSkipped,
                    format!(
                        "{p_str}: models {} aren't STEP and have no STEP file beside them",
                        skipped.join(", ")
                    ),
                )
                .with("path", &p_str)
                .with("models", &skipped),
            );
        }
        if !unresolved.is_empty() {
            notices.push(
                Fault::warning(
                    Code::ImportUnresolved,
                    format!(
                        "{p_str}: models {} aren't where the import looked (--models)",
                        unresolved.join(", ")
                    ),
                )
                .with("path", &p_str)
                .with("kind", "model")
                .with("models", &unresolved),
            );
        }
        footprint_ok.insert(id.clone());
        planned.push(Planned {
            kind: ObjKind::Footprint,
            text: doc::to_string(&f, footprint::KIND, footprint::FORMAT),
            id,
        });
    }

    // Symbols and their library components.
    let mut sym_items: Vec<(String, String, &PathBuf, &Result<Item>)> = Vec::new();
    for (path, l) in &symbol_libs {
        let klib = stem(path, ".kicad_sym").unwrap_or_default().to_owned();
        for (n, it) in &l.items {
            sym_items.push((klib.clone(), n.clone(), path, it));
        }
    }
    sym_items.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut sym_taken = BTreeSet::new();
    let mut comp_taken = BTreeSet::new();
    let sym_ids: BTreeMap<(String, String), Id> = sym_items
        .iter()
        .filter(|(_, _, _, it)| it.as_ref().is_ok_and(|i| !i.power && i.extends.is_none()))
        .map(|(l, n, _, _)| ((l.clone(), n.clone()), id_for(l, n, &mut sym_taken)))
        .collect();
    let mut symbol_ok: BTreeSet<Id> = BTreeSet::new();
    let mut power: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    let mut demorgan: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    // Symbols first, so a derived symbol knows whether its parent's
    // drawing was taken.
    for (klib, n, path, it) in &sym_items {
        let Ok(it) = it else { continue };
        if it.demorgan {
            demorgan
                .entry(path.to_path_buf())
                .or_default()
                .push(n.to_string());
        }
        let Some(s) = &it.symbol else { continue };
        if it.power {
            continue;
        }
        let id = sym_ids[&(klib.clone(), n.clone())].clone();
        let errors: Vec<Fault> = s.check().into_iter().filter(Fault::is_error).collect();
        if let Some(first) = errors.first() {
            let e = first.error.clone().with("faults", fault::list(&errors));
            refused.push(refusal(path, n, ObjKind::Symbol, &e));
            continue;
        }
        symbol_ok.insert(id.clone());
        planned.push(Planned {
            kind: ObjKind::Symbol,
            text: doc::to_string::<Symbol>(s, symbol::KIND, symbol::FORMAT),
            id,
        });
    }
    for (klib, n, path, it) in &sym_items {
        let it = match it {
            Ok(it) => it,
            Err(e) => {
                refused.push(refusal(path, n, ObjKind::Symbol, e));
                continue;
            }
        };
        if it.power {
            power
                .entry(path.to_path_buf())
                .or_default()
                .push(n.to_string());
            continue;
        }
        // The drawing: its own, or for a derived symbol its parent's,
        // with the parent's properties under its own.
        let (drawn, props) = match &it.extends {
            None => (n.clone(), it.properties.clone()),
            Some(parent) => {
                let found = symbol_libs
                    .iter()
                    .find(|(p, _)| p == *path)
                    .and_then(|(_, l)| l.items.iter().find(|(x, _)| x == parent))
                    .and_then(|(_, r)| r.as_ref().ok());
                if found.is_some_and(|pi| pi.power) {
                    power
                        .entry(path.to_path_buf())
                        .or_default()
                        .push(n.to_string());
                    continue;
                }
                let Some(pi) = found.filter(|pi| pi.symbol.is_some()) else {
                    let e = Error::new(
                        Code::NotFound,
                        format!("{n} extends {parent}, which the file doesn't draw"),
                    )
                    .with("extends", parent);
                    refused.push(refusal(path, n, ObjKind::Libcomp, &e));
                    continue;
                };
                let mut props = pi.properties.clone();
                for (k, v) in &it.properties {
                    match props.iter_mut().find(|(pk, _)| pk == k) {
                        Some(slot) => slot.1 = v.clone(),
                        None => props.push((k.clone(), v.clone())),
                    }
                }
                (parent.clone(), props)
            }
        };
        let sid = sym_ids[&(klib.clone(), drawn.clone())].clone();
        if !symbol_ok.contains(&sid) {
            let e = Error::new(
                Code::NotFound,
                format!("{n}'s drawing, {drawn}, wasn't imported"),
            );
            refused.push(refusal(path, n, ObjKind::Libcomp, &e));
            continue;
        }
        let mut uses = Vec::new();
        if let Some((_, fp)) = props.iter().find(|(k, _)| k == "Footprint")
            && let Some((flib, fname)) = fp.split_once(':')
        {
            let key = (flib.to_owned(), fname.to_owned());
            let id = fp_ids
                .get(&key)
                .filter(|id| footprint_ok.contains(*id))
                .cloned()
                .or_else(|| {
                    let id = name::free(&format!("{flib} {fname}"), |_| false);
                    lib.footprints.contains_key(&id).then_some(id)
                });
            match id {
                Some(id) => uses.push(Use {
                    footprint: LibRef::local(id),
                    pins: BTreeMap::new(),
                }),
                None => {
                    unresolved_fp
                        .entry(path.to_path_buf())
                        .or_default()
                        .insert(fp.clone());
                }
            }
        }
        let c = component(&props, LibRef::local(sid), uses);
        let cid = id_for(klib, n, &mut comp_taken);
        planned.push(Planned {
            kind: ObjKind::Libcomp,
            text: doc::to_string(&c, libcomp::KIND, libcomp::FORMAT),
            id: cid,
        });
    }
    for (path, l) in &symbol_libs {
        notices.extend(notes_for(
            path,
            &l.notes,
            power.get(path).map_or(&[][..], Vec::as_slice),
            demorgan.get(path).map_or(&[][..], Vec::as_slice),
        ));
    }
    for (path, fps) in &unresolved_fp {
        let p = shown(path);
        let list: Vec<&String> = fps.iter().collect();
        notices.push(
            Fault::warning(
                Code::ImportUnresolved,
                format!(
                    "{p}: footprints {} aren't where the import looked (--footprints)",
                    list.iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with("path", &p)
            .with("kind", "footprint")
            .with("footprints", &list),
        );
    }

    // The files that differ from what the library holds.
    let files = session.snapshot();
    let mut writes: Vec<(String, Arc<[u8]>)> = Vec::new();
    let mut tallies: BTreeMap<ObjKind, Tally> = BTreeMap::new();
    for pl in &planned {
        let path = lib.path(pl.kind, &pl.id);
        let at = ref_text(&alias, pl.kind, &pl.id);
        let t = tallies.entry(pl.kind).or_default();
        match files.get(&path) {
            Some(b) if **b == *pl.text.as_bytes() => t.unchanged += 1,
            Some(_) => {
                t.updated.push(at);
                writes.push((path, pl.text.as_bytes().into()));
            }
            None => {
                t.added.push(at);
                writes.push((path, pl.text.as_bytes().into()));
            }
        }
    }
    let models_dir = join(&join(&lib.base, library::DIR), library::MODELS);
    for (name, bytes) in model_files.values() {
        writes.push((join(&models_dir, name), bytes.clone()));
    }
    if !model_files.is_empty() {
        let at = join(&lib.base, GITATTRIBUTES);
        let now = std::fs::read_to_string(path_of(session.root(), &at)).unwrap_or_default();
        if !now.lines().any(|l| l.trim() == MODELS_LINE) {
            let mut text = now;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(MODELS_LINE);
            text.push('\n');
            writes.push((at, text.into_bytes().into()));
        }
    }
    writes.sort_by(|a, b| a.0.cmp(&b.0));
    models_added.sort();

    // What the library holds after, judged in memory: the written
    // objects' faults.
    let mut after_files = files.clone();
    for (path, bytes) in &writes {
        after_files.insert(path.clone(), bytes.clone());
    }
    let aliases = resolve(session.root(), &p.manifest);
    let after = Project::load_from(session.root(), &after_files, &aliases)?;
    let faults = written_faults(after.library(&alias)?, &planned);

    let written: Vec<&str> = writes.iter().map(|(p, _)| p.as_str()).collect();
    if !opts.dry_run {
        for (path, bytes) in &writes {
            write_atomic(&path_of(session.root(), path), bytes)?;
        }
        session.reload()?;
    }
    // A dry run's is the library's as it is: nothing moved.
    let revision = session.project().library(&alias)?.revision.clone();
    notices.sort_by(|a, b| {
        let key = |f: &Fault| {
            (
                f.error.details.get("path").map(Value::to_string),
                f.error.message.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    let tally = |k: ObjKind| {
        tallies
            .get(&k)
            .map_or_else(|| Tally::default().json(), Tally::json)
    };
    Ok(json!({
        "library": alias,
        "dry_run": opts.dry_run,
        "revision": revision,
        "symbols": tally(ObjKind::Symbol),
        "footprints": tally(ObjKind::Footprint),
        "components": tally(ObjKind::Libcomp),
        "models": {"added": models_added, "reused": models_reused},
        "files": written,
        "refused": refused,
        "notices": fault::list(&notices),
        "faults": faults,
    }))
}

/// Each written object's faults, by reference, as the library holds it
/// after the import: only objects with any.
fn written_faults(lib: &Library, planned: &[Planned]) -> Value {
    let mut out = Vec::new();
    for pl in planned {
        let faults = match pl.kind {
            ObjKind::Symbol => lib.symbols.get(&pl.id).map(|l| &l.faults),
            ObjKind::Footprint => lib.footprints.get(&pl.id).map(|l| &l.faults),
            ObjKind::Libcomp => lib.components.get(&pl.id).map(|l| &l.faults),
        };
        if let Some(f) = faults.filter(|f| !f.is_empty()) {
            out.push(json!({
                "reference": ref_text(&lib.alias, pl.kind, &pl.id),
                "faults": fault::list(f),
            }));
        }
    }
    Value::Array(out)
}
