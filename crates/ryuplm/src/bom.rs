//! BOMs (design 734): an item's own lines (`bom` in its file) plus the
//! lines its representations give, each line held in exactly one place.
//! This file judges own lines; the joined BOM, `bom show`, the findings
//! and the diff build on it.

use crate::error::{Code, Error, Result};
use crate::fault::{Fault, Severity};
use crate::id::{Id, ProjectId};
use crate::ids::item_path;
use crate::item::ItemRef;
use crate::item::Line;
use crate::load::Catalogue;
use crate::reps::{At, Reps};
use crate::session::Session;
use crate::write::{Writes, commit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The version of the shape `bom show`, `bom diff` and `where-used`
/// answer with. Adding a field is not a change.
pub const SCHEMA: u64 = 1;

/// Whether `q` is a line's quantity: a positive decimal number as
/// written (`4`, `0.5`), digits only, never signed, exponent or unit.
pub fn quantity_ok(q: &str) -> bool {
    let (int, frac) = q.split_once('.').unwrap_or((q, "0"));
    !int.is_empty()
        && !frac.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
        && q.bytes().any(|b| (b'1'..=b'9').contains(&b))
}

/// The child's ID when it is this catalogue's: named bare, or by this
/// catalogue's project ID.
pub fn local<'a>(own: &ProjectId, r: &'a ItemRef) -> Option<&'a Id> {
    match &r.catalogue {
        None => Some(&r.item),
        Some(p) if p == own => Some(&r.item),
        Some(_) => None,
    }
}

/// Each item's own lines' children that the catalogue holds, in line
/// order.
pub fn own_edges(cat: &Catalogue) -> BTreeMap<Id, Vec<Id>> {
    let own = &cat.manifest.project;
    cat.items
        .iter()
        .map(|(id, item)| {
            let kids = item
                .bom
                .iter()
                .filter_map(|l| local(own, &l.item))
                .filter(|c| cat.items.contains_key(*c))
                .cloned()
                .collect();
            (id.clone(), kids)
        })
        .collect()
}

/// The cycles of `edges`, one per edge back to an item still being
/// walked, each rotated to start at its least ID; the list sorted, so
/// the answer doesn't depend on where the walk began.
pub fn cycles(edges: &BTreeMap<Id, Vec<Id>>) -> Vec<Vec<Id>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done,
    }
    let none: Vec<Id> = Vec::new();
    let mut mark: BTreeMap<&Id, Mark> = BTreeMap::new();
    let mut found: BTreeSet<Vec<Id>> = BTreeSet::new();
    for start in edges.keys() {
        if mark.contains_key(start) {
            continue;
        }
        let mut path: Vec<&Id> = vec![start];
        let mut next: Vec<usize> = vec![0];
        mark.insert(start, Mark::Open);
        while let Some(&node) = path.last() {
            let kids = edges.get(node).unwrap_or(&none);
            let i = next.last().copied().unwrap_or(0);
            if i < kids.len() {
                if let Some(n) = next.last_mut() {
                    *n += 1;
                }
                let k = &kids[i];
                match mark.get(k).copied() {
                    None => {
                        mark.insert(k, Mark::Open);
                        path.push(k);
                        next.push(0);
                    }
                    Some(Mark::Open) => {
                        let from = path.iter().position(|n| *n == k).unwrap_or(0);
                        let mut c: Vec<Id> = path[from..].iter().map(|n| (*n).clone()).collect();
                        let least = (0..c.len()).min_by(|a, b| c[*a].cmp(&c[*b])).unwrap_or(0);
                        c.rotate_left(least);
                        found.insert(c);
                    }
                    Some(Mark::Done) => {}
                }
            } else {
                mark.insert(node, Mark::Done);
                path.pop();
                next.pop();
            }
        }
    }
    found.into_iter().collect()
}

/// `bom_cycle` for one cycle, named at its first item.
pub fn cycle_error(c: &[Id]) -> Error {
    let names: Vec<&str> = c.iter().map(Id::as_str).collect();
    Error::new(
        Code::BomCycle,
        format!(
            "items {} name each other in a loop of BOM lines",
            names.join(" -> ")
        ),
    )
    .with("item", &c[0])
    .with("cycle", c)
}

