//! The output job: a design's canvases paged into one PDF. Each sheet is
//! a page at its sheet's paper, depth first from the root in canvas
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
use crate::schem::{self, BORDER, Laid, TITLE_BAND};
use ryusch::font::{Anchor, MIN_SIZE};
use ryusch::geom::{Nm, Rect};
use ryusch::layout::LabelKind;
use ryusch::paint::Item;
use ryusch::style::{Look, Role};
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
/// A block that can't be laid out (its files don't read, its
/// instructions don't hold) has no pages: its error is in `failed`.
fn walk(
    p: &Project,
    laid: &mut BTreeMap<Id, Laid>,
    failed: &mut BTreeMap<Id, Error>,
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
        let b = match p.conn.block(&block) {
            Ok(b) => b,
            Err(e) => {
                failed.insert(block.clone(), e);
                i += 1;
                continue;
            }
        };
        if !laid.contains_key(&block) && !failed.contains_key(&block) {
            match schem::lay(p, b, None, &[], ryusch::input::Relayout::None) {
                Ok(l) => {
                    laid.insert(block.clone(), l);
                }
                Err(e) => {
                    failed.insert(block.clone(), e);
                }
            }
        }
        let Some(l) = laid.get(&block) else {
            i += 1;
            continue;
        };
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

/// The text of a cross-reference to `pages` (numbered from 1), each with
/// its zone where it has one (`→ 2 B4, 3`; plan 2.E).
fn xref(pages: &[(usize, Option<String>)]) -> String {
    let list: Vec<String> = pages.iter().map(|(p, z)| page_zone(*p, z)).collect();
    format!("→ {}", list.join(", "))
}

/// A page (from 0) and the zone its net is drawn in there, if any.
type PageZone = (usize, Option<String>);

/// `n` (from 1) and its zone.
fn page_zone(p: usize, zone: &Option<String>) -> String {
    match zone {
        Some(z) => format!("{} {z}", p + 1),
        None => (p + 1).to_string(),
    }
}

/// `output`: design `design`'s PDF and what it holds.
pub fn output(p: &Project, design: Option<&str>, layers: Layers) -> Result<(Value, Vec<u8>)> {
    let (v, design_name, pages, tree) = output_pages(p, design, layers)?;
    Ok((v, pdf::write(&design_name, &pages, &tree)))
}

/// What [`output`] pages: its listing, the design's name, each page and
/// the bookmarks, before they are written as a PDF.
fn output_pages(
    p: &Project,
    design: Option<&str>,
    layers: Layers,
) -> Result<(Value, String, Vec<Page>, Vec<Mark>)> {
    let d = netlist::one_design(p, design)?;
    let root = netlist::design_root(p, &d)?;
    let mut laid: BTreeMap<Id, Laid> = BTreeMap::new();
    let mut failed: BTreeMap<Id, Error> = BTreeMap::new();
    let nodes = walk(p, &mut laid, &mut failed, &root, layers)?;
    // Nothing to page without the root.
    if let Some(e) = failed.get(&root) {
        return Err(e.clone());
    }
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
    // Pages first, so links and cross-references can number them.
    struct Sheet {
        node: usize,
        sheet: String,
        name: String,
        count: usize,
    }
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut first_page: Vec<usize> = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        first_page.push(sheets.len());
        let Some(l) = laid.get(&node.block) else {
            continue;
        };
        let order = ryusch::canvas::order(&l.outcome.layout);
        for s in &order {
            sheets.push(Sheet {
                node: i,
                sheet: s.clone(),
                name: l.outcome.layout.sheets[s].name.clone(),
                count: order.len(),
            });
        }
    }
    // Where each flattened net is drawn by labels.
    let mut painted: Vec<ryusch::paint::Painted> = Vec::new();
    let mut looks_of: BTreeMap<Id, BTreeMap<String, ryusch::style::Look>> = BTreeMap::new();
    let mut net_pages: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    // The least zone (row, column) of each flattened net's connectors,
    // flags and labels on each page (plan 2.E).
    let mut net_zones: BTreeMap<(usize, usize), (usize, usize)> = BTreeMap::new();
    // Each node's designators, its block's layers read once.
    let mut layers_of: BTreeMap<Id, Vec<schem::Layer>> = BTreeMap::new();
    let mut refdes: Vec<BTreeMap<Id, String>> = Vec::new();
    for node in &nodes {
        let list = layers_of
            .entry(node.block.clone())
            .or_insert_with(|| schem::layers(p, &node.block));
        refdes.push(
            list.iter()
                .find(|x| x.design == d && x.path == node.path)
                .map(|x| x.refdes.clone())
                .unwrap_or_default(),
        );
    }
    for (pg, s) in sheets.iter().enumerate() {
        let node = &nodes[s.node];
        let l = &laid[&node.block];
        let mut built = l.built.clone();
        built.designators(&refdes[s.node]);
        // In the project's look, light (plan 2.E): the PDF is always so.
        let look = looks_of
            .entry(node.block.clone())
            .or_insert_with(|| {
                p.conn.block(&node.block).map_or_else(
                    |_| BTreeMap::new(),
                    |b| {
                        crate::schemcmd::looks(
                            p,
                            b,
                            &l.built,
                            &l.outcome.layout,
                            ryusch::style::Theme::Light,
                        )
                    },
                )
            })
            .get(&s.sheet)
            .cloned()
            .unwrap_or_default();
        let paint = ryusch::paint::sheet_in(&built.input, &l.outcome.layout, &s.sheet, &look);
        // Every flattened net a pin on the page is on (plan 2.C): a
        // connector's or a port's cross-reference names the pages its net
        // is drawn on.
        for key in &paint.nets {
            if let Some(f) = flat_of[s.node].get(key) {
                net_pages.entry(*f).or_default().insert(pg);
            }
        }
        // Where on the page each is: its paper and the sheet's origin as
        // the page draws them.
        let area = l.built.input.area_in(&l.outcome.layout, &s.sheet);
        let paper = [area[0] + 2 * BORDER, area[1] + 2 * BORDER + TITLE_BAND];
        let origin = [BORDER, BORDER + TITLE_BAND];
        let mut drawn: Vec<(Vec<String>, Rect)> = paint
            .labels
            .iter()
            .map(|l| (vec![l.net.clone()], l.rect))
            .collect();
        drawn.extend(paint.ports.iter().map(|(n, r)| (vec![n.clone()], *r)));
        drawn.extend(paint.buses.iter().map(|c| (c.nets.clone(), c.rect)));
        for (nets, r) in drawn {
            let c = [(r.min[0] + r.max[0]) / 2, (r.min[1] + r.max[1]) / 2];
            let z = crate::pdf::zone_at(paper, [origin[0] + c[0], origin[1] + c[1]]);
            for n in &nets {
                if let Some(f) = flat_of[s.node].get(n) {
                    let e = net_zones.entry((*f, pg)).or_insert(z);
                    *e = (*e).min(z);
                }
            }
        }
        painted.push(paint);
    }
    let mut pages: Vec<Page> = Vec::new();
    let mut listing = Vec::new();
    let style_layers = crate::cascade::Layers::of(p);
    for (pg, s) in sheets.iter().enumerate() {
        let node = &nodes[s.node];
        let b = p.conn.block(&node.block)?;
        let block_name = b
            .block
            .item
            .as_ref()
            .map_or(node.block.to_string(), |x| x.name.clone());
        let lv = schem::levels(
            p,
            b,
            BTreeMap::from([
                ("design", design_name.clone()),
                ("block", block_name.clone()),
                ("sheet", s.name.clone()),
                ("sheets", s.count.to_string()),
            ]),
        );
        let fill = |f: crate::style::TitleField| {
            crate::props::Template::parse(&style_layers.title_field(f))
                .and_then(|t| t.render(&|n| Some(lv.get(n).unwrap_or_default())))
                .unwrap_or_default()
        };
        let mut design_block = format!("{design_name} · {block_name}");
        if !node.layer.is_empty() {
            design_block.push_str(&format!(" · layer {}", node.layer));
        }
        let title = crate::pdf::TitleBlock {
            title: fill(crate::style::TitleField::Title),
            number: fill(crate::style::TitleField::Number),
            organisation: fill(crate::style::TitleField::Organisation),
            design_block,
            revision: fill(crate::style::TitleField::Revision),
            date: fill(crate::style::TitleField::Date),
            sheet: format!("{}/{}", pg + 1, sheets.len()),
        };
        let paint = &painted[pg];
        let mut items = paint.items.clone();
        let mut links = Vec::new();
        // A block symbol links to its block's first page.
        for (key, body) in &paint.blocks {
            let Some(iid) = key.strip_prefix("instance:") else {
                continue;
            };
            if let Some(child) = nodes.iter().position(|n| {
                laid.contains_key(&n.block)
                    && n.parent
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
        for (rect, others) in cross_references(paint, &flat_of[s.node], &net_pages, &net_zones, pg)
        {
            xref_items(
                [rect.min[0], rect.min[1] - MIN_SIZE],
                &others,
                &mut items,
                &mut links,
            );
        }
        listing.push(json!({
            "page": pg + 1,
            "block": node.block,
            "path": node.path.iter().map(Id::as_str).collect::<Vec<_>>().join("/"),
            "layer": node.layer,
            "sheet": s.sheet,
            "name": s.name,
        }));
        // Its sheet's own paper (plan 2.A): the drawing area, the border
        // and the title band at its foot (plan 2.E).
        let l = &laid[&node.block];
        let area = l.built.input.area_in(&l.outcome.layout, &s.sheet);
        pages.push(Page {
            paper: [area[0] + 2 * BORDER, area[1] + 2 * BORDER + TITLE_BAND],
            title,
            origin: [BORDER, BORDER + TITLE_BAND],
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
            // A block that couldn't be laid out has no pages to open.
            let paged = sheets.iter().any(|(n, _, _)| *n == j);
            if paged && n.parent.as_ref().is_some_and(|(up, _)| *up == i) {
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
    let mut v = json!({
        "design": d,
        "layers": layers,
        "pages": listing,
    });
    if !failed.is_empty() {
        let list: Vec<Value> = nodes
            .iter()
            .filter_map(|n| {
                let e = failed.get(&n.block)?;
                Some(json!({
                    "block": n.block,
                    "path": n.path.iter().map(Id::as_str).collect::<Vec<_>>().join("/"),
                    "error": e,
                }))
            })
            .collect();
        v["failed"] = json!(list);
    }
    Ok((v, design_name, pages, tree))
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

/// Each off-sheet connector, a bus's included, and each block port's flag
/// on page `pg`: its box and its nets' other pages (by `flat`, the page's
/// net keys' flattened nets, and `net_pages`, each flattened net's pages),
/// for the cross-reference under it (plan 2.C), each page with the least
/// zone its nets are drawn in there (`net_zones`, plan 2.E). A local
/// net label continues nowhere, so it carries none; neither does a
/// connector whose nets are on no other page.
fn cross_references(
    paint: &ryusch::paint::Painted,
    flat: &BTreeMap<String, usize>,
    net_pages: &BTreeMap<usize, BTreeSet<usize>>,
    net_zones: &BTreeMap<(usize, usize), (usize, usize)>,
    pg: usize,
) -> Vec<(Rect, Vec<PageZone>)> {
    let mut under: Vec<(Vec<String>, Rect)> = paint
        .labels
        .iter()
        .filter(|l| l.kind == LabelKind::OffSheet)
        .map(|l| (vec![l.net.clone()], l.rect))
        .collect();
    under.extend(paint.ports.iter().map(|(n, r)| (vec![n.clone()], *r)));
    under.extend(paint.buses.iter().map(|c| (c.nets.clone(), c.rect)));
    under
        .into_iter()
        .filter_map(|(nets, rect)| {
            let fs: Vec<usize> = nets.iter().filter_map(|n| flat.get(n)).copied().collect();
            let others: BTreeSet<usize> = fs
                .iter()
                .filter_map(|f| net_pages.get(f))
                .flatten()
                .copied()
                .filter(|x| *x != pg)
                .collect();
            let zoned = others
                .into_iter()
                .map(|o| {
                    let z = fs
                        .iter()
                        .filter_map(|f| net_zones.get(&(*f, o)))
                        .min()
                        .map(|z| crate::pdf::zone_text(*z));
                    (o, z)
                })
                .collect::<Vec<_>>();
            (!zoned.is_empty()).then_some((rect, zoned))
        })
        .collect()
}

/// A cross-reference's text at `at` (`→ 2 B4, 3`) and a link on each
/// page number and its zone.
fn xref_items(
    at: ryusch::geom::Point,
    others: &[(usize, Option<String>)],
    items: &mut Vec<Item>,
    links: &mut Vec<Link>,
) {
    let text = xref(others);
    let mut x = at[0] + ryusch::font::width("→ ", MIN_SIZE);
    for (k, (o, zone)) in others.iter().enumerate() {
        let num = page_zone(*o, zone);
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
        colour: Look::default().role(Role::Label),
        bold: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_block_reads_the_style_and_the_properties() {
        let (_dir, p) =
            crate::netlist::tests::project_with_properties(&[("doc_number", "SB-1"), ("rev", "C")]);
        let (_, _, pages, _) = output_pages(&p, None, Layers::All).unwrap();
        let t = &pages[0].title;
        assert_eq!((t.number.as_str(), t.revision.as_str()), ("SB-1", "C"));
        assert_eq!(t.date, "", "no date property: empty, never a time stamp");
        assert!(t.sheet.starts_with("1/"));
        assert!(!t.title.is_empty(), "the design's name by default");
    }

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

    #[test]
    fn a_block_that_cant_be_laid_out_loses_only_its_pages() {
        let (_dir, p) = crate::netlist::tests::project(&[(
            "blocks/s/block.json",
            serde_json::json!({"kind": "ryueda.block", "format": 1, "name": 7}),
        )]);
        let (v, bytes) = output(&p, None, Layers::All).unwrap();
        let blocks: Vec<&str> = v["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["block"].as_str().unwrap())
            .collect();
        assert_eq!(blocks, ["top"]);
        let failed: Vec<(&str, &str)> = v["failed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["block"].as_str().unwrap(), x["path"].as_str().unwrap()))
            .collect();
        assert_eq!(failed, [("s", "ch_a"), ("s", "ch_b")]);
        assert!(bytes.starts_with(b"%PDF-"));
    }

    /// Each page's texts.
    fn texts(pages: &[Page]) -> Vec<Vec<String>> {
        pages
            .iter()
            .map(|pg| {
                pg.items
                    .iter()
                    .filter_map(|i| match i {
                        Item::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    /// A bus leaving its sheet: its connector carries its members' other
    /// pages, as a net's does.
    #[test]
    fn a_bus_connector_carries_its_members_pages() {
        use ryusch::layout::Instruction;
        let mut b = ryusch::testing::bus_block();
        b.instructions.insert(
            "i1".into(),
            Instruction::Sheet {
                parts: vec!["u2".into()],
                sheet: "Away".into(),
            },
        );
        let out = ryusch::engine::lay_out(&b).unwrap();
        let sid = out.layout.placed["u1"].sheet.clone();
        let paint = ryusch::paint::sheet(&b, &out.layout, &sid);
        assert_eq!(paint.buses.len(), 1, "{:?}", paint.buses);
        let bus = &paint.buses[0];
        assert_eq!(bus.bus, "bus:q");
        assert_eq!(bus.nets, ["d0", "d1", "d2", "d3"]);
        // d0 to d3 flatten to 0 to 3; d2 is also on page 3, the rest on
        // page 2: one cross-reference under the connector, both pages.
        let flat: BTreeMap<String, usize> = (0..4).map(|i| (format!("d{i}"), i)).collect();
        let mut net_pages: BTreeMap<usize, BTreeSet<usize>> =
            (0..4).map(|i| (i, BTreeSet::from([0, 1]))).collect();
        net_pages.get_mut(&2).unwrap().insert(2);
        let none = BTreeMap::new();
        let refs = cross_references(&paint, &flat, &net_pages, &none, 0);
        assert_eq!(refs, [(bus.rect, vec![(1, None), (2, None)])]);
        // With zones, each page its nets' least.
        let zones = BTreeMap::from([((0, 1), (1, 3)), ((3, 1), (0, 5)), ((2, 2), (2, 0))]);
        let refs = cross_references(&paint, &flat, &net_pages, &zones, 0);
        assert_eq!(
            refs,
            [(
                bus.rect,
                vec![(1, Some("A6".to_owned())), (2, Some("C1".to_owned()))]
            )]
        );
        // On no other page: none.
        let alone: BTreeMap<usize, BTreeSet<usize>> =
            (0..4).map(|i| (i, BTreeSet::from([0]))).collect();
        assert!(cross_references(&paint, &flat, &alone, &none, 0).is_empty());
    }

    #[test]
    fn cross_references_go_on_connectors_and_ports_not_local_labels() {
        use serde_json::json;
        // The top block's R2 asked onto a sheet of its own: SDA continues
        // there from R1's sheet. Block s's port goes back to the top.
        let r2 = json!({
            "format": 1,
            "kind": crate::conn::COMPONENT_KIND,
            "of": {"id": "r_100k"},
            "refdes": "R2",
            "pins": {"1": "sda", "2": {"rail": "gnd"}},
        });
        let (dir, _) = crate::netlist::tests::project(&[("blocks/top/components/r2.json", r2)]);
        let mut session = crate::session::Session::open(&dir.path().join("board")).unwrap();
        crate::schemcmd::place(
            &mut session,
            "top",
            &["idiom R2 none".to_owned(), "sheet R2 Away".to_owned()],
            &[],
            false,
            None,
            None,
        )
        .unwrap();
        session.reload().unwrap();
        let p = session.project();
        let (v, _, pages, _) = output_pages(p, None, Layers::All).unwrap();
        let listing: Vec<(&str, &str)> = v["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["block"].as_str().unwrap(), x["path"].as_str().unwrap()))
            .collect();
        let t = texts(&pages);
        // The top's two sheets point at each other.
        assert_eq!(&listing[..2], [("top", ""), ("top", "")], "{listing:?}");
        assert!(
            t[0].iter().any(|x| x.starts_with("→") && x.contains('2')),
            "{:?}",
            t[0]
        );
        assert!(
            t[1].iter().any(|x| x.starts_with("→") && x.contains('1')),
            "{:?}",
            t[1]
        );
        // Each layer of block s: its SDA port's flag points back up.
        for (pg, (b, _)) in listing.iter().enumerate().skip(2) {
            assert_eq!(*b, "s");
            assert!(
                t[pg].iter().any(|x| x.starts_with("→")),
                "page {}: {:?}",
                pg + 1,
                t[pg]
            );
        }
        // A local net label carries none: block s's sheets have no label
        // but its port's.
        let arrows = |pg: usize| t[pg].iter().filter(|x| x.starts_with("→")).count();
        assert_eq!(arrows(2), 1);
        // Each page a number, then its zone (plan 2.E): `→ 2 B4, 3 A1`.
        for x in t.iter().flatten().filter(|x| x.starts_with("→ ")) {
            for part in x["→ ".len()..].split(", ") {
                let (n, zone) = part.split_once(' ').expect("a page and its zone");
                assert!(n.parse::<usize>().is_ok(), "{x}");
                let mut c = zone.chars();
                assert!(c.next().is_some_and(|l| l.is_ascii_uppercase()), "{x}");
                assert!(c.as_str().parse::<usize>().is_ok(), "{x}");
            }
        }
    }

    #[test]
    fn a_cross_reference_names_the_page_and_zone() {
        assert_eq!(xref(&[(1, Some("B4".into())), (2, None)]), "→ 2 B4, 3");
    }
}
