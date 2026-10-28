//! A library: the `library/` folder of a project, or of a library project
//! another names by alias. Every object is read with what its checks
//! find; a fault fails only what reads that object. Library components
//! are judged once every library is read (see [`crate::project`]), since
//! they name symbols and footprints, perhaps in other libraries.

use crate::doc;
use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use crate::footprint::{self, Footprint, Model};
use crate::id::Id;
use crate::libcomp::{self, LibComp};
use crate::source::{Kind, Source, join};
use crate::symbol::{self, Symbol};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The folder of a project that holds its library.
pub const DIR: &str = "library";
pub const SYMBOLS: &str = "symbols";
pub const FOOTPRINTS: &str = "footprints";
pub const COMPONENTS: &str = "components";
pub const MODELS: &str = "models";

/// What the protocol calls each kind of library object in references
/// (`lib/<alias>/<kind>/<id>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjKind {
    Symbol,
    Footprint,
    Libcomp,
}

impl ObjKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ObjKind::Symbol => "symbol",
            ObjKind::Footprint => "footprint",
            ObjKind::Libcomp => "libcomp",
        }
    }

    pub fn parse(s: &str) -> Option<ObjKind> {
        [ObjKind::Symbol, ObjKind::Footprint, ObjKind::Libcomp]
            .into_iter()
            .find(|k| k.as_str() == s)
    }

    /// The folder under `library/` its files are in.
    pub fn folder(self) -> &'static str {
        match self {
            ObjKind::Symbol => SYMBOLS,
            ObjKind::Footprint => FOOTPRINTS,
            ObjKind::Libcomp => COMPONENTS,
        }
    }
}

/// An object as read: the object, or why its file doesn't read, and what
/// its checks find.
#[derive(Clone, Debug)]
pub struct Loaded<T> {
    pub item: Result<T>,
    pub faults: Vec<Fault>,
}

impl<T> Loaded<T> {
    /// Whether it reads and its checks find no error.
    pub fn is_sound(&self) -> bool {
        self.item.is_ok() && !self.faults.iter().any(Fault::is_error)
    }
}

