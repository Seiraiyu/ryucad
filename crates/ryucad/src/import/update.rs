//! Re-import: a new version of a file the project holds, planned against
//! what the project made of it. The old bytes aren't needed: the import
//! features and imported instances say which products and usages they
//! are, and those are matched with the new file's.
//!
//! Products are matched by PRODUCT id, name and ordinal, then by id, then
//! by name, each in order among equals. A matched product's import
//! features take the new hash (and its product's new name or ordinal);
//! one with more bodies gains import features for them where a part holds
//! all its old ones. A new product makes a part as importing does, where
//! the file was imported whole before (not only `--into` a part, and not
//! the same bytes again); otherwise new products are warned of
//! (`import_products_skipped`). A
//! vanished product's features keep their product, take the new hash and
//! fail `import_product_missing`.
//!
//! Usages are matched within their assembly by id, child product and
//! ordinal: a matched instance takes the file's pose while it is fixed; a
//! new usage is a new instance; an instance whose usage vanished is
//! deleted unless a mate or pattern reads it.

use super::plan::{self, Planned};
use crate::doc::assembly::{ImportUsage, Instance, Pose};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::import::{FileRef, Import, ProductRef};
use crate::doc::project::Project;
use crate::error::Result;
use ryustep::Opened;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// What an update plans, and what it couldn't carry over.
pub struct Update {
    pub planned: Vec<Planned>,
    /// Products the project's features or instances name that the new file
    /// doesn't have, each with the parts naming it.
    pub vanished: Vec<(ProductRef, Vec<Id>)>,
    /// Instances kept though their usage vanished, because a mate or
    /// pattern reads them: `(assembly, instance)`.
    pub kept: Vec<(Id, Id)>,
    /// `import_kind_changed` for each part whose product has children now,
    /// and `import_products_skipped` for new products nothing makes.
    pub warnings: Vec<String>,
}

/// The project's import features of `path`, `(part, feature, import)`,
/// and its instances an import of `path` made, `(assembly, instance,
/// instance)`, each in its part's order, parts by ID.
#[allow(clippy::type_complexity)]
fn made_of(project: &Project, path: &str) -> (Vec<(Id, Id, Import)>, Vec<(Id, Id, Instance)>) {
    let mut features = Vec::new();
    let mut instances = Vec::new();
    for (pid, p) in &project.parts {
        for fid in p.order() {
            match p.features.get(fid) {
                Some(Ok(Feature::Import(i))) if i.file.path == path => {
                    features.push((pid.clone(), fid.clone(), i.clone()));
                }
                Some(Ok(Feature::Instance(i)))
                    if i.import.as_ref().is_some_and(|u| u.file == path) =>
                {
                    instances.push((pid.clone(), fid.clone(), i.clone()));
                }
                _ => {}
            }
        }
    }
    (features, instances)
}

/// Each of `olds` matched with a definition of `opened`: exactly, then by
/// id, then by name, each old product in order against the definitions
/// not yet matched, in definition order.
pub fn match_products(opened: &Opened, olds: &BTreeSet<ProductRef>) -> BTreeMap<ProductRef, usize> {
    let n = opened.products.definitions.len();
    let news: Vec<ProductRef> = (0..n).map(|k| plan::product_of(opened, k)).collect();
    let mut free = vec![true; n];
    let mut out = BTreeMap::new();
    let passes: [fn(&ProductRef, &ProductRef) -> bool; 3] =
        [|a, b| a == b, |a, b| a.id == b.id, |a, b| a.name == b.name];
    for same in passes {
        for r in olds {
            if out.contains_key(r) {
                continue;
            }
            if let Some(k) = (0..n).find(|&k| free[k] && same(&news[k], r)) {
                free[k] = false;
                out.insert(r.clone(), k);
            }
        }
    }
    out
}

