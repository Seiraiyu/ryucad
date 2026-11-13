//! External-reference assemblies (design 448, plan 24.D, #443): an
//! assembly file whose products' shapes are in other STEP files, each
//! named by a document (`DOCUMENT_FILE`, a digital `DOCUMENT`, OCCT's
//! `cad_filename`), as ryustep reads them (`Definition::external`).
//!
//! [`resolve`] finds and opens every file an import reads, depth first
//! from the file given: each externally defined definition's documents
//! relative to the referencing file's folder (CAx-IF's rule), then by
//! file name in each `--search` folder. Files with the same bytes are one
//! [`Node`]. Nothing is written until every file is found:
//! `import_external_missing` lists every document that didn't resolve.
//! [`Planner`] then plans each file as an import of its own, leaves
//! first, the referencing definitions placing the parts their files make.

use super::plan::{self, Planned};
use super::update::{self, Update};
use super::{free, id_from};
use crate::doc::id::Id;
use crate::doc::import::{FileRef, Mode};
use crate::doc::project::ImportFile;
use crate::doc::project::{IMPORTS, Project};
use crate::error::{Code, Error, Result};
use ryustep::Opened;
use ryustep::products::{Definition, External};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A document's names as written, `\` read as `/`: its location, then
/// its name.
fn written(x: &External) -> Vec<String> {
    x.location
        .iter()
        .chain(std::iter::once(&x.name))
        .map(|s| s.trim().replace('\\', "/"))
        .filter(|s| !s.is_empty())
        .collect()
}

/// A document's file names alone (each written name's last segment), in
/// order, once each.
fn file_names(x: &External) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in written(x) {
        if let Some(n) = w.rsplit('/').next().filter(|n| !n.is_empty())
            && !out.iter().any(|o| o == n)
        {
            out.push(n.to_owned());
        }
    }
    out
}

/// Whether a written path is absolute on some system: `/…` or `C:…`.
fn absolute(w: &str) -> bool {
    let b = w.as_bytes();
    w.starts_with('/') || (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic())
}

/// Where a document may be, in order: its location, then its name,
/// relative to `folder` (each unless written absolute); then its file
/// name alone in `folder` and in each of `search`.
pub fn candidates(x: &External, folder: &Path, search: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for w in written(x) {
        if !absolute(&w) {
            push(folder.join(w.trim_start_matches("./")));
        }
    }
    let names = file_names(x);
    for n in &names {
        push(folder.join(n));
    }
    for dir in search {
        for n in &names {
            push(dir.join(n));
        }
    }
    out
}

/// Whether `p` is a file whose every named component is spelled as its
/// folder holds it. A case-insensitive disk (macOS's, Windows') finds
/// `PART1.STEP` for `Part1.step`; this doesn't, so a document resolves to
/// the same path on every system (the ignoring-case pass then names the
/// file as the disk does).
fn exactly(p: &Path) -> bool {
    use std::path::Component;
    if !p.is_file() {
        return false;
    }
    let mut at = PathBuf::new();
    for c in p.components() {
        if let Component::Normal(name) = c {
            let dir = if at.as_os_str().is_empty() {
                Path::new(".")
            } else {
                at.as_path()
            };
            let Ok(entries) = std::fs::read_dir(dir) else {
                return false;
            };
            if !entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name() == name)
            {
                return false;
            }
        }
        at.push(c);
    }
    true
}

/// The file a document is: the first of [`candidates`] that is a file
/// spelled exactly as on disk, else, in `folder` and then each of
/// `search`, the one file whose name matches a file name of it ignoring
/// ASCII case.
pub fn locate(x: &External, folder: &Path, search: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = candidates(x, folder, search)
        .into_iter()
        .find(|p| exactly(p))
    {
        return Some(p);
    }
    let names = file_names(x);
    for dir in std::iter::once(folder).chain(search.iter().map(PathBuf::as_path)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut hits: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                names.iter().any(|w| w.eq_ignore_ascii_case(&n))
            })
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        hits.sort();
        if let [p] = &hits[..] {
            return Some(p.clone());
        }
    }
    None
}

/// Whether a document's format, as its writer names it, is one ryucad
/// reads: unnamed, or naming STEP, Part 21 or ISO 10303.
pub fn is_step(format: Option<&str>) -> bool {
    format.filter(|f| !f.trim().is_empty()).is_none_or(|f| {
        let l = f.to_ascii_lowercase();
        ["step", "stp", "10303", "p21", "part 21", "part21"]
            .iter()
            .any(|s| l.contains(s))
    })
}

/// How deep external files may reference further files.
pub const MAX_DEPTH: usize = 32;

/// One file an import reads.
pub struct Node {
    /// Where it was read: the path given for the first, else the file a
    /// document resolved to.
    pub source: PathBuf,
    pub file: ImportFile,
    pub opened: Arc<Opened>,
    /// Each externally defined definition of this file: the node its
    /// shape is in, and that file's definition it is.
    pub links: BTreeMap<usize, (usize, usize)>,
    /// The documents that resolved to it, as written, in the order met.
    pub documents: Vec<String>,
    /// The instances of this file's documents that resolved: interpreted
    /// in `external_references` (design 448).
    pub interpreted: Vec<u64>,
}

