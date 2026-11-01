//! The output job: a design's canvases paged into one PDF. Each sheet is
//! a page at the project's paper, depth first from the root in canvas
//! order (a block's sheets as its canvas orders them, then the blocks its
//! symbols place, in the order the symbols sit there); a stack's sheets
//! once per layer, each with that layer's designators, or once with its
//! layers listed. Bookmarks follow the hierarchy; a block symbol links to
//! its block's first page; each net label carries the other pages its net
//! is drawn on (`→ 4, 7`), each a link.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::netlist;
use crate::pdf::{self, Link, Mark, Page};
use crate::project::Project;
use crate::schem::{self, BORDER, Laid};
use ryusch::font::{Anchor, MIN_SIZE};
use ryusch::geom::{Nm, Rect};
use ryusch::layout::LabelKind;
use ryusch::paint::{Item, LABEL};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Which layers of a stack get pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layers {
    /// Every layer, its sheets once each.
    #[default]
    All,
    /// The first layer's, its heading listing the others.
    First,
}

/// A block where the job draws it.
struct Node {
    block: Id,
    path: Vec<Id>,
    /// Its stack path, or its layers' when they share its pages.
    layer: String,
    /// The node it hangs under in the bookmarks, and the instance there.
    parent: Option<(usize, Id)>,
}

/// The block occurrences of design `d` in page order, from `root`.
fn walk(
    p: &Project,
    laid: &mut BTreeMap<Id, Laid>,
    root: &Id,
    layers: Layers,
) -> Result<Vec<Node>> {
    let mut out = vec![Node {
        block: root.clone(),
        path: Vec::new(),
        layer: String::new(),
        parent: None,
    }];
    let mut i = 0;
    while i < out.len() {
        let block = out[i].block.clone();
        if out[i].path.len() > 64 {
            return Err(Error::new(
                Code::BlockCycle,
                format!("block {block} holds itself"),
            ));
        }
        let b = p.conn.block(&block)?;
        if !laid.contains_key(&block) {
            let l = schem::lay(p, b, None, &[], ryusch::input::Relayout::None)?;
            laid.insert(block.clone(), l);
        }
        let l = &laid[&block];
        let order = ryusch::canvas::order(&l.outcome.layout);
        // The instances by where their symbols sit: sheet, then top to
        // bottom and left to right; a stack's layers after its front.
        let mut placed: Vec<(usize, Nm, Nm, Id)> = Vec::new();
        for of in &l.built.of {
            let schem::Of::Instance(iid) = of else {
                continue;
            };
            let key = format!("instance:{iid}");
            let at = l.outcome.layout.placed.get(&key);
            let s = at.map_or(usize::MAX, |x| {
                order
                    .iter()
                    .position(|k| *k == x.sheet)
                    .unwrap_or(usize::MAX)
            });
            let (x, y) = at.map_or((0, 0), |x| (x.at[0], x.at[1]));
            placed.push((s, -y, x, iid.clone()));
        }
        placed.sort();
        let mut kids = Vec::new();
        for (_, _, _, front) in placed {
            let members = l
                .built
                .stacks
                .get(&front)
                .cloned()
                .unwrap_or_else(|| vec![front.clone()]);
            let take = match layers {
                Layers::All => members.clone(),
                Layers::First => vec![front.clone()],
            };
            for m in take {
                let Some(inst) = b.instances.get(&m).and_then(|x| x.item.as_ref().ok()) else {
                    continue;
                };
                let mut path = out[i].path.clone();
                path.push(m.clone());
                let (index, count) = netlist::stack_of(b, &m);
                let mine = if count > 1 {
                    crate::props::letter(index)
                } else {
                    String::new()
                };
                let up = &out[i].layer;
                let layer = if layers == Layers::First && members.len() > 1 {
                    let all: Vec<String> = members
                        .iter()
                        .map(|x| crate::props::letter(netlist::stack_of(b, x).0))
                        .collect();
                    join(up, &all.join(", "))
                } else {
                    join(up, &mine)
                };
                kids.push(Node {
                    block: inst.of.clone(),
                    path,
                    layer,
                    parent: Some((i, front.clone())),
                });
            }
        }
        // Depth first: the children go right after this node's subtree,
        // in order, before what follows it.
        let at = subtree_end(&out, i);
        for (k, n) in kids.into_iter().enumerate() {
            out.insert(at + k, n);
        }
        i += 1;
    }
    Ok(out)
}

/// One past the last node under `i` placed so far.
fn subtree_end(nodes: &[Node], i: usize) -> usize {
    let mut j = i + 1;
    while j < nodes.len()
        && nodes[j].path.len() > nodes[i].path.len()
        && nodes[j].path.starts_with(&nodes[i].path)
    {
        j += 1;
    }
    j
}

fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a}/{b}"),
    }
}

