//! What an import writes, planned from a file's product tree before
//! anything is written: a part per product with bodies (an import feature
//! per body), an assembly per product with children (a fixed instance per
//! usage, where the file puts it), and for a product with both an
//! assembly placing a part of its own bodies at its origin. A product
//! several usages place is one part placed many times.
//!
//! The plan is a set of file writes ([`Writes`]): a dry run and the
//! preview of an update rebuild them in a session over the files as they
//! would be, and a real import writes them.

use super::{free, id_from};
use crate::doc::assembly::{ImportUsage, Instance, Pose, Rev};
use crate::doc::files::{Feature, PartFile, PartKind};
use crate::doc::id::Id;
use crate::doc::import::{FileRef, Import, ProductRef};
use crate::error::{Code, Error, Result};
use ryustep::Opened;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Files an import writes, by path relative to the project's root, with
/// `/`: their bytes, or `None` for a file it deletes.
pub type Writes = BTreeMap<String, Option<Vec<u8>>>;

/// One part an import makes or changes.
#[derive(Clone, Debug)]
pub struct Planned {
    pub part: Id,
    pub new: bool,
    pub kind: PartKind,
    /// The definition its features come from: whose bodies a part's
    /// import features read, or whose usages an assembly's instances are;
    /// `None` for the bodies no product reaches.
    pub definition: Option<usize>,
    /// The features it writes, new or rewritten, in the order new ones
    /// are listed.
    pub features: Vec<(Id, Feature)>,
    /// The features it deletes.
    pub removed: Vec<Id>,
    /// What its properties file gets, when the project uses PDM.
    pub properties: BTreeMap<String, Value>,
}

/// Definition `k`'s product as a feature names it: its id and name, and
/// its place among definitions with both equal.
pub fn product_of(opened: &Opened, k: usize) -> ProductRef {
    let defs = &opened.products.definitions;
    let d = &defs[k];
    let ordinal = defs[..k]
        .iter()
        .filter(|e| e.product_id == d.product_id && e.name == d.name)
        .count();
    ProductRef {
        id: d.product_id.to_string(),
        name: d.name.to_string(),
        ordinal: ordinal as u32,
    }
}

/// How many bodies definition `k` has (`None`: the bodies no product
/// reaches).
pub fn bodies(opened: &Opened, k: Option<usize>) -> usize {
    opened.held.iter().filter(|h| h.definition == k).count()
}

/// Whether each definition makes anything: it has bodies, or a child
/// that makes something. The tree has no cycles (ryustep drops them).
pub fn makes(opened: &Opened) -> Vec<bool> {
    let defs = &opened.products.definitions;
    let mut out: Vec<Option<bool>> = vec![None; defs.len()];
    fn visit(k: usize, opened: &Opened, out: &mut Vec<Option<bool>>) -> bool {
        if let Some(m) = out[k] {
            return m;
        }
        // Marked first, so a cycle ryustep missed can't recurse forever.
        out[k] = Some(false);
        let mut m = bodies(opened, Some(k)) > 0;
        for c in &opened.products.definitions[k].children {
            m |= visit(c.definition, opened, out);
        }
        out[k] = Some(m);
        m
    }
    (0..defs.len())
        .map(|k| visit(k, opened, &mut out))
        .collect()
}

/// Whether definition `k` is an assembly: it uses a child that makes
/// something.
pub fn is_assembly(opened: &Opened, makes: &[bool], k: usize) -> bool {
    opened.products.definitions[k]
        .children
        .iter()
        .any(|c| makes[c.definition])
}

/// The pose usage `n` of definition `k` puts its child at, as an
/// instance writes it; `import_failed` naming the usage when the file's
/// placement can't be read.
pub fn pose(opened: &Opened, path: &str, k: usize, n: usize) -> Result<Pose> {
    let o = &opened.products.definitions[k].children[n];
    let r = opened.placement(k, n).map_err(|e| {
        Error::new(
            Code::ImportFailed,
            format!(
                "{path}: usage #{} ({:?}) can't be placed: {e}",
                o.instance, o.id
            ),
        )
        .with("file", path)
        .with("reason", "placement")
        .with("step", vec![format!("#{}", o.instance)])
    })?;
    Ok(crate::assembly::rigid_pose(&r).snapped())
}