/// Every file an import reads: the file given first, then each file a
/// document resolved to, depth first in definition and document order.
pub struct Tree {
    pub nodes: Vec<Node>,
}

/// A document that didn't resolve.
struct Missing {
    file: String,
    location: Option<String>,
    format: Option<String>,
    reason: &'static str,
    products: Vec<String>,
    referenced_by: String,
    searched: Vec<String>,
}

struct Resolver<'a> {
    search: &'a [PathBuf],
    /// The first file as the agent gave it.
    shown: &'a str,
    nodes: Vec<Node>,
    by_hash: BTreeMap<String, usize>,
    missing: Vec<Missing>,
}

/// Every file `file` (read from `source`, `shown` as the agent gave it)
/// needs: the file itself, then depth first each file its externally
/// defined definitions' documents resolve to, looked for as [`locate`]
/// looks, each from the folder of the file naming it, then in `search`.
/// Refusals: `import_external_missing` (every document that didn't
/// resolve), `import_external_ambiguous`, `assembly_cycle`,
/// `invalid_param` (more than [`MAX_DEPTH`] deep), `step_unreadable`
/// (an external file ryustep can't read, with `referenced_by`).
pub fn resolve(
    source: &Path,
    file: ImportFile,
    opened: Arc<Opened>,
    search: &[PathBuf],
    shown: &str,
) -> Result<Tree> {
    let mut r = Resolver {
        search,
        shown,
        nodes: Vec::new(),
        by_hash: BTreeMap::new(),
        missing: Vec::new(),
    };
    r.add(source.to_path_buf(), file, opened);
    r.visit(0, &mut Vec::new())?;
    if !r.missing.is_empty() {
        return Err(r.missing_error());
    }
    Ok(Tree { nodes: r.nodes })
}

/// The definition of `node`'s file a referencing definition `d` stands
/// for: its one root that makes something, else the one whose product id
/// equals `d`'s, else whose name does. `Err` with the roots otherwise.
fn root_for(node: &Node, d: &Definition) -> std::result::Result<usize, Vec<usize>> {
    let o = &node.opened;
    let linked: BTreeSet<usize> = node.links.keys().copied().collect();
    let makes = crate::import::plan::makes_with(o, &linked);
    let roots: Vec<usize> = o
        .products
        .roots
        .iter()
        .copied()
        .filter(|&k| makes[k])
        .collect();
    if let [r] = roots[..] {
        return Ok(r);
    }
    let passes: [fn(&Definition, &Definition) -> bool; 2] = [
        |a, b| !b.product_id.is_empty() && a.product_id == b.product_id,
        |a, b| !b.name.is_empty() && a.name == b.name,
    ];
    for same in passes {
        let hits: Vec<usize> = roots
            .iter()
            .copied()
            .filter(|&r| same(&o.products.definitions[r], d))
            .collect();
        if let [r] = hits[..] {
            return Ok(r);
        }
    }
    Err(roots)
}