/// Own-line faults, all errors a write refuses: a quantity or unit that
/// doesn't read (`invalid_quantity`), a child this catalogue hasn't
/// (`bom_item_missing`), each at its line, then every cycle of own lines.
pub fn judge(cat: &Catalogue) -> Vec<Error> {
    let own = &cat.manifest.project;
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        for (i, l) in item.bom.iter().enumerate() {
            let at = format!("/bom/{i}");
            if !quantity_ok(&l.quantity) {
                out.push(
                    Error::new(
                        Code::InvalidQuantity,
                        format!(
                            "item {id}: line {} has quantity {:?}: a positive decimal number, in the child's unit",
                            l.line, l.quantity
                        ),
                    )
                    .with("item", id)
                    .with("line", &l.line)
                    .with("pointer", format!("{at}/quantity")),
                );
            }
            if let Some(u) = &l.unit
                && !crate::item::UNITS.contains(&u.as_str())
            {
                out.push(
                    Error::new(
                        Code::InvalidQuantity,
                        format!(
                            "item {id}: line {} has unit {u:?}: each, m, m2, m3, kg or l",
                            l.line
                        ),
                    )
                    .with("item", id)
                    .with("line", &l.line)
                    .with("pointer", format!("{at}/unit")),
                );
            }
            if local(own, &l.item).is_some_and(|c| !cat.items.contains_key(c)) {
                out.push(
                    Error::new(
                        Code::BomItemMissing,
                        format!(
                            "item {id}: line {} names item {}, which the catalogue hasn't",
                            l.line,
                            l.item.text()
                        ),
                    )
                    .with("item", id)
                    .with("line", &l.line)
                    .with("child", l.item.text())
                    .with("pointer", format!("{at}/item")),
                );
            }
        }
    }
    out.extend(cycles(&own_edges(cat)).iter().map(|c| cycle_error(c)));
    out
}

/// What `bom add` and `bom edit` set on a line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineFields {
    /// The child item: its ID or number, or `<catalogue project ID>:<item ID>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// The fields `bom edit --unset` may clear.
const UNSETTABLE: [&str; 4] = ["unit", "find", "refdes", "notes"];

/// The child `s` names: another catalogue's by `<project>:<id>`, else
/// this one's by ID or number.
fn child_of(cat: &Catalogue, s: &str) -> Result<ItemRef> {
    if s.contains(':') {
        return ItemRef::parse(s).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("{s:?} isn't <catalogue project ID>:<item ID>"),
            )
            .with("param", "child")
        });
    }
    Ok(ItemRef {
        catalogue: None,
        item: cat.find(s)?,
    })
}

fn write_item(session: &mut Session, id: &Id, it: &crate::item::Item, what: &str) -> Result<Value> {
    let text = crate::items::text_of(id, it)?;
    commit(session, what, Writes(vec![(item_path(id), Some(text))]))?;
    Ok(crate::items::shown(session, id))
}

/// `bom.add`: a new own line on `item`, with a fresh line ID.
pub fn add(
    session: &mut Session,
    item: &str,
    fields: LineFields,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let child = fields
        .child
        .as_deref()
        .ok_or_else(|| Error::new(Code::Usage, "a line needs its child item"))?;
    let child = child_of(session.catalogue(), child)?;
    let quantity = fields
        .quantity
        .clone()
        .ok_or_else(|| Error::new(Code::Usage, "a line needs its quantity"))?;
    let line = crate::ids::random(|s| it.bom.iter().any(|l| l.line.as_str() == s))?;
    it.bom.push(Line {
        line: line.clone(),
        item: child,
        quantity,
        unit: fields.unit,
        find: fields.find,
        refdes: fields.refdes,
        notes: fields.notes,
    });
    let mut v = write_item(
        session,
        &id,
        &it,
        &format!("adding line {line} to item {id}"),
    )?;
    v["line"] = json!(line);
    Ok(v)
}

fn line_index(id: &Id, it: &crate::item::Item, line: &str) -> Result<usize> {
    it.bom
        .iter()
        .position(|l| l.line.as_str() == line)
        .ok_or_else(|| {
            Error::new(Code::NotFound, format!("item {id} has no line {line}")).with("line", line)
        })
}

/// `bom.edit`: the fields given set on the line, those in `unset` cleared.
pub fn edit(
    session: &mut Session,
    item: &str,
    line: &str,
    fields: LineFields,
    unset: Vec<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let i = line_index(&id, &it, line)?;
    let child = fields
        .child
        .as_deref()
        .map(|c| child_of(session.catalogue(), c))
        .transpose()?;
    let l = &mut it.bom[i];
    if let Some(c) = child {
        l.item = c;
    }
    if let Some(q) = fields.quantity {
        l.quantity = q;
    }
    for (slot, v) in [
        (&mut l.unit, fields.unit),
        (&mut l.find, fields.find),
        (&mut l.refdes, fields.refdes),
        (&mut l.notes, fields.notes),
    ] {
        if v.is_some() {
            *slot = v;
        }
    }
    for u in &unset {
        match u.as_str() {
            "unit" => l.unit = None,
            "find" => l.find = None,
            "refdes" => l.refdes = None,
            "notes" => l.notes = None,
            other => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("can't unset {other:?}: {}", UNSETTABLE.join(", ")),
                )
                .with("field", other));
            }
        }
    }
    write_item(
        session,
        &id,
        &it,
        &format!("editing line {line} of item {id}"),
    )
}