/// The text of a cross-reference to `pages` (numbered from 1).
fn xref(pages: &[usize]) -> String {
    let list: Vec<String> = pages.iter().map(|p| (p + 1).to_string()).collect();
    format!("→ {}", list.join(", "))
}

/// `output`: design `design`'s PDF and what it holds.
pub fn output(p: &Project, design: Option<&str>, layers: Layers) -> Result<(Value, Vec<u8>)> {
    let d = netlist::one_design(p, design)?;
    let root = netlist::design_root(p, &d)?;
    let mut laid: BTreeMap<Id, Laid> = BTreeMap::new();
    let nodes = walk(p, &mut laid, &root, layers)?;
    let flat = netlist::flatten(p, &d).ok();
    // Each node's net keys to flattened nets, through its components' pins.
    let mut flat_of: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new(); nodes.len()];
    if let Some(n) = &flat {
        let mut by_ref: BTreeMap<String, &netlist::Occurrence> = BTreeMap::new();
        for o in &n.occurrences {
            by_ref.insert(o.reference.clone(), o);
        }
        for (i, node) in nodes.iter().enumerate() {
            let b = p.conn.block(&node.block)?;
            let mut prefix = format!("{d}:");
            for s in &node.path {
                prefix.push_str(s.as_str());
                prefix.push('/');
            }
            for (cid, c) in &b.components {
                let (Ok(c), Some(o)) = (&c.item, by_ref.get(&format!("{prefix}{cid}"))) else {
                    continue;
                };
                for pin in &o.pins {
                    if let (Some(fl), Some(node)) = (pin.net, c.pins.get(&pin.number))
                        && let Some(key) = net_key(node)
                    {
                        flat_of[i].insert(key, fl);
                    }
                }
            }
        }
    }
    let design_name = p
        .conn
        .designs
        .get(&d)
        .and_then(|x| x.item.as_ref().ok())
        .map_or(d.to_string(), |x| x.name.clone());
    let (w, h) = p.manifest.schematic.sheet.size_nm();
    // Pages first, so links and cross-references can number them.
    struct Sheet {
        node: usize,
        sheet: String,
        name: String,
        index: usize,
        count: usize,
    }
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut first_page: Vec<usize> = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        let l = &laid[&node.block];
        let order = ryusch::canvas::order(&l.outcome.layout);
        first_page.push(sheets.len());
        for (k, s) in order.iter().enumerate() {
            sheets.push(Sheet {
                node: i,
                sheet: s.clone(),
                name: l.outcome.layout.sheets[s].name.clone(),
                index: k + 1,
                count: order.len(),
            });
        }
    }
    // Where each flattened net is drawn by labels.
    let mut painted: Vec<ryusch::paint::Painted> = Vec::new();
    let mut net_pages: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut built_for: Vec<schem::Built> = Vec::new();
    for (pg, s) in sheets.iter().enumerate() {
        let node = &nodes[s.node];
        let l = &laid[&node.block];
        let mut built = schem::build(p, p.conn.block(&node.block)?)?;
        if let Some(layer) = schem::layers(p, &node.block)
            .into_iter()
            .find(|x| x.design == d && x.path == node.path)
        {
            built.designators(&layer.refdes);
        }
        let paint = ryusch::paint::sheet(&built.input, &l.outcome.layout, &s.sheet);
        for lab in &paint.labels {
            if let Some(f) = flat_net(&flat_of[s.node], lab) {
                net_pages.entry(f).or_default().insert(pg);
            }
        }
        painted.push(paint);
        built_for.push(built);
    }
    let mut pages: Vec<Page> = Vec::new();
    let mut listing = Vec::new();
    for (pg, s) in sheets.iter().enumerate() {
        let node = &nodes[s.node];
        let b = p.conn.block(&node.block)?;
        let block_name = b
            .block
            .item
            .as_ref()
            .map_or(node.block.to_string(), |x| x.name.clone());
        let mut heading = format!(
            "{design_name} · {block_name} · sheet {}/{} {}",
            s.index, s.count, s.name
        );
        if !node.layer.is_empty() {
            heading.push_str(&format!(" · layer {}", node.layer));
        }
        let paint = &painted[pg];
        let mut items = paint.items.clone();
        let mut links = Vec::new();
        // A block symbol links to its block's first page.
        for (key, body) in &paint.blocks {
            let Some(iid) = key.strip_prefix("instance:") else {
                continue;
            };
            if let Some(child) = nodes.iter().position(|n| {
                n.parent
                    .as_ref()
                    .is_some_and(|(up, front)| *up == s.node && front.as_str() == iid)
            }) && first_page[child] < sheets.len()
            {
                links.push(Link {
                    rect: *body,
                    to: first_page[child],
                });
            }
        }
        // Each net label: the net's other pages, under it, each a link.
        for lab in paint.labels.iter().filter(|l| l.kind == LabelKind::Net) {
            let Some(f) = flat_net(&flat_of[s.node], lab) else {
                continue;
            };
            let others: Vec<usize> = net_pages[&f].iter().copied().filter(|x| *x != pg).collect();
            if others.is_empty() {
                continue;
            }
            let at = [lab.rect.min[0], lab.rect.min[1] - MIN_SIZE];
            let text = xref(&others);
            let mut x = at[0] + ryusch::font::width("→ ", MIN_SIZE);
            for (k, o) in others.iter().enumerate() {
                let num = (o + 1).to_string();
                let wn = ryusch::font::width(&num, MIN_SIZE);
                links.push(Link {
                    rect: Rect::new([x, at[1] - MIN_SIZE / 4], [x + wn, at[1] + MIN_SIZE]),
                    to: *o,
                });
                x += wn;
                if k + 1 < others.len() {
                    x += ryusch::font::width(", ", MIN_SIZE);
                }
            }
            items.push(Item::Text {
                text,
                at,
                size: MIN_SIZE,
                anchor: Anchor::Start,
                vertical: false,
                colour: LABEL,
            });
        }
        listing.push(json!({
            "page": pg + 1,
            "block": node.block,
            "path": node.path.iter().map(Id::as_str).collect::<Vec<_>>().join("/"),
            "layer": node.layer,
            "sheet": s.sheet,
            "name": s.name,
        }));
        pages.push(Page {
            paper: [w, h],
            heading,
            origin: [BORDER, BORDER],
            items,
            links,
        });
    }
    // Bookmarks: each node its sheets, its children under it.
    fn marks(
        i: usize,
        nodes: &[Node],
        first: &[usize],
        sheets: &[(usize, String, usize)],
        title: &dyn Fn(usize) -> String,
    ) -> Mark {
        let mut children: Vec<Mark> = sheets
            .iter()
            .filter(|(n, _, _)| *n == i)
            .map(|(_, name, page)| Mark {
                title: name.clone(),
                page: *page,
                children: Vec::new(),
            })
            .collect();
        for (j, n) in nodes.iter().enumerate() {
            if n.parent.as_ref().is_some_and(|(up, _)| *up == i) {
                children.push(marks(j, nodes, first, sheets, title));
            }
        }
        Mark {
            title: title(i),
            page: first[i],
            children,
        }
    }
    let sheet_marks: Vec<(usize, String, usize)> = sheets
        .iter()
        .enumerate()
        .map(|(pg, s)| (s.node, s.name.clone(), pg))
        .collect();
    let title = |i: usize| {
        let n = &nodes[i];
        let name = p
            .conn
            .blocks
            .get(&n.block)
            .and_then(|b| b.block.item.as_ref().ok())
            .map_or(n.block.to_string(), |b| b.name.clone());
        match (n.path.last(), n.layer.is_empty()) {
            (None, _) => name,
            (Some(iid), true) => format!("{name} ({iid})"),
            (Some(iid), false) => format!("{name} ({iid}, layer {})", n.layer),
        }
    };
    let tree = if sheets.is_empty() {
        Vec::new()
    } else {
        vec![marks(0, &nodes, &first_page, &sheet_marks, &title)]
    };
    let bytes = pdf::write(&design_name, &pages, &tree);
    let v = json!({
        "design": d,
        "layers": layers,
        "pages": listing,
    });
    Ok((v, bytes))
}