impl Resolver<'_> {
    fn add(&mut self, source: PathBuf, file: ImportFile, opened: Arc<Opened>) -> usize {
        let j = self.nodes.len();
        self.by_hash.insert(file.blake3.clone(), j);
        self.nodes.push(Node {
            source,
            file,
            opened,
            links: BTreeMap::new(),
            documents: Vec::new(),
            interpreted: Vec::new(),
        });
        j
    }

    /// Node `i` as answers name it: the path the agent gave for the first.
    fn shown(&self, i: usize) -> String {
        if i == 0 {
            self.shown.to_owned()
        } else {
            self.nodes[i].source.display().to_string()
        }
    }

    /// Resolves node `i`'s documents, and theirs, depth first; `stack`
    /// holds the nodes being followed above it.
    fn visit(&mut self, i: usize, stack: &mut Vec<usize>) -> Result<()> {
        if stack.len() > MAX_DEPTH {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{} references files more than {MAX_DEPTH} deep: import a file nearer the parts",
                    self.shown
                ),
            )
            .with("param", "external")
            .with("depth", MAX_DEPTH));
        }
        stack.push(i);
        let opened = self.nodes[i].opened.clone();
        let folder = self.nodes[i]
            .source
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        for (k, d) in opened.products.definitions.iter().enumerate() {
            if !d.externally_defined() {
                continue;
            }
            let Some((path, x)) = self.find(i, d, &folder) else {
                continue;
            };
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
            let file = ImportFile::new(bytes);
            let before = self.missing.len();
            let j = match self.by_hash.get(&file.blake3).copied() {
                Some(j) if stack.contains(&j) => return Err(self.cycle(stack, j)),
                Some(j) => j,
                None => {
                    let o = super::opened(&file).map_err(|e| {
                        super::unreadable(&path.display().to_string(), &e)
                            .with("referenced_by", self.shown(i))
                    })?;
                    let j = self.add(path.clone(), file, o);
                    self.visit(j, stack)?;
                    j
                }
            };
            // A file with documents of its own missing is refused for them.
            if self.missing.len() > before {
                continue;
            }
            let r = root_for(&self.nodes[j], d).map_err(|roots| self.ambiguous(i, j, d, &roots))?;
            self.nodes[i].links.insert(k, (j, r));
            if !self.nodes[j].documents.contains(&x.name) {
                self.nodes[j].documents.push(x.name.clone());
            }
            self.nodes[i].interpreted.extend(x.related.iter().copied());
        }
        self.nodes[i].interpreted.sort_unstable();
        self.nodes[i].interpreted.dedup();
        stack.pop();
        Ok(())
    }

    /// The file `d`'s first STEP document that resolves, else `None`
    /// with each of its documents recorded missing.
    fn find(&mut self, i: usize, d: &Definition, folder: &Path) -> Option<(PathBuf, External)> {
        for x in &d.external {
            if is_step(x.format.as_deref())
                && let Some(p) = locate(x, folder, self.search)
            {
                return Some((p, x.clone()));
            }
        }
        let product = if d.product_id.is_empty() {
            d.name.to_string()
        } else {
            d.product_id.to_string()
        };
        let referenced_by = self.shown(i);
        let mut searched: Vec<String> = Vec::new();
        for p in std::iter::once(folder).chain(self.search.iter().map(PathBuf::as_path)) {
            let s = p.display().to_string();
            if !searched.contains(&s) {
                searched.push(s);
            }
        }
        for x in &d.external {
            let reason = if is_step(x.format.as_deref()) {
                "not_found"
            } else {
                "format"
            };
            match self
                .missing
                .iter_mut()
                .find(|m| m.file == x.name && m.referenced_by == referenced_by)
            {
                Some(m) => {
                    if !m.products.contains(&product) {
                        m.products.push(product.clone());
                    }
                }
                None => self.missing.push(Missing {
                    file: x.name.clone(),
                    location: x.location.clone(),
                    format: x.format.clone(),
                    reason,
                    products: vec![product.clone()],
                    referenced_by: referenced_by.clone(),
                    searched: searched.clone(),
                }),
            }
        }
        None
    }

    fn missing_error(&self) -> Error {
        let mut names: Vec<&str> = Vec::new();
        for m in &self.missing {
            if !names.contains(&m.file.as_str()) {
                names.push(&m.file);
            }
        }
        let list: Vec<Value> = self
            .missing
            .iter()
            .map(|m| {
                let mut v = json!({"file": m.file, "products": m.products,
                                   "referenced_by": m.referenced_by,
                                   "searched": m.searched, "reason": m.reason});
                if let Some(l) = &m.location {
                    v["location"] = json!(l);
                }
                if let Some(f) = &m.format {
                    v["format"] = json!(f);
                }
                v
            })
            .collect();
        Error::new(
            Code::ImportExternalMissing,
            format!(
                "{} places parts kept in other files that weren't found: {}. Put each beside the file naming it, or give the folders holding them with --search",
                self.shown,
                names.join(", ")
            ),
        )
        .with("path", self.shown)
        .with("missing", list)
    }

    fn cycle(&self, stack: &[usize], j: usize) -> Error {
        let from = stack.iter().position(|&s| s == j).unwrap_or(0);
        let files: Vec<String> = stack[from..]
            .iter()
            .map(|&s| self.shown(s))
            .chain([self.shown(j)])
            .collect();
        Error::new(
            Code::AssemblyCycle,
            format!(
                "files reference each other in a loop: {}",
                files.join(" > ")
            ),
        )
        .with("files", files)
    }

    fn ambiguous(&self, i: usize, j: usize, d: &Definition, roots: &[usize]) -> Error {
        let o = &self.nodes[j].opened;
        let listed: Vec<Value> = roots
            .iter()
            .map(|&r| {
                let x = &o.products.definitions[r];
                json!({"id": &*x.product_id, "name": &*x.name})
            })
            .collect();
        let (file, by) = (self.shown(j), self.shown(i));
        let message = if roots.is_empty() {
            format!(
                "{file}, which {by} names for product {:?}, holds nothing to import",
                &*d.product_id
            )
        } else {
            format!(
                "{file} has {} roots and none alone is product {:?} that {by} names: import the file whole, or the product --into a part",
                roots.len(),
                &*d.product_id
            )
        };
        Error::new(Code::ImportExternalAmbiguous, message)
            .with("file", file)
            .with("referenced_by", by)
            .with("product", json!({"id": &*d.product_id, "name": &*d.name}))
            .with("roots", listed)
    }
}

impl Tree {
    /// The definitions of node `j` that other files' documents stand for.
    pub fn roots_into(&self, j: usize) -> BTreeSet<usize> {
        self.nodes
            .iter()
            .flat_map(|n| n.links.values())
            .filter(|(c, _)| *c == j)
            .map(|(_, r)| *r)
            .collect()
    }

