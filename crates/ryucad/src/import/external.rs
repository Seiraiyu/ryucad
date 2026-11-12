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

use crate::doc::project::ImportFile;
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

/// The file a document is: the first of [`candidates`] that is a file,
/// else, in `folder` and then each of `search`, the one file whose name
/// matches a file name of it ignoring ASCII case.
pub fn locate(x: &External, folder: &Path, search: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = candidates(x, folder, search).into_iter().find(|p| p.is_file()) {
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
            format!("files reference each other in a loop: {}", files.join(" > ")),
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
        assert_eq!(names, [vec![], vec!["box.stp".to_owned()], vec!["cylinder.stp".to_owned()]]);
        assert!(t.nodes[2].source.ends_with("parts/cylinder.stp"));
        // Both placed products link to their files' one root.
        let o = &t.nodes[0].opened;
        let k = |id: &str| o.products.definitions.iter().position(|d| &*d.product_id == id).unwrap();
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
            ("asm.stp", external_assembly("ASM", &[("A", "a.stp", [0.0; 3]), ("B", "b.stp", [20.0, 0.0, 0.0])])),
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
            ("top.stp", external_assembly("TOP", &[("SUB", "sub/sub.stp", [0.0; 3])])),
            ("sub/sub.stp", external_assembly("SUB", &[("PIN", "pin.stp", [5.0, 0.0, 0.0])])),
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
            ("a/asm.stp", external_assembly("ASM", &[("BOX", "box.stp", [0.0; 3])])),
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
        assert_eq!(locate(&doc("one.stp", None), &a, &s), Some(a.join("one.stp")));
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
            ("asm.stp", external_assembly("ASM", &[
                ("BOX", "box.stp", [0.0; 3]),
                ("SUB", "sub.stp", [10.0, 0.0, 0.0]),
                ("NUT", "nut.stp", [20.0, 0.0, 0.0]),
            ])),
            ("sub.stp", external_assembly("SUB", &[("PIN", "pin.stp", [0.0; 3])])),
        ]);
        let e = tree(d.path(), "asm.stp", &[d.path().join("lib")]).err().unwrap();
        assert_eq!(e.code, Code::ImportExternalMissing);
        let m = e.details["missing"].as_array().unwrap();
        let files: Vec<(&str, &str)> = m
            .iter()
            .map(|x| (x["file"].as_str().unwrap(), x["reason"].as_str().unwrap()))
            .collect();
        assert_eq!(files, [("box.stp", "not_found"), ("pin.stp", "not_found"), ("nut.stp", "not_found")]);
        assert_eq!(m[0]["products"], json!(["BOX"]));
        assert_eq!(m[0]["referenced_by"], "asm.stp");
        assert!(m[1]["referenced_by"].as_str().unwrap().ends_with("sub.stp"));
        assert_eq!(m[0]["searched"].as_array().unwrap().len(), 2, "its folder and lib");
        for f in ["box.stp", "pin.stp", "nut.stp"] {
            assert!(e.message.contains(f), "{}", e.message);
        }
    }

    #[test]
    fn a_file_with_two_roots_needs_the_product_named() {
        let block = |x: f64| {
            ryukei::primitive::block(ryukei::name::OpKey(1), x, 0.01, 0.01, &ryukei::geom::Rigid::IDENTITY)
                .unwrap()
                .0
        };
        let roots = crate::import::tests::written(vec![
            ryustep::Product { id: "A".into(), name: "A".into(), bodies: vec![block(0.01)], ..Default::default() },
            ryustep::Product { id: "B".into(), name: "B".into(), bodies: vec![block(0.02)], ..Default::default() },
        ]);
        let d = folder(&[
            ("x.stp", external_assembly("ASM", &[("X", "two.stp", [0.0; 3])])),
            ("b.stp", external_assembly("ASM", &[("B", "two.stp", [0.0; 3])])),
            ("two.stp", roots),
        ]);
        let e = tree(d.path(), "x.stp", &[]).err().unwrap();
        assert_eq!(e.code, Code::ImportExternalAmbiguous);
        assert_eq!(e.details["roots"], json!([{"id": "A", "name": "A"}, {"id": "B", "name": "B"}]));
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
        assert_eq!(e.details["files"].as_array().unwrap().len(), 3, "{}", e.details);
    }

    #[test]
    fn more_than_32_files_deep_is_refused() {
        let mut files: Vec<(String, Vec<u8>)> = (0..=MAX_DEPTH + 1)
            .map(|n| {
                (
                    format!("f{n}.stp"),
                    external_assembly(&format!("A{n}"), &[(&format!("A{}", n + 1), &format!("f{}.stp", n + 1), [0.0; 3])]),
                )
            })
            .collect();
        files.push((format!("f{}.stp", MAX_DEPTH + 2), part_file("END", 1.0)));
        let refs: Vec<(&str, Vec<u8>)> = files.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
        let d = folder(&refs);
        let e = tree(d.path(), "f0.stp", &[]).err().unwrap();
        assert_eq!((e.code, &e.details["param"]), (Code::InvalidParam, &json!("external")));
    }

    #[test]
    fn a_document_of_another_format_is_missing_for_its_format() {
        let text = String::from_utf8(external_assembly("ASM", &[("BOX", "box.sldprt", [0.0; 3])])).unwrap();
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
        let d = folder(&[("asm.stp", text.into_bytes()), ("box.sldprt", b"binary".to_vec())]);
        let e = tree(d.path(), "asm.stp", &[]).err().unwrap();
        assert_eq!(e.code, Code::ImportExternalMissing);
        assert_eq!(e.details["missing"][0]["reason"], "format");
        assert_eq!(e.details["missing"][0]["format"], "SLDPRT");
    }
}
