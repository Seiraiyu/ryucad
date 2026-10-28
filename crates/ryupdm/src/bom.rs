//! Bills of materials, from files alone ([`crate::structure`]): an
//! assembly's indented BOM (each assembly's children by part and
//! revision, with how many each parent places) and its flat BOM (every
//! leaf with its total), and what makes either unsound.
//!
//! A leaf is a part, or an assembly bought whole (`make_buy: buy`), whose
//! children the indented BOM still lists, marked `bought`, but the flat
//! BOM doesn't sum: you buy the motor, not its screws. A part counted in
//! anything but `each` is quantified by its `amount` per occurrence.

use crate::structure::{Node, Reader, occurrences};
use ryucad_doc::doc::assembly::{Rev, snap_origin};
use ryucad_doc::doc::files::PartKind;
use ryucad_doc::doc::id::Id;
use ryucad_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The version of the shape `bom`, `bom diff` and `where-used` answer
/// with. Adding a field is not a change; changing or removing one is.
pub const SCHEMA: u64 = 1;

/// One row of an indented BOM: a part at a revision, as its parent
/// places it.
#[derive(Clone, Debug)]
pub struct Row {
    pub part: Id,
    pub rev: Rev,
    /// How many times its parent places it (1 for the root).
    pub count: u64,
    /// The instances and copies that place it, as paths in the parent.
    pub instances: Vec<String>,
    /// Its files, or why they can't be read.
    pub node: Result<Arc<Node>>,
    /// It lies inside an assembly bought whole.
    pub bought: bool,
    /// What it places, when it is an assembly.
    pub children: Vec<Row>,
    /// Its children were left out below the depth asked for.
    pub truncated: bool,
    /// Its own features that place nothing that can be read.
    pub problems: Vec<Error>,
}

impl Row {
    /// It is bought whole: `make_buy` is `buy`.
    pub fn buy(&self) -> bool {
        self.node
            .as_ref()
            .is_ok_and(|n| n.text("make_buy") == Some("buy"))
    }

    pub fn is_assembly(&self) -> bool {
        self.node
            .as_ref()
            .is_ok_and(|n| n.kind() == Some(PartKind::Assembly))
    }

    /// The flat BOM sums it rather than what it places.
    pub fn is_leaf(&self) -> bool {
        !self.is_assembly() || self.buy()
    }

    /// Its unit of measure: `each` unless its properties say otherwise.
    pub fn uom(&self) -> String {
        self.node
            .as_ref()
            .ok()
            .and_then(|n| n.text("uom"))
            .unwrap_or("each")
            .to_owned()
    }

    /// How much of its unit of measure one occurrence uses: 1 for `each`,
    /// its `amount` otherwise; `None` when that is missing or unreadable.
    pub fn per(&self) -> Option<f64> {
        let uom = self.uom();
        if uom == "each" {
            return Some(1.0);
        }
        let n = self.node.as_ref().ok()?;
        crate::props::amount(n.text("amount")?, &uom).ok()
    }
}

/// The indented BOM of `part` at `rev`: its children and theirs, to
/// `depth` levels below it (all of them for `None`). Fails only when the
/// root can't be read; any child that can't is a row with its error.
pub fn tree(reader: &mut Reader, part: &Id, rev: &Rev, depth: Option<usize>) -> Result<Row> {
    let node = reader.node(part, rev)?;
    let mut root = Row {
        part: part.clone(),
        rev: rev.clone(),
        count: 1,
        instances: Vec::new(),
        node: Ok(node),
        bought: false,
        children: Vec::new(),
        truncated: false,
        problems: Vec::new(),
    };
    let mut stack = vec![(part.clone(), rev.clone())];
    expand(
        reader,
        &mut root,
        &mut stack,
        depth,
        matches!(rev, Rev::Label(_)),
    );
    Ok(root)
}