/// `bom.remove`: the line taken off the item.
pub fn remove(
    session: &mut Session,
    item: &str,
    line: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let i = line_index(&id, &it, line)?;
    it.bom.remove(i);
    let mut v = write_item(
        session,
        &id,
        &it,
        &format!("removing line {line} from item {id}"),
    )?;
    v["removed"] = json!(line);
    Ok(v)
}

/// Where a joined line comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// The item's own line, by its line ID.
    Own { line: Id },
    /// A line of the item's representation `product` `project`:`part`,
    /// by the representation's key, read at `rev`.
    Rep {
        product: String,
        project: ProjectId,
        part: Id,
        key: String,
        rev: String,
    },
}

/// One line of an item's BOM, own or read.
#[derive(Clone, Debug, PartialEq)]
pub struct Joined {
    pub origin: Origin,
    /// The child item: an own line's, or the item that links a
    /// representation line's child; `None` when no item links it.
    pub item: Option<ItemRef>,
    /// A representation line's child: product, project (`None` for a
    /// ryueda alias with no project) and part.
    pub child: Option<(String, Option<ProjectId>, Id)>,
    /// As written: an own line's text, a representation's count.
    pub quantity: String,
    pub unit: Option<String>,
    pub find: Option<String>,
    pub refdes: Option<String>,
    pub notes: Option<String>,
}

impl Joined {
    /// What tells it from the item's other lines, at any revision: an own
    /// line's ID, a representation line's product, part and key.
    pub fn key(&self) -> String {
        match &self.origin {
            Origin::Own { line } => format!("own:{line}"),
            Origin::Rep {
                product,
                project,
                part,
                key,
                ..
            } => format!("{product}:{project}:{part}:{key}"),
        }
    }
}

/// The BOM of item `id` (design 734): its own lines, then each link's
/// lines in product order, each child resolved to the item linking it
/// (`linked`, from [`crate::links::linked`]). Faults, all warnings: a
/// link whose lines can't be read (`item_link_unresolved`), the children
/// no item links (`bom_unlinked`, one listing them), and each own line
/// naming an item a representation places too (`bom_line_twice`).
pub fn of(
    cat: &Catalogue,
    linked: &BTreeMap<crate::links::Key, Id>,
    id: &Id,
    reps: &mut dyn Reps,
    at: &At,
) -> (Vec<Joined>, Vec<Fault>) {
    let item = &cat.items[id];
    let own = &cat.manifest.project;
    let mut lines: Vec<Joined> = item
        .bom
        .iter()
        .map(|l| Joined {
            origin: Origin::Own {
                line: l.line.clone(),
            },
            item: Some(l.item.clone()),
            child: None,
            quantity: l.quantity.clone(),
            unit: l.unit.clone(),
            find: l.find.clone(),
            refdes: l.refdes.clone(),
            notes: l.notes.clone(),
        })
        .collect();
    let mut faults = Vec::new();
    let mut unlinked = Vec::new();
    let mut placed: BTreeSet<Id> = BTreeSet::new();
    let mut links: Vec<&crate::item::Link> = item.links.iter().collect();
    links.sort_by(|a, b| a.product.cmp(&b.product));
    for link in links {
        let read = match reps.lines(link, at) {
            Ok(r) => r,
            Err(e) => {
                faults.push(Fault::of(Severity::Warning, e.with("item", id)));
                continue;
            }
        };
        for rl in read.lines {
            let child_item = rl
                .project
                .as_ref()
                .and_then(|p| linked.get(&(link.product.clone(), p.clone(), rl.part.clone())));
            match child_item {
                Some(c) => {
                    placed.insert(c.clone());
                }
                None => unlinked.push(json!({
                    "product": link.product, "project": rl.project, "part": rl.part, "key": rl.key,
                })),
            }
            lines.push(Joined {
                origin: Origin::Rep {
                    product: link.product.clone(),
                    project: link.project.clone(),
                    part: link.part.clone(),
                    key: rl.key.clone(),
                    rev: read.rev.clone(),
                },
                item: child_item.map(|c| ItemRef {
                    catalogue: None,
                    item: c.clone(),
                }),
                child: Some((link.product.clone(), rl.project.clone(), rl.part.clone())),
                quantity: rl.quantity.to_string(),
                unit: None,
                find: rl.find.clone(),
                refdes: rl.refdes.clone(),
                notes: None,
            });
        }
    }
    if !unlinked.is_empty() {
        faults.push(Fault::of(
            Severity::Warning,
            Error::new(
                Code::BomUnlinked,
                format!(
                    "item {id}: {} line(s) of its representations name parts no item links; link them to items",
                    unlinked.len()
                ),
            )
            .with("item", id)
            .with("unlinked", unlinked),
        ));
    }
    for (i, l) in item.bom.iter().enumerate() {
        if let Some(c) = local(own, &l.item)
            && placed.contains(c)
        {
            faults.push(Fault::of(
                Severity::Warning,
                Error::new(
                    Code::BomLineTwice,
                    format!(
                        "item {id}: line {} names item {c}, which its representation places too; remove the line",
                        l.line
                    ),
                )
                .with("item", id)
                .with("line", &l.line)
                .with("child", c)
                .with("pointer", format!("/bom/{i}")),
            ));
        }
    }
    (lines, faults)
}