    /// The ID node `j`'s definition `r`'s part starts from: the first
    /// referencing product's id, else its name.
    pub fn base(&self, j: usize, r: usize) -> Option<Id> {
        self.nodes
            .iter()
            .find_map(|n| {
                n.links
                    .iter()
                    .find(|(_, l)| **l == (j, r))
                    .map(|(&k, _)| &n.opened.products.definitions[k])
            })
            .and_then(|d| id_from(&d.product_id).or_else(|| id_from(&d.name)))
    }

    /// Node `j`'s definitions that `roots` reach through usages within
    /// its file.
    pub fn reach(&self, j: usize, roots: &BTreeSet<usize>) -> BTreeSet<usize> {
        let defs = &self.nodes[j].opened.products.definitions;
        let mut out = BTreeSet::new();
        let mut todo: Vec<usize> = roots.iter().copied().collect();
        while let Some(k) = todo.pop() {
            if out.insert(k) {
                todo.extend(defs[k].children.iter().map(|c| c.definition));
            }
        }
        out
    }
}

/// Whether a planned import feature or imported instance names `path`.
pub fn names(all: &[Planned], path: &str) -> bool {
    use crate::doc::files::Feature;
    all.iter()
        .flat_map(|pl| &pl.features)
        .any(|(_, f)| match f {
            Feature::Import(i) => i.file.path == path,
            Feature::Instance(i) => i.import.as_ref().is_some_and(|u| u.file == path),
            _ => false,
        })
}

/// `planned` with each part once: the features, removals and properties
/// of a part several files plan, in order.
pub fn merge(planned: Vec<Planned>) -> Vec<Planned> {
    let mut out: Vec<Planned> = Vec::new();
    for pl in planned {
        match out.iter_mut().find(|o| o.part == pl.part) {
            Some(o) => {
                for f in pl.features {
                    if !o.features.iter().any(|(id, _)| *id == f.0) {
                        o.features.push(f);
                    }
                }
                o.removed.extend(pl.removed);
                for (k, v) in pl.properties {
                    o.properties.entry(k).or_insert(v);
                }
            }
            None => out.push(pl),
        }
    }
    out
}

/// One file's share of an import across files.
pub struct Group {
    pub node: usize,
    /// Its path under `imports/`.
    pub path: String,
    /// A fresh file whose bytes the project held there already.
    pub reused: bool,
    /// An update's: the BLAKE3 the stored file had.
    pub from: Option<String>,
    pub planned: Vec<Planned>,
    /// An update's own account (its `planned` moved to the group's).
    pub update: Option<Update>,
}

impl Group {
    /// Whether the import writes this file's bytes (`blake3` the node's):
    /// a fresh file some planned feature names that the project doesn't
    /// hold, or an update's changed file.
    pub fn writes_bytes(&self, all: &[Planned], blake3: &str) -> bool {
        match &self.from {
            Some(old) => old != blake3,
            None => !self.reused && names(all, &self.path),
        }
    }
}

/// Plans every file of a [`Tree`] as an import of its own, leaves first,
/// each externally defined definition placing the part its file made.
pub struct Planner<'a> {
    pub project: &'a Project,
    pub tree: &'a Tree,
    pub mode: Option<Mode>,
    pub pdm: bool,
    /// `--part`: the first file's one root.
    pub part: Option<Id>,
    /// Whether an ID is taken in the project: by a part, or retired.
    pub taken: &'a dyn Fn(&str) -> bool,
    /// What each file plans, in the order planned.
    pub groups: Vec<Group>,
    paths: Vec<String>,
    ids: Vec<String>,
    planned: BTreeSet<usize>,
    part_of: BTreeMap<(usize, usize), Id>,
}