fn expand(
    reader: &mut Reader,
    row: &mut Row,
    stack: &mut Vec<(Id, Rev)>,
    depth: Option<usize>,
    released: bool,
) {
    if !row.is_assembly() {
        return;
    }
    if depth == Some(0) {
        row.truncated = true;
        return;
    }
    let node = row.node.as_ref().expect("an assembly reads").clone();
    let (occ, problems) = occurrences(&node.part);
    row.problems = problems
        .into_iter()
        .map(|e| e.with("part", &row.part))
        .collect();
    // By part and revision, in the order they first appear.
    let mut groups: Vec<((Id, Rev), Vec<String>)> = Vec::new();
    for o in occ {
        let key = (o.part, o.rev);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, paths)) => paths.push(o.path),
            None => groups.push((key, vec![o.path])),
        }
    }
    let bought = row.bought || row.buy();
    for ((part, rev), instances) in groups {
        let node = if released && rev == Rev::Working {
            Err(Error::new(
                Code::InstanceNotReleased,
                format!(
                    "{} at {} places {part} as it is (working): a release holds only releases",
                    row.part,
                    row.rev.text()
                ),
            )
            .with("part", &part)
            .with("instances", &instances))
        } else if stack.contains(&(part.clone(), rev.clone())) {
            let path: Vec<String> = stack
                .iter()
                .map(|(p, r)| format!("{p} ({})", r.text()))
                .collect();
            Err(Error::new(
                Code::AssemblyCycle,
                format!(
                    "{part} ({}) places itself: {}",
                    rev.text(),
                    path.join(" > ")
                ),
            )
            .with("part", &part)
            .with("path", &path))
        } else {
            reader.node(&part, &rev)
        };
        let mut child = Row {
            part: part.clone(),
            rev: rev.clone(),
            count: instances.len() as u64,
            instances,
            node,
            bought,
            children: Vec::new(),
            truncated: false,
            problems: Vec::new(),
        };
        stack.push((part, rev.clone()));
        expand(
            reader,
            &mut child,
            stack,
            depth.map(|d| d - 1),
            released || matches!(rev, Rev::Label(_)),
        );
        stack.pop();
        row.children.push(child);
    }
}

/// One row of a flat BOM: a leaf, at a revision, with its total.
#[derive(Clone, Debug)]
pub struct Flat {
    /// The row as first met, walking the tree in order; its `count` is
    /// that place's, not the total.
    pub row: Row,
    /// How many occurrences every path from the root makes.
    pub total: u64,
}

/// The flat BOM of the indented `root` (read with no depth limit): every
/// leaf by part and revision with its total, sorted by part number (parts
/// without one last), then part, then revision.
pub fn flat(root: &Row) -> Vec<Flat> {
    let mut out: Vec<Flat> = Vec::new();
    fn walk(row: &Row, times: u64, out: &mut Vec<Flat>) {
        if row.is_leaf() {
            let total = times * row.count;
            match out
                .iter_mut()
                .find(|f| f.row.part == row.part && f.row.rev == row.rev)
            {
                Some(f) => f.total += total,
                None => out.push(Flat {
                    row: Row {
                        children: Vec::new(),
                        ..row.clone()
                    },
                    total,
                }),
            }
            return;
        }
        for c in &row.children {
            walk(c, times * row.count, out);
        }
    }
    walk(root, 1, &mut out);
    let number = |f: &Flat| {
        f.row
            .node
            .as_ref()
            .ok()
            .and_then(|n| n.text("number").map(str::to_owned))
    };
    out.sort_by(|a, b| {
        let (na, nb) = (number(a), number(b));
        (na.is_none(), na, &a.row.part, &a.row.rev).cmp(&(
            nb.is_none(),
            nb,
            &b.row.part,
            &b.row.rev,
        ))
    });
    out
}

/// A quantity as output gives it: the count, and for a part counted in
/// anything but `each`, `count × amount` in its unit of measure.
pub fn quantity(row: &Row, count: u64) -> Value {
    let uom = row.uom();
    let mut v = json!({"count": count, "uom": uom});
    if uom != "each"
        && let Some(per) = row.per()
    {
        let value = snap_origin(per * count as f64);
        v["value"] = json!(value);
        v["text"] = json!(format!("{value} {uom}"));
    }
    v
}

