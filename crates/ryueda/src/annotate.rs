//! `annotate`: a design's designators numbered in canvas order (design
//! 348, plan 2.F). Blocks as the PDF pages them (depth first from the
//! root, each block once: a stack's layers share their block's numbers,
//! and their formats give each layer its own); on each sheet, by region
//! in reading order, then down each column and across (`down`) or across
//! each row and down (`across`). Locked components keep their
//! designators and the numbers count past them; components of blocks
//! the design doesn't reach keep theirs, and their numbers are taken.

use crate::conn::{self, Component, numbered, refdes_placeholder};
use crate::design::{Part, part_path};
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::netlist;
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::write::{self, Writes, commit};
use ryusch::geom::{GRID, Nm, Point};
use ryusch::layout::Layout;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Places within this many nanometres across (`down`) or up and down
/// (`across`) read as one column or row: ten grid steps, 12.7 mm.
pub const BAND: Nm = 10 * GRID;

/// The order designators run in on a sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Down each column, the columns left to right.
    #[default]
    Down,
    /// Across each row, the rows top to bottom.
    Across,
}

/// Which components `annotate` numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reset {
    /// Those with no designator or a placeholder (`R?`).
    #[default]
    Unannotated,
    /// Every unlocked one with a number or none.
    All,
}

/// What `annotate` reads and writes, as one revision: the project's and
/// every block's (designators are numbered across blocks, and those of
/// blocks a design doesn't reach are taken). `--expect` takes it.
pub fn revision(p: &Project) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ryueda.annotate.1");
    h.update(p.revision.as_bytes());
    for (id, b) in &p.conn.blocks {
        h.update(id.as_str().as_bytes());
        h.update(b"=");
        h.update(b.revision.as_bytes());
        h.update(b";");
    }
    h.finalize().to_hex()[..32].to_owned()
}

/// Where `at` reads in `order`: smaller first.
fn reading(at: Point, order: Order) -> (Nm, Nm, Nm) {
    let (x, down) = (at[0], -at[1]);
    match order {
        Order::Down => (x.div_euclid(BAND), down, x),
        Order::Across => (down.div_euclid(BAND), x, down),
    }
}

/// A place's sort key: sheet in canvas order, region in reading order
/// (none after the regions), then the place itself.
type Key = (usize, usize, (Nm, Nm, Nm));

/// Each component a block's layout draws, by where its first unit reads,
/// and each instance it draws (a stack by its front) by where its symbol
/// sits.
fn places(layout: &Layout, order: Order) -> (BTreeMap<String, Key>, BTreeMap<String, Key>) {
    let sheets = ryusch::canvas::order(layout);
    let sheet = |s: &str| sheets.iter().position(|k| k == s).unwrap_or(usize::MAX);
    let mut regions: Vec<(usize, (Nm, Nm, Nm), &String)> = layout
        .regions
        .iter()
        .map(|(id, r)| {
            let f = r.frame_on_sheet();
            (sheet(&r.sheet), reading([f.min[0], f.max[1]], order), id)
        })
        .collect();
    regions.sort();
    let mut region_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (rank, (_, _, id)) in regions.iter().enumerate() {
        for part in &layout.regions[*id].parts {
            region_of.insert(part, rank);
        }
    }
    let (mut components, mut instances) = (BTreeMap::new(), BTreeMap::new());
    for (key, p) in &layout.placed {
        let at = (
            sheet(&p.sheet),
            region_of.get(key.as_str()).copied().unwrap_or(usize::MAX),
            reading(p.at, order),
        );
        if let Some(i) = key.strip_prefix("instance:") {
            instances.insert(i.to_owned(), at);
        } else if !key.starts_with("port:") {
            // `<component>` or `<component>:<unit>`: its first unit.
            let c = key.split(':').next().unwrap_or(key).to_owned();
            components
                .entry(c)
                .and_modify(|k: &mut Key| *k = (*k).min(at))
                .or_insert(at);
        }
    }
    (components, instances)
}