impl<'a> Planner<'a> {
    pub fn new(
        project: &'a Project,
        tree: &'a Tree,
        mode: Option<Mode>,
        pdm: bool,
        part: Option<Id>,
        taken: &'a dyn Fn(&str) -> bool,
    ) -> Planner<'a> {
        Planner {
            project,
            tree,
            mode,
            pdm,
            ids: part.iter().map(|p| p.to_string()).collect(),
            part,
            taken,
            groups: Vec::new(),
            paths: Vec::new(),
            planned: BTreeSet::new(),
            part_of: BTreeMap::new(),
        }
    }

    /// `--flatten` across files (design 448, 24.D): one part of every
    /// body of every file the walk reaches (`plan::walk_files`), each
    /// feature naming its own file. Every file gets a path; only those a
    /// feature names are written ([`Group::writes_bytes`]).
    pub fn flatten(&mut self) -> Result<()> {
        let tree = self.tree;
        let stored: Vec<(String, bool)> = (0..tree.nodes.len()).map(|j| self.store(j)).collect();
        let refs: Vec<FileRef> = tree
            .nodes
            .iter()
            .zip(&stored)
            .map(|(n, (p, _))| FileRef {
                path: p.clone(),
                blake3: n.file.blake3.clone(),
            })
            .collect();
        let files = plan::Files {
            opened: tree.nodes.iter().map(|n| &*n.opened).collect(),
            paths: refs.iter().map(|r| r.path.as_str()).collect(),
            links: tree.nodes.iter().map(|n| &n.links).collect(),
        };
        let stem = Path::new(&stored[0].0)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ids = self.ids.clone();
        let base = self.taken;
        let taken = move |s: &str| base(s) || ids.iter().any(|t| t == s);
        let flat = plan::Flat {
            opened: &tree.nodes[0].opened,
            file: &refs[0],
            stem: &stem,
            part: self.part.clone(),
            taken: &taken,
            pdm: self.pdm,
            mode: self.mode,
            cross: Some((&files, &refs)),
        }
        .plan()?;
        self.record(0, std::slice::from_ref(&flat));
        let mut flat = Some(flat);
        for (j, (path, reused)) in stored.into_iter().enumerate() {
            let planned = if j == 0 {
                flat.take().into_iter().collect()
            } else {
                Vec::new()
            };
            self.groups.push(Group {
                node: j,
                path,
                reused,
                from: None,
                planned,
                update: None,
            });
            self.planned.insert(j);
        }
        Ok(())
    }

    /// Where node `j`'s bytes are kept: where the project holds them
    /// already, else `imports/<stem>.step` free in the project and this
    /// import.
    fn store(&mut self, j: usize) -> (String, bool) {
        let node = &self.tree.nodes[j];
        if let Some((p, _)) = self
            .project
            .imports
            .iter()
            .find(|(_, f)| f.blake3 == node.file.blake3)
        {
            return (p.clone(), true);
        }
        let stem = node
            .source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());
        let base = stem
            .as_deref()
            .and_then(id_from)
            .unwrap_or_else(|| Id::parse("step", "file").expect("an ID"));
        let root = &self.project.root;
        let id = free(&base, |s| {
            let p = format!("{IMPORTS}/{s}.step");
            self.project.imports.contains_key(&p)
                || root.join(&p).exists()
                || self.paths.contains(&p)
        });
        let p = format!("{IMPORTS}/{id}.step");
        self.paths.push(p.clone());
        (p, false)
    }

    fn record(&mut self, j: usize, planned: &[Planned]) {
        for pl in planned {
            self.ids.push(pl.part.to_string());
            if let Some(r) = pl.definition {
                self.part_of
                    .entry((j, r))
                    .or_insert_with(|| pl.part.clone());
            }
        }
    }

    /// Plans node `j` and, first, every file it references: an update of
    /// the stored file `old` with node `j`'s bytes, or a fresh import.
    /// An update of a referenced file whose bytes the project holds
    /// already plans nothing for it.
    pub fn visit(&mut self, j: usize, old: Option<String>) -> Result<()> {
        if !self.planned.insert(j) {
            return Ok(());
        }
        let tree = self.tree;
        let node = &tree.nodes[j];
        let olds: BTreeMap<usize, Id> = match &old {
            Some(p) => self
                .project
                .imports
                .get(p)
                .and_then(|f| super::opened(f).ok())
                .map(|o| update::external_parts(self.project, p, &o, &node.opened))
                .unwrap_or_default(),
            None => BTreeMap::new(),
        };
        let mut external = BTreeMap::new();
        for (&k, &(c, r)) in &node.links {
            let held = olds
                .get(&k)
                .and_then(|p| Some((p.clone(), update::file_of(self.project, p)?)));
            let part = match held {
                Some((p, stored)) => {
                    self.visit(c, Some(stored))?;
                    self.part_of.entry((c, r)).or_insert_with(|| p.clone());
                    p
                }
                None => {
                    self.visit(c, None)?;
                    self.part_of.get(&(c, r)).cloned().ok_or_else(|| {
                        Error::new(
                            Code::ImportExternalAmbiguous,
                            format!(
                                "{} makes nothing for product {:?}",
                                tree.nodes[c].source.display(),
                                &*node.opened.products.definitions[k].product_id
                            ),
                        )
                    })?
                }
            };
            external.insert(k, part);
        }
        let blake3 = node.file.blake3.clone();
        let stem_of = |path: &str| {
            Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let ids = self.ids.clone();
        let base = self.taken;
        let taken = move |s: &str| base(s) || ids.iter().any(|t| t == s);
        match old {
            Some(path) => {
                let from = self.project.imports.get(&path).map(|f| f.blake3.clone());
                if j != 0 && from.as_deref() == Some(blake3.as_str()) {
                    return Ok(());
                }
                let file_ref = FileRef {
                    path: path.clone(),
                    blake3,
                };
                let stem = stem_of(&path);
                let fresh = plan::Fresh {
                    opened: &node.opened,
                    file: &file_ref,
                    stem: &stem,
                    part: None,
                    taken: &taken,
                    pdm: self.pdm,
                    mode: self.mode,
                    cross: plan::Cross {
                        external,
                        ..Default::default()
                    },
                };
                let mut u = update::plan(self.project, &node.opened, &file_ref, &fresh)?;
                let planned = std::mem::take(&mut u.planned);
                self.record(j, &planned);
                self.groups.push(Group {
                    node: j,
                    path,
                    reused: false,
                    from,
                    planned,
                    update: Some(u),
                });
            }
            None => {
                let (path, reused) = self.store(j);
                let file_ref = FileRef {
                    path: path.clone(),
                    blake3,
                };
                let stem = stem_of(&path);
                let into = tree.roots_into(j);
                let fresh = plan::Fresh {
                    opened: &node.opened,
                    file: &file_ref,
                    stem: &stem,
                    part: if j == 0 { self.part.clone() } else { None },
                    taken: &taken,
                    pdm: self.pdm,
                    mode: self.mode,
                    cross: plan::Cross {
                        external,
                        scope: (j != 0).then(|| tree.reach(j, &into)),
                        bases: into
                            .iter()
                            .filter_map(|&r| Some((r, tree.base(j, r)?)))
                            .collect(),
                    },
                };
                let planned = if j == 0 {
                    fresh.plan()?
                } else {
                    fresh.make(&BTreeMap::new(), false, &[])?
                };
                self.record(j, &planned);
                self.groups.push(Group {
                    node: j,
                    path,
                    reused,
                    from: None,
                    planned,
                    update: None,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, location: Option<&str>) -> External {
        External {
            name: name.into(),
            location: location.map(str::to_owned),
            format: None,
            instance: 1,
            related: vec![1],
        }
    }

    use crate::import::tests::{external_assembly, folder, part_file};

    /// Resolves `name` under `dir` as an import of it would.
    fn tree(dir: &Path, name: &str, search: &[PathBuf]) -> crate::error::Result<Tree> {
        let source = dir.join(name);
        let file = ImportFile::new(std::fs::read(&source).unwrap());
        let opened = crate::import::opened(&file).unwrap();
        resolve(&source, file, opened, search, name)
    }

    #[test]
    fn the_committed_set_resolves_to_three_files() {
        let set = PathBuf::from(crate::import::tests::set_path(""));
        let t = tree(&set, "asm.stp", &[]).unwrap();
        assert_eq!(t.nodes.len(), 3);
        let names: Vec<Vec<String>> = t.nodes.iter().map(|n| n.documents.clone()).collect();
        assert_eq!(
            names,
            [
                vec![],
                vec!["box.stp".to_owned()],
                vec!["cylinder.stp".to_owned()]
            ]
        );
        assert!(t.nodes[2].source.ends_with("parts/cylinder.stp"));
        // Both placed products link to their files' one root.
        let o = &t.nodes[0].opened;
        let k = |id: &str| {
            o.products
                .definitions
                .iter()
                .position(|d| &*d.product_id == id)
                .unwrap()
        };
        assert_eq!(t.nodes[0].links.get(&k("BOX")).map(|l| l.0), Some(1));
        assert_eq!(t.nodes[0].links.get(&k("PIN")).map(|l| l.0), Some(2));
        // The documents' instances are interpreted.
        for i in [100, 103, 110, 116, 117] {
            assert!(t.nodes[0].interpreted.contains(&i), "#{i}");
        }
    }

    #[test]
    fn files_of_the_same_bytes_are_one_node_named_both_ways() {
        let pin = part_file("PIN", 2.0);
        let d = folder(&[
            (
                "asm.stp",
                external_assembly(
                    "ASM",
                    &[("A", "a.stp", [0.0; 3]), ("B", "b.stp", [20.0, 0.0, 0.0])],
                ),
            ),
            ("a.stp", pin.clone()),
            ("b.stp", pin),
        ]);
        let t = tree(d.path(), "asm.stp", &[]).unwrap();
        assert_eq!(t.nodes.len(), 2);
        assert_eq!(t.nodes[1].documents, ["a.stp", "b.stp"]);
    }

    #[test]
    fn a_nested_assembly_is_followed_from_its_own_folder() {
        let d = folder(&[
            (
                "top.stp",
                external_assembly("TOP", &[("SUB", "sub/sub.stp", [0.0; 3])]),
            ),
            (
                "sub/sub.stp",
                external_assembly("SUB", &[("PIN", "pin.stp", [5.0, 0.0, 0.0])]),
            ),
            ("sub/pin.stp", part_file("PIN", 2.0)),
        ]);
        let t = tree(d.path(), "top.stp", &[]).unwrap();
        assert_eq!(t.nodes.len(), 3);
        assert!(t.nodes[2].source.ends_with("sub/pin.stp"));
        assert_eq!(t.nodes[1].links.len(), 1, "the sub-assembly links its pin");
    }

    #[test]
    fn search_folders_are_looked_in_after_the_files_own() {
        let d = folder(&[
            (
                "a/asm.stp",
                external_assembly("ASM", &[("BOX", "box.stp", [0.0; 3])]),
            ),
            ("lib/box.stp", part_file("BOX", 10.0)),
        ]);
        let t = tree(&d.path().join("a"), "asm.stp", &[d.path().join("lib")]).unwrap();
        assert_eq!(t.nodes.len(), 2);
    }

    #[test]
    fn a_file_alone_is_a_tree_of_one() {
        let d = folder(&[("box.stp", part_file("BOX", 10.0))]);
        let t = tree(d.path(), "box.stp", &[]).unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert!(t.nodes[0].links.is_empty());
    }

    #[test]
    fn a_document_is_looked_for_by_location_name_then_file_name() {
        let f = Path::new("/a/asm");
        let s = [PathBuf::from("/s1"), PathBuf::from("/s2")];
        assert_eq!(
            candidates(&doc("sub\\x.stp", Some("parts/x.stp")), f, &s),
            [
                PathBuf::from("/a/asm/parts/x.stp"),
                PathBuf::from("/a/asm/sub/x.stp"),
                PathBuf::from("/a/asm/x.stp"),
                PathBuf::from("/s1/x.stp"),
                PathBuf::from("/s2/x.stp"),
            ]
        );
        // Absolute names from another machine: by file name only.
        assert_eq!(
            candidates(&doc("C:\\Users\\me\\Part1.STEP", None), f, &[]),
            [PathBuf::from("/a/asm/Part1.STEP")]
        );
        assert_eq!(
            candidates(&doc("/home/me/p.stp", None), f, &[]),
            [PathBuf::from("/a/asm/p.stp")]
        );
    }

    #[test]
    fn a_file_is_found_exactly_else_by_one_name_ignoring_case() {
        let d = crate::import::tests::folder(&[
            ("a/one.stp", b"1".to_vec()),
            ("s/Part1.step", b"2".to_vec()),
        ]);
        let a = d.path().join("a");
        let s = [d.path().join("s")];
        assert_eq!(
            locate(&doc("one.stp", None), &a, &s),
            Some(a.join("one.stp"))
        );
        assert_eq!(
            locate(&doc("PART1.STEP", None), &a, &s),
            Some(s[0].join("Part1.step"))
        );
        assert_eq!(locate(&doc("none.stp", None), &a, &s), None);
    }

    #[test]
    fn formats_other_than_step_are_not_read() {
        assert!(is_step(None));
        assert!(is_step(Some("")));
        assert!(is_step(Some("ISO 10303-21")));
        assert!(is_step(Some("STEP AP214")));
        assert!(!is_step(Some("CATIA V5")));
        assert!(!is_step(Some("SLDPRT")));
    }

    #[test]
    fn every_missing_document_is_named_at_once() {
        let d = folder(&[
            (
                "asm.stp",
                external_assembly(
                    "ASM",
                    &[
                        ("BOX", "box.stp", [0.0; 3]),
                        ("SUB", "sub.stp", [10.0, 0.0, 0.0]),
                        ("NUT", "nut.stp", [20.0, 0.0, 0.0]),
                    ],
                ),
            ),
            (
                "sub.stp",
                external_assembly("SUB", &[("PIN", "pin.stp", [0.0; 3])]),
            ),
        ]);
        let e = tree(d.path(), "asm.stp", &[d.path().join("lib")])
            .err()
            .unwrap();
        assert_eq!(e.code, Code::ImportExternalMissing);
        let m = e.details["missing"].as_array().unwrap();
        let files: Vec<(&str, &str)> = m
            .iter()
            .map(|x| (x["file"].as_str().unwrap(), x["reason"].as_str().unwrap()))
            .collect();
        assert_eq!(
            files,
            [
                ("box.stp", "not_found"),
                ("pin.stp", "not_found"),
                ("nut.stp", "not_found")
            ]
        );
        assert_eq!(m[0]["products"], json!(["BOX"]));
        assert_eq!(m[0]["referenced_by"], "asm.stp");
        assert!(m[1]["referenced_by"].as_str().unwrap().ends_with("sub.stp"));
        assert_eq!(
            m[0]["searched"].as_array().unwrap().len(),
            2,
            "its folder and lib"
        );
        for f in ["box.stp", "pin.stp", "nut.stp"] {
            assert!(e.message.contains(f), "{}", e.message);
        }
    }

    #[test]
    fn a_file_with_two_roots_needs_the_product_named() {
        let block = |x: f64| {
            ryukei::primitive::block(
                ryukei::name::OpKey(1),
                x,
                0.01,
                0.01,
                &ryukei::geom::Rigid::IDENTITY,
            )
            .unwrap()
            .0
        };
        let roots = crate::import::tests::written(vec![
            ryustep::Product {
                id: "A".into(),
                name: "A".into(),
                bodies: vec![block(0.01)],
                ..Default::default()
            },
            ryustep::Product {
                id: "B".into(),
                name: "B".into(),
                bodies: vec![block(0.02)],
                ..Default::default()
            },
        ]);
        let d = folder(&[
            (
                "x.stp",
                external_assembly("ASM", &[("X", "two.stp", [0.0; 3])]),
            ),
            (
                "b.stp",
                external_assembly("ASM", &[("B", "two.stp", [0.0; 3])]),
            ),
            ("two.stp", roots),
        ]);
        let e = tree(d.path(), "x.stp", &[]).err().unwrap();
        assert_eq!(e.code, Code::ImportExternalAmbiguous);
        assert_eq!(
            e.details["roots"],
            json!([{"id": "A", "name": "A"}, {"id": "B", "name": "B"}])
        );
        assert_eq!(e.details["product"]["id"], "X");
        // Named by id, it is that root.
        let t = tree(d.path(), "b.stp", &[]).unwrap();
        let (j, r) = *t.nodes[0].links.values().next().unwrap();
        assert_eq!(&*t.nodes[j].opened.products.definitions[r].product_id, "B");
    }

    #[test]
    fn files_naming_each_other_are_a_cycle() {
        let d = folder(&[
            ("a.stp", external_assembly("A", &[("B", "b.stp", [0.0; 3])])),
            ("b.stp", external_assembly("B", &[("A", "a.stp", [0.0; 3])])),
        ]);
        let e = tree(d.path(), "a.stp", &[]).err().unwrap();
        assert_eq!(e.code, Code::AssemblyCycle);
        assert_eq!(
            e.details["files"].as_array().unwrap().len(),
            3,
            "{}",
            e.details
        );
    }

    #[test]
    fn more_than_32_files_deep_is_refused() {
        let mut files: Vec<(String, Vec<u8>)> = (0..=MAX_DEPTH + 1)
            .map(|n| {
                (
                    format!("f{n}.stp"),
                    external_assembly(
                        &format!("A{n}"),
                        &[(&format!("A{}", n + 1), &format!("f{}.stp", n + 1), [0.0; 3])],
                    ),
                )
            })
            .collect();
        files.push((format!("f{}.stp", MAX_DEPTH + 2), part_file("END", 1.0)));
        let refs: Vec<(&str, Vec<u8>)> =
            files.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
        let d = folder(&refs);
        let e = tree(d.path(), "f0.stp", &[]).err().unwrap();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidParam, &json!("external"))
        );
    }

    #[test]
    fn a_document_of_another_format_is_missing_for_its_format() {
        let text = String::from_utf8(external_assembly("ASM", &[("BOX", "box.sldprt", [0.0; 3])]))
            .unwrap();
        // The document's format, as CAx-IF writes it.
        let text = text.replace(
            "ENDSEC;\nEND-ISO-10303-21;",
            "#900=REPRESENTATION_CONTEXT('','document parameters');\n\
             #901=PROPERTY_DEFINITION('document property','',#206);\n\
             #902=PROPERTY_DEFINITION_REPRESENTATION(#901,#903);\n\
             #903=REPRESENTATION('document format',(#904),#900);\n\
             #904=DESCRIPTIVE_REPRESENTATION_ITEM('data format','SLDPRT');\n\
             ENDSEC;\nEND-ISO-10303-21;",
        );
        let d = folder(&[
            ("asm.stp", text.into_bytes()),
            ("box.sldprt", b"binary".to_vec()),
        ]);
        let e = tree(d.path(), "asm.stp", &[]).err().unwrap();
        assert_eq!(e.code, Code::ImportExternalMissing);
        assert_eq!(e.details["missing"][0]["reason"], "format");
        assert_eq!(e.details["missing"][0]["format"], "SLDPRT");
    }

    #[test]
    fn the_set_plans_two_parts_then_the_assembly_placing_them() {
        let (_dir, s) = crate::testing::fresh();
        let set = PathBuf::from(crate::import::tests::set_path(""));
        let t = tree(&set, "asm.stp", &[]).unwrap();
        let taken = |_: &str| false;
        let mut p = Planner::new(s.project(), &t, None, false, None, &taken);
        p.visit(0, None).unwrap();
        let order: Vec<(usize, &str, Vec<&str>)> = p
            .groups
            .iter()
            .map(|g| {
                (
                    g.node,
                    g.path.as_str(),
                    g.planned.iter().map(|pl| pl.part.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                (1, "imports/box.step", vec!["box"]),
                (2, "imports/cylinder.step", vec!["pin"]),
                (0, "imports/asm.step", vec!["asm"]),
            ]
        );
        let all: Vec<Planned> = p.groups.iter().flat_map(|g| g.planned.clone()).collect();
        for path in [
            "imports/box.step",
            "imports/cylinder.step",
            "imports/asm.step",
        ] {
            assert!(names(&all, path), "{path}");
        }
    }

    #[test]
    fn merge_keeps_one_entry_per_part() {
        use crate::doc::files::PartKind;
        let pl = |part: &str, f: &str| Planned {
            part: Id::parse(part, "part").unwrap(),
            new: false,
            kind: PartKind::Part,
            definition: None,
            features: vec![(
                Id::parse(f, "feature").unwrap(),
                crate::doc::files::Feature::Import(crate::import::tests::import_of(f)),
            )],
            removed: Vec::new(),
            properties: Default::default(),
            props_of: None,
        };
        let m = merge(vec![pl("a", "x"), pl("b", "y"), pl("a", "z")]);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].features.len(), 2);
    }
}