/// Plans the update of `project`'s import of `file` (its new bytes,
/// `opened`): `fresh` makes parts for new products.
pub fn plan(
    project: &Project,
    opened: &Opened,
    file: &FileRef,
    fresh: &plan::Fresh,
) -> Result<Update> {
    let path = file.path.as_str();
    let defs = &opened.products.definitions;
    let makes = plan::makes(opened);
    let (features, instances) = made_of(project, path);
    let olds: BTreeSet<ProductRef> = features
        .iter()
        .filter_map(|(_, _, i)| i.product.clone())
        .chain(
            instances
                .iter()
                .filter_map(|(_, _, i)| i.import.as_ref().map(|u| u.product.clone())),
        )
        .collect();
    let matched = match_products(opened, &olds);
    let def_of = |r: &ProductRef| matched.get(r).copied();
    // Each matched definition's part: the assembly placing its usages,
    // else the part holding its bodies; the least ID where several do.
    let mut known: BTreeMap<usize, Id> = BTreeMap::new();
    let mut def_of_part: BTreeMap<Id, usize> = BTreeMap::new();
    for (a, _, inst) in &instances {
        if let Some(k) = inst.import.as_ref().and_then(|u| def_of(&u.product)) {
            known.entry(k).or_insert_with(|| a.clone());
            def_of_part.entry(a.clone()).or_insert(k);
        }
    }
    for (p, _, i) in &features {
        if let Some(k) = i.product.as_ref().and_then(def_of) {
            known.entry(k).or_insert_with(|| p.clone());
            def_of_part.entry(p.clone()).or_insert(k);
        }
    }
    // New products make parts only where the file was imported whole
    // before. A file only ever imported `--into` a part makes none, nor do
    // the same bytes again: a whole import of them made everything already.
    // An assembly file imported whole leaves imported instances; a flat one
    // founds a part for every product it has bodies of, so one whose
    // products aren't all in parts it founded was taken `--into` a part
    // (an empty one, perhaps). A flat file whose new version is an
    // assembly file left no instances: it was whole where two of its
    // products are each in a part it founded, since `--into` takes one.
    // Left over: a flat file with several roots whose one product was
    // taken `--into` an empty part reads as a whole import of a file with
    // that one root, and a flat file of one root that gains an assembly
    // reads as `--into`; what that skips is warned of.
    let founded = |p: &Id| {
        project.parts.get(p).is_some_and(|x| {
            x.order().first().is_some_and(|f| {
                matches!(x.features.get(f), Some(Ok(Feature::Import(i))) if i.file.path == path)
            })
        })
    };
    let same_bytes = features
        .iter()
        .any(|(_, _, i)| i.file.blake3 == file.blake3);
    let assembly_file = (0..defs.len()).any(|k| plan::is_assembly(opened, &makes, k));
    let groups: BTreeSet<&Option<ProductRef>> =
        features.iter().map(|(_, _, i)| &i.product).collect();
    let in_founded = |g: &Option<ProductRef>| {
        features
            .iter()
            .any(|(p, _, i)| &i.product == g && founded(p))
    };
    let whole = !same_bytes
        && if !instances.is_empty() {
            true
        } else if assembly_file {
            groups
                .iter()
                .filter(|g| g.is_some() && in_founded(g))
                .count()
                >= 2
        } else {
            !groups.is_empty() && groups.iter().all(|g| in_founded(g))
        };
    let had_unplaced = features.iter().any(|(_, _, i)| i.product.is_none());
    let unplaced = plan::bodies(opened, None) > 0 && !had_unplaced;
    let made = if whole {
        fresh.make(&known, unplaced, &[])?
    } else {
        Vec::new()
    };
    // Every definition's part, made or known: what a new usage places.
    let mut part_of: BTreeMap<usize, Id> = known.clone();
    for pl in &made {
        if let Some(k) = pl.definition {
            part_of.entry(k).or_insert_with(|| pl.part.clone());
        }
    }
    let mut planned: BTreeMap<Id, Planned> = BTreeMap::new();
    let mut warnings = Vec::new();
    // Products of a new version that nothing matched and nothing makes,
    // and bodies no product reaches that no feature held before.
    if !whole && !same_bytes {
        let skipped: Vec<String> = (0..defs.len())
            .filter(|k| makes[*k] && !known.contains_key(k))
            .map(|k| format!("{:?}", &*defs[k].product_id))
            .collect();
        if !skipped.is_empty() {
            warnings.push(format!(
                "import_products_skipped: product{} {} of {path} {} no part, since the file doesn't read as imported whole: import {} --into a part, or delete the file's parts and import it again",
                if skipped.len() == 1 { "" } else { "s" },
                skipped.join(", "),
                if skipped.len() == 1 { "has" } else { "have" },
                if skipped.len() == 1 { "it" } else { "them" },
            ));
        }
        if unplaced {
            warnings.push(format!(
                "import_products_skipped: the bodies no product reaches in {path} have no part, since the file doesn't read as imported whole: delete the file's parts and import it again"
            ));
        }
    }
    let entry =
        |planned: &mut BTreeMap<Id, Planned>, part: &Id, kind: PartKind, k: Option<usize>| {
            planned.entry(part.clone()).or_insert_with(|| Planned {
                part: part.clone(),
                new: false,
                kind,
                definition: k,
                features: Vec::new(),
                removed: Vec::new(),
                properties: BTreeMap::new(),
            });
        };
    // Import features: the new hash, the product as it is now, and the
    // bodies a product gained where a part holds all its old ones.
    let mut held: BTreeMap<(Id, Option<usize>), Vec<u32>> = BTreeMap::new();
    for (p, fid, i) in &features {
        let group = match &i.product {
            Some(r) => def_of(r).map(Some),
            None => Some(None),
        };
        let product = match group {
            Some(g) => g.map(|k| plan::product_of(opened, k)),
            None => i.product.clone(),
        };
        let new = Import {
            file: file.clone(),
            product,
            body: i.body,
        };
        if let Some(g) = group {
            held.entry((p.clone(), g)).or_default().push(i.body);
        }
        if new != *i {
            entry(&mut planned, p, PartKind::Part, group.flatten());
            let pl = planned.get_mut(p).expect("entered");
            pl.features.push((fid.clone(), Feature::Import(new)));
        }
    }
    for ((p, g), mut have) in held {
        have.sort_unstable();
        let complete = have.iter().enumerate().all(|(n, b)| *b as usize == n);
        let now = plan::bodies(opened, g);
        if !complete || now <= have.len() {
            continue;
        }
        let part = project.part(&p)?;
        entry(&mut planned, &p, PartKind::Part, g);
        let pl = planned.get_mut(&p).expect("entered");
        let mut taken: Vec<String> = part
            .features
            .keys()
            .map(|f| f.to_string())
            .chain(pl.features.iter().map(|(f, _)| f.to_string()))
            .collect();
        let product = g.map(|k| plan::product_of(opened, k));
        let extra = plan::import_features(file, product, have.len()..now, &mut taken);
        pl.features.extend(extra);
    }
    // A part whose product has children now can't place them.
    for (&k, p) in &known {
        let is_part = project
            .parts
            .get(p)
            .is_some_and(|x| crate::server::part_kind(x) == PartKind::Part);
        if is_part && plan::is_assembly(opened, &makes, k) {
            warnings.push(format!(
                "import_kind_changed: product {:?} of {path} has children now; part {p} can't place them: delete it and import the file again to make an assembly of it",
                &*defs[k].product_id
            ));
        }
    }
    // Instances, assembly by assembly.
    let mut kept = Vec::new();
    let mut by_asm: BTreeMap<Id, Vec<(Id, Instance)>> = BTreeMap::new();
    for (a, iid, inst) in &instances {
        by_asm
            .entry(a.clone())
            .or_default()
            .push((iid.clone(), inst.clone()));
    }
    for (a, mine) in &by_asm {
        let part = project.part(a)?;
        let mut ids: Vec<String> = part.features.keys().map(|f| f.to_string()).collect();
        let parents: BTreeSet<ProductRef> = mine
            .iter()
            .filter_map(|(_, i)| i.import.as_ref().map(|u| u.product.clone()))
            .collect();
        for r in parents {
            let k = def_of(&r);
            let olds_of: Vec<&(Id, Instance)> = mine
                .iter()
                .filter(|(_, i)| i.import.as_ref().is_some_and(|u| u.product == r))
                .collect();
            // The usages the file has now, by (usage, child, ordinal); the
            // product's own bodies as no usage.
            type Key = (Option<String>, Option<usize>, u32);
            let mut wanted: Vec<(Key, Option<usize>)> = Vec::new();
            if let Some(k) = k {
                if plan::bodies(opened, Some(k)) > 0 && plan::is_assembly(opened, &makes, k) {
                    wanted.push(((None, Some(k), 0), None));
                }
                for (n, o) in defs[k].children.iter().enumerate() {
                    if makes[o.definition] {
                        let key = (
                            Some(o.id.to_string()),
                            Some(o.definition),
                            plan::usage_ordinal(opened, k, n),
                        );
                        wanted.push((key, Some(n)));
                    }
                }
            }
            let key_of = |i: &Instance| -> Key {
                let u = i.import.as_ref().expect("imported");
                (
                    u.usage.clone(),
                    def_of_part.get(&i.part).copied(),
                    u.ordinal,
                )
            };
            let mut used = vec![false; wanted.len()];
            for (iid, inst) in &olds_of {
                let found = wanted
                    .iter()
                    .enumerate()
                    .find(|(w, (key, _))| !used[*w] && *key == key_of(inst));
                match (k, found) {
                    (Some(k), Some((w, (_, n)))) => {
                        used[w] = true;
                        let mut new = inst.clone();
                        let u = new.import.as_mut().expect("imported");
                        u.product = plan::product_of(opened, k);
                        if inst.fixed {
                            new.pose = match n {
                                Some(n) => plan::pose(opened, path, k, *n)?,
                                None => Pose::IDENTITY,
                            };
                        }
                        if new != *inst {
                            entry(&mut planned, a, PartKind::Assembly, Some(k));
                            let pl = planned.get_mut(a).expect("entered");
                            pl.features.push((iid.clone(), Feature::Instance(new)));
                        }
                    }
                    _ => {
                        entry(&mut planned, a, PartKind::Assembly, k);
                        if crate::graph::dependents(part, iid).is_empty() {
                            let pl = planned.get_mut(a).expect("entered");
                            pl.removed.push(iid.clone());
                        } else {
                            kept.push((a.clone(), iid.clone()));
                        }
                    }
                }
            }
            let Some(k) = k else {
                continue;
            };
            let product = plan::product_of(opened, k);
            for (w, (_, n)) in wanted.iter().enumerate() {
                if used[w] {
                    continue;
                }
                let (child, pose, usage) = match n {
                    Some(n) => {
                        let o = &defs[k].children[*n];
                        // A part this assembly already places for the
                        // child's product, else the child's part.
                        let child = olds_of
                            .iter()
                            .map(|(_, i)| &i.part)
                            .find(|p| def_of_part.get(*p) == Some(&o.definition))
                            .or_else(|| part_of.get(&o.definition))
                            .cloned();
                        let Some(child) = child else {
                            continue;
                        };
                        let usage = ImportUsage {
                            file: path.to_owned(),
                            product: product.clone(),
                            usage: Some(o.id.to_string()),
                            ordinal: plan::usage_ordinal(opened, k, *n),
                        };
                        (child, plan::pose(opened, path, k, *n)?, usage)
                    }
                    None => {
                        // The product gained bodies: a part of its own.
                        let base = Id::parse(&format!("{a}-body"), "part")
                            .unwrap_or_else(|_| Id::parse("body", "part").expect("an ID"));
                        let taken_now: Vec<String> = made
                            .iter()
                            .map(|p| p.part.to_string())
                            .chain(planned.keys().map(|p| p.to_string()))
                            .collect();
                        let own = super::free(&base, |s| {
                            (fresh.taken)(s) || taken_now.iter().any(|t| t == s)
                        });
                        let n = plan::bodies(opened, Some(k));
                        planned.insert(
                            own.clone(),
                            Planned {
                                part: own.clone(),
                                new: true,
                                kind: PartKind::Part,
                                definition: Some(k),
                                features: plan::import_features(
                                    file,
                                    Some(product.clone()),
                                    0..n,
                                    &mut Vec::new(),
                                ),
                                removed: Vec::new(),
                                properties: BTreeMap::new(),
                            },
                        );
                        let usage = ImportUsage {
                            file: path.to_owned(),
                            product: product.clone(),
                            usage: None,
                            ordinal: 0,
                        };
                        (own, Pose::IDENTITY, usage)
                    }
                };
                let iid = match n {
                    Some(n) => plan::instance_id(opened, k, *n, &child, &ids),
                    None => super::free(&child, |s| ids.iter().any(|t| t == s)),
                };
                ids.push(iid.to_string());
                entry(&mut planned, a, PartKind::Assembly, Some(k));
                let pl = planned.get_mut(a).expect("entered");
                pl.features.push((iid, plan::instance(&child, pose, usage)));
            }
        }
    }
    let vanished = olds
        .iter()
        .filter(|r| !matched.contains_key(*r))
        .map(|r| {
            let mut parts: Vec<Id> = features
                .iter()
                .filter(|(_, _, i)| i.product.as_ref() == Some(r))
                .map(|(p, _, _)| p.clone())
                .chain(
                    instances
                        .iter()
                        .filter(|(_, _, i)| i.import.as_ref().is_some_and(|u| u.product == *r))
                        .map(|(a, _, _)| a.clone()),
                )
                .collect();
            parts.dedup();
            (r.clone(), parts)
        })
        .collect();
    Ok(Update {
        planned: planned.into_values().chain(made).collect(),
        vanished,
        kept,
        warnings,
    })
}