/// Usage `n` of definition `k`'s ordinal: how many usages before it in
/// `k` have its id and its child.
pub fn usage_ordinal(opened: &Opened, k: usize, n: usize) -> u32 {
    let children = &opened.products.definitions[k].children;
    let o = &children[n];
    children[..n]
        .iter()
        .filter(|c| c.id == o.id && c.definition == o.definition)
        .count() as u32
}

/// Import features for `bodies` of the bodies `product` has, from
/// `first`, each the first free of `body`, `body-2`, … past `taken`.
pub fn import_features(
    file: &FileRef,
    product: Option<ProductRef>,
    bodies: std::ops::Range<usize>,
    taken: &mut Vec<String>,
) -> Vec<(Id, Feature)> {
    bodies
        .map(|body| {
            let base = Id::parse("body", "feature").expect("an ID");
            let fid = free(&base, |s| taken.iter().any(|t| t == s));
            taken.push(fid.to_string());
            (
                fid,
                Feature::Import(Import {
                    file: file.clone(),
                    product: product.clone(),
                    body: body as u32,
                }),
            )
        })
        .collect()
}

/// An instance of `part` an import makes: fixed, at `pose`, as `usage`.
pub fn instance(part: &Id, pose: Pose, usage: ImportUsage) -> Feature {
    Feature::Instance(Instance {
        part: part.clone(),
        rev: Rev::Working,
        pose,
        fixed: true,
        import: Some(usage),
    })
}

/// The instance ID for usage `n` of definition `k`: its id made an ID,
/// else the child's part's, the first free past `taken`.
pub fn instance_id(opened: &Opened, k: usize, n: usize, child: &Id, taken: &[String]) -> Id {
    let o = &opened.products.definitions[k].children[n];
    let base = id_from(&o.id).unwrap_or_else(|| child.clone());
    free(&base, |s| taken.iter().any(|t| t == s))
}

/// What a fresh import of a whole file makes.
pub struct Fresh<'a> {
    pub opened: &'a Opened,
    pub file: &'a FileRef,
    /// The stored file's stem, for parts no product names.
    pub stem: &'a str,
    /// The part `--part` names: the file's one root.
    pub part: Option<Id>,
    /// Whether an ID is taken: by a part, or retired by a release record.
    pub taken: &'a dyn Fn(&str) -> bool,
    /// Whether the project uses PDM: new parts get names and descriptions.
    pub pdm: bool,
}