/// What makes the BOM under `root` unsound, as `check` findings: a part
/// counted in anything but `each` with no `amount` (`bom_amount_missing`),
/// and an assembly placing one part at two revisions
/// (`bom_mixed_revisions`). Each once, in the order the walk meets them.
pub fn findings(root: &Row) -> Vec<Value> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    fn walk(row: &Row, out: &mut Vec<Value>, seen: &mut Vec<String>) {
        if row.uom() != "each" && row.per().is_none() {
            let key = format!("amount\0{}\0{:?}", row.part, row.rev);
            if !seen.contains(&key) {
                seen.push(key);
                out.push(json!({
                    "code": "bom_amount_missing", "severity": "warning", "source": "bom",
                    "part": row.part, "rev": row.rev.to_json(),
                    "message": format!("{} is counted in {} but has no amount, so a BOM can only count it: set its amount (how much one use takes)", row.part, row.uom()),
                }));
            }
        }
        let mut revs: BTreeMap<&Id, Vec<&Rev>> = BTreeMap::new();
        for c in &row.children {
            revs.entry(&c.part).or_default().push(&c.rev);
        }
        for (part, rs) in revs {
            let key = format!("mixed\0{}\0{:?}\0{part}", row.part, row.rev);
            if rs.len() > 1 && !seen.contains(&key) {
                seen.push(key);
                let texts: Vec<String> = rs.iter().map(|r| r.text()).collect();
                out.push(json!({
                    "code": "bom_mixed_revisions", "severity": "warning", "source": "bom",
                    "part": row.part, "child": part,
                    "revs": rs.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
                    "message": format!("{} places {part} at {}: one item, two revisions", row.part, texts.join(" and ")),
                }));
            }
        }
        for c in &row.children {
            walk(c, out, seen);
        }
    }
    walk(root, &mut out, &mut seen);
    out
}

/// Everything under `root` that couldn't be read, as errors with the
/// part that holds them: rows' own errors and assemblies' features.
pub fn problems(root: &Row) -> Vec<Error> {
    let mut out = Vec::new();
    fn walk(row: &Row, out: &mut Vec<Error>) {
        if let Err(e) = &row.node {
            out.push(e.clone());
        }
        out.extend(row.problems.iter().cloned());
        for c in &row.children {
            walk(c, out);
        }
    }
    walk(root, &mut out);
    out
}

/// What a row says about its part, whatever its place: its revision,
/// what it is, its properties as written, where it was read.
fn about(row: &Row) -> Value {
    let mut v = json!({"part": row.part, "rev": row.rev.to_json(), "uom": row.uom()});
    match &row.node {
        Ok(n) => {
            v["kind"] = json!(match n.kind() {
                Some(PartKind::Assembly) => "assembly",
                Some(PartKind::Part) => "part",
                None => "unknown",
            });
            v["make_buy"] = json!(n.text("make_buy").unwrap_or("make"));
            v["properties"] = match &n.properties {
                Ok(p) => json!(p.values),
                Err(_) => json!({}),
            };
            if let Err(e) = &n.properties {
                v["problem"] = json!(e);
            }
            if let Err(e) = &n.part.file {
                v["problem"] = json!(e);
            }
            if let Some(c) = &n.commit {
                v["commit"] = json!(c);
            }
            if n.obsolete {
                v["obsolete"] = json!(true);
            }
        }
        Err(e) => {
            v["kind"] = json!("unknown");
            v["problem"] = json!(e);
        }
    }
    if row.bought {
        v["bought"] = json!(true);
    }
    v
}

/// An indented row as output gives it, `states` giving the state of each
/// part read at `working`.
pub fn row_json(row: &Row, states: &BTreeMap<Id, Value>) -> Value {
    let mut v = about(row);
    v["quantity"] = quantity(row, row.count);
    v["instances"] = json!(row.instances);
    if row.rev == Rev::Working
        && let Some(s) = states.get(&row.part)
    {
        v["state"] = s.clone();
    }
    if row.is_assembly() {
        v["children"] = row.children.iter().map(|c| row_json(c, states)).collect();
    }
    if row.truncated {
        v["truncated"] = json!(true);
    }
    if !row.problems.is_empty() {
        v["problems"] = json!(row.problems);
    }
    v
}