/// A model file under `library/models/`.
#[derive(Clone, Debug)]
pub struct ModelFile {
    pub blake3: String,
    pub len: usize,
    /// Why it isn't a STEP exchange structure, if it isn't.
    pub unreadable: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Library {
    /// `self`, or the alias this project names it by.
    pub alias: String,
    /// The library project's folder relative to this project (`""` for
    /// this project's own library, `../passives` for an alias's).
    pub base: String,
    /// A hash of every file the library holds: writes are guarded by it.
    pub revision: String,
    pub symbols: BTreeMap<Id, Loaded<Symbol>>,
    pub footprints: BTreeMap<Id, Loaded<Footprint>>,
    pub components: BTreeMap<Id, Loaded<LibComp>>,
    /// By file name (`r_0402.step`).
    pub models: BTreeMap<String, ModelFile>,
    /// Files in the library that are none of these: a bad name, or a
    /// folder or file ryueda doesn't know.
    pub faults: Vec<Fault>,
}

/// Each Part 21 check by BLAKE3, so a model shared by many footprints, or
/// read again after an edit elsewhere, is parsed once per process.
fn parsed(blake3: &str, bytes: &[u8]) -> Option<String> {
    static SEEN: OnceLock<Mutex<BTreeMap<String, Option<String>>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(r) = seen.lock().unwrap_or_else(|p| p.into_inner()).get(blake3) {
        return r.clone();
    }
    let r = ryu_part21::parse(bytes).err().map(|e| e.to_string());
    seen.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(blake3.to_owned(), r.clone());
    r
}

/// A file of one folder: its name and bytes.
type Named = (String, Arc<[u8]>);

/// Feeds one length-prefixed piece to a revision hash.
fn piece(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// The ID a file is named by (`r.json` is `r`), if its name is one.
fn file_id(name: &str, ext: &str) -> Option<Id> {
    Id::parse(name.strip_suffix(ext)?, "library object").ok()
}

/// Reads the library of the project at `base` (relative to the project
/// being loaded, `""` for itself), named `alias`.
pub fn load(src: &dyn Source, alias: &str, base: &str) -> Result<Library> {
    let dir = join(base, DIR);
    let mut faults = Vec::new();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ryueda.library.1");
    // Every file, in path order, for the revision and the loaders.
    let mut files: BTreeMap<&'static str, Vec<Named>> = BTreeMap::new();
    for (name, kind) in src.list(&dir)? {
        let at = join(&dir, &name);
        let known = [SYMBOLS, FOOTPRINTS, COMPONENTS, MODELS]
            .into_iter()
            .find(|f| *f == name);
        let (Some(folder), Kind::Dir) = (known, kind) else {
            faults.push(unknown(&at));
            continue;
        };
        for (file, kind) in src.list(&at)? {
            let path = join(&at, &file);
            if kind != Kind::File {
                faults.push(unknown(&path));
                continue;
            }
            let bytes = src
                .read_shared(&path)?
                .ok_or_else(|| Error::new(Code::NotFound, format!("{path} vanished")))?;
            piece(&mut hasher, join(folder, &file).as_bytes());
            piece(&mut hasher, &bytes);
            files.entry(folder).or_default().push((file, bytes));
        }
    }
    let take = |folder: &str| files.get(folder).cloned().unwrap_or_default();
    let symbols = read_all(&take(SYMBOLS), &dir, SYMBOLS, &mut faults, |b, p| {
        let s: Symbol = doc::read(b, p, symbol::KIND, symbol::FORMAT)?;
        let faults = s.check();
        Ok((s, faults))
    });
    let mut models = BTreeMap::new();
    for (file, bytes) in take(MODELS) {
        let is_step = file
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| ext == "step" && Id::parse(stem, "model").is_ok());
        if !is_step {
            faults.push(unknown(&join(&join(&dir, MODELS), &file)));
            continue;
        }
        let blake3 = blake3::hash(&bytes).to_hex().to_string();
        models.insert(
            file,
            ModelFile {
                unreadable: None,
                len: bytes.len(),
                blake3,
            },
        );
    }
    // Model files parse in parallel, each once per process.
    let model_bytes = take(MODELS);
    let checked: Vec<(String, Option<String>)> = model_bytes
        .par_iter()
        .filter_map(|(file, bytes)| {
            let m = models.get(file)?;
            Some((file.clone(), parsed(&m.blake3, bytes)))
        })
        .collect();
    for (file, why) in checked {
        models.get_mut(&file).expect("listed").unreadable = why;
    }
    let footprints = read_all(&take(FOOTPRINTS), &dir, FOOTPRINTS, &mut faults, |b, p| {
        let f: Footprint = doc::read(b, p, footprint::KIND, footprint::FORMAT)?;
        let mut faults = f.check();
        faults.extend(model_faults(&f, &models));
        Ok((f, faults))
    });
    let components = read_all(&take(COMPONENTS), &dir, COMPONENTS, &mut faults, |b, p| {
        let c: LibComp = doc::read(b, p, libcomp::KIND, libcomp::FORMAT)?;
        Ok((c, Vec::new()))
    });
    Ok(Library {
        alias: alias.to_owned(),
        base: base.to_owned(),
        revision: hasher.finalize().to_hex()[..32].to_owned(),
        symbols,
        footprints,
        components,
        models,
        faults,
    })
}

fn unknown(path: &str) -> Fault {
    Fault::warning(
        Code::InvalidDocument,
        format!("{path} isn't a file ryueda's library holds"),
    )
    .with("path", path)
}

/// Reads every `<id>.json` of one folder in parallel, each with its
/// faults and its path in their details.
fn read_all<T: Send>(
    files: &[Named],
    dir: &str,
    folder: &str,
    faults: &mut Vec<Fault>,
    read: impl Fn(&[u8], &str) -> Result<(T, Vec<Fault>)> + Sync,
) -> BTreeMap<Id, Loaded<T>> {
    let at = join(dir, folder);
    let mut named = Vec::new();
    for (file, bytes) in files {
        match file_id(file, ".json") {
            Some(id) => named.push((id, join(&at, file), bytes)),
            None => faults.push(unknown(&join(&at, file))),
        }
    }
    named
        .into_par_iter()
        .map(|(id, path, bytes)| {
            let loaded = match read(bytes, &path) {
                Ok((item, faults)) => Loaded {
                    item: Ok(item),
                    faults: faults.into_iter().map(|f| f.with("path", &path)).collect(),
                },
                Err(e) => Loaded {
                    item: Err(e),
                    faults: Vec::new(),
                },
            };
            (id, loaded)
        })
        .collect()
}

/// What a footprint's models find: a path outside `library/models/`, a
/// file that isn't there, isn't the bytes the footprint names, or isn't
/// STEP.
fn model_faults(f: &Footprint, models: &BTreeMap<String, ModelFile>) -> Vec<Fault> {
    let mut out = Vec::new();
    for (i, placed) in f.models.iter().enumerate() {
        let Model::Step(m) = &placed.model;
        let ptr = format!("/models/{i}/model/step");
        let file = m
            .path
            .strip_prefix("library/models/")
            .filter(|f| !f.contains('/'));
        let Some(found) = file.and_then(|f| models.get(f)) else {
            out.push(
                Fault::error(
                    Code::ModelFileMissing,
                    format!("{} isn't a model file under library/models/", m.path),
                )
                .with("pointer", &ptr)
                .with("model", &m.path),
            );
            continue;
        };
        if found.blake3 != m.blake3 {
            out.push(
                Fault::error(
                    Code::ModelFileChanged,
                    format!(
                        "{} isn't the file the footprint was written against",
                        m.path
                    ),
                )
                .with("pointer", &ptr)
                .with("model", &m.path)
                .with("expected", &m.blake3)
                .with("found", &found.blake3),
            );
        } else if let Some(why) = &found.unreadable {
            out.push(
                Fault::error(
                    Code::ModelUnreadable,
                    format!("{} isn't STEP: {why}", m.path),
                )
                .with("pointer", &ptr)
                .with("model", &m.path)
                .with("reason", why),
            );
        }
    }
    out
}

impl Library {
    /// The path of an object's file, relative to the project.
    pub fn path(&self, kind: ObjKind, id: &Id) -> String {
        join(
            &join(&join(&self.base, DIR), kind.folder()),
            &format!("{id}.json"),
        )
    }