/// A joined line as answers give it: `from` (`own`, `ryucad`, `ryueda`),
/// its `line` ID or representation `key` and `rev`, the child `item`
/// with its `number` and `name` when this catalogue holds it, the child
/// `representation`, `quantity` and the line's optional fields.
pub fn line_json(cat: &Catalogue, j: &Joined) -> Value {
    let mut v = json!({"quantity": j.quantity});
    match &j.origin {
        Origin::Own { line } => {
            v["from"] = json!("own");
            v["line"] = json!(line);
        }
        Origin::Rep {
            product, key, rev, ..
        } => {
            v["from"] = json!(product);
            v["key"] = json!(key);
            v["rev"] = json!(rev);
        }
    }
    if let Some(r) = &j.item {
        v["item"] = json!(r.text());
        if let Some(c) = local(&cat.manifest.project, r).and_then(|c| cat.items.get(c)) {
            v["number"] = json!(c.number);
            if let Some(n) = &c.name {
                v["name"] = json!(n);
            }
        }
    }
    if let Some((product, project, part)) = &j.child {
        v["representation"] = json!({"product": product, "project": project, "part": part});
    }
    for (k, x) in [
        ("unit", &j.unit),
        ("find", &j.find),
        ("refdes", &j.refdes),
        ("notes", &j.notes),
    ] {
        if let Some(x) = x {
            v[k] = json!(x);
        }
    }
    v
}

/// The catalogue at `rev` (`None` for the working files) and what its
/// representations are read at.
pub fn at_rev(session: &mut Session, rev: Option<&str>) -> Result<(Catalogue, At)> {
    match rev {
        None => Ok((session.catalogue().clone(), At::Working)),
        Some(r) => {
            let (repo, _) = crate::release::history(session)?;
            let commit = crate::reps::commit_hex(repo.gix(), r)?;
            let files =
                ryupdm::history::files_for::<crate::Plm>(repo.gix(), &commit, repo.prefix())?;
            Ok((Catalogue::load(&files)?, At::Commit(commit)))
        }
    }
}

/// The disk reader for the session's catalogue, searching its work tree,
/// `within` and the `pdm.dependents` preference.
pub fn disk(session: &Session, within: &[String]) -> crate::reps::Disk {
    let folders = crate::check::elsewhere(within);
    let tops = crate::reps::tops(session.root(), &folders);
    crate::reps::Disk::new(crate::reps::Index::scan(&tops), session.root())
}

/// Item `id`'s lines as answers give them, each line's child of this
/// catalogue followed `depth - 1` levels deeper (`children`), never
/// round a loop (`path` holds the items above).
#[allow(clippy::too_many_arguments)]
fn tree(
    cat: &Catalogue,
    linked: &BTreeMap<crate::links::Key, Id>,
    id: &Id,
    reps: &mut dyn Reps,
    at: &At,
    depth: usize,
    path: &mut Vec<Id>,
    faults: &mut Vec<Fault>,
) -> Vec<Value> {
    let (lines, f) = of(cat, linked, id, reps, at);
    faults.extend(f);
    let mut out = Vec::new();
    for j in &lines {
        let mut v = line_json(cat, j);
        let deeper = j
            .item
            .as_ref()
            .and_then(|r| local(&cat.manifest.project, r))
            .filter(|c| cat.items.contains_key(*c) && !path.contains(c))
            .cloned();
        if depth > 1
            && let Some(c) = deeper
        {
            path.push(c.clone());
            v["children"] = json!(tree(cat, linked, &c, reps, at, depth - 1, path, faults));
            path.pop();
        }
        out.push(v);
    }
    out
}