/// A flat row as output gives it.
pub fn flat_json(f: &Flat, states: &BTreeMap<Id, Value>) -> Value {
    let mut v = about(&f.row);
    v["quantity"] = quantity(&f.row, f.total);
    if f.row.rev == Rev::Working
        && let Some(s) = states.get(&f.row.part)
    {
        v["state"] = s.clone();
    }
    v
}

/// The CSV columns before the declared properties.
pub const CSV_COLUMNS: [&str; 11] = [
    "level",
    "item",
    "number",
    "name",
    "part",
    "revision",
    "quantity",
    "uom",
    "make_buy",
    "material",
    "description",
];

/// One field as RFC 4180 writes it: quoted when it holds a comma, a
/// quote or a line break, quotes doubled.
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// The declared properties' names, in the order the settings give them.
fn custom(config: &crate::config::Config) -> Vec<String> {
    config
        .decls()
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| !crate::config::CORE.contains(&n.as_str()))
        .collect()
}

fn csv_line(level: &str, row: &Row, count: u64, custom: &[String]) -> String {
    let props = row
        .node
        .as_ref()
        .ok()
        .and_then(|n| n.properties.as_ref().ok());
    let text = |name: &str| match props.and_then(|p| p.values.get(name)) {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    let quantity = match quantity(row, count) {
        q if row.uom() == "each" => q["count"].to_string(),
        q => q.get("value").map(Value::to_string).unwrap_or_default(),
    };
    let rev = match &row.rev {
        Rev::Working => "working".to_owned(),
        Rev::Label(l) => l.clone(),
    };
    let make_buy = match text("make_buy") {
        s if s.is_empty() && row.node.is_ok() => "make".to_owned(),
        s => s,
    };
    let mut fields = vec![
        level.to_owned(),
        String::new(),
        text("number"),
        text("name"),
        row.part.to_string(),
        rev,
        quantity,
        row.uom(),
        make_buy,
        text("material"),
        text("description"),
    ];
    fields.extend(custom.iter().map(|c| text(c)));
    let mut out = fields
        .iter()
        .map(|f| field(f))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    out
}

fn csv_header(custom: &[String]) -> String {
    let mut h: Vec<String> = CSV_COLUMNS.iter().map(|c| (*c).to_owned()).collect();
    h.extend(custom.iter().cloned());
    let mut out = h.iter().map(|f| field(f)).collect::<Vec<_>>().join(",");
    out.push('\n');
    out
}

/// The indented BOM under `root` as CSV: a header, then every row below
/// the root in order, its `level` its place (`1`, `1.2`, `1.2.1`).
pub fn csv_tree(root: &Row, config: &crate::config::Config) -> String {
    let custom = custom(config);
    let mut out = csv_header(&custom);
    fn walk(row: &Row, at: &str, custom: &[String], out: &mut String) {
        for (i, c) in row.children.iter().enumerate() {
            let level = if at.is_empty() {
                format!("{}", i + 1)
            } else {
                format!("{at}.{}", i + 1)
            };
            out.push_str(&csv_line(&level, c, c.count, custom));
            walk(c, &level, custom, out);
        }
    }
    walk(root, "", &custom, &mut out);
    out
}

/// The flat BOM as CSV: a header, then each row, `level` left empty.
pub fn csv_flat(rows: &[Flat], config: &crate::config::Config) -> String {
    let custom = custom(config);
    let mut out = csv_header(&custom);
    for f in rows {
        out.push_str(&csv_line("", &f.row, f.total, &custom));
    }
    out
}

/// One side of a comparison: a row with the count it is compared by.
#[derive(Clone, Copy)]
struct Side<'a> {
    row: &'a Row,
    count: u64,
}