    /// The IDs of every object of `kind`, as files name them.
    pub fn ids(&self, kind: ObjKind) -> Vec<&Id> {
        match kind {
            ObjKind::Symbol => self.symbols.keys().collect(),
            ObjKind::Footprint => self.footprints.keys().collect(),
            ObjKind::Libcomp => self.components.keys().collect(),
        }
    }

    /// Whether any object of `kind` has the ID `id`.
    pub fn has(&self, kind: ObjKind, id: &str) -> bool {
        self.ids(kind).iter().any(|i| i.as_str() == id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::footprint::tests::r0402;
    use crate::libcomp::tests::r_100k;
    use crate::symbol::tests::resistor;

    /// A library's files, as a project holds them.
    pub fn files(step: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut f = r0402();
        let Model::Step(m) = &mut f.models[0].model;
        m.blake3 = blake3::hash(step).to_hex().to_string();
        [
            (
                "library/symbols/r.json",
                doc::to_string(&resistor(), symbol::KIND, 1).into_bytes(),
            ),
            (
                "library/footprints/r_0402_1005metric.json",
                doc::to_string(&f, footprint::KIND, 1).into_bytes(),
            ),
            (
                "library/components/r_100k.json",
                doc::to_string(&r_100k(), libcomp::KIND, 1).into_bytes(),
            ),
            ("library/models/r_0402_1005metric.step", step.to_vec()),
        ]
        .into_iter()
        .map(|(p, b)| (p.to_owned(), b))
        .collect()
    }

    pub const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('r','',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";

    #[test]
    fn a_library_reads_every_object_with_its_faults() {
        let lib = load(&files(STEP), "self", "").unwrap();
        assert_eq!(lib.symbols.len(), 1);
        assert!(lib.symbols.values().all(Loaded::is_sound));
        let f = &lib.footprints[&Id::parse("r_0402_1005metric", "f").unwrap()];
        assert!(f.is_sound(), "{:?}", f.faults);
        assert_eq!(lib.models["r_0402_1005metric.step"].len, STEP.len());
        assert!(lib.faults.is_empty());
        assert_eq!(lib.revision.len(), 32);
        assert_eq!(
            lib.path(ObjKind::Libcomp, &Id::parse("r_100k", "c").unwrap()),
            "library/components/r_100k.json"
        );
    }

    #[test]
    fn models_must_be_there_as_written_and_step() {
        let mut fs = files(STEP);
        fs.insert(
            "library/models/r_0402_1005metric.step".into(),
            b"not step".to_vec(),
        );
        let lib = load(&fs, "self", "").unwrap();
        let codes = |lib: &Library| -> Vec<Code> {
            lib.footprints
                .values()
                .flat_map(|f| f.faults.iter().map(|x| x.error.code))
                .collect()
        };
        assert_eq!(codes(&lib), [Code::ModelFileChanged]);
        let mut fs = files(b"not step");
        fs.insert(
            "library/models/r_0402_1005metric.step".into(),
            b"not step".to_vec(),
        );
        let lib = load(&fs, "self", "").unwrap();
        assert_eq!(codes(&lib), [Code::ModelUnreadable]);
        let mut fs = files(STEP);
        fs.remove("library/models/r_0402_1005metric.step");
        let lib = load(&fs, "self", "").unwrap();
        assert_eq!(codes(&lib), [Code::ModelFileMissing]);
        let f = &lib.footprints.values().next().unwrap().faults[0];
        assert_eq!(
            f.error.details["path"],
            "library/footprints/r_0402_1005metric.json"
        );
    }

    #[test]
    fn the_revision_follows_every_file_and_bad_files_are_faults() {
        let a = load(&files(STEP), "self", "").unwrap();
        let mut fs = files(STEP);
        fs.insert("library/symbols/Bad Name.json".into(), b"{}".to_vec());
        fs.insert("library/notes.txt".into(), b"hi".to_vec());
        fs.insert("library/symbols/broken.json".into(), b"{".to_vec());
        let b = load(&fs, "self", "").unwrap();
        assert_ne!(a.revision, b.revision);
        assert_eq!(b.faults.len(), 2);
        let broken = &b.symbols[&Id::parse("broken", "s").unwrap()];
        assert_eq!(broken.item.as_ref().unwrap_err().code, Code::InvalidJson);
        // A library project's files sit under its own folder.
        let moved: BTreeMap<String, Vec<u8>> = files(STEP)
            .into_iter()
            .map(|(p, b)| (format!("../lib/{p}"), b))
            .collect();
        let c = load(&moved, "lib", "../lib").unwrap();
        assert_eq!(c.revision, a.revision);
        assert_eq!(
            c.path(ObjKind::Symbol, &Id::parse("r", "s").unwrap()),
            "../lib/library/symbols/r.json"
        );
    }
}