/// The definition of `opened` (the file at `path`) that `part` is: the
/// product its instances from that file belong to, else its import
/// features', as an update reads it.
fn definition_of(project: &Project, part: &Id, path: &str, opened: &Opened) -> Option<usize> {
    let p = project.parts.get(part)?;
    let products = || {
        p.order()
            .iter()
            .filter_map(|f| p.features.get(f)?.as_ref().ok())
    };
    let r = products()
        .find_map(|f| match f {
            Feature::Instance(i) => i
                .import
                .as_ref()
                .filter(|u| u.file == path)
                .map(|u| &u.product),
            _ => None,
        })
        .or_else(|| {
            products().find_map(|f| match f {
                Feature::Import(i) if i.file.path == path => i.product.as_ref(),
                _ => None,
            })
        })?;
    (0..opened.products.definitions.len()).find(|&k| plan::product_of(opened, k) == *r)
}

/// `import_usage_missing` for each instance of assembly `part` an import
/// made whose usage its file no longer has: kept by an update because a
/// mate or pattern reads it.
pub fn usage_warnings(project: &Project, part: &Id) -> Vec<Value> {
    let Some(p) = project.parts.get(part) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for fid in p.order() {
        let Some(Ok(Feature::Instance(i))) = p.features.get(fid) else {
            continue;
        };
        let Some(u) = &i.import else {
            continue;
        };
        let Some(Ok(opened)) = project.imports.get(&u.file).map(super::opened) else {
            continue;
        };
        let defs = &opened.products.definitions;
        let k = (0..defs.len()).find(|&k| plan::product_of(&opened, k) == u.product);
        let child = definition_of(project, &i.part, &u.file, &opened);
        let found = k.is_some_and(|k| match &u.usage {
            None => plan::bodies(&opened, Some(k)) > 0,
            Some(id) => defs[k].children.iter().enumerate().any(|(n, o)| {
                *o.id == **id
                    && child == Some(o.definition)
                    && plan::usage_ordinal(&opened, k, n) == u.ordinal
            }),
        });
        if !found {
            let usage = u.usage.as_deref().unwrap_or("its own bodies");
            out.push(json!({
                "code": "import_usage_missing",
                "feature": fid,
                "file": u.file,
                "message": format!(
                    "instance {fid} places {} as {usage:?} of product {:?}, which {} no longer has; a mate or pattern reads it, so it was kept",
                    i.part, u.product.id, u.file
                ),
            }));
        }
    }
    out
}