impl Fresh<'_> {
    /// The parts and assemblies the file makes, each product in
    /// definition order (an assembly before the part of its own bodies),
    /// then the bodies no product reaches.
    pub fn plan(&self) -> Result<Vec<Planned>> {
        let opened = self.opened;
        let makes = makes(opened);
        let unplaced = bodies(opened, None) > 0;
        let mut roots: Vec<Option<usize>> = opened
            .products
            .roots
            .iter()
            .copied()
            .filter(|&k| makes[k])
            .map(Some)
            .collect();
        if unplaced {
            roots.push(None);
        }
        if roots.is_empty() {
            return Err(Error::new(
                Code::ImportBodyMissing,
                format!("{} holds no solid to import", self.file.path),
            )
            .with("path", &self.file.path));
        }
        if self.part.is_some() && roots.len() != 1 {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "{} has {} roots, each a part or assembly: part names one, so leave it out",
                    self.file.path,
                    roots.len()
                ),
            )
            .with("param", "part"));
        }
        self.make(&BTreeMap::new(), unplaced, &roots)
    }

    /// The parts and assemblies of every definition that makes something
    /// but those `known` gives parts already (their parts are only placed),
    /// and with `unplaced` one of the bodies no product reaches. `roots`
    /// are the file's, for `--part`.
    pub fn make(
        &self,
        known: &BTreeMap<usize, Id>,
        unplaced: bool,
        roots: &[Option<usize>],
    ) -> Result<Vec<Planned>> {
        let opened = self.opened;
        let defs = &opened.products.definitions;
        let makes = makes(opened);
        // IDs first: an assembly's instances name its children's parts.
        let mut taken: Vec<String> = Vec::new();
        let mut take = |base: Id, named: bool| -> Id {
            let id = if named {
                base
            } else {
                free(&base, |s| taken.iter().any(|t| t == s) || (self.taken)(s))
            };
            taken.push(id.to_string());
            id
        };
        let named = |k: Option<usize>| {
            self.part
                .clone()
                .filter(|_| roots.len() == 1 && roots[0] == k)
        };
        let fallback =
            || id_from(self.stem).unwrap_or_else(|| Id::parse("part", "part").expect("an ID"));
        let mut part_of: Vec<Option<Id>> = vec![None; defs.len()];
        let mut own_of: Vec<Option<Id>> = vec![None; defs.len()];
        for k in (0..defs.len()).filter(|&k| makes[k]) {
            if let Some(id) = known.get(&k) {
                part_of[k] = Some(id.clone());
                continue;
            }
            let id = match named(Some(k)) {
                Some(p) => take(p, true),
                None => {
                    let base = id_from(&defs[k].product_id)
                        .or_else(|| id_from(&defs[k].name))
                        .unwrap_or_else(fallback);
                    take(base, false)
                }
            };
            if is_assembly(opened, &makes, k) && bodies(opened, Some(k)) > 0 {
                let base = Id::parse(&format!("{id}-body"), "part")
                    .unwrap_or_else(|_| Id::parse("body", "part").expect("an ID"));
                own_of[k] = Some(take(base, false));
            }
            part_of[k] = Some(id);
        }
        let unplaced_part = unplaced.then(|| match named(None) {
            Some(p) => take(p, true),
            None => take(fallback(), false),
        });
        let mut out = Vec::new();
        for k in (0..defs.len()).filter(|&k| makes[k] && !known.contains_key(&k)) {
            let id = part_of[k].clone().expect("named");
            let product = product_of(opened, k);
            if is_assembly(opened, &makes, k) {
                let mut features = Vec::new();
                let mut ids: Vec<String> = Vec::new();
                if let Some(own) = &own_of[k] {
                    let iid = free(own, |s| ids.iter().any(|t| t == s));
                    ids.push(iid.to_string());
                    let usage = ImportUsage {
                        file: self.file.path.clone(),
                        product: product.clone(),
                        usage: None,
                        ordinal: 0,
                    };
                    features.push((iid, instance(own, Pose::IDENTITY, usage)));
                }
                for (n, o) in defs[k].children.iter().enumerate() {
                    let Some(child) = &part_of[o.definition] else {
                        continue;
                    };
                    let iid = instance_id(opened, k, n, child, &ids);
                    ids.push(iid.to_string());
                    let usage = ImportUsage {
                        file: self.file.path.clone(),
                        product: product.clone(),
                        usage: Some(o.id.to_string()),
                        ordinal: usage_ordinal(opened, k, n),
                    };
                    let pose = pose(opened, &self.file.path, k, n)?;
                    features.push((iid, instance(child, pose, usage)));
                }
                out.push(Planned {
                    part: id,
                    new: true,
                    kind: PartKind::Assembly,
                    definition: Some(k),
                    features,
                    removed: Vec::new(),
                    properties: self.properties(Some(k)),
                });
                if let Some(own) = &own_of[k] {
                    let n = bodies(opened, Some(k));
                    out.push(Planned {
                        part: own.clone(),
                        new: true,
                        kind: PartKind::Part,
                        definition: Some(k),
                        features: import_features(self.file, Some(product), 0..n, &mut Vec::new()),
                        removed: Vec::new(),
                        properties: BTreeMap::new(),
                    });
                }
            } else {
                let n = bodies(opened, Some(k));
                out.push(Planned {
                    part: id,
                    new: true,
                    kind: PartKind::Part,
                    definition: Some(k),
                    features: import_features(self.file, Some(product), 0..n, &mut Vec::new()),
                    removed: Vec::new(),
                    properties: self.properties(Some(k)),
                });
            }
        }
        if let Some(id) = unplaced_part {
            let n = bodies(opened, None);
            out.push(Planned {
                part: id,
                new: true,
                kind: PartKind::Part,
                definition: None,
                features: import_features(self.file, None, 0..n, &mut Vec::new()),
                removed: Vec::new(),
                properties: BTreeMap::new(),
            });
        }
        Ok(out)
    }

    /// What definition `k`'s part's properties file gets: the product's
    /// name and description, where the project uses PDM.
    pub fn properties(&self, k: Option<usize>) -> BTreeMap<String, Value> {
        properties(self.opened, k, self.pdm)
    }
}