/// Pairs rows by part: per part, equal revisions first, then the rest in
/// revision order; the rest of each side is added or removed.
fn pair<'a>(
    from: Vec<Side<'a>>,
    to: Vec<Side<'a>>,
) -> (Vec<Side<'a>>, Vec<Side<'a>>, Vec<(Side<'a>, Side<'a>)>) {
    let mut parts: Vec<&Id> = from.iter().chain(&to).map(|s| &s.row.part).collect();
    parts.sort();
    parts.dedup();
    let (mut added, mut removed, mut both) = (Vec::new(), Vec::new(), Vec::new());
    for part in parts {
        let mut a: Vec<Side> = from
            .iter()
            .filter(|s| &s.row.part == part)
            .copied()
            .collect();
        let mut b: Vec<Side> = to.iter().filter(|s| &s.row.part == part).copied().collect();
        a.sort_by(|x, y| x.row.rev.cmp(&y.row.rev));
        b.sort_by(|x, y| x.row.rev.cmp(&y.row.rev));
        let mut rest_a = Vec::new();
        for x in a {
            match b.iter().position(|y| y.row.rev == x.row.rev) {
                Some(j) => both.push((x, b.remove(j))),
                None => rest_a.push(x),
            }
        }
        let n = rest_a.len().min(b.len());
        let tail_b = b.split_off(n);
        let tail_a = rest_a.split_off(n);
        both.extend(rest_a.into_iter().zip(b));
        removed.extend(tail_a);
        added.extend(tail_b);
    }
    (added, removed, both)
}

fn properties(row: &Row) -> BTreeMap<String, Value> {
    row.node
        .as_ref()
        .ok()
        .and_then(|n| n.properties.as_ref().ok())
        .map(|p| p.values.clone())
        .unwrap_or_default()
}

/// What changed between two sides of one part, `None` when nothing did.
fn change(a: Side, b: Side, config: &crate::config::Config) -> Option<Value> {
    let mut v = json!({"part": b.row.part});
    let mut changed = false;
    if a.row.rev != b.row.rev {
        v["rev"] = json!({"from": a.row.rev.to_json(), "to": b.row.rev.to_json()});
        changed = true;
    } else {
        v["rev"] = b.row.rev.to_json();
    }
    let (qa, qb) = (quantity(a.row, a.count), quantity(b.row, b.count));
    if qa != qb {
        v["quantity"] = json!({"from": qa, "to": qb});
        changed = true;
    }
    let (pa, pb) = (properties(a.row), properties(b.row));
    let mut props = serde_json::Map::new();
    for k in pa.keys().chain(pb.keys()) {
        let (x, y) = (pa.get(k), pb.get(k));
        if x != y && !props.contains_key(k) {
            let revising = config.decl(k).is_none_or(|d| d.revising);
            props.insert(k.clone(), json!({"from": x, "to": y, "revising": revising}));
        }
    }
    if !props.is_empty() {
        v["properties"] = Value::Object(props);
        changed = true;
    }
    changed.then_some(v)
}

/// The changes between two lists of rows: `{added, removed, changed}`.
fn compare(from: Vec<Side>, to: Vec<Side>, config: &crate::config::Config) -> Value {
    let (added, removed, both) = pair(from, to);
    let whole = |s: &Side| {
        let mut v = about(s.row);
        v["quantity"] = quantity(s.row, s.count);
        v
    };
    json!({
        "added": added.iter().map(whole).collect::<Vec<_>>(),
        "removed": removed.iter().map(whole).collect::<Vec<_>>(),
        "changed": both.iter().filter_map(|(a, b)| change(*a, *b, config)).collect::<Vec<_>>(),
    })
}

fn sides(f: &[Flat]) -> Vec<Side<'_>> {
    f.iter()
        .map(|f| Side {
            row: &f.row,
            count: f.total,
        })
        .collect()
}

fn kids(r: &Row) -> Vec<Side<'_>> {
    r.children
        .iter()
        .map(|c| Side {
            row: c,
            count: c.count,
        })
        .collect()
}

/// Every assembly under `root` (itself included), by part and revision,
/// as first met.
fn assemblies(root: &Row) -> Vec<&Row> {
    let mut out: Vec<&Row> = Vec::new();
    fn walk<'a>(row: &'a Row, out: &mut Vec<&'a Row>) {
        if row.is_assembly() && !out.iter().any(|r| r.part == row.part && r.rev == row.rev) {
            out.push(row);
        }
        for c in &row.children {
            walk(c, out);
        }
    }
    walk(root, &mut out);
    out
}