/// Sorts `ids` by their places where `at` has them, the rest after in ID
/// order.
fn by_place(ids: &mut [Id], at: &BTreeMap<String, Key>) {
    ids.sort_by(|a, b| match (at.get(a.as_str()), at.get(b.as_str())) {
        (Some(x), Some(y)) => x.cmp(y).then(a.cmp(b)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    });
}

/// One designator `annotate` sets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    /// `<block>/<component>`.
    pub component: String,
    pub from: Option<String>,
    pub to: String,
}

/// What `annotate` would do: its changes, the files they write, and the
/// components it leaves as they are and why.
pub struct Plan {
    pub design: Id,
    pub changes: Vec<Change>,
    pub writes: Writes,
    pub skipped: Vec<Value>,
}

/// The components of design `design`'s blocks in annotate's order, each
/// block once: `(block, component)`. Blocks depth first from the root as
/// the PDF pages them, each block's children by where their symbols sit;
/// a block's components by where its layout file draws them. Nothing is
/// laid out: a block with no layout file that reads orders its
/// components and children by ID.
fn in_order(p: &Project, design: &Id, order: Order) -> Result<Vec<(Id, Id)>> {
    let root = netlist::design_root(p, design)?;
    let mut seen: BTreeSet<Id> = BTreeSet::new();
    let mut out = Vec::new();
    let mut todo = vec![root];
    while let Some(b) = todo.pop() {
        if !seen.insert(b.clone()) {
            continue;
        }
        let Ok(files) = p.conn.block(&b) else {
            continue;
        };
        let (components, instances) = match &files.layout {
            Some(Ok(l)) => places(l, order),
            _ => Default::default(),
        };
        let mut list: Vec<Id> = files.components.keys().cloned().collect();
        by_place(&mut list, &components);
        out.extend(list.into_iter().map(|c| (b.clone(), c)));
        let mut kids: Vec<Id> = files.instances.keys().cloned().collect();
        by_place(&mut kids, &instances);
        // Depth first in that order: the first child next.
        for i in kids.iter().rev() {
            if let Some(Ok(inst)) = files.instances.get(i).map(|x| &x.item) {
                todo.push(inst.of.clone());
            }
        }
    }
    Ok(out)
}

/// The prefix a component is numbered under: its designator's letters,
/// else its library component's prefix.
fn prefix_of(p: &Project, c: &Component) -> Option<String> {
    if let Some(r) = &c.refdes {
        let letters = r
            .find(|ch: char| !ch.is_ascii_alphabetic())
            .unwrap_or(r.len());
        if letters > 0 {
            return Some(r[..letters].to_owned());
        }
    }
    crate::partcmd::placed(p, &c.of)
        .ok()
        .map(|(lc, _)| lc.prefix.clone())
        .filter(|s| !s.is_empty())
}

/// Two locked components with one designator, which nothing may number
/// round: `annotate_conflict`, naming both.
fn locked_clashes(p: &Project) -> Result<()> {
    let mut seen: BTreeMap<&str, String> = BTreeMap::new();
    for (bid, b) in &p.conn.blocks {
        for (cid, c) in &b.components {
            let Ok(c) = &c.item else { continue };
            let (true, Some(r)) = (c.refdes_locked, c.refdes.as_deref()) else {
                continue;
            };
            let here = format!("{bid}/{cid}");
            if let Some(other) = seen.insert(r, here.clone()) {
                return Err(Error::new(
                    Code::AnnotateConflict,
                    format!("{other} and {here} are both locked as {r}: unlock one"),
                )
                .with("refdes", r)
                .with("components", json!([other, here])));
            }
        }
    }
    Ok(())
}

