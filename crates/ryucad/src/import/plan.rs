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

use super::{body_missing, free, id_from};
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
    /// The definition whose product its properties come from (design
    /// 448, plan 24.C): a plain part's, an imported assembly's own, a
    /// flattened part's single root; `None` for the part of an assembly's
    /// own bodies and the bodies no product reaches.
    pub props_of: Option<usize>,
}

/// What one file's plan takes from the other files of an import (design
/// 448, plan 24.D): empty for a file alone.
#[derive(Clone, Debug, Default)]
pub struct Cross {
    /// Externally defined definitions with the parts their files made:
    /// placed by their usages like any child, made by nothing here.
    pub external: BTreeMap<usize, Id>,
    /// The definitions to make parts of, for a file another file's
    /// documents name: those its referenced roots reach. Every one when
    /// `None`.
    pub scope: Option<std::collections::BTreeSet<usize>>,
    /// The ID a definition's part starts from, before its product's id
    /// or name: the referencing product's.
    pub bases: BTreeMap<usize, Id>,
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

/// Whether each definition makes anything: it has bodies, its shape is
/// in another file (`external`: [`Cross::external`]'s), or it has a
/// child that makes something. The tree has no cycles (ryustep drops
/// them).
pub fn makes_with(opened: &Opened, external: &std::collections::BTreeSet<usize>) -> Vec<bool> {
    let defs = &opened.products.definitions;
    let mut out: Vec<Option<bool>> = vec![None; defs.len()];
    fn visit(
        k: usize,
        opened: &Opened,
        external: &std::collections::BTreeSet<usize>,
        out: &mut Vec<Option<bool>>,
    ) -> bool {
        if let Some(m) = out[k] {
            return m;
        }
        // Marked first, so a cycle ryustep missed can't recurse forever.
        out[k] = Some(false);
        let mut m = bodies(opened, Some(k)) > 0 || external.contains(&k);
        for c in &opened.products.definitions[k].children {
            m |= visit(c.definition, opened, external, out);
        }
        out[k] = Some(m);
        m
    }
    (0..defs.len())
        .map(|k| visit(k, opened, external, &mut out))
        .collect()
}

/// [`makes_with`] for a file alone.
pub fn makes(opened: &Opened) -> Vec<bool> {
    makes_with(opened, &std::collections::BTreeSet::new())
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

/// The name the file gives each body of definition `k` (`None`: the
/// bodies no product reaches), in body order: empty for none.
pub fn body_names(opened: &Opened, k: Option<usize>) -> Vec<String> {
    opened
        .held
        .iter()
        .filter(|h| h.definition == k)
        .map(|h| h.name.clone())
        .collect()
}

/// Import features for `bodies` of the bodies `product` has, each the
/// first free past `taken` of its name in the file where that is an ID
/// (ryucad writes each solid's ID there, so a part exported and
/// imported again keeps its solids' IDs, #412), else of `body`:
/// `body`, `body-2`, ….
pub fn import_features(
    file: &FileRef,
    product: Option<ProductRef>,
    mode: Option<crate::doc::import::Mode>,
    names: &[String],
    bodies: std::ops::Range<usize>,
    taken: &mut Vec<String>,
) -> Vec<(Id, Feature)> {
    bodies
        .map(|body| {
            let base = names
                .get(body)
                .and_then(|n| Id::parse(n, "feature").ok())
                .unwrap_or_else(|| Id::parse("body", "feature").expect("an ID"));
            let fid = free(&base, |s| taken.iter().any(|t| t == s));
            taken.push(fid.to_string());
            (
                fid,
                Feature::Import(Import {
                    file: file.clone(),
                    product: product.clone(),
                    body: body as u32,
                    mode,
                    place: None,
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
    /// The mode new import features take (none: a solid).
    pub mode: Option<crate::doc::import::Mode>,
    /// What the import's other files give this one (design 448, 24.D).
    pub cross: Cross,
}

impl Fresh<'_> {
    /// [`makes_with`] for this file and the definitions [`Cross::external`]
    /// gives parts.
    pub fn makes(&self) -> Vec<bool> {
        makes_with(self.opened, &self.cross.external.keys().copied().collect())
    }

    /// The parts and assemblies the file makes, each product in
    /// definition order (an assembly before the part of its own bodies),
    /// then the bodies no product reaches.
    pub fn plan(&self) -> Result<Vec<Planned>> {
        let opened = self.opened;
        let makes = self.makes();
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
            return Err(body_missing(opened, &self.file.path));
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
        let makes = self.makes();
        // Definitions whose parts come from other files are placed, never made.
        let mut known = known.clone();
        for (k, p) in &self.cross.external {
            known.entry(*k).or_insert_with(|| p.clone());
        }
        let known = &known;
        let in_scope = |k: usize| self.cross.scope.as_ref().is_none_or(|s| s.contains(&k));
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
        for k in (0..defs.len()).filter(|&k| makes[k] && in_scope(k)) {
            if let Some(id) = known.get(&k) {
                part_of[k] = Some(id.clone());
                continue;
            }
            let id = match named(Some(k)) {
                Some(p) => take(p, true),
                None => {
                    let base = self
                        .cross
                        .bases
                        .get(&k)
                        .cloned()
                        .or_else(|| id_from(&defs[k].product_id))
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
        for k in (0..defs.len()).filter(|&k| makes[k] && in_scope(k) && !known.contains_key(&k)) {
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
                    props_of: Some(k),
                });
                if let Some(own) = &own_of[k] {
                    let n = bodies(opened, Some(k));
                    let names = body_names(opened, Some(k));
                    out.push(Planned {
                        part: own.clone(),
                        new: true,
                        kind: PartKind::Part,
                        definition: Some(k),
                        features: import_features(
                            self.file,
                            Some(product),
                            self.mode,
                            &names,
                            0..n,
                            &mut Vec::new(),
                        ),
                        removed: Vec::new(),
                        properties: BTreeMap::new(),
                        props_of: None,
                    });
                }
            } else {
                let n = bodies(opened, Some(k));
                let names = body_names(opened, Some(k));
                out.push(Planned {
                    part: id,
                    new: true,
                    kind: PartKind::Part,
                    definition: Some(k),
                    features: import_features(
                        self.file,
                        Some(product),
                        self.mode,
                        &names,
                        0..n,
                        &mut Vec::new(),
                    ),
                    removed: Vec::new(),
                    properties: self.properties(Some(k)),
                    props_of: Some(k),
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
                features: import_features(
                    self.file,
                    None,
                    self.mode,
                    &body_names(opened, None),
                    0..n,
                    &mut Vec::new(),
                ),
                removed: Vec::new(),
                properties: BTreeMap::new(),
                props_of: None,
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

/// Bodies a flattened import makes at most.
pub const MAX_FLAT: usize = 10_000;

/// One body of a flattened walk: definition `definition`'s (`None`: the
/// bodies no product reaches) body `body`, placed by `place` (none for a
/// root's own bodies, at the part's origin); `steps` are the definitions
/// each of `place`'s usages places, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Walked {
    /// The file the body is in: an index into [`Files`]; 0 for a file
    /// alone.
    pub file: usize,
    pub definition: Option<usize>,
    pub body: usize,
    pub place: Option<crate::doc::import::Place>,
    pub steps: Vec<usize>,
}

/// The files a flattened walk crosses (design 448, plan 24.D): each
/// file's opened bytes and stored path, and where its externally
/// defined definitions' shapes are (`external::Node::links`). The first
/// is the file imported.
pub struct Files<'a> {
    pub opened: Vec<&'a Opened>,
    pub paths: Vec<&'a str>,
    pub links: Vec<&'a BTreeMap<usize, (usize, usize)>>,
}

static NO_LINKS: BTreeMap<usize, (usize, usize)> = BTreeMap::new();

/// [`walk_files`] of a file alone.
pub fn walk(opened: &Opened, path: &str) -> Result<Vec<Walked>> {
    walk_files(&Files {
        opened: vec![opened],
        paths: vec![path],
        links: vec![&NO_LINKS],
    })
}

/// Every body of every occurrence of every product of the first file,
/// placed where its assemblies put it (#499): roots in the file's order,
/// at each definition its own bodies, then its usages in order, depth
/// first; a definition whose shape is another file's walked there, at
/// its usage's place (24.D); the first file's bodies no product reaches
/// last. More than [`MAX_FLAT`] is `invalid_param`; a placement that
/// can't be read `import_failed`.
pub fn walk_files(files: &Files) -> Result<Vec<Walked>> {
    use crate::doc::import::{Place, Via};
    use ryukei::geom::Rigid;
    let path = files.paths[0];
    let makes: Vec<Vec<bool>> = files
        .opened
        .iter()
        .zip(&files.links)
        .map(|(o, l)| makes_with(o, &l.keys().copied().collect()))
        .collect();
    let too_many = || {
        Error::new(
            Code::InvalidParam,
            format!(
                "{path} flattens to more than {MAX_FLAT} bodies: import it whole, or a product of it with --into"
            ),
        )
        .with("param", "flatten")
    };
    let mut out = Vec::new();
    // (file, definition, its placement in the root's frame, the usages
    // to it, the definitions they place)
    type Item = (usize, usize, Rigid, Vec<Via>, Vec<usize>);
    let mut stack: Vec<Item> = files.opened[0]
        .products
        .roots
        .iter()
        .rev()
        .copied()
        .filter(|&k| makes[0][k])
        .map(|k| (0, k, Rigid::IDENTITY, Vec::new(), Vec::new()))
        .collect();
    while let Some((f, k, world, via, steps)) = stack.pop() {
        // A definition whose shape is another file's: walked there.
        if let Some(&(j, r)) = files.links[f].get(&k) {
            stack.push((j, r, world, via, steps));
            continue;
        }
        let opened = files.opened[f];
        let defs = &opened.products.definitions;
        let place = (!via.is_empty()).then(|| Place {
            pose: crate::assembly::rigid_pose(&world).snapped(),
            via: via.clone(),
        });
        for body in 0..bodies(opened, Some(k)) {
            out.push(Walked {
                file: f,
                definition: Some(k),
                body,
                place: place.clone(),
                steps: steps.clone(),
            });
            if out.len() > MAX_FLAT {
                return Err(too_many());
            }
        }
        // Children pushed last first, so the first usage pops first.
        for (n, c) in defs[k].children.iter().enumerate().rev() {
            if !makes[f][c.definition] {
                continue;
            }
            let r = crate::assembly::pose_rigid(&pose(opened, files.paths[f], k, n)?)?;
            let mut v = via.clone();
            v.push(Via {
                usage: c.id.to_string(),
                product: product_of(opened, c.definition),
                ordinal: usage_ordinal(opened, k, n),
            });
            let mut d = steps.clone();
            d.push(c.definition);
            stack.push((f, c.definition, world.then_after(&r), v, d));
        }
    }
    for body in 0..bodies(files.opened[0], None) {
        out.push(Walked {
            file: 0,
            definition: None,
            body,
            place: None,
            steps: Vec::new(),
        });
        if out.len() > MAX_FLAT {
            return Err(too_many());
        }
    }
    Ok(out)
}

/// What `import --flatten` makes of a whole file: one part of an import
/// feature per body [`walk`] gives, each placed where the file's
/// assemblies put it (#499).
pub struct Flat<'a> {
    pub opened: &'a Opened,
    pub file: &'a FileRef,
    /// The stored file's stem, for a part no product names.
    pub stem: &'a str,
    /// The part `--part` names.
    pub part: Option<Id>,
    /// Whether an ID is taken: by a part, or retired by a release record.
    pub taken: &'a dyn Fn(&str) -> bool,
    /// Whether the project uses PDM: the part gets its one root's name.
    pub pdm: bool,
    /// The mode its import features take (none: a solid).
    pub mode: Option<crate::doc::import::Mode>,
    /// The files the walk crosses and each one's stored reference (design
    /// 448, 24.D); `None` for a file alone.
    pub cross: Option<(&'a Files<'a>, &'a [FileRef])>,
}

impl Flat<'_> {
    pub fn plan(&self) -> Result<Planned> {
        let opened = self.opened;
        let defs = &opened.products.definitions;
        let makes = match self.cross {
            Some((f, _)) => makes_with(opened, &f.links[0].keys().copied().collect()),
            None => makes(opened),
        };
        let roots: Vec<usize> = opened
            .products
            .roots
            .iter()
            .copied()
            .filter(|&k| makes[k])
            .collect();
        let walked = match self.cross {
            Some((f, _)) => walk_files(f)?,
            None => walk(opened, &self.file.path)?,
        };
        if walked.is_empty() {
            return Err(body_missing(opened, &self.file.path));
        }
        let part = match &self.part {
            Some(p) => p.clone(),
            None => {
                let base = roots
                    .first()
                    .and_then(|&k| id_from(&defs[k].product_id).or_else(|| id_from(&defs[k].name)))
                    .or_else(|| id_from(self.stem))
                    .unwrap_or_else(|| Id::parse("part", "part").expect("an ID"));
                free(&base, self.taken)
            }
        };
        let mut taken: Vec<String> = Vec::new();
        let features = walked
            .iter()
            .map(|w| match self.cross {
                Some((f, refs)) => flat_feature(f.opened[w.file], &refs[w.file], self.mode, w, &mut taken),
                None => flat_feature(opened, self.file, self.mode, w, &mut taken),
            })
            .collect();
        let properties = match roots.as_slice() {
            [k] => properties(opened, Some(*k), self.pdm),
            _ => BTreeMap::new(),
        };
        let props_of = match roots.as_slice() {
            [k] => Some(*k),
            _ => None,
        };
        Ok(Planned {
            part,
            new: true,
            kind: PartKind::Part,
            definition: roots.first().copied(),
            features,
            removed: Vec::new(),
            properties,
            props_of,
        })
    }
}

/// The import feature of a walked body, its ID the first free past
/// `taken` as [`import_features`] names one.
pub fn flat_feature(
    opened: &Opened,
    file: &FileRef,
    mode: Option<crate::doc::import::Mode>,
    w: &Walked,
    taken: &mut Vec<String>,
) -> (Id, Feature) {
    let product = w.definition.map(|k| product_of(opened, k));
    let names = body_names(opened, w.definition);
    let mut f = import_features(file, product, mode, &names, w.body..w.body + 1, taken);
    let (fid, mut feature) = f.pop().expect("one feature");
    if let Feature::Import(i) = &mut feature {
        i.place = w.place.clone();
    }
    (fid, feature)
}

#[cfg(test)]
mod flat_tests {
    use super::*;

    #[test]
    fn a_frame_file_flattens_to_one_part_placed_as_its_usages_put_it() {
        let bytes = crate::import::tests::frame_file();
        let file = crate::doc::project::ImportFile::new(bytes);
        let opened = crate::import::opened(&file).unwrap();
        let fr = FileRef {
            path: "imports/frame.step".into(),
            blake3: file.blake3.clone(),
        };
        let taken = |_: &str| false;
        let mode = Some(crate::doc::import::Mode::Reference);
        let planned = Flat {
            opened: &opened,
            file: &fr,
            stem: "frame",
            part: None,
            taken: &taken,
            pdm: false,
            mode,
            cross: None,
        }
        .plan()
        .unwrap();
        assert_eq!(planned.kind, PartKind::Part);
        assert_eq!(planned.features.len(), count_bodies(&opened));
        let mut ids: Vec<String> = planned
            .features
            .iter()
            .map(|(i, _)| i.to_string())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), planned.features.len(), "IDs are unique");
        for (_, f) in &planned.features {
            let Feature::Import(i) = f else { panic!() };
            assert_eq!(i.mode, mode);
            if let Some(p) = &i.place {
                assert!(!p.via.is_empty());
            }
        }
        // A placed body's pose is its usages' placements composed, as the
        // whole import's instances carry them, from the root down.
        let fresh = Fresh {
            opened: &opened,
            file: &fr,
            stem: "frame",
            part: None,
            taken: &taken,
            pdm: false,
            mode: None,
            cross: Cross::default(),
        }
        .plan()
        .unwrap();
        // Each part the whole import makes, by the definition it stands for
        // (an assembly's part before the part of its own bodies).
        let mut of_part: BTreeMap<String, usize> = BTreeMap::new();
        for p in &fresh {
            if let Some(k) = p.definition
                && (p.kind == PartKind::Assembly || !of_part.contains_key(p.part.as_str()))
            {
                of_part.insert(p.part.to_string(), k);
            }
        }
        let instances: Vec<(&crate::doc::assembly::ImportUsage, &Id, Pose)> = fresh
            .iter()
            .flat_map(|p| p.features.iter())
            .filter_map(|(_, f)| match f {
                Feature::Instance(i) => Some((i.import.as_ref()?, &i.part, i.pose)),
                _ => None,
            })
            .collect();
        let mut placed = 0;
        let mut deep = 0;
        for (_, f) in &planned.features {
            let Feature::Import(i) = f else { continue };
            let Some(p) = &i.place else { continue };
            placed += 1;
            if p.via.len() > 1 {
                deep += 1;
            }
            let mut cur = planned.definition.unwrap();
            let mut world = ryukei::geom::Rigid::IDENTITY;
            for v in &p.via {
                let parent = product_of(&opened, cur);
                let (_, child, pose) = instances
                    .iter()
                    .find(|(u, child, _)| {
                        u.product == parent
                            && u.usage.as_deref() == Some(v.usage.as_str())
                            && u.ordinal == v.ordinal
                            && of_part.get(child.as_str()).map(|&k| product_of(&opened, k))
                                == Some(v.product.clone())
                    })
                    .unwrap_or_else(|| panic!("no instance for {v:?} in {parent:?}"));
                world = world.then_after(&crate::assembly::pose_rigid(pose).unwrap());
                cur = of_part[child.as_str()];
            }
            assert_eq!(
                crate::assembly::rigid_pose(&world).snapped(),
                p.pose,
                "{p:?}"
            );
        }
        assert!(placed > 0 && deep > 0, "{placed} placed, {deep} deep");
        // The walk is the same twice.
        assert_eq!(
            walk(&opened, &fr.path).unwrap(),
            walk(&opened, &fr.path).unwrap()
        );
    }

    /// Bodies over every occurrence of every product: what flattening makes.
    fn count_bodies(opened: &Opened) -> usize {
        fn under(o: &Opened, k: usize) -> usize {
            bodies(o, Some(k))
                + o.products.definitions[k]
                    .children
                    .iter()
                    .map(|c| under(o, c.definition))
                    .sum::<usize>()
        }
        opened
            .products
            .roots
            .iter()
            .map(|&k| under(opened, k))
            .sum::<usize>()
            + bodies(opened, None)
    }
    /// A walk crosses into another file at a definition whose shape is
    /// there, keeping the usages from the root (design 448, 24.D).
    #[test]
    fn a_walk_crosses_into_the_files_holding_the_shapes() {
        let set = std::path::PathBuf::from(crate::import::tests::set_path(""));
        let source = set.join("asm.stp");
        let file = crate::doc::project::ImportFile::new(std::fs::read(&source).unwrap());
        let opened = crate::import::opened(&file).unwrap();
        let t = crate::import::external::resolve(&source, file, opened, &[], "asm.stp").unwrap();
        let files = Files {
            opened: t.nodes.iter().map(|n| &*n.opened).collect(),
            paths: vec!["imports/asm.step", "imports/box.step", "imports/cylinder.step"],
            links: t.nodes.iter().map(|n| &n.links).collect(),
        };
        let w = walk_files(&files).unwrap();
        let seen: Vec<(usize, Vec<String>)> = w
            .iter()
            .map(|w| (w.file, w.place.as_ref().unwrap().via.iter().map(|v| v.usage.clone()).collect()))
            .collect();
        assert_eq!(seen, [(1, vec!["1".into()]), (2, vec!["2".into()]), (2, vec!["3".into()])]);
        let x = w[1].place.as_ref().unwrap().pose.origin_m[0];
        assert!((x - 0.05).abs() < 1e-12);
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

    /// A definition whose shape is another file's (design 448, 24.D) is
    /// placed by its usages like any child, and made by nothing here.
    #[test]
    fn a_definition_given_a_part_from_another_file_is_placed_not_made() {
        use crate::doc::import::FileRef;
        let bytes = crate::import::tests::external_assembly(
            "ASM",
            &[("BOX", "box.stp", [0.0; 3]), ("PIN", "pin.stp", [50.0, 0.0, 0.0]), ("PIN", "pin.stp", [0.0, 50.0, 0.0])],
        );
        let file = crate::doc::project::ImportFile::new(bytes);
        let opened = crate::import::opened(&file).unwrap();
        let fr = FileRef { path: "imports/asm.step".into(), blake3: file.blake3.clone() };
        let defs = &opened.products.definitions;
        let k = |id: &str| defs.iter().position(|d| &*d.product_id == id).unwrap();
        let taken = |_: &str| false;
        let external: BTreeMap<usize, Id> = [
            (k("BOX"), Id::parse("box", "part").unwrap()),
            (k("PIN"), Id::parse("pin", "part").unwrap()),
        ]
        .into();
        let fresh = Fresh {
            opened: &opened,
            file: &fr,
            stem: "asm",
            part: None,
            taken: &taken,
            pdm: false,
            mode: None,
            cross: Cross { external, ..Default::default() },
        };
        let planned = fresh.plan().unwrap();
        assert_eq!(planned.len(), 1, "only the assembly: its parts are other files'");
        let a = &planned[0];
        assert_eq!((a.part.as_str(), a.kind), ("asm", PartKind::Assembly));
        let placed: Vec<(String, f64, f64)> = a
            .features
            .iter()
            .map(|(_, f)| match f {
                Feature::Instance(i) => (i.part.to_string(), i.pose.origin_m[0], i.pose.origin_m[1]),
                _ => panic!("{f:?}"),
            })
            .collect();
        assert_eq!(placed.len(), 3);
        assert_eq!(placed[0].0, "box");
        assert!(placed[1].0 == "pin" && (placed[1].1 - 0.05).abs() < 1e-12);
        assert!(placed[2].0 == "pin" && (placed[2].2 - 0.05).abs() < 1e-12);
        // Without them, as 24.A left it: nothing to import.
        let alone = Fresh { cross: Cross::default(), ..fresh };
        assert_eq!(alone.plan().unwrap_err().code, Code::ImportBodyMissing);
    }

    /// A referenced file makes only what its referenced roots reach, its
    /// parts named from `bases`.
    #[test]
    fn a_referenced_file_makes_only_its_scope_named_as_referenced() {
        use crate::doc::import::FileRef;
        let file = crate::doc::project::ImportFile::new(crate::import::tests::frame_file());
        let opened = crate::import::opened(&file).unwrap();
        let fr = FileRef { path: "imports/frame.step".into(), blake3: file.blake3.clone() };
        let defs = &opened.products.definitions;
        let pin = defs.iter().position(|d| &*d.product_id == "P-1").unwrap();
        let taken = |_: &str| false;
        let fresh = Fresh {
            opened: &opened,
            file: &fr,
            stem: "frame",
            part: None,
            taken: &taken,
            pdm: false,
            mode: None,
            cross: Cross {
                scope: Some([pin].into()),
                bases: [(pin, Id::parse("dowel", "part").unwrap())].into(),
                ..Default::default()
            },
        };
        let planned = fresh.make(&BTreeMap::new(), false, &[]).unwrap();
        let parts: Vec<&str> = planned.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(parts, ["dowel"]);
    }
}