/// `bom.get`: `{schema, item, number, rev, lines, findings}`, the item's
/// BOM at `rev` (working by default) to `depth` levels (1 by default).
pub fn show(
    session: &mut Session,
    item: &str,
    rev: Option<&str>,
    depth: Option<usize>,
    within: &[String],
) -> Result<Value> {
    let (cat, at) = at_rev(session, rev)?;
    let id = cat.find(item)?;
    let mut reps = disk(session, within);
    let linked = crate::links::linked(&cat);
    let mut faults = Vec::new();
    let lines = tree(
        &cat,
        &linked,
        &id,
        &mut reps,
        &at,
        depth.unwrap_or(1).max(1),
        &mut vec![id.clone()],
        &mut faults,
    );
    Ok(json!({
        "schema": SCHEMA,
        "item": id,
        "number": cat.items[&id].number,
        "rev": rev.unwrap_or("working"),
        "lines": lines,
        "findings": crate::fault::list(&faults),
    }))
}

/// Every linked item's joined findings ([`of`]), then each cycle that
/// representation lines close and own lines alone don't (`bom_cycle`, an
/// error: own-line cycles are [`judge`]'s).
pub fn joined(cat: &Catalogue, reps: &mut dyn Reps, at: &At) -> Vec<Fault> {
    let linked = crate::links::linked(cat);
    let own_cycles: BTreeSet<Vec<Id>> = cycles(&own_edges(cat)).into_iter().collect();
    let mut edges = own_edges(cat);
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        if item.links.is_empty() {
            continue;
        }
        let (lines, faults) = of(cat, &linked, id, reps, at);
        out.extend(faults);
        let kids = edges.entry(id.clone()).or_default();
        for j in &lines {
            if let (Origin::Rep { .. }, Some(r)) = (&j.origin, &j.item)
                && let Some(c) = local(&cat.manifest.project, r)
            {
                kids.push(c.clone());
            }
        }
    }
    for c in cycles(&edges) {
        if !own_cycles.contains(&c) {
            out.push(Fault::of(Severity::Error, cycle_error(&c)));
        }
    }
    out
}

/// `bom_item_missing`, a warning, for each own line naming another
/// catalogue's item that no catalogue in `others` holds.
pub fn foreign_children(cat: &Catalogue, others: &[PathBuf]) -> Vec<Fault> {
    let mut held: BTreeMap<ProjectId, BTreeSet<Id>> = BTreeMap::new();
    for p in others {
        if let Ok(c) = Catalogue::load(&crate::source::Folder(p)) {
            held.entry(c.manifest.project.clone())
                .or_insert_with(|| c.items.keys().cloned().collect());
        }
    }
    let own = &cat.manifest.project;
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        for (i, l) in item.bom.iter().enumerate() {
            let Some(p) = l.item.catalogue.as_ref().filter(|p| *p != own) else {
                continue;
            };
            let why = match held.get(p) {
                None => format!("catalogue {p} isn't in any folder searched"),
                Some(ids) if !ids.contains(&l.item.item) => {
                    format!("catalogue {p} has no item {}", l.item.item)
                }
                Some(_) => continue,
            };
            out.push(Fault::of(
                Severity::Warning,
                Error::new(
                    Code::BomItemMissing,
                    format!("item {id}: line {} names {}: {why}", l.line, l.item.text()),
                )
                .with("item", id)
                .with("line", &l.line)
                .with("child", l.item.text())
                .with("pointer", format!("/bom/{i}/item")),
            ));
        }
    }
    out
}