/// What changed from the BOM `from` to the BOM `to` (either may be
/// missing: then everything is added or removed), both read with no
/// depth limit, properties judged revising by `config`: the flat BOM's
/// rows (`flat`), each assembly both have whose children changed
/// (`tree`), the assemblies only one has (`assemblies`), and counts
/// (`summary`).
pub fn diff(from: Option<&Row>, to: Option<&Row>, config: &crate::config::Config) -> Value {
    let flats = |r: Option<&Row>| r.map(flat).unwrap_or_default();
    let (fa, fb) = (flats(from), flats(to));
    let flat = compare(sides(&fa), sides(&fb), config);
    let (aa, ab) = (
        from.map(assemblies).unwrap_or_default(),
        to.map(assemblies).unwrap_or_default(),
    );
    let key = |r: &Row| json!({"part": r.part, "rev": r.rev.to_json()});
    let mut tree = Vec::new();
    for b in &ab {
        let Some(a) = aa.iter().find(|a| a.part == b.part && a.rev == b.rev) else {
            continue;
        };
        let c = compare(kids(a), kids(b), config);
        let empty = ["added", "removed", "changed"]
            .iter()
            .all(|k| c[k].as_array().is_some_and(Vec::is_empty));
        if !empty {
            let mut v = key(b);
            v["children"] = c;
            tree.push(v);
        }
    }
    let only = |x: &[&Row], y: &[&Row]| -> Vec<Value> {
        x.iter()
            .filter(|r| !y.iter().any(|o| o.part == r.part && o.rev == r.rev))
            .map(|r| key(r))
            .collect()
    };
    let n = |k: &str| flat[k].as_array().map_or(0, Vec::len);
    json!({
        "summary": {"added": n("added"), "removed": n("removed"), "changed": n("changed")},
        "flat": flat,
        "tree": tree,
        "assemblies": {"added": only(&ab, &aa), "removed": only(&aa, &ab)},
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::tests::id;
    use crate::structure::tests::{block, files, instance};
    use crate::structure::{Cache, Reader};
    use ryucad_doc::doc::canonical;
    use ryucad_doc::doc::project::Project;
    use std::path::Path;

    /// `frame` places a plate, two `sub`s (an instance and a copy), two
    /// tubes and a bought motor; `sub` places two bolts, the motor one.
    pub(crate) fn frame() -> BTreeMap<String, Vec<u8>> {
        let w = || json!("working");
        let mut f = files(&[
            ("bolt", &[("b", block())]),
            ("plate", &[("b", block())]),
            ("tube", &[("b", block())]),
            (
                "sub",
                &[("b1", instance("bolt", w())), ("b2", instance("bolt", w()))],
            ),
            ("motor", &[("b1", instance("bolt", w()))]),
            (
                "frame",
                &[
                    ("base", instance("plate", w())),
                    ("s", instance("sub", w())),
                    (
                        "row",
                        json!({"type": "instance_pattern", "form": "linear", "instances": ["s"],
                               "direction": "x", "spacing": "10 mm", "count": 2}),
                    ),
                    ("t1", instance("tube", w())),
                    ("t2", instance("tube", w())),
                    ("m", instance("motor", w())),
                ],
            ),
        ]);
        let mut props = |part: &str, v: Value| {
            let mut v = v;
            v["kind"] = json!("ryupdm.properties");
            v["format"] = json!(if v.get("amount").is_some() { 2 } else { 1 });
            f.insert(
                format!("parts/{part}/properties.json"),
                canonical::to_string(&v).into_bytes(),
            );
        };
        props(
            "bolt",
            json!({"number": "100", "name": "Bolt", "make_buy": "buy"}),
        );
        props(
            "plate",
            json!({"number": "200", "name": "Plate", "material": "6061-T6"}),
        );
        props(
            "tube",
            json!({"number": "300", "uom": "m", "amount": "600 mm"}),
        );
        props("motor", json!({"number": "400", "make_buy": "buy"}));
        f
    }

    pub(crate) fn read(f: &BTreeMap<String, Vec<u8>>, part: &str, depth: Option<usize>) -> Row {
        let project = Project::load_from(Path::new(""), f).unwrap();
        let mut cache = Cache::new();
        let mut r = Reader::new(f, &project, None, None, &mut cache);
        tree(&mut r, &id(part), &Rev::Working, depth).unwrap()
    }

    fn line(r: &Row) -> String {
        format!("{} x{} {}", r.part, r.count, r.instances.join(","))
    }

    #[test]
    fn the_indented_bom_groups_each_parents_children() {
        let root = read(&frame(), "frame", None);
        let lines: Vec<String> = root.children.iter().map(line).collect();
        assert_eq!(
            lines,
            [
                "plate x1 base",
                "sub x2 s,row:1/s",
                "tube x2 t1,t2",
                "motor x1 m"
            ]
        );
        assert_eq!(line(&root.children[1].children[0]), "bolt x2 b1,b2");
        // The bought motor lists its bolt, marked as inside it.
        let motor = &root.children[3];
        assert!(motor.buy() && motor.is_leaf() && !motor.bought);
        assert!(motor.children[0].bought);
        // A depth stops the walk and says so.
        let shallow = read(&frame(), "frame", Some(1));
        assert!(shallow.children[1].truncated);
        assert!(shallow.children[1].children.is_empty());
        let v = row_json(&root, &BTreeMap::new());
        assert_eq!(
            v["children"][2]["quantity"],
            json!({"count": 2, "uom": "m", "value": 1.2, "text": "1.2 m"})
        );
        assert_eq!(v["children"][0]["properties"]["material"], "6061-T6");
        assert_eq!(v["children"][3]["children"][0]["bought"], true);
    }

    #[test]
    fn the_flat_bom_totals_leaves_and_stops_at_bought_assemblies() {
        let root = read(&frame(), "frame", None);
        let rows: Vec<String> = flat(&root)
            .iter()
            .map(|f| format!("{} {}", f.row.part, f.total))
            .collect();
        assert_eq!(rows, ["bolt 4", "plate 1", "tube 2", "motor 1"]);
        // A part's BOM is itself.
        let plate = read(&frame(), "plate", None);
        assert_eq!(flat(&plate).len(), 1);
        assert!(findings(&root).is_empty());
        assert!(problems(&root).is_empty());
    }

    #[test]
    fn what_cant_be_read_is_a_row_that_says_why() {
        let mut f = frame();
        // A missing part, a missing amount and a part placed at two
        // revisions.
        f.insert(
            "parts/frame/features/ghost.json".into(),
            canonical::to_string(&{
                let mut v = instance("ghost", json!("working"));
                v["format"] = json!(4);
                v["kind"] = json!("ryucad.feature");
                v
            })
            .into_bytes(),
        );
        f.insert(
            "parts/frame/part.json".into(),
            canonical::to_string(
                &json!({"features": ["base", "s", "row", "t1", "t2", "m", "ghost"],
                "format": 2, "kind": "ryucad.part", "part_kind": "assembly"}),
            )
            .into_bytes(),
        );
        f.insert(
            "parts/tube/properties.json".into(),
            canonical::to_string(&json!({"format": 1, "kind": "ryupdm.properties", "uom": "m"}))
                .into_bytes(),
        );
        let root = read(&f, "frame", None);
        let ghost = root.children.last().unwrap();
        assert_eq!(ghost.node.as_ref().unwrap_err().code, Code::NotFound);
        assert_eq!(problems(&root).len(), 1);
        let tube = &root.children[2];
        assert_eq!(quantity(tube, 2), json!({"count": 2, "uom": "m"}));
        let codes: Vec<Value> = findings(&root).iter().map(|f| f["code"].clone()).collect();
        assert_eq!(codes, [json!("bom_amount_missing")]);
    }

    #[test]
    fn an_assembly_that_places_itself_stops_there() {
        let w = || json!("working");
        let f = files(&[
            ("a", &[("b", instance("b", w()))]),
            ("b", &[("a", instance("a", w()))]),
        ]);
        let root = read(&f, "a", None);
        let b = &root.children[0];
        let back = &b.children[0];
        assert_eq!(back.node.as_ref().unwrap_err().code, Code::AssemblyCycle);
        assert!(back.children.is_empty());
    }

    #[test]
    fn csv_writes_every_row_with_its_level_and_quotes_what_it_must() {
        let mut f = frame();
        f.insert(
            "ryupdm.json".into(),
            canonical::to_string(&json!({"format": 1, "kind": "ryupdm.config",
                "properties": {"finish": {"type": "string"}}}))
            .into_bytes(),
        );
        f.insert(
            "parts/plate/properties.json".into(),
            canonical::to_string(&json!({"format": 1, "kind": "ryupdm.properties",
                "number": "200", "name": "Plate, \"thick\"", "finish": "anodized"}))
            .into_bytes(),
        );
        let config = crate::config::Config::read(&f).unwrap();
        let root = read(&f, "frame", None);
        let csv = csv_tree(&root, &config);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "level,item,number,name,part,revision,quantity,uom,make_buy,material,description,finish"
        );
        assert_eq!(
            lines[1],
            "1,,200,\"Plate, \"\"thick\"\"\",plate,working,1,each,make,,,anodized"
        );
        assert_eq!(lines[2], "2,,,,sub,working,2,each,make,,,");
        assert_eq!(lines[3], "2.1,,100,Bolt,bolt,working,2,each,buy,,,");
        assert_eq!(lines[4], "3,,300,,tube,working,1.2,m,make,,,");
        assert_eq!(lines.len(), 7);
        let flat = csv_flat(&flat(&root), &config);
        assert_eq!(
            flat.lines().nth(1),
            Some(",,100,Bolt,bolt,working,4,each,buy,,,")
        );
        assert!(csv.ends_with('\n') && !csv.contains('\r'));
    }

    #[test]
    fn a_diff_lists_rows_added_removed_and_changed() {
        let config = crate::config::Config::default();
        let a = frame();
        let before = read(&a, "frame", None);
        assert_eq!(
            diff(Some(&before), Some(&before), &config)["summary"],
            json!({"added": 0, "removed": 0, "changed": 0})
        );
        let mut b = a.clone();
        // Two more bolts in sub, the plate renamed, the tube gone.
        let mut put = |p: &str, v: Value| {
            b.insert(p.to_owned(), canonical::to_string(&v).into_bytes());
        };
        let mut b3 = instance("bolt", json!("working"));
        b3["format"] = json!(4);
        b3["kind"] = json!("ryucad.feature");
        put("parts/sub/features/b3.json", b3);
        put(
            "parts/sub/part.json",
            json!({"features": ["b1", "b2", "b3"], "format": 2, "kind": "ryucad.part",
                   "part_kind": "assembly"}),
        );
        put(
            "parts/plate/properties.json",
            json!({"format": 1, "kind": "ryupdm.properties", "number": "200",
                   "name": "Base plate", "material": "6061-T6"}),
        );
        put(
            "parts/frame/part.json",
            json!({"features": ["base", "s", "row", "m"], "format": 2,
                   "kind": "ryucad.part", "part_kind": "assembly"}),
        );
        let after = read(&b, "frame", None);
        let d = diff(Some(&before), Some(&after), &config);
        assert_eq!(
            d["summary"],
            json!({"added": 0, "removed": 1, "changed": 2})
        );
        assert_eq!(d["flat"]["removed"][0]["part"], "tube");
        let changed = &d["flat"]["changed"];
        assert_eq!(changed[0]["part"], "bolt");
        assert_eq!(changed[0]["quantity"]["from"]["count"], 4);
        assert_eq!(changed[0]["quantity"]["to"]["count"], 6);
        assert_eq!(
            changed[1]["properties"],
            json!({"name": {"from": "Plate", "to": "Base plate", "revising": false}})
        );
        // Where it sits: frame lost its tubes, sub gained a bolt.
        let tree: Vec<&Value> = d["tree"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["part"])
            .collect();
        assert_eq!(tree, [&json!("frame"), &json!("sub")]);
        assert_eq!(
            d["tree"][1]["children"]["changed"][0]["quantity"]["to"]["count"],
            3
        );
        // From nothing, everything is added.
        let d = diff(None, Some(&after), &config);
        assert_eq!(d["summary"]["added"], 3);
        assert_eq!(d["assemblies"]["added"].as_array().unwrap().len(), 3);
    }
}