/// One version of a file's products, each with what makes it what it is:
/// the hashes of its bodies healed under one key for the file, and its
/// usages (id, child, ordinal and pose bits); `None` is the bodies no
/// product reaches.
fn signatures(
    path: &str,
    opened: &Opened,
) -> BTreeMap<Option<ProductRef>, (Vec<String>, Vec<String>)> {
    use rayon::prelude::*;
    let key = ryukei::name::OpKey::from_bytes(format!("ryucad.diff.import\0{path}").as_bytes());
    let opts = ryustep::ReadOptions {
        key,
        heal: Default::default(),
    };
    let hashes: Vec<String> = (0..opened.held.len())
        .into_par_iter()
        .map(|k| match opened.body(k, &opts).result {
            Ok((b, _, _)) => blake3::hash(b.dump().as_bytes()).to_hex()[..32].to_owned(),
            Err(e) => format!("failed: {e}"),
        })
        .collect();
    let defs = &opened.products.definitions;
    let mut out = BTreeMap::new();
    let groups = (0..defs.len()).map(Some).chain([None]);
    for g in groups {
        let bodies: Vec<String> = opened
            .held
            .iter()
            .zip(&hashes)
            .filter(|(h, _)| h.definition == g)
            .map(|(_, x)| x.clone())
            .collect();
        let usages: Vec<String> = match g {
            None => Vec::new(),
            Some(k) => defs[k]
                .children
                .iter()
                .enumerate()
                .map(|(n, o)| {
                    let pose = match opened.placement(k, n) {
                        Ok(r) => format!("{:?}", crate::assembly::rigid_pose(&r)),
                        Err(e) => e.to_string(),
                    };
                    let child = plan::product_of(opened, o.definition);
                    format!(
                        "{} {child:?} {} {pose}",
                        o.id,
                        plan::usage_ordinal(opened, k, n)
                    )
                })
                .collect(),
        };
        if g.is_none() && bodies.is_empty() {
            continue;
        }
        out.insert(g.map(|k| plan::product_of(opened, k)), (bodies, usages));
    }
    out
}