/// What `check` adds for structure: links followed on disk
/// ([`crate::links::resolve`]), the joined findings ([`joined`]; a link
/// `resolve` already found unresolved isn't reported twice), and other
/// catalogues' missing children ([`foreign_children`]), representations
/// read at `at`.
pub fn findings(
    cat: &Catalogue,
    index: &crate::reps::Index,
    reps: &mut dyn Reps,
    others: &[PathBuf],
    at: &At,
) -> Vec<Fault> {
    let detail = |f: &Fault, k: &str| f.error.details[k].as_str().unwrap_or("").to_owned();
    let mut out = crate::links::resolve(cat, index);
    let told: BTreeSet<(String, String)> = out
        .iter()
        .filter(|f| f.error.code == Code::ItemLinkUnresolved)
        .map(|f| (detail(f, "item"), detail(f, "product")))
        .collect();
    out.extend(joined(cat, reps, at).into_iter().filter(|f| {
        f.error.code != Code::ItemLinkUnresolved
            || !told.contains(&(detail(f, "item"), detail(f, "product")))
    }));
    out.extend(foreign_children(cat, others));
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::item::{Item, ItemRef, Line};
    use crate::judge::tests::{codes, files};

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "item").unwrap()
    }

    /// An own line `n` (its ID `000000000000<n>`) naming `child`.
    pub(crate) fn line(n: u8, child: &str, quantity: &str) -> Line {
        Line {
            line: id(&format!("000000000000{n}")),
            item: ItemRef::parse(child).unwrap(),
            quantity: quantity.into(),
            unit: None,
            find: None,
            refdes: None,
            notes: None,
        }
    }

    #[test]
    fn quantities_read_as_positive_decimals() {
        for q in ["1", "4", "0.5", "12.250", "10"] {
            assert!(quantity_ok(q), "{q}");
        }
        for q in ["0", "0.0", "", "-1", "1.", ".5", "1e3", "2 mm", " 1"] {
            assert!(!quantity_ok(q), "{q}");
        }
    }

    #[test]
    fn own_lines_are_judged_in_the_catalogue() {
        let mut a = Item::new("A-1");
        a.bom = vec![
            line(1, "0000000000002", "4"),
            line(2, "0000000000009", "1"), // not in the catalogue
            line(3, "0000000000002", "0"), // bad quantity
        ];
        a.bom[0].unit = Some("pcs".into()); // bad unit
        let mut b = Item::new("A-2");
        // Another catalogue's item: check's business, not a write's.
        b.bom = vec![line(
            1,
            "0123456789abcdef0123456789abcdef:0000000000009",
            "1",
        )];
        let f = files(&[], None, &[("0000000000001", a), ("0000000000002", b)]);
        assert_eq!(
            codes(&f),
            [
                ("invalid_quantity".into(), "error", "0000000000001".into()),
                ("bom_item_missing".into(), "error", "0000000000001".into()),
                ("invalid_quantity".into(), "error", "0000000000001".into()),
            ]
        );
    }

    #[test]
    fn a_loop_of_own_lines_is_one_cycle() {
        let mut a = Item::new("A-1");
        a.bom = vec![line(1, "0000000000002", "1")];
        let mut b = Item::new("A-2");
        b.bom = vec![line(1, "0000000000003", "1")];
        let mut c = Item::new("A-3");
        c.bom = vec![line(1, "0000000000001", "1")];
        let mut s = Item::new("A-4");
        s.bom = vec![line(1, "0000000000004", "1")]; // itself
        let f = files(
            &[],
            None,
            &[
                ("0000000000002", b),
                ("0000000000003", c),
                ("0000000000001", a),
                ("0000000000004", s),
            ],
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let found = cycles(&own_edges(&cat));
        assert_eq!(
            found,
            [
                vec![
                    id("0000000000001"),
                    id("0000000000002"),
                    id("0000000000003")
                ],
                vec![id("0000000000004")],
            ]
        );
        assert_eq!(
            codes(&f),
            [
                ("bom_cycle".into(), "error", "0000000000001".into()),
                ("bom_cycle".into(), "error", "0000000000004".into()),
            ]
        );
    }

    #[test]
    fn own_lines_from_add_to_remove() {
        use crate::proto::Request;
        use crate::server::tests::{ask, catalogue};
        let (_d, root) = catalogue();
        for n in ["KIT-1", "SCREW-1", "LABEL-1"] {
            ask(
                &root,
                Request::ItemCreate {
                    fields: Default::default(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        let fields = |v: serde_json::Value| -> LineFields { serde_json::from_value(v).unwrap() };
        let added = ask(
            &root,
            Request::BomAdd {
                item: "KIT-1".into(),
                fields: fields(
                    serde_json::json!({"child": "SCREW-1", "quantity": "4", "find": "10"}),
                ),
                expected_revision: None,
            },
        )
        .unwrap();
        let line = added["line"].as_str().unwrap().to_owned();
        assert_eq!(added["item"]["bom"][0]["quantity"], "4");
        assert_eq!(added["item"]["bom"][0]["find"], "10");
        // A bad quantity is refused by the judge, at its pointer.
        let bad = ask(
            &root,
            Request::BomAdd {
                item: "KIT-1".into(),
                fields: fields(serde_json::json!({"child": "LABEL-1", "quantity": "lots"})),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(bad.code, Code::InvalidQuantity);
        // A loop is refused.
        let lp = ask(
            &root,
            Request::BomAdd {
                item: "SCREW-1".into(),
                fields: fields(serde_json::json!({"child": "KIT-1", "quantity": "1"})),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(lp.code, Code::BomCycle);
        // The child can't be deleted while a line names it.
        let del = ask(
            &root,
            Request::ItemDelete {
                item: "SCREW-1".into(),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(del.code, Code::BomItemMissing);
        let edited = ask(
            &root,
            Request::BomEdit {
                item: "KIT-1".into(),
                line: line.clone(),
                fields: fields(serde_json::json!({"quantity": "6", "notes": "M3x8"})),
                unset: vec!["find".into()],
                expected_revision: None,
            },
        )
        .unwrap();
        let l = &edited["item"]["bom"][0];
        assert_eq!(
            (l["quantity"].clone(), l["notes"].clone()),
            (serde_json::json!("6"), serde_json::json!("M3x8"))
        );
        assert!(l.get("find").is_none(), "{l}");
        let removed = ask(
            &root,
            Request::BomRemove {
                item: "KIT-1".into(),
                line: line.clone(),
                expected_revision: None,
            },
        )
        .unwrap();
        assert!(removed["item"].get("bom").is_none(), "{removed}");
        let gone = ask(
            &root,
            Request::BomRemove {
                item: "KIT-1".into(),
                line,
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(gone.code, Code::NotFound);
    }

    use crate::catalogue::tests::pid;
    use crate::reps::{At, Fixed, RepLine};

    pub(crate) fn link(product: &str, project: u8, part: &str) -> crate::item::Link {
        crate::item::Link {
            product: product.into(),
            project: pid(project),
            part: Id::parse(part, "part").unwrap(),
        }
    }

    pub(crate) fn rep(project: Option<u8>, part: &str, quantity: u64) -> RepLine {
        RepLine {
            key: part.into(),
            project: project.map(pid),
            part: Id::parse(part, "part").unwrap(),
            quantity,
            find: None,
            refdes: None,
        }
    }

    /// A PCBA item 1 linked to a ryucad assembly `box` and a ryueda design
    /// `main`, with own lines (glue, item 4; and the screw, item 2, which
    /// the assembly places too); item 2 linked to ryucad `screw`, item 3
    /// to ryueda `mcu` of library project 5.
    pub(crate) fn pcba() -> (std::collections::BTreeMap<String, Vec<u8>>, Fixed) {
        let mut a = Item::new("PCBA-1");
        a.links = vec![link("ryucad", 3, "box"), link("ryueda", 4, "main")];
        a.bom = vec![
            line(1, "0000000000004", "0.5"),
            line(2, "0000000000002", "4"),
        ];
        a.bom[0].unit = Some("kg".into());
        let mut screw = Item::new("SCREW-1");
        screw.links = vec![link("ryucad", 3, "screw")];
        let mut mcu = Item::new("MCU-1");
        mcu.links = vec![link("ryueda", 5, "mcu")];
        let f = files(
            &[],
            None,
            &[
                ("0000000000001", a),
                ("0000000000002", screw),
                ("0000000000003", mcu),
                ("0000000000004", Item::new("GLUE-1")),
            ],
        );
        let mut reps = Fixed::default();
        reps.0.insert(
            ("ryucad".into(), pid(3), Id::parse("box", "part").unwrap()),
            vec![rep(Some(3), "screw", 4), rep(Some(3), "lid", 1)],
        );
        reps.0.insert(
            ("ryueda".into(), pid(4), Id::parse("main", "part").unwrap()),
            vec![rep(Some(5), "mcu", 1), rep(None, "c100n", 2)],
        );
        for (p, n) in [("ryucad", 3), ("ryueda", 5)] {
            let part = if p == "ryucad" { "screw" } else { "mcu" };
            reps.0
                .insert((p.into(), pid(n), Id::parse(part, "part").unwrap()), vec![]);
        }
        (f, reps)
    }

    #[test]
    fn the_bom_is_own_lines_then_each_representations() {
        let (f, mut reps) = pcba();
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let linked = crate::links::linked(&cat);
        let (lines, faults) = of(&cat, &linked, &id("0000000000001"), &mut reps, &At::Working);
        let shown: Vec<serde_json::Value> = lines.iter().map(|j| line_json(&cat, j)).collect();
        let got: Vec<(String, String, String)> = shown
            .iter()
            .map(|v| {
                (
                    v["from"].as_str().unwrap().to_owned(),
                    v["number"].as_str().unwrap_or("-").to_owned(),
                    v["quantity"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("own".into(), "GLUE-1".into(), "0.5".into()),
                ("own".into(), "SCREW-1".into(), "4".into()),
                ("ryucad".into(), "SCREW-1".into(), "4".into()),
                ("ryucad".into(), "-".into(), "1".into()),
                ("ryueda".into(), "MCU-1".into(), "1".into()),
                ("ryueda".into(), "-".into(), "2".into()),
            ]
        );
        assert_eq!(shown[0]["unit"], "kg");
        assert_eq!(shown[2]["rev"], "working");
        assert_eq!(shown[3]["representation"]["part"], "lid");
        let codes: Vec<String> = faults
            .iter()
            .map(|f| {
                serde_json::to_value(f.error.code)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(codes, ["bom_unlinked", "bom_line_twice"]);
        assert_eq!(
            faults[0].error.details["unlinked"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(faults[1].error.details["pointer"], "/bom/1");
        assert!(faults.iter().all(|f| !f.is_error()));
    }

    #[cfg(feature = "cad")]
    #[test]
    fn bom_show_reads_the_linked_assembly_and_goes_deeper_on_asking() {
        use crate::proto::Request;
        use crate::server::tests::{ask, catalogue};
        let (_d, root) = catalogue();
        crate::reps::tests::cad_project(
            &root.join("mech"),
            &pid(3),
            &["screw"],
            &[("box", &[("screw", 4)])],
        );
        let mk = |n: &str| {
            ask(
                &root,
                Request::ItemCreate {
                    fields: Default::default(),
                    number: Some(n.into()),
                    next: None,
                    expected_revision: None,
                },
            )
            .unwrap()
        };
        mk("BOX-1");
        mk("SCREW-1");
        mk("THREADLOCK-1");
        for (item, part) in [("BOX-1", "box"), ("SCREW-1", "screw")] {
            ask(
                &root,
                Request::ItemLink {
                    item: item.into(),
                    product: "ryucad".into(),
                    project: pid(3).to_string(),
                    part: part.into(),
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        ask(
            &root,
            Request::BomAdd {
                item: "SCREW-1".into(),
                fields: serde_json::from_value(
                    serde_json::json!({"child": "THREADLOCK-1", "quantity": "0.01", "unit": "l"}),
                )
                .unwrap(),
                expected_revision: None,
            },
        )
        .unwrap();
        let show = |depth: Option<usize>| {
            ask(
                &root,
                Request::BomGet {
                    item: "BOX-1".into(),
                    rev: None,
                    depth,
                    within: vec![],
                },
            )
            .unwrap()
        };
        let one = show(None);
        assert_eq!(one["schema"], 1);
        assert_eq!(one["lines"].as_array().unwrap().len(), 1, "{one}");
        assert_eq!(one["lines"][0]["number"], "SCREW-1");
        assert_eq!(one["lines"][0]["quantity"], "4");
        assert_eq!(one["findings"], serde_json::json!([]));
        assert!(one["lines"][0].get("children").is_none());
        let two = show(Some(2));
        assert_eq!(two["lines"][0]["children"][0]["number"], "THREADLOCK-1");
    }

    #[test]
    fn check_finds_what_only_representations_show() {
        let (mut f, mut reps) = pcba();
        // MCU-1's design places PCBA-1: a loop only representation lines close.
        reps.0.insert(
            ("ryueda".into(), pid(5), Id::parse("mcu", "part").unwrap()),
            vec![rep(Some(4), "main", 1)],
        );
        // An own line naming another catalogue's item no folder holds.
        let mut glue: Item = crate::item::Item::read(
            &id("0000000000004"),
            &f[&crate::ids::item_path(&id("0000000000004"))],
        )
        .unwrap();
        glue.bom = vec![line(
            1,
            "0123456789abcdef0123456789abcdef:0000000000009",
            "1",
        )];
        f.insert(
            crate::ids::item_path(&id("0000000000004")),
            glue.to_text().into_bytes(),
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let mut got: Vec<(String, bool, String)> = joined(&cat, &mut reps, &At::Working)
            .into_iter()
            .chain(foreign_children(&cat, &[]))
            .map(|x| {
                (
                    serde_json::to_value(x.error.code)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    x.is_error(),
                    x.error.details["item"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("bom_cycle".into(), true, "0000000000001".into()),
                ("bom_item_missing".into(), false, "0000000000004".into()),
                ("bom_line_twice".into(), false, "0000000000001".into()),
                ("bom_unlinked".into(), false, "0000000000001".into()),
            ]
        );
    }
}