/// What annotating design `design` would change.
pub fn plan(p: &Project, design: Option<&str>, order: Order, reset: Reset) -> Result<Plan> {
    let d = netlist::one_design(p, design)?;
    locked_clashes(p)?;
    let list = in_order(p, &d, order)?;
    let mine: BTreeSet<(&Id, &Id)> = list.iter().map(|(b, c)| (b, c)).collect();
    // Every designator nothing here renumbers is taken.
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut todo: Vec<(&Id, &Id, Component, String, String)> = Vec::new();
    let mut skipped = Vec::new();
    for (bid, b) in &p.conn.blocks {
        for (cid, c) in &b.components {
            if mine.contains(&(bid, cid)) {
                continue;
            }
            if let Some(r) = c.item.as_ref().ok().and_then(|c| c.refdes.clone()) {
                taken.insert(r);
            }
        }
    }
    for (bid, cid) in &list {
        let b = p.conn.block(bid)?;
        let Ok(c) = &b.components[cid].item else {
            skipped
                .push(json!({"component": format!("{bid}/{cid}"), "why": "its file doesn't read"}));
            continue;
        };
        let r = c.refdes.as_deref();
        let renumber = !c.refdes_locked
            && match (r, reset) {
                (None, _) => true,
                (Some(r), _) if refdes_placeholder(r) => true,
                (Some(r), Reset::All) => numbered(r).is_some(),
                (Some(_), Reset::Unannotated) => false,
            };
        if !renumber {
            if let Some(r) = r {
                taken.insert(r.to_owned());
            }
            continue;
        }
        let Some(prefix) = prefix_of(p, c) else {
            skipped.push(json!({"component": format!("{bid}/{cid}"),
                                "why": "it has no designator and its library component no prefix"}));
            continue;
        };
        let rest = r
            .and_then(numbered)
            .map_or("", |(_, _, rest)| rest)
            .to_owned();
        todo.push((bid, cid, c.clone(), prefix, rest));
    }
    // Each prefix and rest counts from 1, past what is taken.
    let mut next: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut changes = Vec::new();
    let mut writes = Writes::default();
    for (bid, cid, mut c, prefix, rest) in todo {
        let n = next.entry((prefix.clone(), rest.clone())).or_insert(1);
        let to = loop {
            let t = format!("{prefix}{n}{rest}");
            *n += 1;
            if !taken.contains(&t) && conn::refdes_ok(&t) {
                break t;
            }
        };
        taken.insert(to.clone());
        if c.refdes.as_deref() == Some(to.as_str()) {
            continue;
        }
        changes.push(Change {
            component: format!("{bid}/{cid}"),
            from: c.refdes.clone(),
            to: to.clone(),
        });
        c.refdes = Some(to);
        writes.put(
            part_path(bid, Part::Component, cid),
            &c,
            conn::COMPONENT_KIND,
        );
    }
    Ok(Plan {
        design: d,
        changes,
        writes,
        skipped,
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_read_down_each_column_or_across_each_row() {
        let step = GRID;
        // Two columns a band apart; in each, higher first.
        let mut pts = vec![
            [0, 0],
            [0, 20 * step],
            [12 * step, 30 * step],
            [12 * step, 5 * step],
        ];
        pts.sort_by_key(|p| reading(*p, Order::Down));
        assert_eq!(
            pts,
            [
                [0, 20 * step],
                [0, 0],
                [12 * step, 30 * step],
                [12 * step, 5 * step]
            ]
        );
        pts.sort_by_key(|p| reading(*p, Order::Across));
        assert_eq!(
            pts,
            [
                [12 * step, 30 * step],
                [0, 20 * step],
                [12 * step, 5 * step],
                [0, 0]
            ]
        );
        // Within a band, a small offset doesn't start a new column.
        assert_eq!(
            reading([3 * step, 0], Order::Down).0,
            reading([0, 0], Order::Down).0
        );
        // Below zero the bands go on (div_euclid): -1 is the band before 0.
        assert_eq!(reading([-step, 0], Order::Down).0, -1);
    }
}