/// What changed between two versions of the imported file at `path`, by
/// product, matched as an update matches them: `added`, `removed`, and
/// `changed` with whether its bodies and its usages did. A side that isn't
/// there, or isn't a STEP file this build reads, has no products.
pub fn products_diff(path: &str, from: Option<&[u8]>, to: Option<&[u8]>) -> Value {
    let read = |b: Option<&[u8]>| {
        let file = crate::doc::project::ImportFile::new(b?.to_vec());
        let opened = super::opened(&file).ok()?;
        Some((signatures(path, &opened), opened))
    };
    let (a, b) = (read(from), read(to));
    let empty = BTreeMap::new();
    let sa = a.as_ref().map_or(&empty, |(s, _)| s);
    let sb = b.as_ref().map_or(&empty, |(s, _)| s);
    let product = |r: &Option<ProductRef>| r.as_ref().map_or(Value::Null, super::product_json);
    // Products as an update would match them; the bodies no product
    // reaches with each other.
    let olds: BTreeSet<ProductRef> = sa.keys().flatten().cloned().collect();
    let matched: BTreeMap<ProductRef, ProductRef> = match &b {
        Some((_, opened)) => match_products(opened, &olds)
            .into_iter()
            .map(|(r, k)| (r, plan::product_of(opened, k)))
            .collect(),
        None => BTreeMap::new(),
    };
    let to_of = |r: &Option<ProductRef>| match r {
        Some(r) => matched.get(r).cloned().map(Some),
        None => sb.contains_key(&None).then_some(None),
    };
    let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    for (r, (bodies, usages)) in sa {
        match to_of(r) {
            None => removed.push(product(r)),
            Some(t) => {
                let (tb, tu) = &sb[&t];
                if tb != bodies || tu != usages || t != *r {
                    changed.push(json!({"product": product(&t), "from": product(r),
                                        "bodies": tb != bodies, "usages": tu != usages}));
                }
            }
        }
    }
    let reached: BTreeSet<Option<ProductRef>> = sa.keys().filter_map(to_of).collect();
    for r in sb.keys().filter(|r| !reached.contains(*r)) {
        added.push(product(r));
    }
    json!({"added": added, "removed": removed, "changed": changed})
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::import::tests::{frame_version, listed, occt};
    use crate::server::tests::{call, fresh};
    use serde_json::{Value, json};

    /// `bytes` written outside the project, as an agent would have them.
    fn outside(bytes: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.step");
        std::fs::write(&path, bytes).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    /// A project with the first frame imported.
    fn imported() -> (tempfile::TempDir, crate::session::Session) {
        let (src, path) = outside(&frame_version(1));
        let (dir, mut s) = fresh();
        call(&mut s, "import", json!({"path": path})).unwrap();
        drop(src);
        (dir, s)
    }

    fn update(
        s: &mut crate::session::Session,
        bytes: &[u8],
        extra: Value,
    ) -> crate::error::Result<Value> {
        let (_src, path) = outside(bytes);
        let mut params = json!({"path": path, "update": "imports/frame.step"});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        call(s, "import", params)
    }

    #[test]
    fn the_same_file_again_changes_nothing() {
        let (dir, mut s) = imported();
        let before = crate::session::files(dir.path()).unwrap();
        let bytes = |s: &crate::session::Session| s.snapshot();
        let was = bytes(&s);
        let v = update(&mut s, &frame_version(1), json!({})).unwrap();
        assert_eq!(v["parts"], json!([]), "{v}");
        assert_eq!(v["update"], true);
        assert!(
            v.get("failing").is_none() && v.get("vanished").is_none(),
            "{v}"
        );
        assert_eq!(crate::session::files(dir.path()).unwrap(), before);
        assert_eq!(bytes(&s), was);
    }

    /// `bytes` with text after the end, which readers ignore: the same
    /// file under another hash.
    fn touched(bytes: &[u8]) -> Vec<u8> {
        let mut b = bytes.to_vec();
        b.extend_from_slice(b"\n/* again */\n");
        b
    }

    /// Updates of `imports/frame.step` to `bytes` and to them touched, each
    /// a dry run and then written, that plan and write no parts.
    fn updates_make_nothing(
        dir: &tempfile::TempDir,
        s: &mut crate::session::Session,
        bytes: &[u8],
    ) {
        let created = |v: &Value| {
            let parts = v["parts"].as_array().unwrap();
            parts.iter().filter(|p| p["created"] == true).count()
        };
        for b in [bytes.to_vec(), touched(bytes)] {
            let before = crate::session::files(dir.path()).unwrap();
            let v = update(s, &b, json!({"dry_run": true})).unwrap();
            assert_eq!(created(&v), 0, "{v}");
            assert_eq!(crate::session::files(dir.path()).unwrap(), before);
            let v = update(s, &b, json!({})).unwrap();
            assert_eq!(created(&v), 0, "{v}");
            let parts = |files: Vec<std::path::PathBuf>| -> Vec<std::path::PathBuf> {
                files
                    .into_iter()
                    .filter(|f| f.ends_with("part.json"))
                    .collect()
            };
            assert_eq!(
                parts(crate::session::files(dir.path()).unwrap()),
                parts(before)
            );
        }
    }

    /// The first product of `bytes` (imported as `frame.step`) taken
    /// `--into` part `p`, with a box first unless `empty`.
    fn into(s: &mut crate::session::Session, bytes: &[u8], p: &str, empty: bool, product: usize) {
        let (_src, path) = outside(bytes);
        call(s, "part.create", json!({"part": p})).unwrap();
        if !empty {
            call(
                s,
                "feature.add",
                json!({"part": p, "type": "box", "id": "body", "params": {"x": 1, "y": 1, "z": 1}}),
            )
            .unwrap();
        }
        let e = call(s, "import", json!({"path": path, "into": p})).unwrap_err();
        let one = e.details["products"][product]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            s,
            "import",
            json!({"path": path, "into": p, "product": one}),
        )
        .unwrap();
    }

    /// Two roots, `A-1` and `B-1`, a block each: a flat file.
    fn two_roots() -> Vec<u8> {
        roots(&["A-1", "B-1"], false)
    }

    /// A block for each of `ids`, roots of a flat file, or with `assembly`
    /// an assembly `X-1` placing each once.
    fn roots(ids: &[&str], assembly: bool) -> Vec<u8> {
        use ryukei::geom::{Rigid, Vector3};
        let block =
            ryukei::primitive::block(ryukei::name::OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY)
                .unwrap()
                .0;
        let product = |id: &str, bodies, children| ryustep::Product {
            id: id.into(),
            name: id.into(),
            bodies,
            children,
            ..Default::default()
        };
        let mut products: Vec<ryustep::Product> = ids
            .iter()
            .map(|id| product(id, vec![block.clone()], vec![]))
            .collect();
        if assembly {
            let children = (0..ids.len())
                .map(|n| ryustep::Child {
                    product: n,
                    place: Rigid::translation(Vector3::new(0.02 * n as f64, 0.0, 0.0)),
                    id: String::new(),
                })
                .collect();
            products.push(product("X-1", vec![], children));
        }
        crate::import::tests::written(products)
    }

    /// The parts `v`'s answer created, with their kinds.
    fn created(v: &Value) -> Vec<(String, String)> {
        v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["created"] == true)
            .map(|p| {
                (
                    p["part"].as_str().unwrap().to_owned(),
                    p["kind"].as_str().unwrap_or("part").to_owned(),
                )
            })
            .collect()
    }

    /// The `import_products_skipped` warnings of `v`'s answer.
    fn skipped(v: &Value) -> Vec<String> {
        v["warnings"]
            .as_array()
            .map(|w| {
                w.iter()
                    .filter_map(|w| w.as_str())
                    .filter(|w| w.starts_with("import_products_skipped"))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_flat_file_imported_whole_makes_the_assembly_its_new_version_adds() {
        let (_src, path) = outside(&two_roots());
        let (dir, mut s) = fresh();
        call(&mut s, "import", json!({"path": path})).unwrap();
        let v = update(&mut s, &roots(&["A-1", "B-1"], true), json!({})).unwrap();
        assert_eq!(created(&v), [("x-1".into(), "assembly".into())], "{v}");
        assert!(skipped(&v).is_empty(), "{v}");
        let x = listed(dir.path(), "x-1");
        let placed: Vec<&Value> = x.iter().map(|(_, f)| &f["part"]).collect();
        assert_eq!(placed, [&json!("a-1"), &json!("b-1")]);
    }

    #[test]
    fn products_an_update_makes_no_part_for_are_warned_of() {
        // A flat file of one root imported whole reads as `--into` once it
        // gains an assembly (the limit): the assembly is named.
        let (_src, path) = outside(&roots(&["A-1"], false));
        let (_dir, mut s) = fresh();
        call(&mut s, "import", json!({"path": path})).unwrap();
        let v = update(&mut s, &roots(&["A-1"], true), json!({"dry_run": true})).unwrap();
        assert!(created(&v).is_empty(), "{v}");
        let w = skipped(&v);
        assert_eq!(w.len(), 1, "{v}");
        assert!(w[0].contains("\"X-1\""), "{w:?}");
        // One product of an assembly file taken `--into` a part: a new
        // version names the others; the same bytes again say nothing.
        let asm = occt("assembly.stp");
        let (_dir, mut s) = fresh();
        into(&mut s, &asm, "p", false, 0);
        let v = update(&mut s, &asm, json!({"dry_run": true})).unwrap();
        assert!(skipped(&v).is_empty(), "{v}");
        let v = update(&mut s, &touched(&asm), json!({"dry_run": true})).unwrap();
        assert!(created(&v).is_empty(), "{v}");
        assert_eq!(skipped(&v).len(), 1, "{v}");
    }

    #[test]
    fn bodies_no_product_reaches_an_update_makes_no_part_for_are_warned_of() {
        // `A-1` taken `--into` a part; the new version's `B-1` loses its
        // shape's link to its product, so its block is reached by none.
        let (_dir, mut s) = fresh();
        into(&mut s, &two_roots(), "p", false, 0);
        let text = String::from_utf8(two_roots()).unwrap();
        let link = text
            .lines()
            .filter(|l| l.contains("SHAPE_DEFINITION_REPRESENTATION("))
            .nth(1)
            .unwrap();
        let bytes = text.replacen(&format!("{link}\n"), "", 1).into_bytes();
        let v = update(&mut s, &bytes, json!({"dry_run": true})).unwrap();
        assert!(created(&v).is_empty(), "{v}");
        let w = skipped(&v);
        assert_eq!(w.len(), 1, "{v}");
        assert!(w[0].contains("bodies no product reaches"), "{w:?}");
    }

    #[test]
    fn a_file_only_imported_into_a_part_makes_no_parts() {
        // One product's bodies of an assembly file added to a part; the
        // rest of the file left.
        let asm = occt("assembly.stp");
        let (dir, mut s) = fresh();
        into(&mut s, &asm, "p", false, 0);
        updates_make_nothing(&dir, &mut s, &asm);
        // The same into a part of its own, made empty for it.
        let (dir, mut s) = fresh();
        into(&mut s, &asm, "p", true, 0);
        updates_make_nothing(&dir, &mut s, &asm);
        // A flat file's roots, one into an empty part and one into a part
        // with a box: not a whole import, which founds a part for each.
        let flat = two_roots();
        let (dir, mut s) = fresh();
        into(&mut s, &flat, "p", true, 0);
        into(&mut s, &flat, "q", false, 1);
        updates_make_nothing(&dir, &mut s, &flat);
        // One root into an empty part: the same bytes make nothing; a new
        // version reads as a whole import of a file of that one root (the
        // limit).
        let (dir, mut s) = fresh();
        into(&mut s, &flat, "p", true, 0);
        let before = crate::session::files(dir.path()).unwrap();
        let v = update(&mut s, &flat, json!({"dry_run": true})).unwrap();
        assert_eq!(v["parts"], json!([]), "{v}");
        assert_eq!(crate::session::files(dir.path()).unwrap(), before);
    }

    #[test]
    fn an_instance_placing_another_part_than_its_usage_is_warned_of() {
        // The sub-assembly's first pin made by hand to place the loose
        // block: its usage's id and ordinal are still the file's, its
        // child isn't.
        let (dir, mut s) = imported();
        let file = dir.path().join("parts/s-1/features/1.json");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"part\": \"p-1\""), "{text}");
        std::fs::write(
            &file,
            text.replace("\"part\": \"p-1\"", "\"part\": \"l-1\""),
        )
        .unwrap();
        s.reload().unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "s-1"})).unwrap();
        let warned: Vec<&Value> = r["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"] == "import_usage_missing")
            .map(|w| &w["feature"])
            .collect();
        assert_eq!(warned, [&json!("1")], "{r}");
    }

    #[test]
    fn a_new_version_moves_adds_and_removes_what_the_import_made() {
        let (dir, mut s) = imported();
        let root = dir.path();
        // A dry run first: it says everything and writes nothing.
        let before = s.snapshot();
        let dry = update(&mut s, &frame_version(2), json!({"dry_run": true})).unwrap();
        assert_eq!(s.snapshot(), before);
        let v = update(&mut s, &frame_version(2), json!({})).unwrap();
        assert_eq!(dry["parts"], v["parts"]);
        let new = crate::doc::project::ImportFile::new(frame_version(2)).blake3;
        assert_eq!(v["file"]["blake3"], new);
        assert_eq!(
            std::fs::read(root.join("imports/frame.step")).unwrap(),
            frame_version(2)
        );
        // The frame's pin moved with the file, and the sub-assembly sits on
        // the thicker plate.
        let frame = listed(root, "f-1");
        assert_eq!(frame[1].1["pose"]["origin_m"], json!([0, 0, 0.012]));
        assert_eq!(frame[2].1["pose"]["origin_m"], json!([0.06, 0.05, 0.012]));
        // The sub-assembly's second pin is gone, a third is new.
        let sub = listed(root, "s-1");
        let ids: Vec<&str> = sub.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
        assert_eq!(sub[1].1["pose"]["origin_m"], json!([0.03, 0, 0]));
        assert_eq!(sub[1].1["import"]["usage"], "3");
        // Every import feature reads the new bytes.
        for part in ["f-1-body", "p-1", "l-1"] {
            assert_eq!(listed(root, part)[0].1["file"]["blake3"], new, "{part}");
        }
        // The new block is a new part; the loose block's product is gone,
        // so its part stays and fails.
        let made: Vec<&str> = v["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["created"] == true)
            .map(|p| p["part"].as_str().unwrap())
            .collect();
        assert_eq!(made, ["n-1"]);
        assert_eq!(
            v["vanished"],
            json!([{"product": {"id": "L-1", "name": "Loose"}, "parts": ["l-1"]}])
        );
        let failing = v["failing"].as_array().unwrap();
        assert_eq!(failing.len(), 1, "{v}");
        assert_eq!(
            (&failing[0]["part"], &failing[0]["error"]["code"]),
            (&json!("l-1"), &json!("import_product_missing"))
        );
        let r = call(&mut s, "rebuild", json!({"part": "f-1"})).unwrap();
        assert_eq!(r["parts"][0]["failed"], 0, "{r}");
        // Undone, it is the first version again.
        let mut live = crate::live::Live::new();
        crate::server::tests::call_live(&mut s, &mut live, None, "undo", json!({}))
            .unwrap_or_default();
    }

    #[test]
    fn freed_and_mated_instances_are_left_where_they_are() {
        let (dir, mut s) = imported();
        let root = dir.path();
        // The sub-assembly's second pin freed and fastened 10 mm along x
        // from the first, where it is.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "s-1", "id": "1-2", "fixed": false}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "s-1", "type": "mate", "id": "m", "form": "fastened",
                   "a": "s-1/1", "b": "s-1/1-2", "a_offset": {"origin": ["10 mm", 0, 0]}}),
        )
        .unwrap();
        let v = update(&mut s, &frame_version(2), json!({})).unwrap();
        assert_eq!(
            v["kept"],
            json!([{"part": "s-1", "instance": "1-2"}]),
            "{v}"
        );
        let warned = v["warnings"].as_array().unwrap().iter().any(|w| {
            w.as_str()
                .unwrap()
                .starts_with("import_usage_missing: s-1/1-2")
        });
        assert!(warned, "{v}");
        let sub = listed(root, "s-1");
        let ids: Vec<&str> = sub.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(ids, ["1", "1-2", "m", "3"]);
        assert_eq!(sub[1].1["pose"]["origin_m"], json!([0.02, 0, 0]));
        let r = call(&mut s, "rebuild", json!({"part": "s-1"})).unwrap();
        let w = &r["warnings"];
        assert_eq!(
            (&w[0]["code"], &w[0]["feature"]),
            (&json!("import_usage_missing"), &json!("1-2")),
            "{r}"
        );
    }

    #[test]
    fn updates_are_refused_before_anything_is_written() {
        let (dir, mut s) = imported();
        let before = crate::session::files(dir.path()).unwrap();
        let e = update(
            &mut s,
            &frame_version(2),
            json!({"update": "imports/other.step"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = update(&mut s, &frame_version(2), json!({"part": "x"})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        let e = update(&mut s, &frame_version(2), json!({"expected_revision": "0"})).unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let e = update(&mut s, b"not step", json!({})).unwrap_err();
        assert_eq!(e.code, Code::StepUnreadable);
        assert_eq!(crate::session::files(dir.path()).unwrap(), before);
    }

    #[test]
    fn an_update_is_one_change_undone_whole() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (dir, mut s) = imported();
        let before = s.snapshot();
        let (_src, path) = outside(&frame_version(2));
        let mut live = Live::new();
        call_live(
            &mut s,
            &mut live,
            None,
            "import",
            json!({"path": path, "update": "imports/frame.step"}),
        )
        .unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        assert_eq!(log["entries"][0]["method"], "import", "{log}");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("imports/frame.step")).unwrap(),
            frame_version(1)
        );
        assert_eq!(s.snapshot(), before);
    }

    #[test]
    fn a_model_diff_says_which_products_changed() {
        use crate::git::tests::git;
        let (dir, mut s) = imported();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "frame 1"]);
        update(&mut s, &frame_version(2), json!({})).unwrap();
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let file = &v["imports"][0];
        assert_eq!(file["path"], "imports/frame.step");
        let products = &file["products"];
        assert_eq!(products["added"], json!([{"id": "N-1", "name": "New"}]));
        assert_eq!(products["removed"], json!([{"id": "L-1", "name": "Loose"}]));
        let changed: Vec<(&str, bool, bool)> = products["changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["product"]["id"].as_str().unwrap(),
                    c["bodies"] == true,
                    c["usages"] == true,
                )
            })
            .collect();
        // The frame's plate and its placements; the sub-assembly's pins;
        // the pin itself is as it was.
        assert_eq!(changed, [("F-1", true, true), ("S-1", false, true)]);
        // Without the model, the hashes only.
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert!(v["imports"][0].get("products").is_none());
    }

    #[test]
    fn a_file_replaced_by_hand_is_taken_by_doctors_repair() {
        let (dir, mut s) = imported();
        let root = dir.path();
        std::fs::write(root.join("imports/frame.step"), frame_version(2)).unwrap();
        s.reload().unwrap();
        let v = call(&mut s, "doctor", json!({})).unwrap();
        let issue = v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["code"] == "import_file_changed")
            .unwrap()
            .clone();
        let repair = issue["repairs"][0]["id"].as_str().unwrap().to_owned();
        assert!(
            repair.starts_with("import_file_changed:import_update:"),
            "{repair}"
        );
        call(&mut s, "doctor.repair", json!({"repairs": [repair]})).unwrap();
        // As an update to those bytes leaves it.
        let sub = listed(root, "s-1");
        let ids: Vec<&str> = sub.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
        assert!(root.join("parts/n-1/part.json").is_file());
        let v = call(&mut s, "doctor", json!({})).unwrap();
        let codes: Vec<&str> = v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["code"].as_str().unwrap())
            .collect();
        assert!(!codes.contains(&"import_file_changed"), "{v}");
    }

    #[test]
    fn names_of_unchanged_geometry_survive_and_references_to_the_rest_fail() {
        // The box imported, a fillet on a bottom edge and one on a top
        // edge; then the box made taller.
        let bytes = occt("box_ap214.stp");
        let (_src, path) = outside(&bytes);
        let (_dir, mut s) = fresh();
        call(&mut s, "import", json!({"path": path, "part": "b"})).unwrap();
        let v = call(&mut s, "inspect", json!({"part": "b"})).unwrap();
        let edges = v["bodies"][0]["edges"].as_array().unwrap().clone();
        // An edge whose ends are both at height z, by its vertices.
        let vertices = v["bodies"][0]["vertices"].as_array().unwrap().clone();
        let at = |e: &Value, end: &str| {
            let name = e[end].as_str().unwrap();
            vertices.iter().find(|x| x["name"] == name).unwrap()["point"][2]
                .as_f64()
                .unwrap()
        };
        let edge_at = |z: f64| {
            edges
                .iter()
                .find(|e| at(e, "start") == z && at(e, "end") == z)
                .unwrap()["name"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let (bottom, top) = (edge_at(0.0), edge_at(0.03));
        for (id, e) in [("low", &bottom), ("high", &top)] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "b", "type": "fillet", "id": id, "radius": "1 mm",
                       "edges": [format!("b/body/{e}")], "propagate": false}),
            )
            .unwrap();
        }
        // The same file again renames nothing.
        let (_same, same) = outside(&bytes);
        let v = call(
            &mut s,
            "import",
            json!({"path": same, "update": "imports/frame.step"}),
        )
        .unwrap();
        assert_eq!(v["parts"], json!([]));
        let text = String::from_utf8(bytes).unwrap();
        let taller = text
            .replace(",30.))", ",40.))")
            .replace("(30.,0.)", "(40.,0.)");
        let (_t, taller_path) = outside(taller.as_bytes());
        let v = call(
            &mut s,
            "import",
            json!({"path": taller_path, "update": "imports/frame.step", "dry_run": true}),
        )
        .unwrap();
        // The fillet on the top edge would fail; the one below wouldn't.
        let failing = v["failing"].as_array().unwrap();
        assert_eq!(failing.len(), 1, "{v}");
        assert_eq!(failing[0]["feature"], "high");
        let e = &failing[0]["error"];
        assert_eq!(e["code"], "reference_unresolved", "{e}");
        assert_eq!(e["details"]["removed_by"], "body", "{e}");
        call(
            &mut s,
            "import",
            json!({"path": taller_path, "update": "imports/frame.step"}),
        )
        .unwrap();
        let r = match call(&mut s, "rebuild", json!({"part": "b"})) {
            Ok(v) => v,
            Err(e) => e.details,
        };
        let state: Vec<(&str, bool)> = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["id"].as_str().unwrap(), f["ok"] == true))
            .collect();
        assert_eq!(state, [("body", true), ("low", true), ("high", false)]);
    }
}