/// The layout key of what a pin's connection names.
fn net_key(n: &crate::conn::Node) -> Option<String> {
    match n {
        crate::conn::Node::Net(id) => Some(id.to_string()),
        crate::conn::Node::Rail(id) => Some(format!("rail:{id}")),
        crate::conn::Node::Port(id) => Some(format!("port:{id}")),
        crate::conn::Node::Nc => None,
    }
}

/// The flattened net a label of a block occurrence draws.
fn flat_net(flat: &BTreeMap<String, usize>, lab: &ryusch::paint::Placed) -> Option<usize> {
    flat.get(&lab.net).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_design_pages_its_stack_once_per_layer() {
        let (_dir, p) = crate::netlist::tests::project(&[]);
        let (v, bytes) = output(&p, None, Layers::All).unwrap();
        let pages = v["pages"].as_array().unwrap();
        // The root's sheet, then block s for ch_a and for ch_b.
        let at: Vec<(&str, &str)> = pages
            .iter()
            .map(|x| (x["path"].as_str().unwrap(), x["layer"].as_str().unwrap()))
            .collect();
        assert_eq!(at, [("", ""), ("ch_a", "A"), ("ch_b", "B")]);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("/Subtype /Link"),
            "the stack links to its block"
        );
        assert_eq!(bytes, output(&p, None, Layers::All).unwrap().1);
        let (v, _) = output(&p, None, Layers::First).unwrap();
        let at: Vec<&str> = v["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["layer"].as_str().unwrap())
            .collect();
        assert_eq!(at, ["", "A, B"]);
    }
}