/// The product's name and description where non-empty, when `pdm`.
pub fn properties(opened: &Opened, k: Option<usize>, pdm: bool) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if let (true, Some(k)) = (pdm, k) {
        let d = &opened.products.definitions[k];
        for (key, v) in [("name", &d.name), ("description", &d.description)] {
            if !v.trim().is_empty() {
                out.insert(key.to_owned(), json!(&**v));
            }
        }
    }
    out
}

/// The files `planned` writes into the project at `root`, whose parts
/// are `project`'s.
pub fn writes(project: &crate::doc::project::Project, planned: &[Planned]) -> Result<Writes> {
    use crate::doc::canonical;
    use crate::doc::project::{feature_path, part_path, properties_path, relative};
    let root = &project.root;
    let rel = |p: std::path::PathBuf| relative(root, &p);
    let mut out = Writes::new();
    for pl in planned {
        let mut order = match project.parts.get(&pl.part) {
            Some(p) if !pl.new => crate::server::order(p)?,
            _ => Vec::new(),
        };
        order.retain(|f| !pl.removed.contains(f));
        for fid in &pl.removed {
            out.insert(rel(feature_path(root, &pl.part, fid)), None);
        }
        for (fid, f) in &pl.features {
            if !order.contains(fid) {
                order.push(fid.clone());
            }
            let bytes = canonical::to_string(&f.to_json()).into_bytes();
            out.insert(rel(feature_path(root, &pl.part, fid)), Some(bytes));
        }
        // Instances an import adds or removes give and take item numbers as
        // any write does.
        let current = project.parts.get(&pl.part).filter(|_| !pl.new);
        let before = current.map(ryupdm::items::placed).unwrap_or_default();
        let feature = |fid: &Id| match pl.features.iter().find(|(w, _)| w == fid) {
            Some((_, f)) => Some(f),
            None => current?.features.get(fid)?.as_ref().ok(),
        };
        let after = ryupdm::items::placed_by(&order, feature);
        let whole = order.iter().all(|f| feature(f).is_some());
        let items = current
            .and_then(|p| p.file.as_ref().ok())
            .map(|f| f.items.clone())
            .unwrap_or_default();
        let list = PartFile {
            kind: pl.kind,
            features: order,
            items: ryupdm::items::assigned(&items, &before, &after, whole),
        };
        let bytes = canonical::to_string(&list.to_json()).into_bytes();
        out.insert(rel(part_path(root, &pl.part)), Some(bytes));
        if !pl.properties.is_empty() {
            let props = ryupdm::props::Properties {
                values: pl.properties.clone(),
            };
            let bytes = canonical::to_string(&props.to_json()).into_bytes();
            out.insert(rel(properties_path(root, &pl.part)), Some(bytes));
        }
    }
    Ok(out)
}

/// Writes `writes` into the project at `root`: each file atomically,
/// side by side (each waits for its disk, and a file of a thousand parts
/// writes thousands), then each deletion with the folders it empties.
/// The first error, in path order, is the answer's.
pub fn apply(root: &std::path::Path, writes: &Writes) -> Result<()> {
    use crate::doc::project::{remove_file, write_atomic};
    use rayon::prelude::*;
    let path = |rel: &str| rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    let written: Vec<Result<()>> = writes
        .par_iter()
        .filter_map(|(rel, bytes)| Some(write_atomic(&path(rel), bytes.as_ref()?)))
        .collect();
    written.into_iter().collect::<Result<()>>()?;
    for (rel, _) in writes.iter().filter(|(_, b)| b.is_none()) {
        let p = path(rel);
        if p.exists() {
            remove_file(root, &p)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_written_and_deletions_empty_their_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("parts/old/features")).unwrap();
        std::fs::write(root.join("parts/old/features/a.json"), "{}").unwrap();
        let mut writes = Writes::new();
        for k in 0..200 {
            let path = format!("parts/p{k}/features/body.json");
            writes.insert(path, Some(format!("{{\"k\": {k}}}").into_bytes()));
        }
        writes.insert("parts/old/features/a.json".into(), None);
        writes.insert("parts/gone/part.json".into(), None);
        apply(root, &writes).unwrap();
        for k in 0..200 {
            let text = std::fs::read_to_string(root.join(format!("parts/p{k}/features/body.json")));
            assert_eq!(text.unwrap(), format!("{{\"k\": {k}}}"));
        }
        assert!(!root.join("parts/old").exists(), "emptied, so gone");
    }
}
