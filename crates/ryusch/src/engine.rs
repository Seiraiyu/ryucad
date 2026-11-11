//! Laying a block out: keep what the previous layout holds where it still
//! fits, place what is new (by flow from nothing, else into free space
//! near its neighbours), then on each sheet draw rails and label nets,
//! route the rest, and put each part's text in the first free place.
//! Each step works in a fixed order (part keys, net keys), never a hash's.

use crate::cluster::Cluster;
use crate::font;
use crate::geom::{GRID, Nm, Place, Point, Rect, Side};
use crate::idiom;
use crate::input::{Block, NetKind, PartKind, Relayout};
use crate::instr;
use crate::label;
use crate::layout::{Instruction, Label, LabelKind, Layout, NetStyle, Placed, Sheet};
use crate::place::{self, Hints, MARGIN};
use crate::route::{self, Field, NO_NET};
use crate::shape::{Shape, grid_origin};
use crate::text;
use crate::text::Line;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Space kept between parts placed into a layout that has others.
pub const SPACING: Nm = 2 * GRID;
/// Between sheets on a block's canvas.
pub const SHEET_GAP: Nm = crate::canvas::GAP;
/// A net whose wires would run longer than this share of a sheet's width
/// takes labels.
const LONG_SHARE: (Nm, Nm) = (1, 4);
/// How long a bypass wire may run, as a share of the sheet's width: a
/// decoupling capacitor wired to its pin is the standard drawing however
/// far its idiom stood it, so it takes more than a signal's quarter (a
/// buck's input capacitors on an A4 sheet paper fit chose, plan 2.A).
const BYPASS_SHARE: (Nm, Nm) = (1, 2);
/// A connection crossing more wires than this takes labels instead.
const CROWDED: usize = 2;
/// How far a rail's pin with no room for its port may wire to its rail.
const RAIL_REACH: Nm = 10 * GRID;
/// How far a rail's pin wires to its rail rather than take a port.
const NEAR_RAIL: Nm = 4 * GRID;
/// How far a pin on a net drawn by labels wires to its net's own (a wire
/// or label of it near by) rather than take a label of its own. Plan 1.D
/// said 12 steps; on the corpus from nothing 16 wires three more parts
/// (the RP2040's switches and test pads) and longer reaches add none.
const LOCAL_REACH: Nm = 16 * GRID;
/// The router's net id of bus `i`'s spines and trunk: far past any net.
const BUS_NET: usize = usize::MAX / 2;

/// A warning about the result: an instruction whose subject is gone, a
/// sheet full.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    pub code: String,
    pub message: String,
    pub details: Value,
}

/// What a layout gives.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub layout: Layout,
    pub notes: Vec<Note>,
    /// Each sheet's nets that fell back to labels, by sheet: what lint
    /// reads as wires that didn't fit.
    pub fallbacks: BTreeMap<String, BTreeSet<String>>,
    /// The idioms found, as drawn.
    pub idioms: Vec<idiom::Idiom>,
    /// Parts an instruction lets run against the flow.
    pub flow: BTreeSet<String>,
    /// Parts a label of which found no place where they stand.
    pub crowded: BTreeSet<usize>,
    /// What stood where a relative instruction wanted a part, by part key:
    /// why it couldn't hold.
    pub blocked: BTreeMap<String, String>,
}

/// A part's sheet, place and whether a move holds it, if placed.
type Slot = Option<(usize, Place, bool)>;

/// Work in progress: each part's sheet and place.
#[derive(Clone)]
pub(crate) struct State<'a> {
    pub block: &'a Block,
    pub sheets: Vec<(String, Sheet)>,
    pub place: Vec<Option<(usize, Place, bool)>>,
    pub notes: Vec<Note>,
    /// The regions, by ID: their sheets, origins, titles and parts (their
    /// frames are worked out from what they hold once drawn).
    pub regions: Vec<(String, crate::layout::Region)>,
    /// What blocked a relative instruction's parts, by part key.
    pub blocked: BTreeMap<String, String>,
}

impl<'a> State<'a> {
    pub fn new(block: &'a Block) -> State<'a> {
        State {
            block,
            sheets: Vec::new(),
            place: vec![None; block.parts.len()],
            notes: Vec::new(),
            regions: Vec::new(),
            blocked: BTreeMap::new(),
        }
    }

    /// The sheet with ID `id`, or a new one of that ID.
    pub fn sheet_id(&mut self, id: &str, name: &str) -> usize {
        if let Some(i) = self.sheets.iter().position(|(s, _)| s == id) {
            return i;
        }
        let at = [
            self.sheets
                .iter()
                .map(|(_, s)| s.at[0] + self.block.area_of(s)[0] + SHEET_GAP)
                .max()
                .unwrap_or(0),
            0,
        ];
        self.sheets
            .push((id.to_owned(), Sheet::new(name.to_owned(), at)));
        self.sheets.len() - 1
    }

    /// A new sheet, its ID one past the largest `s<n>`, its name the one
    /// given, else `Sheet <n>` with the first `n` from there no sheet has.
    pub fn new_sheet(&mut self, name: Option<&str>) -> usize {
        let n = self
            .sheets
            .iter()
            .filter_map(|(id, _)| id.strip_prefix('s')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("s{n}");
        let name = match name {
            Some(x) => x.to_owned(),
            None => (n..)
                .map(|k| format!("Sheet {k}"))
                .find(|x| !self.sheets.iter().any(|(_, s)| &s.name == x))
                .expect("some name is free"),
        };
        self.sheet_id(&id, &name)
    }

    /// The area sheet `s`'s contents stay inside: its own paper's.
    pub fn area(&self, s: usize) -> Rect {
        Rect::new([0, 0], self.size(s))
    }

    /// Sheet `s`'s drawing area's size.
    pub fn size(&self, s: usize) -> [Nm; 2] {
        self.block.area_of(&self.sheets[s].1)
    }

    /// The room a placed part takes: its extent, its text and its rails'
    /// ports or labels.
    pub fn room(&self, part: usize) -> Rect {
        let (_, place, _) = self.place[part].expect("placed");
        Cluster::single(part, place).room(self.block, [0, 0])
    }

    pub fn shape(&self, part: usize) -> Option<Shape> {
        self.place[part].map(|(_, p, _)| Shape::of(&self.block.parts[part], p))
    }

    /// The parts on sheet `s`, in key order.
    pub fn on_sheet(&self, s: usize) -> Vec<usize> {
        let mut out: Vec<usize> = (0..self.block.parts.len())
            .filter(|p| self.place[*p].is_some_and(|(x, _, _)| x == s))
            .collect();
        out.sort_by(|a, b| self.block.parts[*a].key.cmp(&self.block.parts[*b].key));
        out
    }
}

/// A cluster's members' places at `origin`, on the grid.
fn at(c: &Cluster, origin: Point) -> Vec<(usize, Place)> {
    c.places(origin)
}

/// How a part is turned when nothing says otherwise: a connector faces
/// into the sheet from its left edge, so its pins point right; a part of
/// two pins standing up with ground on top, or a supply below, stands the
/// other way up, so its ground symbol points down and its supply up
/// straight off their pins.
pub fn default_turn(block: &Block, p: usize) -> (u16, bool) {
    let part = &block.parts[p];
    if crate::cluster::is_connector(block, p) {
        let left = part.pins.iter().filter(|x| x.side == Side::Left).count();
        if left * 2 > part.pins.len() {
            return (0, true);
        }
    }
    if let [a, b] = &part.pins[..]
        && !matches!(part.kind, PartKind::Port { .. })
    {
        let kind = |pin: &crate::input::Pin| pin.net.map(|n| block.nets[n].kind);
        let (up, down) = match (a.side, b.side) {
            (Side::Up, Side::Down) => (a, b),
            (Side::Down, Side::Up) => (b, a),
            _ => return (0, false),
        };
        let wrong = kind(up) == Some(NetKind::Ground) || kind(down) == Some(NetKind::Supply);
        let right = kind(up) == Some(NetKind::Supply) || kind(down) == Some(NetKind::Ground);
        if wrong && !right {
            return (180, false);
        }
    }
    (0, false)
}

/// A part alone, turned so, its first pin on the grid at the origin.
fn single(block: &Block, p: usize, (r, m): (u16, bool)) -> Cluster {
    let place = Place::new([0, 0], r, m);
    let o = grid_origin(&block.parts[p], &place, [0, 0]);
    Cluster::single(p, Place::new(o, r, m))
}

/// The sheet named `name`, or a new one so named.
fn sheet_named(st: &mut State, name: &str) -> usize {
    match st.sheets.iter().position(|(_, s)| s.name == name) {
        Some(i) => i,
        None => st.new_sheet(Some(name)),
    }
}

/// A row of clusters, left to right two steps apart: a `group`.
fn row(block: &Block, parts: &[&Cluster]) -> Cluster {
    let mut members = Vec::new();
    let mut x = 0;
    for c in parts {
        let r = c.room(block, [0, 0]);
        let dx = crate::geom::snap(x - r.min[0]);
        let dy = crate::geom::snap(-r.max[1]);
        for (p, place) in c.places([dx, dy]) {
            members.push((p, place));
        }
        x = r.shift([dx, dy]).max[0] + 2 * GRID;
    }
    Cluster {
        members,
        idiom: None,
        frame: 0,
    }
}

/// Lays `block` out, or refuses when its instructions can't hold.
pub fn lay_out(block: &Block) -> Result<Outcome, instr::Refusal> {
    let r = instr::resolve(block)?;
    let fresh = block.previous.is_none() || block.relayout == Relayout::Block;
    let mut out = attempt(block, &r, 0, &BTreeSet::new());
    // From nothing, spread the columns where labels or text didn't fit;
    // over a previous layout nothing kept may move, but a part whose label
    // found no place where it stands (its label grew: a stack's new layer,
    // a longer name) is placed again, into free space near its neighbours.
    if fresh {
        for spread in [4 * GRID, 8 * GRID, 12 * GRID] {
            if crate::rules::check(block, &out.layout).is_empty() {
                break;
            }
            out = attempt(block, &r, spread, &BTreeSet::new());
        }
        // A fitted paper too tight to keep the hard rules: every sheet on
        // the project's paper, as with fit off.
        if !block.papers.is_empty() && !crate::rules::check(block, &out.layout).is_empty() {
            let mut whole = block.clone();
            whole.papers.clear();
            let mut again = attempt(&whole, &r, 0, &BTreeSet::new());
            for spread in [4 * GRID, 8 * GRID, 12 * GRID] {
                if crate::rules::check(&whole, &again.layout).is_empty() {
                    break;
                }
                again = attempt(&whole, &r, spread, &BTreeSet::new());
            }
            if crate::rules::check(&whole, &again.layout).is_empty() {
                out = again;
            }
        }
    } else {
        // A part a move put where it stands is held there: never placed
        // again for its labels (a shift that leaves one without room is
        // refused below, not undone).
        let again: BTreeSet<usize> = out
            .crowded
            .iter()
            .copied()
            .filter(|p| !r.moved.contains(p))
            .collect();
        if !again.is_empty() {
            out = attempt(block, &r, 0, &again);
        }
    }
    // A move is applied as asked and holds its part where it puts it: one
    // that leaves a hard rule broken is refused, naming it -- a shift for
    // any (a part over another's room, or off the sheet), another move
    // where the rule names a part it moved (a connector turned to face
    // the sheet's edge, its pins' labels with no room; plan 2.G).
    if !block.moves.is_empty()
        && let Some(v) = crate::rules::check(block, &out.layout).first()
        && let Some(n) = block.moves.iter().position(|m| {
            matches!(m, Instruction::Shift { .. })
                || m.parts()
                    .iter()
                    .flat_map(|s| instr::parts_named(block, s))
                    .any(|p| names_part(&v.refs, &block.parts[p].key))
        })
    {
        let id = format!("move {}", n + 1);
        return Err(instr::Refusal {
            code: "layout_rule",
            message: format!("{id} would break a hard rule ({}): {}", v.rule, v.message),
            details: json!({"instructions": [id], "rule": v.rule}),
        });
    }
    Ok(out)
}

/// Whether a hard rule's refs (a part's key, `label N at KEY/PIN`, `KEY's
/// value`, `KEY's connector` …) name the part keyed `key`.
fn names_part(refs: &[String], key: &str) -> bool {
    refs.iter().any(|r| {
        r == key
            || r.starts_with(&format!("{key}'s "))
            || r.split(' ')
                .any(|w| w.split_once('/').is_some_and(|(k, _)| k == key))
    })
}

/// Parts a released instruction moved, back where they stood before it:
/// not placed yet, named by no lasting instruction, their sheet still
/// there and their room there clear of every part placed.
fn restore(st: &mut State, idioms: &[idiom::Idiom]) {
    let block = st.block;
    let named: BTreeSet<usize> = block
        .instructions
        .values()
        .flat_map(|i| i.parts().iter().flat_map(|s| instr::parts_named(block, s)))
        .collect();
    for (key, at) in &block.restore {
        let Some(p) = block.part(key) else { continue };
        if st.place[p].is_some() || named.contains(&p) {
            continue;
        }
        let Some(s) = st.sheets.iter().position(|(id, _)| *id == at.sheet) else {
            continue;
        };
        // Its extent a step clear of every placed part's, as the spacing
        // rule asks (text and labels find their places when drawn).
        let place = Place::new(at.at, at.rotation, at.mirror);
        let shape = Shape::of(&block.parts[p], place);
        let e = shape.extent;
        // A part of two pins across the page, by its row (as the rule
        // reads a bank's members: on different rows, extents apart is
        // enough).
        let row = |sh: &Shape| {
            (sh.pins.len() == 2
                && sh.pins.iter().all(|x| x.side.horizontal())
                && sh.pins[0].end[1] == sh.pins[1].end[1])
                .then(|| sh.pins[0].end[1])
        };
        let clear = (0..block.parts.len()).all(|q| match st.place[q] {
            Some((sq, _, _)) if sq == s && q != p => {
                let o = st.shape(q).expect("placed");
                match (row(&shape), row(&o)) {
                    (Some(a), Some(b)) if a != b => !e.overlaps(&o.extent),
                    _ => !e.grow(GRID / 2).overlaps(&o.extent.grow(GRID / 2 - 1)),
                }
            }
            _ => true,
        });
        if clear && Rect::new([0, 0], st.size(s)).contains(&e) {
            st.place[p] = Some((s, place, false));
            // Back in the region of the part its idiom serves, where that
            // is on its sheet.
            let host = idioms
                .iter()
                .find(|i| i.parts.contains(&p))
                .and_then(|i| i.anchor)
                .map(|(a, _)| block.parts[a].key.clone());
            if let Some(host) = host
                && let Some(i) = st
                    .regions
                    .iter()
                    .position(|(_, r)| r.sheet == at.sheet && r.parts.contains(&host))
            {
                for (_, r) in st.regions.iter_mut() {
                    r.parts.retain(|k| k != key);
                }
                let parts = &mut st.regions[i].1.parts;
                parts.push(key.clone());
                parts.sort();
            }
        }
    }
}

/// One layout of `block` under `r`, its columns spread by `spread`.
fn attempt(block: &Block, r: &instr::Resolved, spread: Nm, again: &BTreeSet<usize>) -> Outcome {
    // A network's parts the walk from its pin never reaches are left
    // where they are, with a note.
    let mut r = r.clone();
    let mut networks = Vec::new();
    let mut unreached_notes = Vec::new();
    // A part a `region` instruction holds apart from the pin's part isn't
    // the network's, as an idiom's isn't.
    let named_region = |p: usize| r.regions.iter().position(|(_, set)| set.contains(&p));
    for (parts, (a, k)) in std::mem::take(&mut r.networks) {
        let (inside, apart): (Vec<usize>, Vec<usize>) = parts
            .iter()
            .partition(|q| named_region(**q) == named_region(a));
        let (reached, mut rest) = idiom::network_walk(block, &inside, (a, k));
        rest.extend(apart);
        for q in rest {
            let pin = format!(
                "{}.{}",
                block.parts[a].refdes, block.parts[a].pins[k].number
            );
            let why = if block.parts[q].pins.len() == 2
                && matches!(block.parts[q].kind, PartKind::Symbol { .. })
            {
                format!("isn't joined to {pin} through the others")
            } else {
                "isn't a part of two pins".to_owned()
            };
            unreached_notes.push(Note {
                code: "network_unresolved".into(),
                message: format!(
                    "{} {why}: left where it is (near {} alone, or a region instruction holding it, would move it)",
                    crate::lint::who(&block.parts[q]),
                    pin
                ),
                details: json!({"part": block.parts[q].key, "pin": pin}),
            });
        }
        let reached: Vec<usize> = reached.into_iter().map(|(q, _)| q).collect();
        if !reached.is_empty() {
            networks.push((reached, (a, k)));
        }
    }
    r.networks = networks;
    let r = &r;
    let mut st = State::new(block);
    st.notes.extend(unreached_notes);
    st.notes.extend(r.notes.iter().cloned());
    let prev = match &block.relayout {
        Relayout::Block => None,
        _ => block.previous.as_ref(),
    };
    let fresh_sheet = match &block.relayout {
        Relayout::Sheet(s) => Some(s.as_str()),
        _ => None,
    };
    // Sheets the previous layout had, in ID order.
    if let Some(prev) = prev {
        for (id, s) in &prev.sheets {
            let i = st.sheet_id(id, &s.name);
            // Its paper as the previous layout had it.
            let sheet = &mut st.sheets[i].1;
            sheet.size = s.size.clone();
            sheet.orientation = s.orientation;
            sheet.pinned = s.pinned;
        }
    }
    // What stays where it was: not what this batch moves, nor what a
    // lasting instruction says should be elsewhere. One the previous layout
    // was made with and couldn't keep is left as it is: trying again from
    // another start would only move its parts on every edit.
    // Parts a relative instruction (`beside`, `near`, `align`) moves stay
    // where they were to start, their satellites with them, and `relate`
    // moves them from there the least distance (round 1, F5: re-placed
    // first, they were thrown across the sheet). Any other reason to lay
    // a part out again wins.
    let relative = |i: &Instruction| {
        matches!(
            i,
            Instruction::Beside { .. }
                | Instruction::Near { .. }
                | Instruction::Align { .. }
                | Instruction::Shift { .. }
        )
    };
    let mut unkeep: BTreeSet<usize> = BTreeSet::new();
    let mut soft: BTreeSet<usize> = BTreeSet::new();
    for ins in &block.moves {
        let to = if relative(ins) {
            &mut soft
        } else {
            &mut unkeep
        };
        for s in ins.parts() {
            to.extend(instr::parts_named(block, s));
        }
    }
    // Whatever else resolve counts as moved (anything named by a move).
    unkeep.extend(r.moved.iter().filter(|p| !soft.contains(p)));
    if let Some(prev) = prev {
        for ins in block.instructions.values() {
            let tried = prev.instructions.values().any(|x| x == ins);
            if positional(ins) && !tried && !instr::achieved(block, prev, ins) {
                let to = if relative(ins) {
                    &mut soft
                } else {
                    &mut unkeep
                };
                for s in ins.parts() {
                    to.extend(instr::parts_named(block, s));
                }
            }
        }
    }
    let mut idioms = idiom::find(block, &r.idiom);
    // A network places its own parts: no idiom takes them.
    let networked: BTreeSet<usize> = r
        .networks
        .iter()
        .flat_map(|(p, _)| p.iter().copied())
        .collect();
    idioms.retain(|i| !i.parts.iter().any(|p| networked.contains(p)));
    // A part held to an edge joins no other by an idiom, and none joins
    // it: their cluster would carry it off the edge, or push it from it.
    idioms.retain(|i| match i.anchor {
        None => true,
        Some((a, _)) => !r.edge.contains_key(&a) && !i.parts.iter().any(|p| r.edge.contains_key(p)),
    });
    // A part a `region` instruction names leaves an idiom whose other
    // parts or anchor that region doesn't hold: the agent's region wins.
    let named_region = |p: usize| r.regions.iter().position(|(_, set)| set.contains(&p));
    idioms.retain(|i| {
        let mut all: Vec<usize> = i.parts.clone();
        all.extend(i.anchor.map(|(a, _)| a));
        let first = named_region(all[0]);
        all.iter().all(|p| named_region(*p) == first)
    });
    // A part an instruction moves brings the idioms serving it: kept where
    // they were, they would hold it there. A part placed again for its
    // label alone goes by itself.
    for i in &idioms {
        if i.anchor.is_some_and(|(a, _)| unkeep.contains(&a)) {
            unkeep.extend(i.parts.iter().copied());
        }
    }
    // A network goes where its pin's part goes.
    for (parts, (a, _)) in &r.networks {
        if unkeep.contains(a) {
            unkeep.extend(parts.iter().copied());
        }
    }
    let mut reformed: BTreeSet<usize> = BTreeSet::new();
    // A lasting `idiom X kind` the previous layout wasn't made with
    // re-forms the idioms of that kind X is in or anchors: their parts
    // are laid out again (their holds dropped), the anchor kept (round 1,
    // F4: a stage's feedback, moved away, came back into shape only so).
    if let Some(prev) = prev {
        for ins in block.instructions.values() {
            let Instruction::Idiom {
                parts,
                kind: crate::layout::IdiomChoice::Kind(k),
            } = ins
            else {
                continue;
            };
            if prev.instructions.values().any(|x| x == ins) {
                continue;
            }
            let named: BTreeSet<usize> = parts
                .iter()
                .flat_map(|s| instr::parts_named(block, s))
                .collect();
            for i in idioms.iter().filter(|i| i.kind == *k) {
                let anchored = i.anchor.is_some_and(|(a, _)| named.contains(&a));
                if anchored || i.parts.iter().any(|p| named.contains(p)) {
                    unkeep.extend(i.parts.iter().copied());
                    // Its own signal nets are asked for as wires: labels
                    // the previous layout gave them would otherwise stay.
                    let mut with: Vec<usize> = i.parts.clone();
                    with.extend(i.anchor.map(|(a, _)| a));
                    for &p in &i.parts {
                        for n in block.parts[p].pins.iter().filter_map(|x| x.net) {
                            let own = block.nets[n].kind == NetKind::Signal
                                && block.parts.iter().enumerate().any(|(q, part)| {
                                    q != p
                                        && with.contains(&q)
                                        && part.pins.iter().any(|x| x.net == Some(n))
                                });
                            if own {
                                reformed.insert(n);
                            }
                        }
                    }
                }
            }
        }
    }
    unkeep.extend(again.iter().copied());
    if let Relayout::Parts(keys) = &block.relayout {
        for k in keys {
            unkeep.extend(instr::parts_named(block, k));
        }
    }
    soft.retain(|p| !unkeep.contains(p));
    for i in &idioms {
        if i.anchor.is_some_and(|(a, _)| soft.contains(&a)) {
            soft.extend(i.parts.iter().filter(|p| !unkeep.contains(p)));
        }
    }
    keep(&mut st, prev, fresh_sheet, &unkeep);
    restore(&mut st, &idioms);
    named_regions(&mut st, r);
    pin_papers(&mut st, r);
    // What `relate` may not move: kept parts no relative instruction moves.
    let kept: BTreeSet<usize> = (0..block.parts.len())
        .filter(|p| st.place[*p].is_some() && !soft.contains(p))
        .collect();
    let turn = |p: usize| -> (u16, bool) {
        if let Some(t) = r.orient.get(&p) {
            return *t;
        }
        prev.and_then(|l| l.placed.get(&block.parts[p].key))
            .map_or_else(|| default_turn(block, p), |x| (x.rotation, x.mirror))
    };
    // Every part's own cluster (a kept part where it is), joined by
    // idioms, then groups.
    let singles: Vec<Cluster> = (0..block.parts.len())
        .map(|p| match st.place[p] {
            Some((_, place, _)) => Cluster::single(p, place),
            None => single(block, p, turn(p)),
        })
        .collect();
    let asks = idiom::Asks {
        networks: r.networks.clone(),
        ports: r.port_rows.clone(),
    };
    let mut clusters = idiom::clusters(block, &idioms, singles, &asks);
    for g in &r.groups {
        let inside: Vec<usize> = (0..clusters.len())
            .filter(|i| clusters[*i].members.iter().any(|(p, _)| g.contains(p)))
            .collect();
        if inside.iter().any(|i| {
            clusters[*i]
                .members
                .iter()
                .any(|(p, _)| st.place[*p].is_some())
        }) {
            continue;
        }
        let made = row(
            block,
            &inside.iter().map(|i| &clusters[*i]).collect::<Vec<_>>(),
        );
        let mut i = 0;
        clusters.retain(|_| {
            let gone = inside.contains(&i);
            i += 1;
            !gone
        });
        clusters.push(made);
    }
    let kept_any = st.place.iter().any(Option::is_some);
    let is_new = |c: &Cluster| c.members.iter().all(|(p, _)| st.place[*p].is_none());
    // The sheet a cluster is asked onto: its first member's that names one.
    let named = |c: &Cluster| c.members.iter().find_map(|(p, _)| r.sheet.get(p).cloned());
    let bank_nets = idiom::bank_nets(block, &idioms, &clusters);
    // A port asked onto a banked row stays with the ports.
    for (&f, &(a, k)) in &r.port_rows {
        if block.parts[a].pins[k]
            .net
            .is_some_and(|n| bank_nets.contains(&n))
        {
            st.notes.push(Note {
                code: "port_row_banked".into(),
                message: format!(
                    "{}'s row is a bank's, which ends in its pull: the flag stays with the ports",
                    crate::lint::who(&block.parts[a])
                ),
                details: json!({"port": block.parts[f].key, "pin": format!("{}/{}", block.parts[a].key, block.parts[a].pins[k].number)}),
            });
        }
    }
    let mut hints = Hints {
        spread,
        apart: bank_nets.clone(),
        ..Hints::default()
    };
    for (i, c) in clusters.iter().enumerate() {
        if let Some(side) = c.members.iter().find_map(|(p, _)| r.edge.get(p)) {
            hints.edge.insert(i, *side);
        }
    }
    let mut fresh = Vec::new();
    let mut again = Vec::new();
    let mut mixed = Vec::new();
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in clusters.iter().enumerate() {
        let from_sheet = fresh_sheet.is_some_and(|sid| {
            c.members.iter().all(|(p, _)| {
                prev.and_then(|l| l.placed.get(&block.parts[*p].key))
                    .is_some_and(|x| x.sheet == sid)
            })
        });
        if !is_new(c) {
            mixed.push(i);
        } else if let Some(name) = named(c).filter(|_| !kept_any) {
            by_name.entry(name).or_default().push(i);
        } else if from_sheet {
            again.push(i);
        } else {
            fresh.push(i);
        }
    }
    if !kept_any && again.is_empty() {
        // Sheets asked for by name first, so the rest never flows onto one.
        let only_named = fresh.is_empty() && !by_name.is_empty();
        if let Some(id) = fresh_sheet.filter(|_| !only_named) {
            // The sheet laid out again keeps its ID.
            st.sheet_id(id, id);
        }
        let proposed = crate::region::propose(block, &idioms, r);
        for (name, list) in by_name {
            pack_regions(
                &mut st,
                &clusters,
                list,
                Target::Named(&name),
                &hints,
                r,
                &proposed,
            );
        }
        if !only_named {
            pack_regions(
                &mut st,
                &clusters,
                fresh,
                Target::Fresh(fresh_sheet),
                &hints,
                r,
                &proposed,
            );
        }
    } else {
        if !again.is_empty() {
            flow_onto_sheets(
                &mut st,
                &clusters,
                again,
                Target::Fresh(fresh_sheet),
                &hints,
            );
        }
        insert(&mut st, prev, &clusters, &fresh, &mixed, r, &hints);
    }
    // Shifts: each part nudged from where it stands, with the cluster it
    // leads, before relative instructions are judged.
    for (parts, side, steps) in &r.shift {
        let d = side.step();
        let by = [d[0] * Nm::from(*steps), d[1] * Nm::from(*steps)];
        for &p in parts {
            let moving: Vec<usize> = clusters
                .iter()
                .find(|c| c.first() == p)
                .map_or_else(|| vec![p], |c| c.members.iter().map(|(q, _)| *q).collect());
            for q in moving {
                if let Some((sh, pl, held)) = st.place[q] {
                    let at = [pl.at[0] + by[0], pl.at[1] + by[1]];
                    st.place[q] = Some((sh, Place::new(at, pl.rotation, pl.mirror), held));
                }
            }
        }
    }
    let before: Vec<Option<(usize, Place, bool)>> = st.place.clone();
    // Regions a relative instruction names as its subject (`beside
    // region:r3 U1 right`) move whole.
    let whole: Vec<BTreeSet<usize>> = block
        .instructions
        .values()
        .chain(&block.moves)
        .filter(|i| {
            matches!(
                i,
                Instruction::Beside { .. } | Instruction::Near { .. } | Instruction::Align { .. }
            )
        })
        .flat_map(|i| i.parts().iter().filter(|s| s.starts_with("region:")))
        .map(|s| instr::parts_named(block, s).into_iter().collect())
        .filter(|set: &BTreeSet<usize>| !set.is_empty())
        .collect();
    relate(&mut st, prev, &clusters, r, &kept, &whole);
    regions_follow(&mut st, &before, r, &whole);
    for p in &r.moved {
        if let Some((s, place, _)) = st.place[*p] {
            st.place[*p] = Some((s, place, true));
        }
    }
    // Each decoupling capacitor's supply pin, with the pin it serves: drawn
    // joined by a wire, on which their rail's one port sits.
    let bypass: Vec<Bypass> = idioms
        .iter()
        .filter(|i| i.kind == crate::layout::IdiomKind::Decoupling)
        .filter_map(|i| {
            let (a, k) = i.anchor?;
            let c = i.parts[0];
            let n = block.parts[a].pins[k].net?;
            let kc = block.parts[c].pins.iter().position(|x| x.net == Some(n))?;
            Some(Bypass {
                net: n,
                served: (a, k),
                cap: (c, kc),
            })
        })
        .collect();
    // A buck's output capacitors likewise, joined by a wire to the
    // inductor's far end.
    let mut bypass = bypass;
    for i in idioms
        .iter()
        .filter(|i| i.kind == crate::layout::IdiomKind::Buck)
    {
        let Some((a, k)) = i.anchor else { continue };
        let l = i.parts[0];
        let sw = block.parts[a].pins[k].net;
        let Some(lo) = block.parts[l].pins.iter().position(|x| x.net != sw) else {
            continue;
        };
        let Some(n) = block.parts[l].pins[lo]
            .net
            .filter(|n| block.nets[*n].kind == NetKind::Supply)
        else {
            continue;
        };
        for &c in &i.parts[1..] {
            if let Some(kc) = block.parts[c].pins.iter().position(|x| x.net == Some(n)) {
                bypass.push(Bypass {
                    net: n,
                    served: (l, lo),
                    cap: (c, kc),
                });
            }
        }
    }
    let mut styles = r.styles.clone();
    for n in reformed {
        styles.entry(n).or_insert(crate::layout::NetStyle::Wire);
    }
    pin_papers(&mut st, r);
    let rows = bank_rows(&st, &idioms);
    let mut out = draw(st, prev, &styles, &r.bus_styles, &bypass, &rows, &r.text);
    out.layout.instructions = block.instructions.clone();
    // What each lasting instruction moved stood before it, while it lasts.
    if let Some(p) = &block.previous {
        out.layout.before = p
            .before
            .iter()
            .filter(|(id, _)| block.instructions.contains_key(*id))
            .map(|(id, m)| (id.clone(), m.clone()))
            .collect();
    }
    out.idioms = idioms;
    out.flow = r.flow.iter().map(|p| block.parts[*p].key.clone()).collect();
    out
}

/// After `relate`: a part a `beside` or `near` instruction moved joins the
/// region of the part it stands by (with the parts that moved with it),
/// and a region left with nothing goes.
fn regions_follow(
    st: &mut State,
    before: &[Option<(usize, Place, bool)>],
    r: &instr::Resolved,
    whole: &[BTreeSet<usize>],
) {
    let block = st.block;
    // A region moved whole: its origin moves with it, so its parts keep
    // their places in it.
    for (_, g) in st.regions.iter_mut() {
        let offsets: BTreeSet<Option<Point>> = g
            .parts
            .iter()
            .filter_map(|k| block.part(k))
            .map(|p| match (before[p], st.place[p]) {
                (Some((a, x, _)), Some((b, y, _))) if a == b => {
                    Some([y.at[0] - x.at[0], y.at[1] - x.at[1]])
                }
                _ => None,
            })
            .collect();
        if let [Some(d)] = offsets.iter().copied().collect::<Vec<_>>().as_slice()
            && *d != [0, 0]
            && g.parts.len() > 1
        {
            g.at = [g.at[0] + d[0], g.at[1] + d[1]];
        }
    }
    let in_whole = |p: usize| whole.iter().any(|w| w.contains(&p));
    let region_of = |st: &State, p: usize| {
        st.regions
            .iter()
            .position(|(_, g)| g.parts.contains(&block.parts[p].key))
    };
    let targets: Vec<(usize, usize)> = r
        .beside
        .iter()
        .map(|(p, (q, _))| (*p, *q))
        .chain(r.near.iter().map(|(p, (q, _))| (*p, *q)))
        .collect();
    let moved: BTreeSet<usize> = (0..block.parts.len())
        .filter(|p| st.place[*p].map(|x| x.1) != before[*p].map(|x| x.1))
        .collect();
    for (p, q) in targets {
        if !moved.contains(&p) || in_whole(p) {
            continue;
        }
        let (Some(from), Some(to)) = (region_of(st, p), region_of(st, q)) else {
            continue;
        };
        if from == to || st.regions[from].1.named {
            continue;
        }
        // The parts of p's region that moved with it go too.
        let going: Vec<String> = st.regions[from]
            .1
            .parts
            .iter()
            .filter(|k| block.part(k).is_some_and(|x| x == p || moved.contains(&x)))
            .cloned()
            .collect();
        st.regions[from].1.parts.retain(|k| !going.contains(k));
        let dest = &mut st.regions[to].1;
        dest.parts.extend(going);
        dest.parts.sort();
        dest.parts.dedup();
    }
    st.regions.retain(|(_, g)| !g.parts.is_empty());
}

/// Whether a cluster is a guest, placed beside its host rather than by
/// flow: every member a symbol or a child block's symbol of at most 8
/// pins, no connector, nothing an `edge` or `sheet` instruction holds;
/// and no powered part (a power input on a supply) or child block with
/// satellites (an IC and its decoupling, a sensor block and its pull-ups:
/// those are hosts). A clamp array and its terminations are guests.
pub(crate) fn guest(block: &Block, cl: &Cluster, r: &instr::Resolved) -> bool {
    let small = |p: usize| {
        let part = &block.parts[p];
        part.pins.len() <= 8
            && matches!(part.kind, PartKind::Symbol { .. } | PartKind::Block)
            && !crate::cluster::is_connector(block, p)
            && !r.edge.contains_key(&p)
            && !r.sheet.contains_key(&p)
            && !on_bus(block, p)
    };
    let powered = |p: usize| {
        matches!(block.parts[p].kind, PartKind::Block)
            || block.parts[p].pins.iter().any(|x| {
                x.kind == crate::input::PinKind::PowerIn
                    && x.drawn_net()
                        .is_some_and(|n| block.nets[n].kind == NetKind::Supply)
            })
    };
    let m = &cl.members;
    m.iter().all(|(p, _)| small(*p)) && (m.len() == 1 || !m.iter().any(|(p, _)| powered(*p)))
}

/// Whether part `p` has a pin on as many of one bus's members as a
/// bundle draws (plan 2.C): it stands where flow puts it, a channel
/// wide enough for the bus from its neighbour, never beside a host.
fn on_bus(block: &Block, p: usize) -> bool {
    block.buses.iter().any(|b| {
        b.members
            .iter()
            .filter(|n| block.parts[p].pins.iter().any(|x| x.net == Some(**n)))
            .count()
            >= crate::bus::MIN_MEMBERS
    })
}

/// The signal nets a set of parts' pins are drawn on.
fn signal_nets(block: &Block, inside: &BTreeSet<usize>) -> BTreeSet<usize> {
    inside
        .iter()
        .flat_map(|p| block.parts[*p].pins.iter().filter_map(|x| x.drawn_net()))
        .filter(|n| block.nets[*n].kind == NetKind::Signal)
        .collect()
}

/// A cluster's host among every other part: the part sharing most of
/// its signal nets, ties to a connector, then the least key; a child
/// block's symbol is a guest only of a part with more pins.
pub(crate) fn host(block: &Block, cl: &Cluster) -> Option<usize> {
    host_among(block, cl, |_| true)
}

/// [`host`] among the parts `may` allows.
fn host_among(block: &Block, cl: &Cluster, may: impl Fn(usize) -> bool) -> Option<usize> {
    let inside: BTreeSet<usize> = cl.members.iter().map(|(p, _)| *p).collect();
    let mine = signal_nets(block, &inside);
    if mine.is_empty() {
        return None;
    }
    let mut host: Option<((usize, bool), usize)> = None;
    let mut by_key: Vec<usize> = (0..block.parts.len()).collect();
    by_key.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
    for q in by_key {
        if inside.contains(&q) || !may(q) {
            continue;
        }
        let shared = block.parts[q]
            .pins
            .iter()
            .filter_map(|x| x.drawn_net())
            .filter(|n| mine.contains(n))
            .collect::<BTreeSet<_>>()
            .len();
        if shared == 0 {
            continue;
        }
        let score = (shared, crate::cluster::is_connector(block, q));
        if host.is_none_or(|(h, _)| score > h) {
            host = Some((score, q));
        }
    }
    let (_, q) = host?;
    let biggest_block = cl
        .members
        .iter()
        .filter(|(p, _)| matches!(block.parts[*p].kind, PartKind::Block))
        .map(|(p, _)| block.parts[*p].pins.len())
        .max();
    if biggest_block.is_some_and(|b| block.parts[q].pins.len() <= b) {
        return None;
    }
    Some(q)
}

/// [`beside_host`]'s answer: the sheet, the origin wanted, the leading
/// member (its part, its place in the cluster, its pin) and the host's
/// pin end.
type Beside = (usize, Point, (usize, Place, usize), Point);

/// Where a guest cluster stands beside its host: the host is the placed
/// part (on sheet `only`, if given) sharing most of the cluster's signal
/// nets, ties to a connector, then the least key; a child block's symbol
/// is a guest only of a larger part (a status LED block beside its PHY,
/// never a sensor block beside the connector it talks through). The
/// cluster's origin puts its member's pin on their first shared net four
/// steps out of the host's pin. Gives the sheet, the origin wanted, the
/// member and pin that lead, and the host's pin end.
fn beside_host(st: &State, cl: &Cluster, only: Option<usize>) -> Option<Beside> {
    let block = st.block;
    let inside: BTreeSet<usize> = cl.members.iter().map(|(p, _)| *p).collect();
    let mine = signal_nets(block, &inside);
    let q = host_among(block, cl, |q| {
        st.place[q].is_some_and(|(s, _, _)| only.is_none_or(|o| o == s))
    })?;
    let s = st.place[q].expect("placed").0;
    let hs = st.shape(q)?;
    // A member's pin and the host's on the first shared net.
    let (m, k, j) = cl.members.iter().enumerate().find_map(|(m, (p, _))| {
        block.parts[*p].pins.iter().enumerate().find_map(|(k, x)| {
            let n = x.drawn_net().filter(|n| mine.contains(n))?;
            let j = block.parts[q]
                .pins
                .iter()
                .position(|y| y.drawn_net() == Some(n))?;
            Some((m, k, j))
        })
    })?;
    let (mp, mrel) = cl.members[m];
    let end = hs.pins[j].end;
    let step = hs.pins[j].side.step();
    let off = Shape::of(&block.parts[mp], mrel).pins[k].end;
    let want = [end[0] + 4 * step[0] - off[0], end[1] + 4 * step[1] - off[1]];
    Some((s, want, (mp, mrel, k), end))
}

/// Cross-cluster placement (plan 1.D, work item 4530): a guest cluster
/// (see [`guest`]) flowed from nothing moves, whole, to the free spot
/// nearest where [`beside_host`] wants it on its sheet, when that brings
/// the leading pins closer and they are more than 8 steps apart. Clusters
/// with fewest pins move first. Layouts made over a previous one never
/// take this pass, so stability is unchanged; [`insert`] places a new
/// guest the same way.
fn attract(st: &mut State, clusters: &[Cluster], r: &instr::Resolved) {
    let block = st.block;
    let mut order: Vec<usize> = (0..clusters.len())
        .filter(|c| guest(block, &clusters[*c], r))
        .collect();
    let pins_of = |c: usize| -> usize {
        clusters[c]
            .members
            .iter()
            .map(|(p, _)| block.parts[*p].pins.len())
            .sum()
    };
    order.sort_by(|a, b| {
        (pins_of(*a), &block.parts[clusters[*a].first()].key)
            .cmp(&(pins_of(*b), &block.parts[clusters[*b].first()].key))
    });
    for c in order {
        let cl = &clusters[c];
        let first = cl.first();
        let Some((s, _, _)) = st.place[first] else {
            continue;
        };
        if cl
            .members
            .iter()
            .any(|(p, _)| st.place[*p].is_none_or(|(x, _, _)| x != s))
        {
            continue;
        }
        let Some((_, want, (mp, mrel, k), end)) = beside_host(st, cl, Some(s)) else {
            continue;
        };
        let now = Shape::of(&block.parts[mp], st.place[mp].expect("placed").1).pins[k].end;
        let d = (end[0] - now[0]).abs() + (end[1] - now[1]).abs();
        if d <= 8 * GRID {
            continue;
        }
        let saved: Vec<_> = cl.members.iter().map(|(p, _)| (*p, st.place[*p])).collect();
        for (p, _) in &cl.members {
            st.place[*p] = None;
        }
        let e0 = cl.room_with_labels(block, [0, 0]);
        let moved = spot(st, s, e0, want, SPACING, &[]).filter(|o| {
            let ne = Shape::of(&block.parts[mp], mrel).pins[k].end;
            let ne = [ne[0] + o[0], ne[1] + o[1]];
            (ne[0] - end[0]).abs() + (ne[1] - end[1]).abs() < d
        });
        match moved {
            Some(o) => {
                for (p, place) in cl.places(o) {
                    st.place[p] = Some((s, place, false));
                }
            }
            None => {
                for (p, was) in saved {
                    st.place[p] = was;
                }
            }
        }
    }
}

/// Whether an instruction says where parts go.
fn positional(i: &Instruction) -> bool {
    matches!(
        i,
        Instruction::Sheet { .. }
            | Instruction::Orient { .. }
            | Instruction::Beside { .. }
            | Instruction::Near { .. }
            | Instruction::Edge { .. }
            | Instruction::Align { .. }
            | Instruction::Group { .. }
    )
}

/// Moves what `beside`, `near` and `align` say is out of place, in
/// instruction order: a part leading its cluster moves with the cluster,
/// any other alone, to the free spot nearest where it should be.
/// Parts `kept` where the previous layout put them stay there.
fn relate(
    st: &mut State,
    prev: Option<&Layout>,
    clusters: &[Cluster],
    r: &instr::Resolved,
    kept: &BTreeSet<usize>,
    whole: &[BTreeSet<usize>],
) {
    let drawn = drawn(prev);
    // What moves with `p`: its whole region where an instruction names
    // the region, else its whole cluster if it leads one, else itself.
    let movers = |p: usize| -> Vec<usize> {
        if let Some(set) = whole.iter().find(|w| w.contains(&p)) {
            return set.iter().copied().collect();
        }
        clusters
            .iter()
            .find(|c| c.first() == p)
            .map_or_else(|| vec![p], |c| c.members.iter().map(|(q, _)| *q).collect())
    };
    // A region moved whole moves once: by its first part.
    let skip = |p: usize| {
        whole
            .iter()
            .any(|w| w.contains(&p) && w.first() != Some(&p))
    };
    // The room `list` takes as it lies, and its sheet.
    let room = |st: &State, list: &[usize]| -> Option<(usize, Rect)> {
        let s = st.place[list[0]]?.0;
        let r = list
            .iter()
            .filter(|q| st.place[**q].is_some())
            .map(|q| st.room(*q))
            .reduce(Rect::union)?;
        Some((s, r))
    };
    // Moves `list` so its room's middle lands nearest `centre` on sheet
    // `onto`; with `axis`, on `centre`'s line first (sliding along a row,
    // or a column, from the spot square to it), so a part lined up moves
    // the least and lands on the line where the line has room.
    let lay = |st: &mut State,
               list: &[usize],
               centre: Point,
               onto: usize,
               axis: Option<crate::layout::Axis>| {
        if list.iter().all(|p| kept.contains(p)) {
            return;
        }
        let Some((_, now)) = room(st, list) else {
            return;
        };
        let s = onto;
        let saved: Vec<_> = list.iter().map(|p| st.place[*p]).collect();
        let lead = st.place[list[0]].expect("placed").1.at;
        let rel: Vec<(usize, Place)> = list
            .iter()
            .map(|p| {
                let pl = st.place[*p].expect("placed").1;
                (
                    *p,
                    Place::new(
                        [pl.at[0] - lead[0], pl.at[1] - lead[1]],
                        pl.rotation,
                        pl.mirror,
                    ),
                )
            })
            .collect();
        for p in list {
            st.place[*p] = None;
        }
        let e0 = now.shift([-lead[0], -lead[1]]);
        let want = [centre[0] - e0.center()[0], centre[1] - e0.center()[1]];
        let sid = st.sheets[s].0.clone();
        let blocked = drawn.get(&sid).cloned().unwrap_or_default();
        let found = axis
            .and_then(|a| spot_along(st, s, e0, want, a, SPACING, &blocked))
            .or_else(|| spot(st, s, e0, want, SPACING, &blocked));
        // Not where it was asked: say what stood there.
        if found != Some(want) {
            let at = e0.shift(want).grow(SPACING);
            let mut who: Vec<String> = (0..st.block.parts.len())
                .filter(|q| !list.contains(q))
                .filter(|q| st.place[*q].is_some_and(|x| x.0 == s))
                .filter(|q| st.room(*q).overlaps(&at))
                .map(|q| crate::lint::who(&st.block.parts[q]))
                .collect();
            who.sort();
            who.dedup();
            if blocked.iter().any(|b| b.overlaps(&at)) {
                who.push("a wire or label".into());
            }
            let why = if who.is_empty() {
                format!(
                    "sheet {} has no free place for it there",
                    st.sheets[s].1.name
                )
            } else {
                let verb = if who.len() == 1 { "stands" } else { "stand" };
                format!("{} {verb} where it would go", who.join(", "))
            };
            for p in list {
                st.blocked
                    .insert(st.block.parts[*p].key.clone(), why.clone());
            }
        }
        match found {
            Some(o) => {
                for (p, pl) in &rel {
                    let at = [pl.at[0] + o[0], pl.at[1] + o[1]];
                    st.place[*p] = Some((s, Place::new(at, pl.rotation, pl.mirror), false));
                }
            }
            None => {
                for (p, x) in list.iter().zip(saved) {
                    st.place[*p] = x;
                }
            }
        }
    };
    // Whether an instruction holds as things lie now.
    let holds = |st: &State, ins: Instruction| {
        let mut l = Layout::default();
        for (id, sh) in &st.sheets {
            l.sheets.insert(id.clone(), sh.clone());
        }
        for (p, x) in st.place.iter().enumerate() {
            if let Some((s, pl, _)) = x {
                l.placed.insert(
                    st.block.parts[p].key.clone(),
                    Placed {
                        sheet: st.sheets[*s].0.clone(),
                        at: pl.at,
                        rotation: pl.rotation,
                        mirror: pl.mirror,
                        held: false,
                    },
                );
            }
        }
        instr::achieved(st.block, &l, &ins)
    };
    let key = |p: usize| st.block.parts[p].key.clone();
    for (&p, &(of, side)) in &r.beside {
        if skip(p) {
            continue;
        }
        if holds(
            st,
            Instruction::Beside {
                parts: vec![key(p)],
                of: key(of),
                side,
            },
        ) {
            continue;
        }
        let list = movers(p);
        let (Some((onto, target)), Some((own, mine))) = (room(st, &movers(of)), room(st, &list))
        else {
            continue;
        };
        // Onto the other's sheet, unless an instruction names this one's.
        let onto = if r.sheet.contains_key(&p) { own } else { onto };
        let (c, half) = (target.center(), [mine.width() / 2, mine.height() / 2]);
        let centre = match side {
            Side::Left => [target.min[0] - SPACING - half[0], c[1]],
            Side::Right => [target.max[0] + SPACING + half[0], c[1]],
            Side::Up => [c[0], target.max[1] + SPACING + half[1]],
            Side::Down => [c[0], target.min[1] - SPACING - half[1]],
        };
        lay(st, &list, centre, onto, None);
        moved_sheet(st, p, own);
    }
    for (&p, &(to, pin)) in &r.near {
        if skip(p) {
            continue;
        }
        let to_text = match pin {
            Some(k) => format!("{}/{}", key(to), st.block.parts[to].pins[k].number),
            None => key(to),
        };
        if holds(
            st,
            Instruction::Near {
                parts: vec![key(p)],
                to: to_text,
            },
        ) {
            continue;
        }
        let list = movers(p);
        let (Some((onto, target)), Some((own, mine))) = (room(st, &[to]), room(st, &list)) else {
            continue;
        };
        let onto = if r.sheet.contains_key(&p) { own } else { onto };
        let centre = match pin.and_then(|k| st.shape(to).map(|sh| sh.pins[k])) {
            Some(pin) => {
                let d = pin.side.step();
                let reach = (mine.width().max(mine.height()) / 2 / GRID + 2) * GRID;
                [
                    pin.end[0] + d[0] / GRID * reach,
                    pin.end[1] + d[1] / GRID * reach,
                ]
            }
            None => [
                target.max[0] + SPACING + mine.width() / 2,
                target.center()[1],
            ],
        };
        lay(st, &list, centre, onto, None);
        moved_sheet(st, p, own);
    }
    for (list, axis) in &r.align {
        let text = |(p, k): (usize, Option<usize>)| match k {
            Some(k) => format!("{}/{}", key(p), st.block.parts[p].pins[k].number),
            None => key(p),
        };
        if holds(
            st,
            Instruction::Align {
                parts: list.iter().map(|x| text(*x)).collect(),
                axis: *axis,
            },
        ) {
            continue;
        }
        // The line: the lead pin's end, else the lead body's middle.
        let line = |st: &State, (p, k): (usize, Option<usize>)| -> Option<Point> {
            let sh = st.shape(p)?;
            Some(match k {
                Some(k) => sh.pins[k].end,
                None => sh.body.center(),
            })
        };
        let Some(lc) = line(st, list[0]) else {
            continue;
        };
        // Each part on the lead's sheet moves along the axis onto the
        // lead's line; a part on another sheet stays there (only `sheet`
        // moves a part between sheets), and the instruction says why.
        let lead_sheet = st.place[list[0].0].expect("placed").0;
        for &(p, k) in &list[1..] {
            let moving = movers(p);
            let (Some(c), Some((own, mine))) = (line(st, (p, k)), room(st, &moving)) else {
                continue;
            };
            if own != lead_sheet {
                continue;
            }
            let shift = match axis {
                crate::layout::Axis::Row => [0, lc[1] - c[1]],
                crate::layout::Axis::Column => [lc[0] - c[0], 0],
            };
            let m = mine.center();
            lay(
                st,
                &moving,
                [m[0] + shift[0], m[1] + shift[1]],
                own,
                Some(*axis),
            );
        }
    }
}

/// Notes a part an instruction moved off its sheet `was` (`beside` and
/// `near` take a part to the other's sheet).
fn moved_sheet(st: &mut State, p: usize, was: usize) {
    let Some((now, _, _)) = st.place[p] else {
        return;
    };
    if now == was {
        return;
    }
    let key = st.block.parts[p].key.clone();
    let (from, to) = (st.sheets[was].0.clone(), st.sheets[now].0.clone());
    st.notes.push(Note {
        code: "moved_sheet".into(),
        message: format!("{key} moved from sheet {from} to sheet {to}"),
        details: json!({"part": key, "from": from, "to": to}),
    });
}

/// The free origin nearest `want` that keeps `want`'s line: for a row its
/// y (sliding along the row, nearest first), for a column its x; `None`
/// when the line has no room on the sheet.
fn spot_along(
    st: &State,
    s: usize,
    e0: Rect,
    want: Point,
    axis: crate::layout::Axis,
    gap: Nm,
    drawn: &[Rect],
) -> Option<Point> {
    let block = st.block;
    let a = st.area(s).grow(-MARGIN);
    let others: Vec<Rect> = (0..block.parts.len())
        .filter(|p| st.place[*p].is_some_and(|(x, _, _)| x == s))
        .map(|p| st.room(p))
        .collect();
    let fits = |o: Point| {
        let e = e0.shift(o);
        a.contains(&e)
            && !others.iter().any(|r| r.overlaps(&e.grow(gap)))
            && !drawn.iter().any(|r| r.overlaps(&e.grow(GRID)))
    };
    let want = [crate::geom::snap(want[0]), crate::geom::snap(want[1])];
    let size = st.size(s);
    let max = size[0].max(size[1]) / GRID;
    for d in 0..=max {
        for k in if d == 0 { vec![0] } else { vec![-d, d] } {
            let o = match axis {
                crate::layout::Axis::Row => [want[0] + k * GRID, want[1]],
                crate::layout::Axis::Column => [want[0], want[1] + k * GRID],
            };
            if fits(o) {
                return Some(o);
            }
        }
    }
    None
}

/// From nothing (plan 2.A): `which`'s clusters packed in two levels.
/// Each proposed region's clusters are flowed onto a sheet of their own
/// (and drawn to their hosts) into one framed region cluster, a region
/// running past one sheet split into regions of its own (`<title> (2)`);
/// then the region clusters and the block's ports are flowed onto the
/// target's sheets, so signals between regions order their columns and
/// regions joined to the rest only by rails go in the top band. A port in
/// an idiom's cluster is taken out of it: ports are in no region.
fn pack_regions(
    st: &mut State,
    clusters: &[Cluster],
    which: Vec<usize>,
    target: Target,
    hints: &Hints,
    r: &instr::Resolved,
    proposed: &[crate::region::Proposed],
) {
    let block = st.block;
    let is_port = |p: usize| matches!(block.parts[p].kind, PartKind::Port { .. });
    // The pool: `which`'s clusters, each in its first part's region (a
    // port in an idiom's cluster, beside the pin it meets, moves with the
    // region but is in none).
    let pool: Vec<Cluster> = which.iter().map(|c| clusters[*c].clone()).collect();
    let edge_of: Vec<Option<Side>> = which.iter().map(|c| hints.edge.get(c).copied()).collect();
    let region_of = |c: &Cluster| {
        // A named region taking any member takes the cluster.
        let named = proposed.iter().position(|g| {
            g.basis == crate::region::Basis::Named
                && c.members.iter().any(|(p, _)| g.parts.contains(p))
        });
        if named.is_some() {
            return named;
        }
        let p = c.members.iter().map(|(p, _)| *p).find(|p| !is_port(*p))?;
        proposed.iter().position(|g| g.parts.contains(&p))
    };
    let sub_hints = Hints {
        spread: hints.spread,
        apart: hints.apart.clone(),
        ..Hints::default()
    };
    let mut packed: Vec<Cluster> = Vec::new();
    let mut packed_edge: Vec<Option<Side>> = Vec::new();
    // Each packed cluster's region: its title, whether named, and its parts.
    let mut made: Vec<Option<(String, bool, Vec<usize>)>> = Vec::new();
    for (g, prop) in proposed.iter().enumerate() {
        let mine: Vec<usize> = (0..pool.len())
            .filter(|c| region_of(&pool[*c]) == Some(g))
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut sub = State::new(block);
        flow_onto_sheets(
            &mut sub,
            &pool,
            mine.clone(),
            Target::Fresh(None),
            &sub_hints,
        );
        attract(&mut sub, &pool, r);
        let side = mine.iter().find_map(|c| edge_of[*c]);
        for k in 0..sub.sheets.len() {
            let members: Vec<(usize, Place)> = mine
                .iter()
                .flat_map(|c| pool[*c].members.iter().map(|(p, _)| *p))
                .filter_map(|p| match sub.place[p] {
                    Some((s, place, _)) if s == k => Some((p, place)),
                    _ => None,
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            let title = if k == 0 {
                prop.title.clone()
            } else {
                format!("{} ({})", prop.title, k + 1)
            };
            let mut parts: Vec<usize> = members
                .iter()
                .map(|(p, _)| *p)
                .filter(|p| !is_port(*p))
                .collect();
            parts.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
            packed.push(Cluster {
                members,
                idiom: None,
                frame: crate::region::FRAME,
            });
            packed_edge.push(side);
            made.push(Some((
                title,
                prop.basis == crate::region::Basis::Named,
                parts,
            )));
        }
    }
    // Clusters no region took (ports), as they are.
    for c in 0..pool.len() {
        if region_of(&pool[c]).is_none() {
            packed.push(pool[c].clone());
            packed_edge.push(edge_of[c]);
            made.push(None);
        }
    }
    let mut top_hints = Hints {
        spread: hints.spread,
        apart: hints.apart.clone(),
        ..Hints::default()
    };
    for (i, e) in packed_edge.iter().enumerate() {
        if let Some(side) = e {
            top_hints.edge.insert(i, *side);
        }
    }
    let all: Vec<usize> = (0..packed.len()).collect();
    let before = st.clone();
    flow_onto_sheets(st, &packed, all.clone(), target, &top_hints);
    fit_paper(st, &before, &packed, &all, target, &top_hints);
    for (i, m) in made.into_iter().enumerate() {
        let Some((title, named, parts)) = m else {
            continue;
        };
        let (p0, rel) = packed[i].members[0];
        let Some((s, place, _)) = st.place[p0] else {
            continue;
        };
        let at = [place.at[0] - rel.at[0], place.at[1] - rel.at[1]];
        let id = format!("r{}", st.regions.len() + 1);
        st.regions.push((
            id,
            crate::layout::Region {
                sheet: st.sheets[s].0.clone(),
                at,
                frame: Rect::new([0, 0], [0, 0]),
                title,
                named,
                parts: parts.iter().map(|p| block.parts[*p].key.clone()).collect(),
            },
        ));
    }
}

/// The box the packed clusters `which` take on sheet `s`, with labels on
/// every signal pin.
fn drawn_box(st: &State, packed: &[Cluster], which: &[usize], s: usize) -> Option<Rect> {
    which
        .iter()
        .filter_map(|c| {
            let (p0, rel) = packed[*c].members[0];
            let (sheet, place, _) = st.place[p0]?;
            (sheet == s).then(|| {
                let o = [place.at[0] - rel.at[0], place.at[1] - rel.at[1]];
                packed[*c].room_labelled(st.block, o, |_| true)
            })
        })
        .reduce(Rect::union)
}

/// The smallest paper of the series holding a drawing `w` by `h` with
/// the margins.
fn paper_for(block: &Block, w: Nm, h: Nm) -> Option<&crate::input::Paper> {
    block
        .papers
        .iter()
        .find(|p| w + 2 * MARGIN + 2 * GRID <= p.area[0] && h + 2 * MARGIN + 2 * GRID <= p.area[1])
}

/// Clusters `which` packed in rows on an area `size` from its top left,
/// tallest first (ties in order), a channel's least width apart; `None`
/// when they don't all fit inside the margins.
fn shelves(
    block: &Block,
    packed: &[Cluster],
    which: &[usize],
    size: [Nm; 2],
) -> Option<BTreeMap<usize, Point>> {
    const GAP: Nm = 6 * GRID;
    let mut order: Vec<(usize, Rect)> = which
        .iter()
        .map(|c| (*c, packed[*c].room_labelled(block, [0, 0], |_| true)))
        .collect();
    order.sort_by_key(|(c, r)| (std::cmp::Reverse(r.height()), *c));
    let (mut x, mut top, mut row) = (MARGIN, size[1] - MARGIN, 0);
    let mut out = BTreeMap::new();
    for (c, r) in order {
        if x > MARGIN && x + r.width() > size[0] - MARGIN {
            x = MARGIN;
            top -= row + GAP;
            row = 0;
        }
        let o = [
            crate::geom::snap(x - r.min[0] + GRID / 2),
            crate::geom::snap(top - r.max[1] - GRID / 2),
        ];
        let at = r.shift(o);
        if at.min[0] < MARGIN || at.max[0] > size[0] - MARGIN || at.min[1] < MARGIN {
            return None;
        }
        out.insert(c, o);
        x = at.max[0] + GAP;
        row = row.max(top - at.min[1]);
    }
    Some(out)
}

/// Paper fit from nothing (plan 2.A): each sheet the packing made takes
/// the smallest paper of the series holding its drawing; a lone sheet
/// covering under half of it has its channels widened a step at a time,
/// up to `region::WIDEN`, while the drawing still fits; then each drawing
/// is centred on its paper, on the grid. With no series nothing changes.
fn fit_paper<'a>(
    st: &mut State<'a>,
    before: &State<'a>,
    packed: &[Cluster],
    which: &[usize],
    target: Target,
    hints: &Hints,
) {
    let block = st.block;
    if block.papers.is_empty() {
        return;
    }
    let sheets_of = |st: &State| -> Vec<usize> {
        let mut out: Vec<usize> = which
            .iter()
            .filter_map(|c| st.place[packed[*c].members[0].0].map(|(s, _, _)| s))
            .collect();
        out.sort();
        out.dedup();
        out
    };
    let set_paper = |st: &mut State, s: usize, paper: &crate::input::Paper| {
        let last = block.papers.last() == Some(paper);
        let sheet = &mut st.sheets[s].1;
        sheet.size = (!last).then(|| paper.size.clone());
        sheet.orientation = None;
    };
    // Widening a lone sheet.
    let sheets = sheets_of(st);
    if let [s] = sheets.as_slice()
        && let Some(d) = drawn_box(st, packed, which, *s)
        && let Some(paper) = paper_for(block, d.width(), d.height())
    {
        let half = Rect::new([0, 0], paper.area).area() / 2;
        let mut widened = 0;
        let mut d = d;
        while d.area() < half && widened < crate::region::WIDEN {
            widened += GRID;
            let mut t = before.clone();
            let wider = Hints {
                spread: hints.spread + widened,
                ..hints.clone()
            };
            flow_onto_sheets(&mut t, packed, which.to_vec(), target, &wider);
            let ts = sheets_of(&t);
            let Some(td) = (ts.len() == 1)
                .then(|| drawn_box(&t, packed, which, ts[0]))
                .flatten()
            else {
                break;
            };
            if td.width() + 2 * MARGIN + 2 * GRID > paper.area[0]
                || td.height() + 2 * MARGIN + 2 * GRID > paper.area[1]
            {
                break;
            }
            *st = t;
            d = td;
        }
    }
    // A sheet whose drawing needs more than the series' smallest paper
    // (a strip of columns an overflow left wide and sparse) is flowed again
    // on each smaller paper, smallest first, and takes the first that holds
    // all of it.
    for s in sheets_of(st) {
        let Some(d) = drawn_box(st, packed, which, s) else {
            continue;
        };
        let Some(needs) = paper_for(block, d.width(), d.height())
            .and_then(|p| block.papers.iter().position(|q| q == p))
        else {
            continue;
        };
        let mine: Vec<usize> = which
            .iter()
            .copied()
            .filter(|c| st.place[packed[*c].members[0].0].is_some_and(|(x, _, _)| x == s))
            .collect();
        for paper in &block.papers[..needs] {
            let flowed = place::flow(block, packed, &mine, paper.area, hints);
            // Flow keeps its layers' columns; where they don't fit, the
            // clusters packed in rows, tallest first.
            let origins = if flowed.overflow.is_empty() {
                flowed.origins
            } else if let Some(o) = shelves(block, packed, &mine, paper.area) {
                o
            } else {
                continue;
            };
            let Some(td) = origins
                .iter()
                .map(|(c, o)| packed[*c].room_labelled(block, *o, |_| true))
                .reduce(Rect::union)
            else {
                continue;
            };
            if td.width() + 2 * MARGIN + 2 * GRID > paper.area[0]
                || td.height() + 2 * MARGIN + 2 * GRID > paper.area[1]
            {
                continue;
            }
            for (c, o) in &origins {
                for (p, place) in at(&packed[*c], *o) {
                    st.place[p] = Some((s, place, false));
                }
            }
            break;
        }
    }
    for s in sheets_of(st) {
        let Some(d) = drawn_box(st, packed, which, s) else {
            continue;
        };
        let Some(paper) = paper_for(block, d.width(), d.height()).cloned() else {
            continue;
        };
        set_paper(st, s, &paper);
        // Centred on its paper.
        let by = [
            crate::geom::snap((paper.area[0] - d.width()) / 2 - d.min[0]),
            crate::geom::snap((paper.area[1] - d.height()) / 2 - d.min[1]),
        ];
        for c in which {
            for (p, _) in &packed[*c].members {
                if let Some((ps, place, held)) = st.place[*p]
                    && ps == s
                {
                    let at = [place.at[0] + by[0], place.at[1] + by[1]];
                    st.place[*p] = Some((ps, Place::new(at, place.rotation, place.mirror), held));
                }
            }
        }
    }
}

/// Where `flow_onto_sheets` starts.
#[derive(Clone, Copy)]
enum Target<'a> {
    /// The sheet laid out again (by ID), else a new one.
    Fresh(Option<&'a str>),
    /// The sheet so named, made if new.
    Named(&'a str),
}

/// Places `which` clusters by flow on their sheet, and what doesn't fit on
/// the next.
fn flow_onto_sheets(
    st: &mut State,
    clusters: &[Cluster],
    mut which: Vec<usize>,
    target: Target,
    hints: &Hints,
) {
    let block = st.block;
    let mut first = true;
    while !which.is_empty() {
        let s = match (first, &target) {
            (true, Target::Fresh(Some(id))) => st.sheet_id(id, id),
            (true, Target::Named(name)) => sheet_named(st, name),
            _ => st.new_sheet(None),
        };
        first = false;
        let placed = place::flow(block, clusters, &which, st.size(s), hints);
        for (c, o) in &placed.origins {
            for (p, place) in at(&clusters[*c], *o) {
                st.place[p] = Some((s, place, false));
            }
        }
        if placed.overflow.len() == which.len() {
            // Nothing fits even alone: put the first where it may.
            let c = which.remove(0);
            let e0 = clusters[c].room(block, [0, 0]);
            let o = [
                crate::geom::snap(MARGIN - e0.min[0]),
                crate::geom::snap(st.size(s)[1] - MARGIN - e0.max[1]),
            ];
            for (p, place) in at(&clusters[c], o) {
                st.place[p] = Some((s, place, false));
            }
            continue;
        }
        if !placed.overflow.is_empty() {
            let names: Vec<String> = placed
                .overflow
                .iter()
                .map(|c| block.parts[clusters[*c].first()].key.clone())
                .collect();
            st.notes.push(Note {
                code: "sheet_full".into(),
                message: format!(
                    "sheet {} has no room for {}; they go on the next sheet",
                    st.sheets[s].0,
                    names.join(", ")
                ),
                details: json!({"sheet": st.sheets[s].0, "parts": names}),
            });
        }
        which = placed.overflow;
    }
}

/// Keeps each part the previous layout placed where it still fits: on a
/// sheet it had, inside its area, clear of the parts kept before it (held
/// ones first, then in key order); never one in `not`.
fn keep(st: &mut State, prev: Option<&Layout>, fresh: Option<&str>, not: &BTreeSet<usize>) {
    let Some(prev) = prev else { return };
    let block = st.block;
    let mut order: Vec<usize> = (0..block.parts.len()).collect();
    order.sort_by_key(|p| {
        let held = prev
            .placed
            .get(&block.parts[*p].key)
            .is_some_and(|x| x.held);
        (!held, block.parts[*p].key.clone())
    });
    let mut kept: Vec<(usize, Rect)> = Vec::new();
    for p in order {
        let part = &block.parts[p];
        let Some(x) = prev.placed.get(&part.key) else {
            continue;
        };
        if not.contains(&p) || Some(x.sheet.as_str()) == fresh {
            continue;
        }
        let Some(s) = st.sheets.iter().position(|(id, _)| *id == x.sheet) else {
            continue;
        };
        let place = Place::new(x.at, x.rotation, x.mirror);
        let shape = Shape::of(part, place);
        if !st.area(s).contains(&shape.extent) {
            continue;
        }
        if kept
            .iter()
            .any(|(ks, r)| *ks == s && r.overlaps(&shape.extent))
        {
            continue;
        }
        kept.push((s, shape.extent));
        st.place[p] = Some((s, place, x.held));
    }
    // Regions, holding the parts kept on their sheets: one left with none
    // goes, so a deletion empties no frame.
    for (id, r) in &prev.regions {
        let mut region = r.clone();
        region.parts.retain(|k| {
            block
                .part(k)
                .is_some_and(|p| st.place[p].is_some_and(|(s, _, _)| st.sheets[s].0 == r.sheet))
        });
        if !region.parts.is_empty() {
            st.regions.push((id.clone(), region));
        }
    }
}

/// Over a previous layout, the `region` instructions hold: each named
/// region holds exactly its parts as placed (made where the layout has no
/// region so titled), its parts leaving any other region; `region none`
/// takes parts out of named regions.
fn named_regions(st: &mut State, r: &instr::Resolved) {
    let block = st.block;
    for (title, set) in &r.regions {
        let keys: Vec<String> = set
            .iter()
            .filter(|p| st.place[**p].is_some())
            .map(|p| block.parts[*p].key.clone())
            .collect();
        let Some(first) = set.iter().copied().find(|p| st.place[*p].is_some()) else {
            continue;
        };
        // A region holding only these parts takes the title, keeping its
        // ID (what `region rename` writes).
        if !st.regions.iter().any(|(_, g)| g.named && &g.title == title)
            && let Some((_, g)) = st.regions.iter_mut().find(|(_, g)| {
                !g.named && !g.parts.is_empty() && g.parts.iter().all(|k| keys.contains(k))
            })
        {
            g.title = title.clone();
            g.named = true;
        }
        for (_, g) in st.regions.iter_mut() {
            if !(g.named && &g.title == title) {
                g.parts.retain(|k| !keys.contains(k));
            }
        }
        let (s, place, _) = st.place[first].expect("placed");
        let sid = st.sheets[s].0.clone();
        match st
            .regions
            .iter_mut()
            .find(|(_, g)| g.named && &g.title == title)
        {
            Some((_, g)) => {
                g.parts = keys;
                g.parts.sort();
            }
            None => {
                let n = st
                    .regions
                    .iter()
                    .filter_map(|(id, _)| id.strip_prefix('r')?.parse::<u64>().ok())
                    .max()
                    .unwrap_or(0);
                let mut parts = keys;
                parts.sort();
                st.regions.push((
                    format!("r{}", n + 1),
                    crate::layout::Region {
                        sheet: sid,
                        at: place.at,
                        frame: Rect::new([0, 0], [0, 0]),
                        title: title.clone(),
                        named: true,
                        parts,
                    },
                ));
            }
        }
    }
    let none: Vec<String> = r
        .no_region
        .iter()
        .map(|p| block.parts[*p].key.clone())
        .collect();
    for (_, g) in st.regions.iter_mut().filter(|(_, g)| g.named) {
        g.parts.retain(|k| !none.contains(k));
    }
    st.regions.retain(|(_, g)| !g.parts.is_empty());
}

/// Each sheet a `paper` instruction names takes its paper, pinned: never
/// fitted or grown again.
fn pin_papers(st: &mut State, r: &instr::Resolved) {
    for (name, (size, orientation)) in &r.papers {
        for (id, sheet) in st.sheets.iter_mut() {
            if id == name || &sheet.name == name {
                sheet.size = Some(size.clone());
                sheet.orientation = *orientation;
                sheet.pinned = true;
            }
        }
    }
}

/// Grows sheet `s` one paper size, for `parts`: noted `sheet_resized`.
/// Its contents keep their coordinates (the area grows right and up).
/// False for a pinned sheet or one at the series' largest.
fn grow_sheet(st: &mut State, s: usize, parts: &[usize]) -> bool {
    let block = st.block;
    let sheet = &st.sheets[s].1;
    if sheet.pinned {
        return false;
    }
    let (Some(now), Some(next)) = (block.paper_of(sheet), block.grown(sheet)) else {
        return false;
    };
    let (from, to) = (now.size.clone(), next.size.clone());
    let last = block.papers.last() == Some(next);
    let sheet = &mut st.sheets[s].1;
    sheet.size = (!last).then(|| next.size.clone());
    sheet.orientation = None;
    let id = st.sheets[s].0.clone();
    let names: Vec<String> = parts.iter().map(|p| block.parts[*p].key.clone()).collect();
    st.notes.push(Note {
        code: "sheet_resized".into(),
        message: format!(
            "sheet {id} grew from {from} to {to} to hold {}",
            names.join(", ")
        ),
        details: json!({"sheet": id, "from": from, "to": to, "parts": names}),
    });
    true
}

/// What the previous layout drew, by sheet: wires, labels and text, as
/// boxes new parts keep clear of.
fn drawn(prev: Option<&Layout>) -> BTreeMap<String, Vec<Rect>> {
    let mut out: BTreeMap<String, Vec<Rect>> = BTreeMap::new();
    let Some(prev) = prev else { return out };
    for sheets in prev.routes.values() {
        for (s, runs) in sheets {
            for run in runs {
                for w in run.windows(2) {
                    out.entry(s.clone())
                        .or_default()
                        .push(Rect::new(w[0], w[1]).grow(GRID / 2));
                }
            }
        }
    }
    for labels in prev.labels.values() {
        for l in labels {
            out.entry(l.sheet.clone())
                .or_default()
                .push(Rect::new(l.at, l.at).grow(3 * GRID));
        }
    }
    // A bus's entries, spines and trunk, and its connector (plan 2.C);
    // a note's box.
    for by in prev.buses.values() {
        for (s, d) in by {
            let mut pts: Vec<Point> = Vec::new();
            for e in &d.entries {
                let (_, on_spine) = crate::bus::entry(e.at, e.side, d.stub);
                pts.extend([e.at, on_spine]);
                out.entry(s.clone())
                    .or_default()
                    .push(Rect::new(e.at, on_spine).grow(GRID));
            }
            for run in &d.trunk {
                for w in run.windows(2) {
                    out.entry(s.clone())
                        .or_default()
                        .push(Rect::new(w[0], w[1]).grow(GRID / 2));
                }
            }
            if let Some(r) = Rect::of(pts) {
                out.entry(s.clone()).or_default().push(r);
            }
            if let Some(c) = &d.connector {
                out.entry(s.clone()).or_default().push(label::label_box(
                    LabelKind::OffSheet,
                    &c.text,
                    c.at,
                    c.side,
                ));
            }
        }
    }
    for (key, fields) in &prev.text {
        let Some(x) = prev.placed.get(key) else {
            continue;
        };
        for at in fields.values() {
            out.entry(x.sheet.clone())
                .or_default()
                .push(Rect::new(*at, [at[0] + 6 * GRID, at[1] + GRID]).grow(GRID / 2));
        }
    }
    out
}

/// The free origin nearest `want` for a cluster whose room at the origin
/// is `e0`, on sheet `s`: inside the area, `gap` clear of every part
/// there and clear of what the previous layout drew.
fn spot(st: &State, s: usize, e0: Rect, want: Point, gap: Nm, drawn: &[Rect]) -> Option<Point> {
    spot_in(st, s, e0, want, gap, drawn, None, &|_| true)
}

/// [`spot`], the room kept `within` a box where one is given.
#[allow(clippy::too_many_arguments)]
fn spot_in(
    st: &State,
    s: usize,
    e0: Rect,
    want: Point,
    gap: Nm,
    drawn: &[Rect],
    within: Option<Rect>,
    ok: &dyn Fn(Rect) -> bool,
) -> Option<Point> {
    let block = st.block;
    let a = st.area(s).grow(-MARGIN);
    let others: Vec<Rect> = (0..block.parts.len())
        .filter(|p| st.place[*p].is_some_and(|(x, _, _)| x == s))
        .map(|p| st.room(p))
        .collect();
    let fits = |o: Point| {
        let e = e0.shift(o);
        a.contains(&e)
            && within.is_none_or(|w| w.contains(&e))
            && ok(e)
            && !others.iter().any(|r| r.overlaps(&e.grow(gap)))
            && !drawn.iter().any(|r| r.overlaps(&e.grow(GRID)))
    };
    let want = [crate::geom::snap(want[0]), crate::geom::snap(want[1])];
    // Within a box, no further than its far corner from where it is wanted.
    let max = match within {
        Some(w) => {
            let c = e0.shift(want).center();
            ((c[0] - w.center()[0]).abs() + (c[1] - w.center()[1]).abs() + w.width() + w.height())
                / GRID
        }
        None => st.size(s)[0].max(st.size(s)[1]) / GRID,
    };
    for r in 0..=max {
        for (dx, dy) in ring(r) {
            let o = [want[0] + dx * GRID, want[1] + dy * GRID];
            if fits(o) {
                return Some(o);
            }
        }
    }
    None
}

/// Places what is new among kept parts: a new member of a cluster that
/// has kept ones where its idiom puts it (else the free spot nearest
/// that), on their sheet; a new guest cluster beside its host (see
/// [`beside_host`]); any other new cluster on the sheet most of its
/// neighbours are on, at the free spot nearest them. Nothing kept moves,
/// and nothing goes where the previous layout drew.
fn insert(
    st: &mut State,
    prev: Option<&Layout>,
    clusters: &[Cluster],
    fresh: &[usize],
    mixed: &[usize],
    r: &instr::Resolved,
    hints: &Hints,
) {
    let block = st.block;
    if st.sheets.is_empty() {
        st.new_sheet(None);
    }
    let drawn = drawn(prev);
    for &ci in mixed {
        let c = &clusters[ci];
        let s = c
            .members
            .iter()
            .find_map(|(p, _)| st.place[*p].map(|(s, _, _)| s))
            .expect("a kept member");
        let sid = st.sheets[s].0.clone();
        let own = c.members.iter().find_map(|(q, _)| {
            st.place[*q]?;
            st.regions
                .iter()
                .position(|(_, g)| g.parts.contains(&block.parts[*q].key))
        });
        let blocked = region_blocked(st, &drawn, &sid, own);
        for &(p, place) in &c.members {
            if st.place[p].is_some() {
                continue;
            }
            let at0 = Place::new([0, 0], place.rotation, place.mirror);
            let e0 = Cluster::single(p, at0).room_with_labels(block, [0, 0]);
            let found = match own {
                Some(g) => spot_in(
                    st,
                    s,
                    e0,
                    place.at,
                    GRID,
                    &blocked,
                    None,
                    &grows_clear(st, g),
                ),
                None => spot(st, s, e0, place.at, GRID, &blocked),
            };
            let found = found.or_else(|| {
                // 3. The sheet one paper larger, then again.
                while grow_sheet(st, s, &[p]) {
                    let again = match own {
                        Some(g) => spot_in(
                            st,
                            s,
                            e0,
                            place.at,
                            GRID,
                            &blocked,
                            None,
                            &grows_clear(st, g),
                        ),
                        None => spot(st, s, e0, place.at, GRID, &blocked),
                    };
                    if again.is_some() {
                        return again;
                    }
                }
                None
            });
            match found {
                Some(o) => {
                    st.place[p] = Some((s, Place::new(o, place.rotation, place.mirror), false));
                    // It joins the region its cluster's kept members are in.
                    let key = &block.parts[p].key;
                    if !matches!(block.parts[p].kind, PartKind::Port { .. })
                        && let Some(g) = c.members.iter().find_map(|(q, _)| {
                            st.regions
                                .iter()
                                .position(|(_, g)| g.parts.contains(&block.parts[*q].key))
                        })
                        && st.regions[g].1.sheet == sid
                    {
                        st.regions[g].1.parts.push(key.clone());
                        st.regions[g].1.parts.sort();
                    }
                }
                None => {
                    let single = Cluster::single(p, at0);
                    full(st, &single, &sid);
                }
            }
        }
    }
    for &ci in fresh {
        let c = &clusters[ci];
        // A part whose host is in a region joins it (plan 2.A).
        if !hints.edge.contains_key(&ci) && into_region(st, &drawn, c, r) {
            continue;
        }
        let before: BTreeSet<usize> = (0..block.parts.len())
            .filter(|p| st.place[*p].is_some())
            .collect();
        insert_one(st, &drawn, c, ci, r, hints);
        // Placed by itself: a region of its own.
        let placed: Vec<usize> = c
            .members
            .iter()
            .map(|(p, _)| *p)
            .filter(|p| !before.contains(p) && st.place[*p].is_some())
            .collect();
        new_region(st, &placed);
    }
}

/// A fresh cluster placed as phase 1 places it: a guest beside its host,
/// else near its neighbours, else on a new sheet.
fn insert_one(
    st: &mut State,
    drawn: &BTreeMap<String, Vec<Rect>>,
    c: &Cluster,
    ci: usize,
    r: &instr::Resolved,
    hints: &Hints,
) {
    let block = st.block;
    {
        // A guest goes beside its host, where cross-cluster placement puts
        // it from nothing.
        if !hints.edge.contains_key(&ci)
            && guest(block, c, r)
            && let Some((s, want, _, _)) = beside_host(st, c, None)
        {
            let sid = st.sheets[s].0.clone();
            let blocked = region_blocked(st, drawn, &sid, None);
            let e0 = c.room_with_labels(block, [0, 0]);
            if let Some(o) = spot(st, s, e0, want, SPACING, &blocked) {
                for (p, place) in at(c, o) {
                    st.place[p] = Some((s, place, false));
                }
                return;
            }
        }
        // The neighbours: kept parts sharing a signal net, by sheet.
        let nets: BTreeSet<usize> = c
            .members
            .iter()
            .flat_map(|(p, _)| block.parts[*p].pins.iter())
            .filter_map(|pin| pin.net)
            .filter(|n| block.nets[*n].kind == NetKind::Signal)
            .collect();
        let mut by_sheet: BTreeMap<usize, Vec<Point>> = BTreeMap::new();
        for (p, part) in block.parts.iter().enumerate() {
            let Some(shape) = st.shape(p) else { continue };
            let s = st.place[p].expect("placed").0;
            for (k, pin) in part.pins.iter().enumerate() {
                if pin.net.is_some_and(|n| nets.contains(&n)) {
                    by_sheet.entry(s).or_default().push(shape.pins[k].end);
                }
            }
        }
        let asked = c.members.iter().find_map(|(p, _)| r.sheet.get(p).cloned());
        if let Some(name) = &asked {
            let s = sheet_named(st, name);
            by_sheet.retain(|k, _| *k == s);
            by_sheet.entry(s).or_default();
        }
        let (s, mut target) = match by_sheet
            .iter()
            .filter(|(_, v)| !v.is_empty() || asked.is_some())
            .max_by_key(|(s, v)| (v.len(), usize::MAX - **s))
        {
            Some((s, pts)) if pts.is_empty() => (*s, st.area(*s).center()),
            Some((s, pts)) => {
                let n = pts.len() as Nm;
                let sum = pts.iter().fold([0, 0], |a, p| [a[0] + p[0], a[1] + p[1]]);
                (*s, [sum[0] / n, sum[1] / n])
            }
            None => (0, st.area(0).center()),
        };
        let a = st.area(s).grow(-MARGIN);
        let sid = st.sheets[s].0.clone();
        let blocked = region_blocked(st, drawn, &sid, None);
        let e0 = c.room_with_labels(block, [0, 0]);
        match hints.edge.get(&ci) {
            Some(Side::Left) => target[0] = a.min[0] + e0.width() / 2,
            Some(Side::Right) => target[0] = a.max[0] - e0.width() / 2,
            Some(Side::Up) => target[1] = a.max[1] - e0.height() / 2,
            Some(Side::Down) => target[1] = a.min[1] + e0.height() / 2,
            None => {}
        }
        let want = [target[0] - e0.center()[0], target[1] - e0.center()[1]];
        match spot(st, s, e0, want, SPACING, &blocked) {
            Some(o) => {
                for (p, place) in at(c, o) {
                    st.place[p] = Some((s, place, false));
                }
            }
            None => full(st, c, &sid),
        }
    }
}

/// Whether region `g` may grow to hold a box: its frame grown to it
/// (margin, and title band above) stays clear of every other region's on
/// its sheet.
fn grows_clear(st: &State, g: usize) -> impl Fn(Rect) -> bool + use<> {
    let region = &st.regions[g].1;
    let frame = region.frame_on_sheet();
    let others: Vec<Rect> = st
        .regions
        .iter()
        .enumerate()
        .filter(|(i, (_, x))| *i != g && x.sheet == region.sheet)
        .map(|(_, (_, x))| x.frame_on_sheet().grow(SPACING))
        .collect();
    move |e: Rect| {
        let mut f = frame.union(e.grow(crate::region::FRAME));
        f.max[1] = f.max[1].max(e.max[1] + crate::region::FRAME + crate::region::TITLE);
        !others.iter().any(|o| o.overlaps(&f))
    }
}

/// What a new cluster on sheet `sid` keeps clear of: what the previous
/// layout drew there, and every region's frame there (but region
/// `except`), grown by a frame's margin and title band and `SPACING`.
fn region_blocked(
    st: &State,
    drawn: &BTreeMap<String, Vec<Rect>>,
    sid: &str,
    except: Option<usize>,
) -> Vec<Rect> {
    let reach = crate::region::FRAME + crate::region::TITLE + SPACING;
    let mut out = drawn.get(sid).cloned().unwrap_or_default();
    out.extend(
        st.regions
            .iter()
            .enumerate()
            .filter(|(i, (_, x))| Some(*i) != except && x.sheet == sid)
            .map(|(_, (_, x))| x.frame_on_sheet().grow(reach)),
    );
    out
}

/// Places a fresh cluster in its host's region (else a named region it is
/// listed in), when that region is kept: into the free space beside the
/// host within its frame grown by `region::WIDEN`, clear of every other
/// region, else the nearest free spot clear of them; else the region moves
/// whole, with it beside, the least
/// distance on its sheet; else the region and it go to a new sheet
/// (`sheet_full`). Gives whether it placed it.
fn into_region(
    st: &mut State,
    drawn: &BTreeMap<String, Vec<Rect>>,
    c: &Cluster,
    r: &instr::Resolved,
) -> bool {
    let block = st.block;
    let is_port = |p: usize| matches!(block.parts[p].kind, PartKind::Port { .. });
    if c.members.iter().all(|(p, _)| is_port(*p)) {
        return false;
    }
    let host = host_among(block, c, |q| st.place[q].is_some());
    let by_host = host.and_then(|h| {
        st.regions
            .iter()
            .position(|(_, g)| g.parts.contains(&block.parts[h].key))
    });
    let by_name = || {
        r.regions.iter().find_map(|(title, set)| {
            if !c.members.iter().any(|(p, _)| set.contains(p)) {
                return None;
            }
            st.regions
                .iter()
                .position(|(_, g)| g.named && &g.title == title)
        })
    };
    let Some(g) = by_host.or_else(by_name) else {
        return false;
    };
    let region = st.regions[g].1.clone();
    let Some(s) = st.sheets.iter().position(|(id, _)| *id == region.sheet) else {
        return false;
    };
    // A sheet instruction sends it elsewhere: not into a region here.
    if c.members.iter().any(|(p, _)| {
        r.sheet
            .get(p)
            .is_some_and(|name| *name != st.sheets[s].1.name)
    }) {
        return false;
    }
    let frame = region.frame_on_sheet();
    let blocked = region_blocked(st, drawn, &region.sheet, Some(g));
    let e0 = c.room_with_labels(block, [0, 0]);
    let want = beside_host(st, c, Some(s)).map_or_else(
        || {
            [
                frame.center()[0] - e0.center()[0],
                frame.center()[1] - e0.center()[1],
            ]
        },
        |(_, w, _, _)| w,
    );
    let join = |st: &mut State, parts: &[usize]| {
        let keys = &mut st.regions[g].1.parts;
        keys.extend(
            parts
                .iter()
                .filter(|p| !is_port(**p))
                .map(|p| block.parts[*p].key.clone()),
        );
        keys.sort();
        keys.dedup();
    };
    let mine: Vec<usize> = c.members.iter().map(|(p, _)| *p).collect();
    // 1. The free space beside it, within the frame grown.
    let within = frame.grow(crate::region::WIDEN);
    let grows_clear = grows_clear(st, g);
    let free = spot_in(
        st,
        s,
        e0,
        want,
        SPACING,
        &blocked,
        Some(within),
        &grows_clear,
    )
    // Else the nearest free spot clear of every other region: the
    // region grows there, and nothing kept moves.
    .or_else(|| spot_in(st, s, e0, want, SPACING, &blocked, None, &grows_clear));
    if let Some(o) = free {
        for (p, place) in at(c, o) {
            st.place[p] = Some((s, place, false));
        }
        join(st, &mine);
        return true;
    }
    // 2. The region moves whole, the cluster to its right.
    let members: Vec<usize> = region
        .parts
        .iter()
        .filter_map(|k| block.part(k))
        .filter(|p| st.place[*p].is_some_and(|(x, _, _)| x == s))
        .collect();
    let saved: Vec<(usize, Slot)> = members.iter().map(|p| (*p, st.place[*p])).collect();
    let mut whole: Vec<(usize, Place)> = members
        .iter()
        .map(|p| (*p, st.place[*p].expect("placed").1))
        .collect();
    let oc = [
        crate::geom::snap(frame.max[0] + SPACING - e0.min[0]),
        crate::geom::snap(frame.max[1] - crate::region::TITLE - e0.max[1]),
    ];
    whole.extend(at(c, oc));
    let combined = Cluster {
        members: whole,
        idiom: None,
        frame: 0,
    };
    let room = combined
        .room_with_labels(block, [0, 0])
        .union(frame)
        .union(c.room_with_labels(block, oc).grow(crate::region::FRAME));
    for p in &members {
        st.place[*p] = None;
    }
    let others: Vec<Rect> = blocked.clone();
    if let Some(o) = spot_in(st, s, room, [0, 0], SPACING, &others, None, &|_| true) {
        for (p, place) in at(&combined, o) {
            let held = saved
                .iter()
                .find(|(q, _)| *q == p)
                .and_then(|(_, x)| x.map(|x| x.2))
                .unwrap_or(false);
            st.place[p] = Some((s, place, held));
        }
        let at0 = &mut st.regions[g].1.at;
        *at0 = [at0[0] + o[0], at0[1] + o[1]];
        join(st, &mine);
        return true;
    }
    // 3. The sheet one paper larger, and steps 1 and 2 again.
    for (p, was) in &saved {
        st.place[*p] = *was;
    }
    if grow_sheet(st, s, &mine) {
        return into_region(st, drawn, c, r);
    }
    // 4. The region and the cluster to a new sheet.
    let sid = region.sheet.clone();
    let to = st.new_sheet(None);
    let e = room;
    let o = [
        crate::geom::snap(MARGIN - e.min[0]),
        crate::geom::snap(st.size(to)[1] - MARGIN - e.max[1]),
    ];
    for (p, place) in at(&combined, o) {
        st.place[p] = Some((to, place, false));
    }
    let to_id = st.sheets[to].0.clone();
    let id = st.regions[g].0.clone();
    st.notes.push(Note {
        code: "sheet_full".into(),
        message: format!(
            "sheet {sid} has no room for region {id} ({}) and {}; they go on sheet {to_id}",
            region.title,
            block.parts[c.first()].key
        ),
        details: json!({
            "sheet": sid,
            "region": id,
            "to": to_id,
            "parts": mine.iter().map(|p| block.parts[*p].key.clone()).collect::<Vec<_>>(),
            "fix": format!("sheet region:{id} {to_id}"),
        }),
    });
    let reg = &mut st.regions[g].1;
    reg.sheet = to_id;
    reg.at = [reg.at[0] + o[0], reg.at[1] + o[1]];
    join(st, &mine);
    true
}

/// A region of its own for parts placed by themselves over a previous
/// layout: titled by its first part's function, else its designator and
/// value, its ID one past the largest.
fn new_region(st: &mut State, placed: &[usize]) {
    let block = st.block;
    let parts: Vec<usize> = placed
        .iter()
        .copied()
        .filter(|p| !matches!(block.parts[*p].kind, PartKind::Port { .. }))
        .collect();
    let Some(&first) = parts.first() else { return };
    let Some((s, place, _)) = st.place[first] else {
        return;
    };
    let part = &block.parts[first];
    let title = if part.function.is_empty() {
        format!("{} {}", part.refdes, part.value).trim().to_owned()
    } else {
        part.function.clone()
    };
    let n = st
        .regions
        .iter()
        .filter_map(|(id, _)| id.strip_prefix('r')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let mut keys: Vec<String> = parts.iter().map(|p| block.parts[*p].key.clone()).collect();
    keys.sort();
    // Its frame until it is drawn: its parts' rooms with labels, margin
    // and title band.
    let mut frame = parts
        .iter()
        .filter_map(|p| st.place[*p].map(|(_, pl, _)| Cluster::single(*p, pl)))
        .map(|c| c.room_with_labels(block, [0, 0]))
        .reduce(Rect::union)
        .expect("a placed part")
        .grow(crate::region::FRAME);
    frame.max[1] += crate::region::TITLE;
    let frame = frame.shift([-place.at[0], -place.at[1]]);
    st.regions.push((
        format!("r{}", n + 1),
        crate::layout::Region {
            sheet: st.sheets[s].0.clone(),
            at: place.at,
            frame,
            title,
            named: false,
            parts: keys,
        },
    ));
}

/// A cluster the sheet `sid` has no room for: onto a new sheet, noted.
fn full(st: &mut State, c: &Cluster, sid: &str) {
    let block = st.block;
    let s = st.new_sheet(None);
    let name = block.parts[c.first()].key.clone();
    st.notes.push(Note {
        code: "sheet_full".into(),
        message: format!(
            "sheet {sid} has no room for {name}; it goes on sheet {}",
            st.sheets[s].0
        ),
        details: json!({"sheet": sid, "parts": [name], "to": st.sheets[s].0}),
    });
    let e0 = c.room(block, [0, 0]);
    let o = [
        crate::geom::snap(MARGIN - e0.min[0]),
        crate::geom::snap(st.size(s)[1] - MARGIN - e0.max[1]),
    ];
    for (p, place) in at(c, o) {
        st.place[p] = Some((s, place, false));
    }
}

/// The offsets at Chebyshev distance `r`, in a fixed order.
fn ring(r: i64) -> Vec<(i64, i64)> {
    if r == 0 {
        return vec![(0, 0)];
    }
    let mut out = Vec::new();
    for d in -r..=r {
        out.push((d, -r));
        out.push((d, r));
    }
    for d in -r + 1..r {
        out.push((-r, d));
        out.push((r, d));
    }
    out.sort_by_key(|(x, y)| (x.abs() + y.abs(), *y, *x));
    out
}

/// How each net is drawn on a sheet holding `pins` of it: rails as power
/// ports, a net met on another sheet or at one pin by labels, the rest
/// by wires.
fn style(block: &Block, net: usize, here: usize, sheets: usize) -> NetStyle {
    match block.nets[net].kind {
        NetKind::Supply | NetKind::Ground => NetStyle::Port,
        NetKind::Signal if sheets > 1 || here < 2 => NetStyle::Label,
        NetKind::Signal => NetStyle::Wire,
    }
}

fn label_kind(block: &Block, net: usize, style: NetStyle) -> LabelKind {
    match (style, block.nets[net].kind) {
        (NetStyle::Port, NetKind::Ground) => LabelKind::Ground,
        (NetStyle::Port, _) => LabelKind::Power,
        _ => LabelKind::Net,
    }
}

/// A decoupling capacitor's supply pin and the pin it serves, on `net`.
#[derive(Clone, Copy, Debug)]
struct Bypass {
    net: usize,
    served: (usize, usize),
    cap: (usize, usize),
}

/// The bank row whose first member is `p`, when `p` stands on it (its
/// pin on the row's net at the row's height): a member kept elsewhere
/// isn't drawn as one.
fn bank_row_of<'r>(st: &State, rows: &'r [idiom::BankRow], p: usize) -> Option<&'r idiom::BankRow> {
    let row = rows
        .iter()
        .find(|r| r.members.first().map(|m| m.0) == Some(p))?;
    let (q, kq) = row.members[0];
    let y = st.shape(row.pin.0)?.pins[row.pin.1].end[1];
    (st.shape(q)?.pins[kq].end[1] == y).then_some(row)
}

/// The banks' rows, from the idioms and where parts stand.
fn bank_rows(st: &State, idioms: &[idiom::Idiom]) -> Vec<idiom::BankRow> {
    let at: BTreeMap<usize, Place> = (0..st.block.parts.len())
        .filter_map(|p| st.place[p].map(|(_, pl, _)| (p, pl)))
        .collect();
    idiom::bank_rows(st.block, idioms, &at)
}

/// Draws every sheet: labels, wires and text.
fn draw(
    st: State,
    prev: Option<&Layout>,
    asked: &BTreeMap<usize, NetStyle>,
    bus_styles: &BTreeMap<usize, NetStyle>,
    bypass: &[Bypass],
    bank_rows: &[idiom::BankRow],
    asks: &BTreeMap<usize, Vec<(crate::layout::TextField, Side)>>,
) -> Outcome {
    let block = st.block;
    let mut layout = Layout::default();
    let mut fallbacks: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut crowded: BTreeSet<usize> = BTreeSet::new();
    // Nets asked for as wires that are drawn by labels all the same, and why.
    let mut not_held: BTreeMap<usize, &'static str> = BTreeMap::new();
    for (id, s) in &st.sheets {
        layout.sheets.insert(id.clone(), s.clone());
    }
    for (id, r) in &st.regions {
        layout.regions.insert(id.clone(), r.clone());
    }
    for (p, x) in st.place.iter().enumerate() {
        if let Some((s, place, held)) = x {
            layout.placed.insert(
                block.parts[p].key.clone(),
                Placed {
                    sheet: st.sheets[*s].0.clone(),
                    at: place.at,
                    rotation: place.rotation,
                    mirror: place.mirror,
                    held: *held,
                },
            );
        }
    }
    // Buses that couldn't be drawn, and why (plan 2.C).
    let mut bus_notes: Vec<Note> = Vec::new();
    // Which sheets each net is on.
    let mut net_sheets: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (p, part) in block.parts.iter().enumerate() {
        let Some((s, _, _)) = st.place[p] else {
            continue;
        };
        for pin in &part.pins {
            for n in pin.nets() {
                net_sheets.entry(n).or_default().insert(s);
            }
        }
    }
    for s in 0..st.sheets.len() {
        let sid = st.sheets[s].0.clone();
        let parts = st.on_sheet(s);
        let mut field = Field::new(st.size(s));
        let mut pins_of: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
        // Nets a stacked pin is on take labels: its one label names them.
        let mut stacked: BTreeSet<usize> = BTreeSet::new();
        for &p in &parts {
            let shape = st.shape(p).expect("on the sheet");
            field.block_body(shape.body);
            for (k, pin) in block.parts[p].pins.iter().enumerate() {
                let at = shape.pins[k];
                let net = pin.drawn_net();
                field.pin(at.base, at.end, at.side, net.unwrap_or(NO_NET));
                if let Some(n) = net {
                    pins_of.entry(n).or_default().push((p, k));
                }
                if pin.stacked() {
                    stacked.extend(pin.nets());
                }
            }
        }
        // The text of parts unmoved since the previous layout, which a new
        // bus goes round (text is kept before anything is drawn).
        let mut kept_text: Vec<Rect> = Vec::new();
        if let Some(l) = prev {
            for &p in &parts {
                let part = &block.parts[p];
                let unmoved = l.placed.get(&part.key).is_some_and(|x| {
                    let (s0, pl, _) = st.place[p].expect("placed");
                    st.sheets[s0].0 == x.sheet && pl == Place::new(x.at, x.rotation, x.mirror)
                });
                if let (true, Some(fields)) = (unmoved, l.text.get(&part.key)) {
                    for (n, t) in part.lines() {
                        if let Some(at) = fields.get(n) {
                            kept_text.extend(text::boxes(&[t], *at));
                        }
                    }
                }
            }
        }
        // Buses before anything else is drawn (plan 2.C): a bundle's
        // members take no labels or wires of their own.
        for (i, bus) in block.buses.iter().enumerate() {
            if bus_styles.get(&i) == Some(&NetStyle::Label) {
                continue;
            }
            let here = crate::bus::sides_on_sheet(block, &parts);
            let leaves = |n: usize| net_sheets.get(&n).is_some_and(|s| s.len() > 1);
            let side_of = |p: usize, k: usize| st.shape(p).expect("placed").pins[k].side;
            let Some(bundle) = crate::bus::bundle_with(block, i, &here, &leaves, &side_of) else {
                if bus_styles.get(&i) == Some(&NetStyle::Bus)
                    && bus.members.iter().any(|n| pins_of.contains_key(n))
                {
                    bus_notes.push(bus_note(block, i, &sid, "its members don't run between one side of one part and one side of another"));
                }
                continue;
            };
            let saved = field.clone();
            let before = prev
                .and_then(|l| l.buses.get(&bus.key))
                .and_then(|b| b.get(&sid));
            match draw_bus(block, &st, &mut field, i, &bundle, before, &kept_text) {
                Ok(d) => {
                    for n in &bundle.members {
                        pins_of.remove(n);
                    }
                    layout
                        .buses
                        .entry(bus.key.clone())
                        .or_default()
                        .insert(sid.clone(), d);
                }
                Err(why) => {
                    field = saved;
                    bus_notes.push(bus_note(block, i, &sid, &why));
                }
            }
        }
        let styles: BTreeMap<usize, NetStyle> = pins_of
            .iter()
            .map(|(n, list)| {
                let mut st = style(block, *n, list.len(), net_sheets[n].len());
                match asked.get(n) {
                    Some(NetStyle::Wire)
                        if st == NetStyle::Label && net_sheets[n].len() == 1 && list.len() >= 2 =>
                    {
                        st = NetStyle::Wire;
                    }
                    Some(NetStyle::Label) => st = NetStyle::Label,
                    Some(NetStyle::Port) => st = NetStyle::Port,
                    Some(NetStyle::Wire)
                        if matches!(block.nets[*n].kind, NetKind::Supply | NetKind::Ground)
                            && list.len() >= 2 =>
                    {
                        st = NetStyle::Wire;
                    }
                    _ => {}
                }
                if stacked.contains(n) && block.nets[*n].kind == NetKind::Signal {
                    st = NetStyle::Label;
                }
                if bank_rows.iter().any(|r| r.names(*n)) {
                    st = NetStyle::Label;
                }
                (*n, st)
            })
            .collect();
        let ends_of = |n: usize| -> Vec<(Point, Side)> {
            let mut v: Vec<(Point, Side)> = pins_of[&n]
                .iter()
                .map(|(p, k)| {
                    let pin = st.shape(*p).expect("placed").pins[*k];
                    (pin.end, pin.side)
                })
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let long = st.size(s)[0] * LONG_SHARE.0 / LONG_SHARE.1;
        let bypass_long = st.size(s)[0] * BYPASS_SHARE.0 / BYPASS_SHARE.1;
        // Nets that take labels though they could be wires: too long to
        // wire from the start, and those whose wires didn't fit.
        let mut instead: BTreeSet<usize> = styles
            .iter()
            .filter(|(n, s)| **s == NetStyle::Wire && spanning(&ends_of(**n)) > long)
            .map(|(n, _)| *n)
            .collect();
        let prev_labels = |n: usize| -> Vec<Label> {
            prev.and_then(|l| l.labels.get(&block.nets[n].key))
                .map(|v| v.iter().filter(|x| x.sheet == sid).cloned().collect())
                .unwrap_or_default()
        };
        let runs_before = prev_runs(block, prev, &sid);
        // A net the previous layout labelled though it could have been
        // wired keeps its labels while its pins are the same, unless an
        // instruction or a move asks for its wires. A pin it wired to the
        // net's own (a short local wire, no label of its own) isn't
        // counted while that wire still ends on it.
        for (n, list) in &pins_of {
            if styles[n] != NetStyle::Wire || asked.get(n) == Some(&NetStyle::Wire) {
                continue;
            }
            let had: BTreeSet<String> = prev_labels(*n)
                .into_iter()
                .filter(|l| l.kind == LabelKind::Net)
                .map(|l| l.on)
                .collect();
            // A port's flag takes no label: its pin isn't counted.
            let ends: BTreeSet<Point> = runs_before
                .get(n)
                .into_iter()
                .flatten()
                .flat_map(|r| [r[0], r[r.len() - 1]])
                .collect();
            let mut now: BTreeSet<String> = BTreeSet::new();
            for (p, k) in list {
                if matches!(block.parts[*p].kind, PartKind::Port { .. }) {
                    continue;
                }
                let on = format!(
                    "{}/{}",
                    block.parts[*p].key, block.parts[*p].pins[*k].number
                );
                let end = st.shape(*p).map(|sh| sh.pins[*k].end);
                if !had.contains(&on) && end.is_some_and(|e| ends.contains(&e)) {
                    continue;
                }
                now.insert(on);
            }
            if !had.is_empty() && had == now {
                instead.insert(*n);
            }
        }
        // Text the previous layout put beside a part that hasn't moved
        // stays, and nothing new may take its place.
        // A part's text an instruction places, kept field by field where
        // every ask still holds in the previous layout.
        let mut kept_fields: BTreeMap<usize, BTreeMap<String, Point>> = BTreeMap::new();
        for &p in &parts {
            let part = &block.parts[p];
            let texts = part.lines();
            if texts.is_empty() {
                continue;
            }
            let unmoved = prev.and_then(|l| l.placed.get(&part.key)).is_some_and(|x| {
                let (s0, pl, _) = st.place[p].expect("placed");
                st.sheets[s0].0 == x.sheet && pl == Place::new(x.at, x.rotation, x.mirror)
            });
            if let Some(list) = asks.get(&p) {
                let Some(prev) = prev.filter(|_| unmoved) else {
                    continue;
                };
                let holds = list.iter().all(|(f, side)| {
                    instr::text_sides(block, prev, p, *f)
                        .iter()
                        .all(|(_, at)| *at == Some(*side))
                });
                let Some(was) = prev.text.get(&part.key) else {
                    continue;
                };
                let fields: Option<BTreeMap<String, Point>> = texts
                    .iter()
                    .map(|(n, _)| was.get(*n).map(|at| ((*n).to_owned(), *at)))
                    .collect();
                let Some(fields) = fields.filter(|_| holds) else {
                    continue;
                };
                let clear = texts.iter().all(|(n, t)| {
                    text::boxes(&[*t], fields[*n])
                        .iter()
                        .all(|b| field.clear(*b))
                });
                if clear {
                    for (n, t) in &texts {
                        for b in text::boxes(&[*t], fields[*n]) {
                            field.block(b);
                        }
                    }
                    kept_fields.insert(p, fields);
                }
                continue;
            }
            // Each field at its own point, where its box is still clear
            // (so a block split by an ask, or a bank member's one line,
            // stays as it was).
            let was = prev.filter(|_| unmoved).and_then(|l| l.text.get(&part.key));
            let fields: Option<BTreeMap<String, Point>> = was.and_then(|w| {
                texts
                    .iter()
                    .map(|(n, _)| w.get(*n).map(|at| ((*n).to_owned(), *at)))
                    .collect()
            });
            if let Some(fields) = fields
                && texts.iter().all(|(n, t)| {
                    text::boxes(&[*t], fields[*n])
                        .iter()
                        .all(|b| field.clear(*b))
                })
            {
                for (n, t) in &texts {
                    for b in text::boxes(&[*t], fields[*n]) {
                        field.block(b);
                    }
                }
                kept_fields.insert(p, fields);
            }
        }
        let base = field;
        let (field_done, labels, routes, stuck) = loop {
            let mut field = base.clone();
            let mut labels: BTreeMap<usize, Vec<Label>> = BTreeMap::new();
            let mut routes: BTreeMap<usize, Vec<Vec<Point>>> = BTreeMap::new();
            // Wires the previous layout drew that still hold, before any
            // label may take their place.
            let mut wired: Vec<usize> = styles
                .iter()
                .filter(|(n, s)| **s == NetStyle::Wire && !instead.contains(n))
                .map(|(n, _)| *n)
                .collect();
            wired.sort_by_key(|n| {
                let r = Rect::of(ends_of(*n).into_iter().map(|e| e.0)).expect("pins");
                (r.width() + r.height(), block.nets[*n].key.clone())
            });
            let mut to_route = Vec::new();
            for n in wired {
                let ends: BTreeSet<Point> = ends_of(n).into_iter().map(|e| e.0).collect();
                match runs_before.get(&n) {
                    Some(runs) if holds(runs, &ends) && mark_all(&mut field, n, runs) => {
                        routes.insert(n, runs.clone());
                    }
                    _ => to_route.push(n),
                }
            }
            // Decoupling capacitors wired to the pins they serve, their
            // rail's port on the wire, before any other port may take its
            // place.
            let mut joined: BTreeSet<(usize, usize)> = BTreeSet::new();
            let mut groups: BTreeMap<(usize, usize), BTreeSet<route::Node>> = BTreeMap::new();
            // New bypass wires go round every label the previous layout drew
            // on the sheet, so none of those is pushed from its place.
            let guard = |field: &Field| -> Field {
                let mut g = field.clone();
                if let Some(prev) = prev {
                    for (key, list) in &prev.labels {
                        for l in list.iter().filter(|l| l.sheet == sid) {
                            if block.net(key).is_some() {
                                g.block_label(label::boxed(block, key, l));
                            }
                        }
                    }
                }
                g
            };
            // Twice: first every capacitor whose previous wire still holds,
            // so a pin's capacitors wired anew never take another's place;
            // then the rest.
            for (search, b) in [false, true]
                .into_iter()
                .flat_map(|search| bypass.iter().map(move |b| (search, b)))
            {
                if styles.get(&b.net) != Some(&NetStyle::Port) || instead.contains(&b.net) {
                    continue;
                }
                let (Some((sa, _, _)), Some((sc, _, _))) =
                    (st.place[b.served.0], st.place[b.cap.0])
                else {
                    continue;
                };
                if sa != s || sc != s || joined.contains(&b.cap) {
                    continue;
                }
                let end = |(p, k): (usize, usize)| {
                    let pin = st.shape(p).expect("placed").pins[k];
                    (pin.end, pin.side)
                };
                // A pin's next capacitor joins the wire its first one made.
                if let Some(tree) = groups.get(&b.served) {
                    let (from, side) = end(b.cap);
                    // Its previous wire, if it still reaches the group.
                    let kept = runs_before.get(&b.net).and_then(|runs| {
                        runs.iter().find_map(|r| {
                            let mut r = r.clone();
                            if r.last() == Some(&from) {
                                r.reverse();
                            }
                            let to = route::node_of(*r.last()?);
                            (r.first() == Some(&from) && tree.contains(&to)).then_some(r)
                        })
                    });
                    let run = match kept.filter(|r| field.can_mark(b.net, r)) {
                        Some(r) => r,
                        None if !search => continue,
                        None => {
                            let Some(path) = guard(&field).search(b.net, from, side, tree) else {
                                continue;
                            };
                            route::run_of(&path)
                        }
                    };
                    if field.mark(b.net, &run).is_err() {
                        continue;
                    }
                    groups
                        .get_mut(&b.served)
                        .expect("a group")
                        .extend(route::expand(&run));
                    routes.entry(b.net).or_default().push(run);
                    joined.insert(b.cap);
                    continue;
                }
                let ends = [end(b.served), end(b.cap)];
                let pair: BTreeSet<Point> = ends.iter().map(|e| e.0).collect();
                let mut tried = field.clone();
                let kept = runs_before
                    .get(&b.net)
                    .and_then(|runs| {
                        runs.iter().find(|r| {
                            [r.first(), r.last()]
                                .into_iter()
                                .flatten()
                                .copied()
                                .collect::<BTreeSet<_>>()
                                == pair
                        })
                    })
                    .cloned()
                    .filter(|run| mark_all(&mut tried, b.net, std::slice::from_ref(run)));
                let run = match kept {
                    Some(run) => run,
                    None if !search => continue,
                    None => match route_net(&guard(&field), b.net, &ends, bypass_long) {
                        Some((_, mut runs))
                            if runs.len() == 1 && tried.mark(b.net, &runs[0]).is_ok() =>
                        {
                            runs.remove(0)
                        }
                        _ => continue,
                    },
                };
                let (p, k) = b.served;
                let name = block.label_text(p, k);
                let on = format!("{}/{}", block.parts[p].key, block.parts[p].pins[k].number);
                let had = prev_labels(b.net)
                    .into_iter()
                    .find(|l| l.on == on && l.kind == LabelKind::Power)
                    .map(|l| l.at);
                // With nowhere on the wire for the port, the pins take
                // ports of their own as any others.
                let intent = block.nets[b.net].intent.clone();
                let Some(at) = port_on_run(&tried, &run, &name, &intent, had) else {
                    continue;
                };
                field = tried;
                groups.insert(b.served, route::expand(&run).into_iter().collect());
                routes.entry(b.net).or_default().push(run);
                field.block_label(label::label_box_with(
                    LabelKind::Power,
                    &name,
                    &intent,
                    at,
                    Side::Up,
                ));
                labels.entry(b.net).or_default().push(Label {
                    text: name,
                    sheet: sid.clone(),
                    at,
                    side: Side::Up,
                    kind: LabelKind::Power,
                    on,
                });
                joined.insert(b.served);
                joined.insert(b.cap);
            }
            // A bank's row on a port's net: a wire from the pin to its
            // members, a net label on it at the pin.
            for row in bank_rows.iter().filter(|r| r.ported) {
                let (p, k) = row.pin;
                if st.place[p].is_none_or(|x| x.0 != s)
                    || row
                        .members
                        .iter()
                        .any(|m| st.place[m.0].is_none_or(|x| x.0 != s))
                {
                    continue;
                }
                let pin = st.shape(p).expect("placed").pins[k];
                let mut tried = field.clone();
                let mut runs = Vec::new();
                let mut tree: BTreeSet<route::Node> = BTreeSet::new();
                let mut ok = true;
                for (i, &(q, kq)) in row.members.iter().enumerate() {
                    let m = st.shape(q).expect("placed").pins[kq];
                    let run = if i == 0 && m.end[1] == pin.end[1] {
                        Some(vec![pin.end, m.end])
                    } else {
                        tried
                            .search(row.net, m.end, m.side, &tree)
                            .map(|path| route::run_of(&path))
                    };
                    match run {
                        Some(run) if tried.mark(row.net, &run).is_ok() => {
                            tree.extend(route::expand(&run));
                            runs.push(run);
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                let name = block.label_text(p, k);
                let b = label::label_box_with(
                    LabelKind::Net,
                    &name,
                    label::intent_on(&block.nets[row.net], LabelKind::Net),
                    pin.end,
                    pin.side,
                );
                // Clear before the row's wire, which lies under its line.
                if !field.clear_label(b) {
                    continue;
                }
                tried.block_label(b);
                field = tried;
                routes.entry(row.net).or_default().extend(runs);
                labels.entry(row.net).or_default().push(Label {
                    text: name,
                    sheet: sid.clone(),
                    at: pin.end,
                    side: pin.side,
                    kind: LabelKind::Net,
                    on: format!("{}/{}", block.parts[p].key, block.parts[p].pins[k].number),
                });
                joined.insert(row.pin);
                joined.extend(row.members.iter().copied());
            }
            // A port's flag beside its sole pin's part on this sheet is
            // wired to that pin by a short run, and neither takes a label:
            // the flag names the net (plan 2.G; a clamp beside a stack).
            for f in 0..block.parts.len() {
                if !matches!(block.parts[f].kind, PartKind::Port { .. }) {
                    continue;
                }
                let Some((p, k)) = place::sole_pin(block, f) else {
                    continue;
                };
                if st.place[f].map(|x| x.0) != Some(s)
                    || st.place[p].map(|x| x.0) != Some(s)
                    || joined.contains(&(p, k))
                {
                    continue;
                }
                let Some(n) = block.parts[p].pins[k].net else {
                    continue;
                };
                // Only a net drawn by labels: a wired net routes its flag
                // with its pins.
                if styles.get(&n).is_some_and(|st| *st == NetStyle::Wire) && !instead.contains(&n) {
                    continue;
                }
                let end = |(q, j): (usize, usize)| {
                    let pin = st.shape(q).expect("placed").pins[j];
                    (pin.end, pin.side)
                };
                if let Some((_, runs)) =
                    route_net(&field, n, &[end((p, k)), end((f, 0))], LOCAL_REACH)
                    && let [run] = runs.as_slice()
                    && route::length(run) <= LOCAL_REACH
                    && field.mark(n, run).is_ok()
                {
                    routes.entry(n).or_default().push(run.clone());
                    joined.insert((p, k));
                    joined.insert((f, 0));
                }
            }
            // Labels, the previous ones where they still fit.
            let mut want_label: Vec<(usize, usize, usize, LabelKind)> = Vec::new();
            for (n, list) in &pins_of {
                if styles[n] == NetStyle::Wire && !instead.contains(n) {
                    continue;
                }
                let kind = if instead.contains(n) {
                    LabelKind::Net
                } else {
                    label_kind(block, *n, styles[n])
                };
                for &(p, k) in list {
                    // A port's flag names its net: no label beside it.
                    let flag = kind == LabelKind::Net
                        && matches!(block.parts[p].kind, PartKind::Port { .. });
                    if !joined.contains(&(p, k)) && !flag {
                        want_label.push((*n, p, k, kind));
                    }
                }
            }
            // A net leaving its sheet takes one off-sheet connector a
            // region (plan 2.C): its pins are grouped by their part's
            // region (a part in no region a group of its own); the pin of
            // the part with the most pins carries the connector, the
            // group's others are wired to it, else take local labels.
            let carriers = carriers(
                &st,
                &mut want_label,
                &net_sheets,
                &stacked,
                bank_rows,
                &prev_labels,
            );
            let stuck = place_labels(
                block,
                &st,
                &mut field,
                &want_label,
                &carriers,
                &prev_labels,
                &runs_before,
                &sid,
                &mut labels,
                &mut routes,
            );
            // The rest, shortest first.
            let mut failed = Vec::new();
            for n in to_route {
                match route_net(&field, n, &ends_of(n), long) {
                    Some((f, runs)) => {
                        field = f;
                        routes.insert(n, runs);
                    }
                    None => failed.push(n),
                }
            }
            if failed.is_empty() {
                break (field, labels, routes, stuck);
            }
            instead.extend(failed);
        };
        let mut field = field_done;
        crowded.extend(stuck);
        if !instead.is_empty() {
            fallbacks
                .entry(sid.clone())
                .or_default()
                .extend(instead.iter().map(|n| block.nets[*n].key.clone()));
        }
        for (n, style) in asked {
            if *style != NetStyle::Wire || !pins_of.contains_key(n) {
                continue;
            }
            if stacked.contains(n) && block.nets[*n].kind == NetKind::Signal {
                not_held.insert(
                    *n,
                    "a stacked pin's net is drawn by labels: one label names each layer's net",
                );
            } else if instead.contains(n) {
                not_held.insert(
                    *n,
                    if spanning(&ends_of(*n)) > long {
                        "its pins are too far apart to wire"
                    } else {
                        "its wires would cross too many others or found no way round the parts"
                    },
                );
            }
        }
        // Text.
        for &p in &parts {
            let part = &block.parts[p];
            if matches!(part.kind, PartKind::Port { .. }) {
                continue;
            }
            let lines = part.lines();
            if lines.is_empty() {
                continue;
            }
            let texts: Vec<Line> = lines.iter().map(|(_, t)| *t).collect();
            let body = st.shape(p).expect("placed").body;
            // #712: never in the room in front of one of its own pin ends.
            let fronts = pin_fronts(&st.shape(p).expect("placed"));
            let clear_of = |bs: &[Rect]| bs.iter().all(|b| fronts.iter().all(|f| !f.overlaps(b)));
            // Text kept field by field (asked parts' too, below).
            if !asks.contains_key(&p)
                && let Some(f) = kept_fields.get(&p)
            {
                layout.text.insert(part.key.clone(), f.clone());
                continue;
            }
            // Text an instruction places: each asked group on its side,
            // then the rest as one block where phase 1 puts it.
            if let Some(list) = asks.get(&p) {
                let fields = match kept_fields.get(&p) {
                    Some(f) => f.clone(),
                    None => {
                        let mut fields: BTreeMap<String, Point> = BTreeMap::new();
                        fn place(
                            field: &mut Field,
                            fields: &mut BTreeMap<String, Point>,
                            group: Vec<(&str, Line)>,
                            at: Option<Point>,
                            fallback: Point,
                        ) {
                            let at = at.unwrap_or(fallback);
                            let ts: Vec<Line> = group.iter().map(|(_, t)| *t).collect();
                            for b in text::boxes(&ts, at) {
                                field.block(b);
                            }
                            for (i, (n, _)) in group.iter().enumerate() {
                                fields
                                    .insert((*n).to_owned(), [at[0], at[1] - text::LINE * i as Nm]);
                            }
                        }
                        let mut done: BTreeSet<&str> = BTreeSet::new();
                        for (f, side) in list {
                            let group: Vec<(&str, Line)> = lines
                                .iter()
                                .copied()
                                .filter(|(n, _)| f.covers(n) && !done.contains(n))
                                .collect();
                            if group.is_empty() {
                                continue;
                            }
                            done.extend(group.iter().map(|(n, _)| *n));
                            let ts: Vec<Line> = group.iter().map(|(_, t)| *t).collect();
                            let cands = text::candidates_on(body, &ts, *side);
                            let at = cands.iter().copied().find(|c| {
                                let bs = text::boxes(&ts, *c);
                                bs.iter().all(|b| field.clear(*b)) && clear_of(&bs)
                            });
                            let fallback = cands
                                .first()
                                .copied()
                                .unwrap_or_else(|| text::candidates(body, &ts)[0]);
                            place(&mut field, &mut fields, group, at, fallback);
                        }
                        let rest: Vec<(&str, Line)> = lines
                            .iter()
                            .copied()
                            .filter(|(n, _)| !done.contains(n))
                            .collect();
                        if !rest.is_empty() {
                            let ts: Vec<Line> = rest.iter().map(|(_, t)| *t).collect();
                            let cands = text::candidates(body, &ts);
                            let at = cands.iter().copied().find(|c| {
                                let bs = text::boxes(&ts, *c);
                                bs.iter().all(|b| field.clear(*b)) && clear_of(&bs)
                            });
                            place(&mut field, &mut fields, rest, at, cands[0]);
                        }
                        fields
                    }
                };
                layout.text.insert(part.key.clone(), fields);
                continue;
            }
            // A bank row's first member: one line on its row past its rail
            // end (rows two steps apart leave no room for two).
            if let Some(row) = bank_row_of(&st, bank_rows, p) {
                let ext = st.shape(p).expect("placed").extent;
                let y = st.shape(row.pin.0).expect("placed").pins[row.pin.1].end[1];
                let base = (y - font::SIZE / 4).div_euclid(GRID) * GRID;
                let w = text::row_width(&texts);
                // Under the row first, then on it (a port at its rail end
                // may take the place under it), stepping out.
                let start = |d: Nm, y: Nm| match row.side {
                    Side::Left => [(ext.min[0] - GRID - w).div_euclid(GRID) * GRID - d, y],
                    _ => [
                        (ext.max[0] + GRID - 1).div_euclid(GRID) * GRID + GRID + d,
                        y,
                    ],
                };
                let fits = |at: Point| {
                    text::row_points(&texts, at)
                        .iter()
                        .zip(&texts)
                        .all(|(q, t)| text::boxes(&[*t], *q).iter().all(|b| field.clear(*b)))
                };
                let at = (0..=24)
                    .flat_map(|k| [start(k * GRID, base), start(k * GRID, base + GRID)])
                    .find(|a| fits(*a));
                if let Some(at) = at {
                    let pts = text::row_points(&texts, at);
                    let mut fields = BTreeMap::new();
                    for ((name, t), q) in lines.iter().zip(&pts) {
                        for b in text::boxes(&[*t], *q) {
                            field.block(b);
                        }
                        fields.insert((*name).to_owned(), *q);
                    }
                    layout.text.insert(part.key.clone(), fields);
                    continue;
                }
            }
            let at = {
                let fits = |at: Point| {
                    let bs = text::boxes(&texts, at);
                    bs.iter().all(|b| field.clear(*b)) && clear_of(&bs)
                };
                let at = text::candidates(body, &texts)
                    .into_iter()
                    .find(|c| fits(*c))
                    .unwrap_or_else(|| text::candidates(body, &texts)[0]);
                for b in text::boxes(&texts, at) {
                    field.block(b);
                }
                at
            };
            let mut fields = BTreeMap::new();
            for (i, (name, _)) in lines.iter().enumerate() {
                fields.insert((*name).to_owned(), [at[0], at[1] - text::LINE * i as Nm]);
            }
            layout.text.insert(part.key.clone(), fields);
        }
        for (n, mut runs) in routes {
            // In a fixed order, whatever order they were found in: a layout
            // laid out again over itself is the same file.
            runs.sort();
            if !runs.is_empty() {
                layout
                    .routes
                    .entry(block.nets[n].key.clone())
                    .or_default()
                    .insert(sid.clone(), runs);
            }
        }
        for (n, list) in labels {
            layout
                .labels
                .entry(block.nets[n].key.clone())
                .or_default()
                .extend(list);
        }
    }
    for list in layout.labels.values_mut() {
        list.sort_by(|a, b| (&a.sheet, &a.on).cmp(&(&b.sheet, &b.on)));
    }
    // A sheet nothing stands on is no sheet (one is always kept).
    let used: BTreeSet<String> = layout.placed.values().map(|x| x.sheet.clone()).collect();
    if !used.is_empty() {
        layout.sheets.retain(|id, _| used.contains(id));
    }
    crate::canvas::arrange(block, &mut layout, prev);
    let blocked = st.blocked;
    let mut notes = st.notes;
    notes.extend(bus_notes);
    for (n, why) in not_held {
        notes.push(Note {
            code: "net_style_not_held".into(),
            message: format!(
                "{} is drawn by labels, not wires: {why}",
                block.nets[n].name
            ),
            details: serde_json::json!({"net": block.nets[n].key, "why": why}),
        });
    }
    crate::region::frames(block, &mut layout);
    // Notes last, in what is left (plan 2.C), and the frames again to hold
    // those in regions.
    if place_notes(block, &mut layout, prev, &mut notes) {
        crate::region::frames(block, &mut layout);
    }
    Outcome {
        layout,
        notes,
        fallbacks,
        idioms: Vec::new(),
        flow: BTreeSet::new(),
        crowded,
        blocked,
    }
}

/// Where a supply port pointing up sits on `run`: where the previous
/// layout had it if that is still on the run and clear, else its first
/// bend, else the node nearest the run's middle; never on a pin's end nor
/// where the run goes on up. `None` when nowhere is clear (the wire alone
/// joins the pins, and the rail's other ports name it).
fn port_on_run(
    field: &Field,
    run: &[Point],
    name: &str,
    intent: &str,
    had: Option<Point>,
) -> Option<Point> {
    let nodes: Vec<Point> = route::expand(run)
        .into_iter()
        .map(route::point_of)
        .collect();
    let (first, last) = (run[0], *run.last().expect("a run has ends"));
    let up = |p: Point| nodes.contains(&[p[0], p[1] + GRID]);
    let ok = |p: Point| {
        p != first
            && p != last
            && !up(p)
            && field.clear_label(label::label_box_with(
                LabelKind::Power,
                name,
                intent,
                p,
                Side::Up,
            ))
    };
    let mut order: Vec<Point> = had.into_iter().filter(|p| nodes.contains(p)).collect();
    order.extend(run[1..run.len() - 1].iter().copied());
    let mid = (nodes.len() / 2) as i64;
    let mut rest: Vec<(i64, Point)> = nodes
        .iter()
        .enumerate()
        .map(|(i, p)| ((i as i64 - mid).abs(), *p))
        .collect();
    rest.sort();
    order.extend(rest.into_iter().map(|(_, p)| p));
    order.into_iter().find(|p| ok(*p))
}

/// The room in front of each of a part's pin ends (#712): three steps out
/// along the pin and a step either side of its row, where its wire or
/// label goes. A connected pin's wire holds it already when text is
/// placed; an unconnected pin's must be kept clear too.
fn pin_fronts(shape: &Shape) -> Vec<Rect> {
    shape
        .pins
        .iter()
        .map(|p| {
            let e = p.end;
            match p.side {
                Side::Right => Rect::new([e[0], e[1] - GRID], [e[0] + 3 * GRID, e[1] + GRID]),
                Side::Left => Rect::new([e[0] - 3 * GRID, e[1] - GRID], [e[0], e[1] + GRID]),
                Side::Up => Rect::new([e[0] - GRID, e[1]], [e[0] + GRID, e[1] + 3 * GRID]),
                Side::Down => Rect::new([e[0] - GRID, e[1] - 3 * GRID], [e[0] + GRID, e[1]]),
            }
        })
        .collect()
}

/// The previous layout's runs on sheet `sid`, by net index.
fn prev_runs(block: &Block, prev: Option<&Layout>, sid: &str) -> BTreeMap<usize, Vec<Vec<Point>>> {
    let mut out = BTreeMap::new();
    let Some(prev) = prev else { return out };
    for (key, sheets) in &prev.routes {
        if let (Some(n), Some(runs)) = (block.net(key), sheets.get(sid)) {
            out.insert(n, runs.clone());
        }
    }
    out
}

/// Whether runs connect exactly the pin ends `ends`: every end on a run,
/// every run's ends on a pin end or on another run.
fn holds(runs: &[Vec<Point>], ends: &BTreeSet<Point>) -> bool {
    let nodes: Vec<BTreeSet<route::Node>> = runs
        .iter()
        .map(|r| route::expand(r).into_iter().collect())
        .collect();
    let all: BTreeSet<route::Node> = nodes.iter().flatten().copied().collect();
    if !ends.iter().all(|e| all.contains(&route::node_of(*e))) {
        return false;
    }
    for (i, r) in runs.iter().enumerate() {
        if r.len() < 2 {
            return false;
        }
        for tip in [r[0], *r.last().expect("a run has an end")] {
            let on_other = nodes
                .iter()
                .enumerate()
                .any(|(j, set)| j != i && set.contains(&route::node_of(tip)));
            if !ends.contains(&tip) && !on_other {
                return false;
            }
        }
    }
    // One tree: every run reaches every other through shared nodes.
    let mut seen = vec![false; runs.len()];
    let mut stack = vec![0];
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        for j in 0..runs.len() {
            if !seen[j] && !nodes[i].is_disjoint(&nodes[j]) {
                stack.push(j);
            }
        }
    }
    seen.iter().all(|s| *s)
}

/// The length a net's wires would take at least: its pin ends' spanning
/// tree's, by Manhattan distance.
fn spanning(ends: &[(Point, Side)]) -> Nm {
    let mut joined = vec![false; ends.len()];
    let mut best = vec![Nm::MAX; ends.len()];
    let mut total = 0;
    if ends.is_empty() {
        return 0;
    }
    best[0] = 0;
    for _ in 0..ends.len() {
        let (i, d) = best
            .iter()
            .enumerate()
            .filter(|(i, _)| !joined[*i])
            .min_by_key(|(_, d)| **d)
            .map(|(i, d)| (i, *d))
            .expect("one left");
        joined[i] = true;
        total += d;
        for j in 0..ends.len() {
            let dd = (ends[i].0[0] - ends[j].0[0]).abs() + (ends[i].0[1] - ends[j].0[1]).abs();
            if !joined[j] && dd < best[j] {
                best[j] = dd;
            }
        }
    }
    total
}

/// Marks every run, all or none.
fn mark_all(field: &mut Field, net: usize, runs: &[Vec<Point>]) -> bool {
    field.mark_all(net, runs)
}

/// Routes a net through its pin ends, each joining the tree the ones
/// before it made, nearest first; `None` when one can't, or the wires
/// run past `long`.
fn route_net(
    field: &Field,
    net: usize,
    ends: &[(Point, Side)],
    long: Nm,
) -> Option<(Field, Vec<Vec<Point>>)> {
    let mut f = field.clone();
    let mut runs = Vec::new();
    let mut joined = vec![ends[0].0];
    let mut tree: BTreeSet<route::Node> = [route::node_of(ends[0].0)].into();
    let mut left: Vec<(Point, Side)> = ends[1..].to_vec();
    let mut total = 0;
    while !left.is_empty() {
        let (i, _) = left
            .iter()
            .enumerate()
            .map(|(i, (p, _))| {
                let d = joined
                    .iter()
                    .map(|q| (p[0] - q[0]).abs() + (p[1] - q[1]).abs())
                    .min()
                    .expect("joined");
                (i, (d, *p))
            })
            .min_by_key(|(_, k)| *k)
            .expect("left");
        let (from, side) = left.remove(i);
        let path = f.search(net, from, side, &tree)?;
        if f.crossings(net, &path) > CROWDED {
            return None;
        }
        let run = route::run_of(&path);
        total += route::length(&run);
        if total > long {
            return None;
        }
        f.mark(net, &run).ok()?;
        tree.extend(path.iter().copied());
        joined.push(from);
        runs.push(run);
    }
    Some((f, runs))
}

/// Gives each net in `want` that leaves its sheet one off-sheet connector
/// a region: in each group of its pins (by their part's region), the pin
/// of the part with the most pins (then the least part key, then the
/// least pin number) is turned into `LabelKind::OffSheet`; the group's
/// other pins keep `Net` and are mapped to their carrier, by `(part,
/// pin)`. A stacked pin's net and a bank row's port net keep their net
/// labels (their labels say more than one net, or stand in a row). Over a
/// previous layout, a pin whose label was a connector stays one, so an
/// edit that joins two regions moves no connector.
fn carriers(
    st: &State,
    want: &mut [(usize, usize, usize, LabelKind)],
    net_sheets: &BTreeMap<usize, BTreeSet<usize>>,
    stacked: &BTreeSet<usize>,
    bank_rows: &[idiom::BankRow],
    prev_labels: &dyn Fn(usize) -> Vec<Label>,
) -> BTreeMap<(usize, usize), (usize, usize)> {
    let block = st.block;
    let region_of: BTreeMap<&str, &str> = st
        .regions
        .iter()
        .flat_map(|(id, r)| r.parts.iter().map(move |k| (k.as_str(), id.as_str())))
        .collect();
    // By net and group, the indices into `want`.
    let mut groups: BTreeMap<(usize, &str), Vec<usize>> = BTreeMap::new();
    for (i, &(n, p, _, kind)) in want.iter().enumerate() {
        let leaves = net_sheets.get(&n).is_some_and(|s| s.len() > 1);
        if !leaves
            || kind != LabelKind::Net
            || block.nets[n].kind != NetKind::Signal
            || stacked.contains(&n)
            || bank_rows.iter().any(|r| r.names(n))
        {
            continue;
        }
        let key = block.parts[p].key.as_str();
        let group = region_of.get(key).copied().unwrap_or(key);
        groups.entry((n, group)).or_default().push(i);
    }
    let mut out = BTreeMap::new();
    for (&(n, _), list) in &groups {
        let was: BTreeSet<String> = prev_labels(n)
            .into_iter()
            .filter(|l| l.kind == LabelKind::OffSheet)
            .map(|l| l.on)
            .collect();
        let on = |i: usize| {
            let (_, p, k, _) = want[i];
            format!("{}/{}", block.parts[p].key, block.parts[p].pins[k].number)
        };
        let kept: Vec<usize> = list
            .iter()
            .copied()
            .filter(|i| was.contains(&on(*i)))
            .collect();
        if let Some(&first) = kept.first() {
            for &i in &kept {
                want[i].3 = LabelKind::OffSheet;
            }
            let (_, cp, ck, _) = want[first];
            for &i in list {
                if !kept.contains(&i) {
                    out.insert((want[i].1, want[i].2), (cp, ck));
                }
            }
            continue;
        }
        let &carrier = list
            .iter()
            .min_by(|&&a, &&b| {
                let (_, pa, ka, _) = want[a];
                let (_, pb, kb, _) = want[b];
                let (x, y) = (&block.parts[pa], &block.parts[pb]);
                y.pins
                    .len()
                    .cmp(&x.pins.len())
                    .then(x.key.cmp(&y.key))
                    .then(x.pins[ka].number.cmp(&y.pins[kb].number))
            })
            .expect("a group");
        want[carrier].3 = LabelKind::OffSheet;
        let (_, cp, ck, _) = want[carrier];
        for &i in list {
            if i != carrier {
                out.insert((want[i].1, want[i].2), (cp, ck));
            }
        }
    }
    out
}

/// Places each note instruction (plan 2.C): at its previous place where
/// that is clear, else in its region at the first clear place from the
/// frame's lower left (rightwards, then up), the frame free to grow down by
/// [`crate::region::WIDEN`]; on a sheet from the drawing area's lower left.
/// Clear is off everything drawn, by half a step, and every other region's
/// frame. A note with no place is noted `note_unplaced`. Whether any
/// note was placed.
fn place_notes(
    block: &Block,
    layout: &mut Layout,
    prev: Option<&Layout>,
    notes: &mut Vec<Note>,
) -> bool {
    let mut any = false;
    for (id, ins) in &block.instructions {
        let Instruction::Note { on, .. } = ins else {
            continue;
        };
        let Some(text) = block.note_text(id) else {
            continue;
        };
        let (sid, room, region) = match on.strip_prefix("region:") {
            Some(r) => match layout.regions.get(r) {
                Some(g) => {
                    let f = g.frame_on_sheet();
                    (
                        g.sheet.clone(),
                        Rect::new([f.min[0], f.min[1] - crate::region::WIDEN], f.max),
                        Some(r.to_owned()),
                    )
                }
                None => {
                    notes.push(unplaced(id, on));
                    continue;
                }
            },
            None => {
                let sid = layout
                    .sheets
                    .iter()
                    .find(|(s, x)| *s == on || &x.name == on)
                    .map(|(s, _)| s.clone());
                let Some(sid) = sid else {
                    notes.push(unplaced(id, on));
                    continue;
                };
                let a = block.area_in(layout, &sid);
                (
                    sid,
                    Rect::new([GRID, GRID], [a[0] - GRID, a[1] - GRID]),
                    None,
                )
            }
        };
        let area = block.area_in(layout, &sid);
        let inside = Rect::new([0, 0], area);
        let mut ink = crate::lint::ink_boxes(block, layout, &sid);
        ink.extend(
            layout
                .regions
                .iter()
                .filter(|(r, g)| g.sheet == sid && Some(r.as_str()) != region.as_deref())
                // A frame grows round what it holds: keep its margin clear.
                .map(|(_, g)| g.frame_on_sheet().grow(crate::region::FRAME)),
        );
        let clear = |b: Rect| {
            room.contains(&b)
                && inside.contains(&b)
                && !ink.iter().any(|x| x.grow(GRID / 2).overlaps(&b))
        };
        let kept = prev
            .and_then(|l| l.notes.get(id))
            .filter(|n| n.sheet == sid && clear(crate::paint::note_box(text, n.at)))
            .map(|n| n.at);
        let lo = [
            room.min[0].div_euclid(GRID) * GRID + GRID,
            room.min[1].div_euclid(GRID) * GRID + GRID,
        ];
        let found = kept.or_else(|| {
            let mut y = lo[1];
            while y < room.max[1] {
                let mut x = lo[0];
                while x < room.max[0] {
                    if clear(crate::paint::note_box(text, [x, y])) {
                        return Some([x, y]);
                    }
                    x += GRID;
                }
                y += GRID;
            }
            None
        });
        match found {
            Some(at) => {
                layout
                    .notes
                    .insert(id.clone(), crate::layout::PlacedNote { sheet: sid, at });
                any = true;
            }
            None => notes.push(unplaced(id, on)),
        }
    }
    any
}

/// A note with no place on `on`.
fn unplaced(id: &str, on: &str) -> Note {
    Note {
        code: "note_unplaced".into(),
        message: format!("no room for note {id} on {on}"),
        details: serde_json::json!({"instruction": id, "on": on}),
    }
}

/// A bus noted `bus_not_drawn` on sheet `sid`, and why.
fn bus_note(block: &Block, i: usize, sid: &str, why: &str) -> Note {
    let bus = &block.buses[i];
    Note {
        code: "bus_not_drawn".into(),
        message: format!("{} is drawn by labels: {why}", bus.name),
        details: serde_json::json!({"bus": bus.key, "sheet": sid, "why": why}),
    }
}

/// A bundle drawn into `field` (plan 2.C): each member's stub (its net's
/// wire) and the name it carries, the spine through each end's entries
/// and a trunk between the spines, all marked; the space the entries
/// cross blocked. `Err` says why it can't be, and marks nothing worth
/// keeping (the caller puts the field back).
fn draw_bus(
    block: &Block,
    st: &State,
    field: &mut Field,
    i: usize,
    bundle: &crate::bus::Bundle,
    prev: Option<&crate::layout::DrawnBus>,
    avoid: &[Rect],
) -> Result<crate::layout::DrawnBus, String> {
    // Over a previous layout, the bus as it was drawn where its entries
    // are the same pins at the same places, its names still fit its stubs
    // and it all marks again.
    if let Some(p) = prev {
        let stub =
            crate::bus::stub_len(bundle.members.iter().map(|n| block.nets[*n].name.as_str()));
        let same = {
            let mut a: Vec<(&str, Point, Side)> = p
                .entries
                .iter()
                .map(|e| (e.on.as_str(), e.at, e.side))
                .collect();
            let mut b: Vec<(String, Point, Side)> = Vec::new();
            for (end, pins) in &bundle.ends {
                let shape = st.shape(end.part).expect("placed");
                for &k in pins {
                    let part = &block.parts[end.part];
                    b.push((
                        format!("{}/{}", part.key, part.pins[k].number),
                        shape.pins[k].end,
                        shape.pins[k].side,
                    ));
                }
            }
            let mut b: Vec<(&str, Point, Side)> =
                b.iter().map(|(o, at, s)| (o.as_str(), *at, *s)).collect();
            a.sort();
            b.sort();
            a == b
        };
        if same && stub <= p.stub {
            let mut g = field.clone();
            if mark_bus(block, &mut g, i, p) {
                *field = g;
                return Ok(p.clone());
            }
        }
    }
    let mut g = field.clone();
    for r in avoid {
        g.block(*r);
    }
    let d = find_bus(block, st, &mut g, i, bundle)?;
    if !mark_bus(block, field, i, &d) {
        return Err(format!(
            "its entries have no room beside {}",
            block.parts[bundle.ends[0].0.part].refdes
        ));
    }
    Ok(d)
}

/// Marks a drawn bus into `field` as [`find_bus`] marks one: each stub
/// (its member's net) and the name on it, each spine and trunk run (the
/// bus's id), the columns its entries cross blocked, its connector's box
/// and the bundle's name. Whether it all marked (else `field` is part
/// marked: the caller works on a clone).
fn mark_bus(block: &Block, field: &mut Field, i: usize, d: &crate::layout::DrawnBus) -> bool {
    let id = BUS_NET + i;
    let mut columns: BTreeMap<Nm, Vec<(Point, Point)>> = BTreeMap::new();
    let mut members: Vec<usize> = Vec::new();
    for e in &d.entries {
        let Some(n) = block.net(&e.net) else {
            return false;
        };
        if !members.contains(&n) {
            members.push(n);
        }
        let (s, on_spine) = crate::bus::entry(e.at, e.side, d.stub);
        let name_box = label::label_box(LabelKind::Net, &block.nets[n].name, e.at, e.side);
        if !field.clear_label(name_box) || field.mark(n, &[e.at, s]).is_err() {
            return false;
        }
        field.block_label(name_box);
        columns
            .entry(on_spine[0])
            .or_default()
            .push((e.at, on_spine));
    }
    for list in columns.values() {
        let ends: Vec<Point> = list.iter().map(|(_, e)| *e).collect();
        let Some(spine) = crate::bus::spine(&ends) else {
            return false;
        };
        if field.mark(id, &spine).is_err() {
            return false;
        }
        let pts = list.iter().flat_map(|(a, e)| [*a, *e]);
        if let Some(r) = Rect::of(pts) {
            let (x0, x1) = (r.min[0] + GRID, r.max[0] - GRID);
            if x0 <= x1 {
                field.block_label(Rect::new([x0, r.min[1]], [x1, r.max[1]]));
            }
        }
    }
    // The connector's box before the trunk that ends at its anchor.
    let flag = d
        .connector
        .as_ref()
        .map(|c| label::label_box(LabelKind::OffSheet, &c.text, c.at, c.side));
    if flag.is_some_and(|b| !field.clear_label(b)) {
        return false;
    }
    for run in &d.trunk {
        if field.mark(id, run).is_err() {
            return false;
        }
    }
    if let Some(b) = flag {
        field.block_label(b);
    } else if let Some(run) = d.trunk.first() {
        let name = crate::bus::name(block, i, &members);
        let side = label::pointing(LabelKind::Net, toward(run[0], run[1]));
        if let Some((p, anchor, _)) = label::drawn(LabelKind::Net, &name, run[0], side).text {
            let b = crate::font::text_box(&name, crate::font::SIZE, p, anchor);
            if field.overlaps_box(b) {
                return false;
            }
            field.block(b);
        }
    }
    true
}

/// Finds where a bundle's bus goes in `field` (a scratch field): see
/// [`draw_bus`].
fn find_bus(
    block: &Block,
    st: &State,
    field: &mut Field,
    i: usize,
    bundle: &crate::bus::Bundle,
) -> Result<crate::layout::DrawnBus, String> {
    use crate::layout::{BusEntry, DrawnBus};
    let id = BUS_NET + i;
    let name_of = |n: usize| block.nets[n].name.as_str();
    let stub = crate::bus::stub_len(bundle.members.iter().map(|n| name_of(*n)));
    let mut entries = Vec::new();
    let mut spines: Vec<[Point; 2]> = Vec::new();
    let mut ends: Vec<(Point, Side)> = Vec::new();
    for (end, pins) in &bundle.ends {
        let who = &block.parts[end.part].refdes;
        let shape = st.shape(end.part).expect("placed");
        let mut on_spine = Vec::new();
        let mut lo = [Nm::MAX, Nm::MAX];
        let mut hi = [Nm::MIN, Nm::MIN];
        for (m, &k) in bundle.members.iter().zip(pins) {
            let pin = shape.pins[k];
            let (s, e) = crate::bus::entry(pin.end, pin.side, stub);
            let name_box = label::label_box(LabelKind::Net, name_of(*m), pin.end, pin.side);
            if !field.clear_label(name_box) {
                return Err(format!("its entries have no room beside {who}"));
            }
            field
                .mark(*m, &[pin.end, s])
                .map_err(|_| format!("its entries have no room beside {who}"))?;
            field.block_label(name_box);
            on_spine.push(e);
            for p in [pin.end, s, e] {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
            entries.push(BusEntry {
                net: block.nets[*m].key.clone(),
                on: format!(
                    "{}/{}",
                    block.parts[end.part].key, block.parts[end.part].pins[k].number
                ),
                at: pin.end,
                side: pin.side,
            });
        }
        let spine = crate::bus::spine(&on_spine)
            .ok_or_else(|| format!("its entries beside {who} don't line up"))?;
        field
            .mark(id, &spine)
            .map_err(|_| format!("its entries have no room beside {who}"))?;
        // Nothing crosses the entries: the columns between the pins' ends
        // and the spine, over the entries' height, are blocked.
        let side = shape.pins[pins[0]].side;
        let (x0, x1) = (lo[0] + GRID, hi[0] - GRID);
        if x0 <= x1 {
            field.block_label(Rect::new([x0, lo[1]], [x1, hi[1]]));
        }
        let mid_y = (spine[0][1] + spine[1][1]).div_euclid(2 * GRID) * GRID;
        ends.push(([spine[0][0], mid_y], side));
        spines.push(spine);
    }
    let mut trunk = Vec::new();
    let name = crate::bus::name(block, i, &bundle.members);
    let mut connector = None;
    if let [(mid, side)] = ends[..] {
        // One end: the members leave the sheet. A short trunk out from the
        // spine's middle along the pins' way (else from its top going up,
        // or its bottom going down, then out) ends at the bus's connector.
        let d = side.step();
        let out = |p: Point, k: Nm| [p[0] + d[0] * k, p[1]];
        let [top, bottom] = spines[0];
        let tries: Vec<Vec<Point>> = vec![
            vec![mid, out(mid, 4)],
            vec![
                top,
                [top[0], top[1] + 2 * GRID],
                out([top[0], top[1] + 2 * GRID], 4),
            ],
            vec![
                bottom,
                [bottom[0], bottom[1] - 2 * GRID],
                out([bottom[0], bottom[1] - 2 * GRID], 4),
            ],
        ];
        let found = tries.into_iter().find_map(|run| {
            let at = *run.last().expect("a run");
            let b = label::label_box(LabelKind::OffSheet, &name, at, side);
            (field.can_mark(id, &run) && field.clear_label(b)).then_some((run, at, b))
        });
        let (run, at, b) = found.ok_or_else(|| {
            format!(
                "no room for its off-sheet connector beside {}",
                block.parts[bundle.ends[0].0.part].refdes
            )
        })?;
        field
            .mark(id, &run)
            .map_err(|_| "no room for its trunk".to_owned())?;
        field.block_label(b);
        trunk.push(run);
        connector = Some(crate::layout::BusConnector {
            at,
            side,
            text: name.clone(),
        });
    }
    if let [(from, side), _] = ends[..] {
        let target: BTreeSet<route::Node> = route::expand(&spines[1]).into_iter().collect();
        let path = field.search(id, from, side, &target).ok_or_else(|| {
            format!(
                "no way for its trunk between {} and {}",
                block.parts[bundle.ends[0].0.part].refdes,
                block.parts[bundle.ends[1].0.part].refdes
            )
        })?;
        let run = route::run_of(&path);
        field
            .mark(id, &run)
            .map_err(|_| "no way for its trunk".to_owned())?;
        trunk.push(run);
    }
    // The bundle's name on its trunk, where no connector says it.
    if let Some(run) = trunk.first().filter(|_| connector.is_none()) {
        let side = label::pointing(LabelKind::Net, toward(run[0], run[1]));
        if let Some((p, anchor, _)) = label::drawn(LabelKind::Net, &name, run[0], side).text {
            let b = crate::font::text_box(&name, crate::font::SIZE, p, anchor);
            if field.overlaps_box(b) {
                return Err(format!("no room for {name} on its trunk"));
            }
            field.block(b);
        }
    }
    Ok(DrawnBus {
        stub,
        trunk,
        entries,
        connector,
    })
}

/// The way from `a` to `b` along a run's first segment.
fn toward(a: Point, b: Point) -> Side {
    match ((b[0] - a[0]).signum(), (b[1] - a[1]).signum()) {
        (1, _) => Side::Right,
        (-1, _) => Side::Left,
        (_, 1) => Side::Up,
        _ => Side::Down,
    }
}

/// Whether a previous label of kind `was` stands for one of kind `now`:
/// the same kind, or a net label where an off-sheet connector is wanted
/// (a layout from before plan 2.C), which keeps its place as one.
fn was_kind(was: LabelKind, now: LabelKind) -> bool {
    was == now || (now == LabelKind::OffSheet && was == LabelKind::Net)
}

/// Places labels for `want` (net, part, pin, kind): the previous label
/// for the pin if it still fits, else the first candidate that does, else
/// the first.
#[allow(clippy::too_many_arguments)]
fn place_labels(
    block: &Block,
    st: &State,
    field: &mut Field,
    want: &[(usize, usize, usize, LabelKind)],
    carriers: &BTreeMap<(usize, usize), (usize, usize)>,
    prev_labels: &dyn Fn(usize) -> Vec<Label>,
    prev_runs: &BTreeMap<usize, Vec<Vec<Point>>>,
    sid: &str,
    labels: &mut BTreeMap<usize, Vec<Label>>,
    routes: &mut BTreeMap<usize, Vec<Vec<Point>>>,
) -> Vec<usize> {
    // Labels that keep their place first, so nothing new takes it: those
    // saying what they said, then renamed ones, then new ones; within
    // each, net labels (which lie along their pins) before ports (which
    // turn off them, and so reach past their neighbours' labels).
    let mut order: Vec<(u8, bool, Nm, std::cmp::Reverse<usize>, usize)> = Vec::new();
    for (i, &(n, p, k, kind)) in want.iter().enumerate() {
        let on = format!("{}/{}", block.parts[p].key, block.parts[p].pins[k].number);
        let had = prev_labels(n)
            .into_iter()
            .find(|l| l.on == on && was_kind(l.kind, kind));
        // A pin of a net the previous layout labelled, wired then to the
        // net's own (no label of its own), keeps that wire with the labels
        // that keep theirs.
        let end = st.shape(p).expect("placed").pins[k].end;
        let wired_before = kind == LabelKind::Net
            && !prev_labels(n).is_empty()
            && prev_runs
                .get(&n)
                .into_iter()
                .flatten()
                .any(|r| r[0] == end || r[r.len() - 1] == end);
        let rank = match had {
            Some(l) if l.text == block.label_text(p, k) && l.kind == kind => 0,
            Some(_) => 1,
            None if wired_before => 0,
            None => 2,
        };
        // Ground drops below its pin and a supply rises above it: from
        // the lowest pin up for ground and the highest down for supplies,
        // each turns off its pin clear of those placed before it.
        let y = st.shape(p).expect("placed").pins[k].end[1];
        let height = match kind {
            LabelKind::Net | LabelKind::OffSheet => 0,
            LabelKind::Ground => y,
            LabelKind::Power => -y,
        };
        // A part with more pins labels first, so a small part beside it
        // joins its label by a short wire rather than the reverse.
        let pins = std::cmp::Reverse(block.parts[p].pins.len());
        order.push((rank, !kind.signal(), height, pins, i));
    }
    order.sort();
    let mut order: Vec<usize> = order.into_iter().map(|(_, _, _, _, i)| i).collect();
    let first_order = order.clone();
    // A label left with no place is tried again first: a neighbour's stub
    // may have taken the place it needs. The fewest left so wins.
    // The fewest (stuck, demoted) so far, and what made them.
    type Best = ((usize, usize), Field, Labels, Routes, Vec<usize>);
    let mut best: Option<Best> = None;
    let mut tried: BTreeSet<usize> = BTreeSet::new();
    loop {
        let mut f = field.clone();
        let mut l = labels.clone();
        let mut r = routes.clone();
        let (stuck, demoted) = place_labels_in(
            block,
            st,
            &mut f,
            want,
            carriers,
            &order,
            &BTreeSet::new(),
            prev_labels,
            prev_runs,
            sid,
            &mut l,
            &mut r,
        );
        // Labels with no place first, then off-sheet connectors drawn as
        // net labels for want of room: the fewest of both wins.
        let count = (stuck.len(), demoted.len());
        if best.as_ref().is_none_or(|b| count < b.0) {
            best = Some((count, f, l, r, stuck.iter().map(|i| want[*i].1).collect()));
        }
        let again = stuck
            .into_iter()
            .chain(demoted)
            .find(|i| !tried.contains(i));
        match again {
            Some(i) if count != (0, 0) => {
                tried.insert(i);
                order.retain(|j| *j != i);
                order.insert(0, i);
            }
            _ => break,
        }
    }
    // Labels still with no place: once more with the stuck labels' parts
    // labelled along their pins (a connector turned to point its pins
    // up or down, its labels a step apart; plan 2.G).
    let along: BTreeSet<usize> = best
        .as_ref()
        .map(|b| b.4.iter().copied().collect())
        .unwrap_or_default();
    if !along.is_empty() {
        let mut f = field.clone();
        let mut l = labels.clone();
        let mut r = routes.clone();
        let (stuck, demoted) = place_labels_in(
            block,
            st,
            &mut f,
            want,
            carriers,
            &first_order,
            &along,
            prev_labels,
            prev_runs,
            sid,
            &mut l,
            &mut r,
        );
        let count = (stuck.len(), demoted.len());
        if best.as_ref().is_none_or(|b| count < b.0) {
            best = Some((count, f, l, r, stuck.iter().map(|i| want[*i].1).collect()));
        }
    }
    let (_, f, l, r, parts) = best.expect("tried once");
    *field = f;
    *labels = l;
    *routes = r;
    parts
}

/// The part of `run` from `from` (either of its ends) to `to`, a point on
/// it; `None` when it doesn't start or end at `from` or doesn't pass `to`.
fn cut_at(run: &[Point], from: Point, to: Point) -> Option<Vec<Point>> {
    let mut r = run.to_vec();
    if r.last() == Some(&from) {
        r.reverse();
    }
    if r.first() != Some(&from) {
        return None;
    }
    let mut out = vec![r[0]];
    for w in r.windows(2) {
        let (a, b) = (w[0], w[1]);
        let between = |v: Nm, p: Nm, q: Nm| p.min(q) <= v && v <= p.max(q);
        let on = (a[0] == b[0] && a[0] == to[0] && between(to[1], a[1], b[1]))
            || (a[1] == b[1] && a[1] == to[1] && between(to[0], a[0], b[0]));
        if on {
            if to != a {
                out.push(to);
            }
            return Some(out);
        }
        out.push(b);
    }
    None
}

/// Wires a pin ending at `end` to its net's wires or labels on the sheet,
/// by a run no longer than `reach`; whether it did.
#[allow(clippy::too_many_arguments)]
fn wire_own(
    field: &mut Field,
    routes: &mut Routes,
    labels: &Labels,
    n: usize,
    sid: &str,
    end: Point,
    side: Side,
    reach: Nm,
    kept: Option<(&Vec<Vec<Point>>, &BTreeSet<Point>)>,
    held: &dyn Fn(&[Point]) -> bool,
) -> bool {
    let tree = own_tree(routes, labels, n, sid);
    let again = kept_run(field, &tree, n, end, reach, kept);
    // Nothing within reach as the crow flies: no search (it would cross
    // the sheet to find what it then can't use).
    let near = tree.iter().any(|t| {
        let p = route::point_of(*t);
        (p[0] - end[0]).abs() + (p[1] - end[1]).abs() <= reach
    });
    if again.is_none() && !near {
        return false;
    }
    let run = match again {
        Some(r) => r,
        None => {
            let Some(path) = field.search(n, end, side, &tree) else {
                return false;
            };
            route::run_of(&path)
        }
    };
    if route::length(&run) > reach || !held(&run) || field.mark(n, &run).is_err() {
        return false;
    }
    routes.entry(n).or_default().push(run);
    true
}

/// The previous layout's wire from this pin (at `end`) to the net's own
/// (`tree`: its wires and labels now; or another of its pins or labels in
/// `kept`), while it still reaches it and nothing is in its way.
fn kept_run(
    field: &Field,
    tree: &BTreeSet<route::Node>,
    n: usize,
    end: Point,
    reach: Nm,
    kept: Option<(&Vec<Vec<Point>>, &BTreeSet<Point>)>,
) -> Option<Vec<Point>> {
    kept.and_then(|(runs, pins)| {
        runs.iter().find_map(|r| {
            let mut r = r.clone();
            if r.last() == Some(&end) {
                r.reverse();
            }
            let to = *r.last()?;
            (r.len() >= 2
                && r[0] == end
                && (tree.contains(&route::node_of(to)) || (to != end && pins.contains(&to)))
                && route::length(&r) <= reach
                && field.can_mark(n, &r))
            .then_some(r)
        })
    })
}

/// The net's own on this sheet: its wires and its labels' places.
fn own_tree(routes: &Routes, labels: &Labels, n: usize, sid: &str) -> BTreeSet<route::Node> {
    let mut tree: BTreeSet<route::Node> = routes
        .get(&n)
        .into_iter()
        .flatten()
        .flat_map(|r| route::expand(r))
        .collect();
    tree.extend(
        labels
            .get(&n)
            .into_iter()
            .flatten()
            .filter(|l| l.sheet == sid)
            .map(|l| route::node_of(l.at)),
    );
    tree
}

type Routes = BTreeMap<usize, Vec<Vec<Point>>>;
type Labels = BTreeMap<usize, Vec<Label>>;

/// Whether a labelled net's wire `run` on sheet `sid` may be drawn: short
/// (within `LOCAL_REACH`), or inside one region (its parts' extents grown
/// by a frame's margin, which the region's frame holds once drawn), or on
/// a sheet with no regions. Design 348: a wire between regions only where
/// it is short, else labels; the RP2040's `qspi_ss` ran 155 mm from one
/// region to another, crossing two wires (#718, plan 2.G).
fn wire_held(st: &State, sid: &str, run: &[Point]) -> bool {
    if route::length(run) <= LOCAL_REACH {
        return true;
    }
    let Some(span) = run
        .windows(2)
        .map(|w| Rect::new(w[0], w[1]))
        .reduce(|a, b| a.union(b))
    else {
        return true;
    };
    let mut any = false;
    for (_, r) in st.regions.iter().filter(|(_, r)| r.sheet == sid) {
        let held = r
            .parts
            .iter()
            .filter_map(|k| st.block.part(k))
            .filter(|p| st.place[*p].is_some_and(|x| st.sheets[x.0].0 == sid))
            .filter_map(|p| st.shape(p))
            .map(|sh| sh.extent)
            .reduce(|a, b| a.union(b));
        let Some(held) = held else { continue };
        any = true;
        if held.grow(crate::region::FRAME).contains(&span) {
            return true;
        }
    }
    !any
}

/// [`place_labels`] in one order; the labels left with no place.
#[allow(clippy::too_many_arguments)]
fn place_labels_in(
    block: &Block,
    st: &State,
    field: &mut Field,
    want: &[(usize, usize, usize, LabelKind)],
    carriers: &BTreeMap<(usize, usize), (usize, usize)>,
    order: &[usize],
    along: &BTreeSet<usize>,
    prev_labels: &dyn Fn(usize) -> Vec<Label>,
    prev_runs: &BTreeMap<usize, Vec<Vec<Point>>>,
    sid: &str,
    labels: &mut BTreeMap<usize, Vec<Label>>,
    routes: &mut Routes,
) -> (Vec<usize>, Vec<usize>) {
    let mut stuck = Vec::new();
    let mut demoted = Vec::new();
    let held = |run: &[Point]| wire_held(st, sid, run);
    // A labelled net's pin the previous layout wired to another of its
    // pins (with no label of its own) keeps that wire while it holds,
    // before any label is placed, so no new label takes its way.
    let mut done: BTreeSet<usize> = BTreeSet::new();
    for &i in order {
        let (n, p, k, kind) = want[i];
        let on = format!("{}/{}", block.parts[p].key, block.parts[p].pins[k].number);
        // Only a net the previous layout labelled too: one it wired whole
        // and labels now is drawn anew.
        let had = prev_labels(n);
        if kind != LabelKind::Net || had.is_empty() || had.iter().any(|l| l.on == on) {
            continue;
        }
        let end = st.shape(p).expect("placed").pins[k].end;
        let mut pin_ends: BTreeSet<Point> = block
            .parts
            .iter()
            .enumerate()
            .flat_map(|(q, part)| {
                part.pins
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.net == Some(n))
                    .filter_map(move |(j, _)| st.shape(q).map(|sh| sh.pins[j].end))
            })
            .collect();
        // Or where the previous layout put another pin's label.
        pin_ends.extend(had.iter().map(|l| l.at));
        let tree = own_tree(routes, labels, n, sid);
        if let Some(run) = kept_run(
            field,
            &tree,
            n,
            end,
            Nm::MAX,
            prev_runs.get(&n).map(|r| (r, &pin_ends)),
        ) && wire_held(st, sid, &run)
            && field.mark(n, &run).is_ok()
        {
            routes.entry(n).or_default().push(run);
            done.insert(i);
        }
    }
    // Local groups: before any new label, a symbol's pin on a labelled
    // signal net within LOCAL_REACH of another of the net's symbol pins
    // still to take its label is wired to that pin, which carries the
    // group's label. Only pins that take a label are targets, so every
    // chain of such wires ends at one. Only on a sheet the previous layout
    // drew nothing of: over one, the wires it drew are kept above.
    if prev_runs.is_empty() {
        for &i in order {
            if done.contains(&i) {
                continue;
            }
            let (n, p, k, kind) = want[i];
            if kind != LabelKind::Net || !matches!(block.parts[p].kind, PartKind::Symbol { .. }) {
                continue;
            }
            let pin = st.shape(p).expect("placed").pins[k];
            // A pin of a net leaving its sheet joins its region's carrier
            // alone, however far inside the region (plan 2.C).
            let carrier = carriers.get(&(p, k)).copied();
            // Each target's label as it would stand straight out of its pin.
            let mut boxes: BTreeMap<route::Node, Rect> = BTreeMap::new();
            let tree: BTreeSet<route::Node> = order
                .iter()
                .filter(|j| **j != i && !done.contains(*j))
                .filter_map(|&j| {
                    let (m, q, l, kind2) = want[j];
                    if m != n || !kind2.signal() || q == p {
                        return None;
                    }
                    if carrier.is_some_and(|c| c != (q, l)) {
                        return None;
                    }
                    if !matches!(block.parts[q].kind, PartKind::Symbol { .. }) {
                        return None;
                    }
                    let at = st.shape(q).expect("placed").pins[l];
                    let e = at.end;
                    let near = carrier.is_some()
                        || (e[0] - pin.end[0]).abs() + (e[1] - pin.end[1]).abs() <= LOCAL_REACH;
                    near.then(|| {
                        let b = label::label_box_with(
                            kind2,
                            &block.label_text(q, l),
                            label::intent_on(&block.nets[m], kind2),
                            e,
                            at.side,
                        );
                        boxes.insert(route::node_of(e), b);
                        route::node_of(e)
                    })
                })
                .collect();
            if tree.is_empty() {
                continue;
            }
            let Some(path) = field.search(n, pin.end, pin.side, &tree) else {
                continue;
            };
            let run = route::run_of(&path);
            // Plan 2.A: a wire arriving through the label its target pin
            // is to carry would leave that label nowhere to go but over
            // it (a hard rule broken on every try); the pins take labels.
            let through_label = path.last().and_then(|t| boxes.get(t)).is_some_and(|b| {
                let inner = b.grow(-1);
                run.windows(2)
                    .any(|w| Rect::new(w[0], w[1]).overlaps(&inner))
            });
            let short = carrier.is_some() || route::length(&run) <= LOCAL_REACH;
            if !through_label && short && field.mark(n, &run).is_ok() {
                routes.entry(n).or_default().push(run);
                done.insert(i);
            }
        }
    }
    for &i in order {
        if done.contains(&i) {
            continue;
        }
        let (n, p, k, kind) = want[i];
        let part = &block.parts[p];
        let on = format!("{}/{}", part.key, part.pins[k].number);
        let pin = st.shape(p).expect("placed").pins[k];
        // Where the net's pins end on this sheet, and where the previous
        // layout put its other pins' labels (kept there while they fit).
        let mut net_pins: BTreeSet<Point> = block
            .parts
            .iter()
            .enumerate()
            .flat_map(|(q, part)| {
                part.pins
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.net == Some(n))
                    .filter_map(move |(j, _)| st.shape(q).map(|sh| sh.pins[j].end))
            })
            .collect();
        net_pins.extend(
            prev_labels(n)
                .into_iter()
                .filter(|l| l.on != on)
                .map(|l| l.at),
        );
        let kept = prev_runs.get(&n).map(|r| (r, &net_pins));
        let name = &block.label_text(p, k);
        let fits_as = |field: &Field, kind: LabelKind, stub: &[Point], at: Point, side: Side| {
            let intent = label::intent_on(&block.nets[n], kind);
            if !field.clear_label(label::label_box_with(kind, name, intent, at, side)) {
                return false;
            }
            stub.is_empty() || field.can_mark(n, stub)
        };
        let fits = |field: &Field, stub: &[Point], at: Point, side: Side| -> bool {
            fits_as(field, kind, stub, at, side)
        };
        // An off-sheet connector with no room for its longer outline is
        // drawn as a net label where one fits (plan 2.C): kept labels
        // keep their places, and the lint says what the sheet lacks.
        let mut kind = kind;
        // The previous label, its stub one of the net's previous runs.
        let before = prev_labels(n)
            .into_iter()
            .find(|l| l.on == on && was_kind(l.kind, kind));
        let mut chosen: Option<(Vec<Point>, Point, Side)> = None;
        if let Some(l) = before {
            let stub = if l.at == pin.end {
                Some(Vec::new())
            } else {
                // A run from the pin to the label, or one from the pin on
                // through it (a bypass capacitor's, the capacitor gone) cut
                // at the label.
                prev_runs
                    .get(&n)
                    .and_then(|runs| runs.iter().find_map(|r| cut_at(r, pin.end, l.at)))
            };
            if let Some(stub) = stub {
                if fits(field, &stub, l.at, l.side) {
                    chosen = Some((stub, l.at, l.side));
                } else if kind == LabelKind::OffSheet
                    && l.kind == LabelKind::Net
                    && fits_as(field, LabelKind::Net, &stub, l.at, l.side)
                {
                    kind = LabelKind::Net;
                    demoted.push(i);
                    chosen = Some((stub, l.at, l.side));
                }
            }
        }
        let mut cands = label::candidates(kind, pin.end, pin.side);
        // A part whose labels found no room otherwise: its signal labels
        // along their pins first, side by side as its pins stand.
        if along.contains(&p) && kind.signal() {
            let mut first = label::sideways(pin.end, pin.side);
            first.extend(cands);
            cands = first;
        }
        let detours = label::detours(kind, pin.end, pin.side);
        let last = label::sideways(pin.end, pin.side);
        // A rail's pin beside another of its rail's (a part's row of
        // supply pins) joins it by a short wire first: one port, a comb.
        let mut wired = chosen.is_none()
            && !kind.signal()
            && wire_own(
                field, routes, labels, n, sid, pin.end, pin.side, NEAR_RAIL, kept, &held,
            );
        // A pin on a labelled signal net beside the net's own (another
        // pin's wire or label) joins it by a short wire: the net is
        // labelled for its far leg, its near pins read as wired.
        if chosen.is_none()
            && !wired
            && kind == LabelKind::Net
            && matches!(part.kind, crate::input::PartKind::Symbol { .. })
        {
            wired = wire_own(
                field,
                routes,
                labels,
                n,
                sid,
                pin.end,
                pin.side,
                LOCAL_REACH,
                kept,
                &held,
            );
        }
        let (stub, at, side) = chosen
            .or_else(|| {
                if wired {
                    return None;
                }
                cands
                    .iter()
                    .find(|(s, a, side)| fits(field, s, *a, *side))
                    .cloned()
            })
            .or_else(|| {
                // No room for a label: a wire to the net's own, if any; a
                // rail's only nearby, since further off a port of its own
                // reads better than a long wire.
                if !wired {
                    let reach = if kind.signal() { Nm::MAX } else { RAIL_REACH };
                    wired = wire_own(
                        field, routes, labels, n, sid, pin.end, pin.side, reach, kept, &held,
                    );
                }
                None
            })
            .or_else(|| {
                if wired {
                    return None;
                }
                detours
                    .iter()
                    .chain(&last)
                    .find(|(s, a, side)| fits(field, s, *a, *side))
                    .cloned()
            })
            .or_else(|| {
                if wired || kind != LabelKind::OffSheet {
                    return None;
                }
                let c = cands
                    .iter()
                    .chain(&detours)
                    .chain(&last)
                    .find(|(s, a, side)| fits_as(field, LabelKind::Net, s, *a, *side))
                    .cloned();
                if c.is_some() {
                    kind = LabelKind::Net;
                    demoted.push(i);
                }
                c
            })
            .unwrap_or_else(|| {
                if !wired {
                    stuck.push(i);
                }
                last[0].clone()
            });
        if wired {
            continue;
        }
        if !stub.is_empty() && field.mark(n, &stub).is_ok() {
            routes.entry(n).or_default().push(stub);
        }
        let intent = label::intent_on(&block.nets[n], kind);
        field.block_label(label::label_box_with(kind, name, intent, at, side));
        labels.entry(n).or_default().push(Label {
            text: name.clone(),
            sheet: sid.to_owned(),
            at,
            side,
            kind,
            on,
        });
    }
    (stuck, demoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::seeded;

    fn lay(b: &Block) -> Outcome {
        lay_out(b).unwrap()
    }

    /// A `beside` that can't hold records what stood where it wanted the
    /// part: a part, by key, or that its sheet had no room.
    #[test]
    fn a_blocked_instruction_says_what_blocks_it() {
        let block = seeded(13, 3);
        let first = lay(&block);
        let keys: Vec<String> = block
            .parts
            .iter()
            .filter(|p| p.key.starts_with('r'))
            .map(|p| p.key.clone())
            .take(6)
            .collect();
        let mut b = block.clone();
        b.previous = Some(first.layout.clone());
        // Five parts all asked to the left of one: some can't fit there.
        for (i, k) in keys[1..].iter().enumerate() {
            b.instructions.insert(
                format!("i{}", i + 1),
                Instruction::Beside {
                    parts: vec![k.clone()],
                    of: keys[0].clone(),
                    side: Side::Left,
                },
            );
        }
        let out = lay(&b);
        for (ins, k) in b.instructions.values().zip(&keys[1..]) {
            if crate::instr::judge(&b, &out.layout, ins).is_err() {
                let why = out.blocked.get(k).expect("a blocker recorded");
                assert!(!why.is_empty());
            }
        }
    }

    /// What a released instruction moved goes back where the layout before
    /// it had it, where that place is still free.
    #[test]
    fn a_released_part_goes_back_where_it_was() {
        let block = seeded(11, 2);
        let first = lay(&block);
        let key = block
            .parts
            .iter()
            .find(|p| p.key.starts_with('r'))
            .expect("a resistor")
            .key
            .clone();
        let mut b = block.clone();
        b.previous = Some(first.layout.clone());
        b.instructions.insert(
            "i1".into(),
            Instruction::Edge {
                parts: vec![key.clone()],
                side: Side::Left,
            },
        );
        let moved = lay(&b);
        assert_ne!(moved.layout.placed[&key].at, first.layout.placed[&key].at);
        let mut back = block.clone();
        back.previous = Some(moved.layout.clone());
        back.relayout = Relayout::Parts(vec![key.clone()]);
        back.restore = [(key.clone(), first.layout.placed[&key].clone())].into();
        let out = lay(&back);
        assert_eq!(
            out.layout.placed[&key], first.layout.placed[&key],
            "restored"
        );
        assert_eq!(crate::rules::check(&back, &out.layout), []);
    }

    /// `orient UA 180` (and `0 mirror`) keeps UA's op-amp stage: the stage is
    /// drawn in shape, feedback above the amplifier; at 90 the stage is
    /// dropped with a note.
    #[test]
    fn a_turned_amplifier_keeps_its_stage() {
        let block = crate::testing::with_stage(seeded(5, 2));
        for (rotation, mirror) in [(180, false), (0, true), (180, true)] {
            let mut b = block.clone();
            b.instructions.insert(
                "i1".into(),
                Instruction::Orient {
                    parts: vec!["ua".into()],
                    rotation,
                    mirror,
                },
            );
            let out = lay(&b);
            assert_eq!(
                crate::rules::check(&b, &out.layout),
                [],
                "{rotation} {mirror}"
            );
            let x = &out.layout.placed["ua"];
            assert_eq!((x.rotation, x.mirror), (rotation, mirror));
            let (found, _) = crate::lint::lint(&b, &out);
            assert!(
                !found
                    .iter()
                    .any(|f| f.code == "sch_topology" && f.refs.iter().any(|r| r == "ua")),
                "{rotation} {mirror}: {:#?}",
                found
                    .iter()
                    .filter(|f| f.code == "sch_topology")
                    .collect::<Vec<_>>()
            );
            assert!(
                out.layout.placed["rf"].at[1] > x.at[1],
                "{rotation} {mirror}: feedback above the amplifier"
            );
        }
        let mut b = block.clone();
        b.instructions.insert(
            "i1".into(),
            Instruction::Orient {
                parts: vec!["ua".into()],
                rotation: 90,
                mirror: false,
            },
        );
        let out = lay(&b);
        assert!(out.notes.iter().any(|n| n.code == "orient_drops_stage"));
    }

    /// `near port:EN U1.EN` stands the flag on EN's row six steps out, even
    /// where the net has other pins (phase 1 does it only for a sole pin).
    #[test]
    fn a_port_near_a_pin_stands_on_its_row() {
        use crate::input::{Flow, Net, PinKind};
        use crate::testing::{ic, net, port, resistor};
        let nets: Vec<Net> = vec![
            net("rail:v", "5V", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("en", "EN", NetKind::Signal),
        ];
        let block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "REG",
                    &[
                        ("VIN", PinKind::PowerIn, Some(0)),
                        ("EN", PinKind::Input, Some(2)),
                        ("X", PinKind::Passive, None),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
                ic(
                    "u2",
                    "U2",
                    "MCU",
                    &[("A", PinKind::Passive, None)],
                    &[
                        ("EN_OUT", PinKind::Output, Some(2)),
                        ("B", PinKind::Passive, None),
                        ("C", PinKind::Passive, None),
                    ],
                ),
                resistor("r1", "R1", "100k", [Some(2), Some(1)]),
                port("port:en", "EN", Flow::In, Some(2)),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let mut b = block.clone();
        b.instructions.insert(
            "i1".into(),
            Instruction::Near {
                parts: vec!["port:en".into()],
                to: "u1/2".into(),
            },
        );
        let out = lay(&b);
        assert_eq!(crate::rules::check(&b, &out.layout), []);
        let shape = |k: &str| {
            let p = b.part(k).unwrap();
            let x = &out.layout.placed[k];
            Shape::of(&b.parts[p], Place::new(x.at, x.rotation, x.mirror))
        };
        let pin = shape("u1").pins[1];
        let flag = shape("port:en").pins[0];
        assert_eq!(flag.end[1], pin.end[1], "on EN's row");
        assert!(
            (flag.end[0] - pin.end[0]).abs() <= 8 * GRID,
            "a few steps out"
        );
    }

    /// `near C5 R1 C6 U1.COMP` keeps the buck's compensation network together
    /// off COMP: one rigid cluster joined to U1's, so a move of U1 moves it
    /// whole, every member within reach of the pin.
    #[test]
    fn a_network_near_a_pin_moves_as_one() {
        use crate::input::{Net, PinKind};
        use crate::testing::{capacitor, ic, net, resistor};
        let nets: Vec<Net> = vec![
            net("rail:v", "5V", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("comp", "COMP", NetKind::Signal),
            net("mid", "MID", NetKind::Signal),
        ];
        let block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "REG",
                    &[
                        ("VIN", PinKind::PowerIn, Some(0)),
                        ("COMP", PinKind::Passive, Some(2)),
                        ("X", PinKind::Passive, None),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
                capacitor("c5", "C5", "1.5n", [Some(2), Some(3)]),
                resistor("r1", "R1", "47k", [Some(3), Some(1)]),
                capacitor("c6", "C6", "27p", [Some(2), Some(1)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let mut b = block.clone();
        b.instructions.insert(
            "i1".into(),
            Instruction::Near {
                parts: vec!["c5".into(), "r1".into(), "c6".into()],
                to: "u1/2".into(),
            },
        );
        let first = lay(&b);
        assert_eq!(crate::rules::check(&b, &first.layout), []);
        let j = crate::instr::judge(&b, &first.layout, &b.instructions["i1"]);
        assert!(j.is_ok(), "{j:?}");
        // U1 moved down by a move: the network comes with it, unchanged
        // relative to U1.
        let mut moved = b.clone();
        moved.previous = Some(first.layout.clone());
        moved.moves.push(Instruction::Edge {
            parts: vec!["u1".into()],
            side: Side::Down,
        });
        let out = lay(&moved);
        let rel = |l: &Layout, k: &str| {
            let (a, u) = (l.placed[k].at, l.placed["u1"].at);
            [a[0] - u[0], a[1] - u[1]]
        };
        for k in ["c5", "r1", "c6"] {
            assert_eq!(
                rel(&out.layout, k),
                rel(&first.layout, k),
                "{k} moved with U1"
            );
        }
    }

    /// `text X refdes left` puts the designator left of X's body and leaves
    /// its value where phase 1 puts it; `both below` moves the whole block;
    /// laid out again the text stays.
    #[test]
    fn text_goes_on_the_side_asked() {
        use crate::layout::TextField;
        let block = seeded(7, 1);
        let first = lay(&block);
        let key = block
            .parts
            .iter()
            .find(|p| p.key.starts_with('r') && !p.value.is_empty())
            .expect("a resistor")
            .key
            .clone();
        for (field, side) in [
            (TextField::Refdes, Side::Left),
            (TextField::Both, Side::Down),
        ] {
            let mut b = block.clone();
            b.previous = Some(first.layout.clone());
            b.instructions.insert(
                "i1".into(),
                Instruction::Text {
                    parts: vec![key.clone()],
                    field,
                    side,
                },
            );
            let out = lay(&b);
            assert_eq!(crate::rules::check(&b, &out.layout), []);
            let ins = &b.instructions["i1"];
            assert!(
                crate::instr::achieved(&b, &out.layout, ins),
                "{field:?} {side:?}"
            );
            let mut again = b.clone();
            again.previous = Some(out.layout.clone());
            assert_eq!(lay(&again).layout, out.layout, "kept");
        }
    }

    /// A one-symbol network pulling four of a dense part's rows stands beside
    /// that side, its signal pins facing the part, and every hard rule holds.
    #[test]
    fn a_one_symbol_network_stands_beside_the_pins_it_serves() {
        let mut block = crate::testing::dense_block(false);
        // R1 to R4 (rows S0 to S3) replaced by one 4-element network to 3V3.
        block
            .parts
            .retain(|p| !["r1", "r2", "r3", "r4"].contains(&p.key.as_str()));
        block.parts.push(crate::testing::network(
            "rn1",
            "RN1",
            "4x10k",
            &[
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(0),
                Some(0),
                Some(0),
                Some(0),
            ],
        ));
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let rn = &out.layout.placed["rn1"];
        let u1 = &out.layout.placed["u1"];
        assert!(rn.at[0] > u1.at[0], "on U1's right, the side it serves");
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found
                .iter()
                .any(|f| f.code == "sch_floating" && f.refs.iter().any(|r| r == "rn1")),
            "{found:#?}"
        );
    }

    /// A bypass capacitor on a dense part's banked side stands past the bank
    /// and is wired to its pin; nothing crosses its wire.
    #[test]
    fn decoupling_stands_past_a_bank() {
        use crate::testing::capacitor;
        let mut block = crate::testing::dense_block(false);
        // A supply pin on the banked side, decoupled: the right side's last
        // row (S11) joins the supply instead of the connector.
        let k = block.parts[0]
            .pins
            .iter()
            .position(|p| p.name == "S11")
            .unwrap();
        block.parts[0].pins[k].net = Some(0);
        block.parts[0].pins[k].kind = crate::input::PinKind::PowerIn;
        // VDD (on the left) off the supply, so S11 is the pin decoupled.
        let vdd = block.parts[0]
            .pins
            .iter()
            .position(|p| p.name == "VDD")
            .unwrap();
        block.parts[0].pins[vdd].net = None;
        block
            .parts
            .push(capacitor("c9", "C9", "100n", [Some(0), Some(1)]));
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let (found, _) = crate::lint::lint(&block, &out);
        for code in ["sch_bypass_apart", "sch_crossings"] {
            assert!(
                !found
                    .iter()
                    .any(|f| f.code == code && f.refs.iter().any(|r| r == "c9")),
                "{code}: {found:#?}"
            );
        }
        let bank_far = (1..=10)
            .map(|i| {
                let x = &out.layout.placed[&format!("r{i}")];
                x.at[0]
            })
            .max()
            .unwrap();
        assert!(out.layout.placed["c9"].at[0] > bank_far, "past the bank");
    }

    /// A bank member's designator and value stand on its row, one line, past
    /// its rail's column: rows two steps apart leave no room for two lines.
    #[test]
    fn a_bank_members_text_is_one_line_on_its_row() {
        let block = crate::testing::dense_block(false);
        let out = lay(&block);
        for i in 1..=10 {
            let key = format!("r{i}");
            let t = &out.layout.text[&key];
            let x = &out.layout.placed[&key];
            let p = block.part(&key).unwrap();
            let s = Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror));
            assert_eq!(t["refdes"][1], t["value"][1], "{key}: one line");
            let row = s.pins[0].end[1];
            assert!(
                (t["refdes"][1] - row).abs() <= GRID,
                "{key}: on its row ({} against {row})",
                t["refdes"][1]
            );
        }
        assert_eq!(crate::rules::check(&block, &out.layout), []);
    }

    /// A bank's row on a port net carries a net label named as the port,
    /// before its pull; the port's flag stands apart from the row (with the
    /// block's ports), not on it.
    #[test]
    fn a_banked_port_net_is_labelled_on_its_row() {
        let block = crate::testing::dense_block(true);
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let u1 = {
            let x = &out.layout.placed["u1"];
            Shape::of(&block.parts[0], Place::new(x.at, x.rotation, x.mirror))
        };
        for (port, pin) in [("port:a", "S0"), ("port:b", "S6")] {
            let k = block.parts[0]
                .pins
                .iter()
                .position(|p| p.name == pin)
                .unwrap();
            let row = u1.pins[k].end[1];
            let net = block.parts[0].pins[k].net.unwrap();
            let labels = &out.layout.labels[&block.nets[net].key];
            assert!(
                labels
                    .iter()
                    .any(|l| l.kind == LabelKind::Net && l.at[1] == row && l.text == pin),
                "{pin}: a net label on its row: {labels:#?}"
            );
            let flag = &out.layout.placed[port];
            let fp = block.part(port).unwrap();
            let fs = Shape::of(
                &block.parts[fp],
                Place::new(flag.at, flag.rotation, flag.mirror),
            );
            assert_ne!(fs.pins[0].end[1], row, "{port}'s flag isn't on the row");
        }
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_floating"),
            "{found:#?}"
        );
    }

    /// A bank row whose net continues on another sheet keeps its net
    /// label on the row and takes no off-sheet connector; the lint asks
    /// none of it (`sch_off_sheet_local` exempts what the engine does).
    #[test]
    fn a_bank_row_leaving_its_sheet_takes_no_connector_and_no_finding() {
        let mut block = crate::testing::dense_block(false);
        // S0 (net 2, R1's row) also reaches J9 on another sheet.
        block
            .parts
            .push(crate::testing::connector("j9", "J9", &[Some(2), None]));
        block.instructions.insert(
            "i1".into(),
            Instruction::Sheet {
                parts: vec!["j9".into()],
                sheet: "Away".into(),
            },
        );
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let sheet = &out.layout.placed["u1"].sheet;
        assert_ne!(&out.layout.placed["j9"].sheet, sheet);
        let labels = &out.layout.labels["s0"];
        assert!(
            labels
                .iter()
                .filter(|l| &l.sheet == sheet)
                .all(|l| l.kind == LabelKind::Net),
            "{labels:#?}"
        );
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found
                .iter()
                .any(|f| f.code == "sch_off_sheet_local" && f.refs.iter().any(|r| r == "s0")),
            "{found:#?}"
        );
    }

    /// #429: a dense part's pulls stand as a bank, each across the page on
    /// its pin's row (its signal pin's end on the row), every hard rule
    /// holding, none of them far from the part or floating, and each rail's
    /// far ends in one column.
    #[test]
    fn a_dense_parts_pulls_stand_on_their_rows() {
        let block = crate::testing::dense_block(false);
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let shape = |key: &str| {
            let p = block.part(key).expect("part");
            let x = &out.layout.placed[key];
            Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror))
        };
        let u1 = shape("u1");
        let mut columns: BTreeMap<usize, BTreeSet<Nm>> = BTreeMap::new();
        for i in 0..10 {
            let key = format!("r{}", i + 1);
            let r = shape(&key);
            let row = u1.pins[block.parts[0]
                .pins
                .iter()
                .position(|p| p.name == format!("S{i}"))
                .expect("pin")]
            .end[1];
            let p = block.part(&key).expect("part");
            let sig = block.parts[p]
                .pins
                .iter()
                .position(|x| x.net == Some(2 + i))
                .expect("signal pin");
            let rail = 1 - sig;
            assert_eq!(r.pins[sig].end[1], row, "{key} on its pin's row");
            assert_eq!(r.pins[rail].end[1], row, "{key} across the page");
            assert!(
                r.pins[sig].end[0] < r.pins[rail].end[0],
                "{key}: its signal end towards U1"
            );
            columns
                .entry(block.parts[p].pins[rail].net.expect("rail"))
                .or_default()
                .insert(r.pins[rail].end[0]);
        }
        for (rail, xs) in &columns {
            assert_eq!(xs.len(), 1, "rail {rail}'s far ends in one column: {xs:?}");
        }
        let (found, _) = crate::lint::lint(&block, &out);
        // The banked rows' nets (s0 to s9) and parts: J1's two rows run
        // round U1 to its other side, as they do without banks.
        let banked = |f: &crate::lint::Finding| {
            f.refs.iter().any(|r| {
                r == "c1" || (0..10).any(|i| *r == format!("s{i}") || *r == format!("r{}", i + 1))
            })
        };
        for code in ["sch_idiom_missed", "sch_floating", "sch_jog"] {
            assert!(
                !found.iter().any(|f| f.code == code && banked(f)),
                "{code}: {:#?}",
                found
                    .iter()
                    .filter(|f| f.code == code && banked(f))
                    .collect::<Vec<_>>()
            );
        }
        // The far rail's bar passes the near rail's rows only: three
        // crossings at most for this interleaving.
        let crossings: u64 = found
            .iter()
            .filter(|f| f.code == "sch_crossings")
            .map(|f| f.details["crossings"].as_u64().unwrap_or(1))
            .sum();
        assert!(crossings <= 3, "{crossings} crossings");
    }

    /// #718 (plan 2.G): a net drawn with labels on a sheet is wired
    /// between regions only where the wire is short; every longer run of
    /// it lies inside one region's frame. The loose RC's R9 joined in turn
    /// to each of U1's signal nets (regions far apart), first from nothing
    /// and then over its own layout.
    #[test]
    fn a_labelled_nets_long_wire_stays_inside_a_region() {
        for net in [2, 3, 6, 9] {
            let mut block = crate::testing::regions_block();
            let r9 = block.part("r9").expect("r9");
            block.parts[r9].pins[0].net = Some(net);
            let first = lay(&block);
            let mut again = block.clone();
            again.previous = Some(first.layout.clone());
            for out in [first, lay(&again)] {
                let l = &out.layout;
                assert_eq!(crate::rules::check(&block, l), [], "net {net}");
                for (key, by) in &l.routes {
                    for (sid, runs) in by {
                        let labelled = l
                            .labels
                            .get(key)
                            .is_some_and(|ls| ls.iter().any(|x| &x.sheet == sid));
                        for run in runs.iter().filter(|_| labelled) {
                            if route::length(run) <= LOCAL_REACH {
                                continue;
                            }
                            let span = run
                                .windows(2)
                                .map(|w| Rect::new(w[0], w[1]))
                                .reduce(|a, b| a.union(b))
                                .expect("a run");
                            assert!(
                                l.regions
                                    .values()
                                    .any(|r| &r.sheet == sid && r.frame_on_sheet().contains(&span)),
                                "net {net}: {key} runs {} mm across regions",
                                route::length(run) / 1_000_000
                            );
                        }
                    }
                }
            }
        }
    }

    /// Over a previous layout a new pull-up joins its host's region, the
    /// region grows into free space beside it, and nothing outside it
    /// moves; a region with no room beside it moves whole.
    #[test]
    fn a_new_part_joins_its_hosts_region_and_nothing_else_moves() {
        use crate::testing::resistor;
        let block = crate::testing::regions_block();
        let before = lay(&block);
        let mut grown = block.clone();
        grown
            .parts
            .push(resistor("r2", "R2", "4k7", [Some(0), Some(2)]));
        grown.previous = Some(before.layout.clone());
        let after = lay(&grown);
        assert_eq!(crate::rules::check(&grown, &after.layout), []);
        let u1 = |l: &Layout| {
            l.regions
                .iter()
                .find(|(_, r)| r.parts.contains(&"u1".to_string()))
                .map(|(id, r)| (id.clone(), r.clone()))
                .unwrap()
        };
        let (id_before, r_before) = u1(&before.layout);
        let (id_after, r_after) = u1(&after.layout);
        assert_eq!(id_before, id_after);
        assert!(
            r_after.parts.contains(&"r2".to_string()),
            "R2 joins U1's region"
        );
        assert!(
            r_after
                .frame_on_sheet()
                .contains(&r_before.frame_on_sheet())
                || r_after.at != r_before.at
        );
        for (k, x) in &before.layout.placed {
            if !r_before.parts.contains(k) {
                assert_eq!(&after.layout.placed[k], x, "{k} outside the region moved");
            }
        }
        let rel = |l: &Layout, k: &str| l.relative().placed[k].at;
        for k in &r_before.parts {
            assert_eq!(
                rel(&before.layout, k),
                rel(&after.layout, k),
                "{k} moved inside its region"
            );
        }
    }

    /// An edit that doesn't fit its sheet's paper grows it one size, noted
    /// `sheet_resized`; a pinned paper never changes, and the region moves
    /// to a new sheet instead.
    #[test]
    fn a_full_sheet_grows_one_size_unless_pinned() {
        use crate::input::Paper;
        use crate::layout::{Instruction, Orientation};
        use crate::testing::resistor;
        let mut block = crate::testing::regions_block();
        let a4 = Paper {
            size: "a4".into(),
            orientation: Orientation::Landscape,
            area: [277_000_000, 190_000_000],
        };
        let a3 = Paper {
            size: "a3".into(),
            orientation: Orientation::Landscape,
            area: [400_000_000, 277_000_000],
        };
        block.sheet = a3.area;
        block.papers = vec![a4, a3];
        let before = lay(&block);
        assert_eq!(before.layout.sheets["s1"].size.as_deref(), Some("a4"));
        // Enough new pull-ups on U1's rail that A4 can't hold them.
        let mut grown = block.clone();
        for i in 0..40 {
            grown.parts.push(resistor(
                &format!("rx{i}"),
                &format!("R{}", 100 + i),
                "10k",
                [Some(0), Some(2)],
            ));
        }
        grown.previous = Some(before.layout.clone());
        let after = lay(&grown);
        assert_eq!(crate::rules::check(&grown, &after.layout), []);
        assert_eq!(
            after.layout.sheets["s1"].size, None,
            "grown to the project's A3"
        );
        assert!(after.notes.iter().any(|n| n.code == "sheet_resized"));
        let mut pinned = grown.clone();
        pinned.instructions.insert(
            "i1".into(),
            Instruction::Paper {
                sheet: "s1".into(),
                size: "a4".into(),
                orientation: None,
            },
        );
        let after = lay(&pinned);
        assert_eq!(after.layout.sheets["s1"].size.as_deref(), Some("a4"));
        assert!(after.layout.sheets["s1"].pinned);
        assert!(
            after.layout.sheets.len() > 1,
            "the region moved to a new sheet"
        );
        assert!(after.notes.iter().any(|n| n.code == "sheet_full"));
    }

    /// From nothing a sheet takes the smallest paper of its series that
    /// holds it, its drawing centred; with fit off every sheet keeps the
    /// project's paper.
    #[test]
    fn a_sheet_from_nothing_takes_the_smallest_paper_that_holds_it() {
        use crate::input::Paper;
        use crate::layout::Orientation;
        let mut block = crate::testing::regions_block();
        let a4 = Paper {
            size: "a4".into(),
            orientation: Orientation::Landscape,
            area: [277_000_000, 190_000_000],
        };
        let a3 = Paper {
            size: "a3".into(),
            orientation: Orientation::Landscape,
            area: [400_000_000, 277_000_000],
        };
        block.sheet = a3.area;
        block.papers = vec![a4.clone(), a3];
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let (_, s) = out.layout.sheets.iter().next().unwrap();
        assert_eq!(s.size.as_deref(), Some("a4"));
        // Centred: the drawing's margins left and right within two steps.
        let all = out
            .layout
            .regions
            .values()
            .map(|r| r.frame_on_sheet())
            .reduce(Rect::union)
            .unwrap();
        let left = all.min[0];
        let right = a4.area[0] - all.max[0];
        assert!((left - right).abs() <= 2 * GRID, "{left} {right}");
        let mut off = block.clone();
        off.papers.clear();
        let out = lay(&off);
        assert!(out.layout.sheets.values().all(|s| s.size.is_none()));
    }

    /// From nothing, each region's parts are laid out by flow into a
    /// framed region, and the regions are packed onto the sheet without
    /// overlapping: the layout lists them with their titles and parts, and
    /// every hard rule holds.
    #[test]
    fn a_sheet_from_nothing_is_packed_as_regions() {
        let block = crate::testing::regions_block();
        let out = lay(&block);
        assert!(
            crate::rules::check(&block, &out.layout).is_empty(),
            "{:#?}",
            crate::rules::check(&block, &out.layout)
        );
        let l = &out.layout;
        let titles: Vec<&str> = l.regions.values().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["TPS54331 converter", "U1 LAN8710A", "J1", "F1"]);
        assert_eq!(
            l.regions.keys().collect::<Vec<_>>(),
            ["r1", "r2", "r3", "r4"]
        );
        let frames: Vec<crate::geom::Rect> =
            l.regions.values().map(|r| r.frame_on_sheet()).collect();
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                assert!(!a.overlaps(b), "regions overlap: {a:?} {b:?}");
            }
        }
        for r in l.regions.values() {
            for k in &r.parts {
                let p = block.part(k).unwrap();
                let x = &l.placed[k];
                let shape = Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror));
                assert!(
                    r.frame_on_sheet().contains(&shape.extent),
                    "{k} outside {}",
                    r.title
                );
            }
        }
        assert!(
            !l.regions
                .values()
                .any(|r| r.parts.contains(&"port:a".to_string()))
        );
        // The same layout twice (one thread and eight is layout_props').
        assert_eq!(lay(&block).layout, out.layout);
    }

    /// What hangs off an IC's left pins stands in stairs, the lowest pin's
    /// nearest: no pin's drop crosses another's wire (the buck's EN, SS and
    /// COMP, plan 1.D's metric bars).
    #[test]
    fn hanging_satellites_stand_in_stairs() {
        use crate::input::{Net, PinKind};
        use crate::testing::{capacitor, ic, net, resistor};
        let nets: Vec<Net> = vec![
            net("rail:v", "5V", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("en", "EN", NetKind::Signal),
            net("ss", "SS", NetKind::Signal),
            net("comp", "COMP", NetKind::Signal),
            net("mid", "MID", NetKind::Signal),
            net("out", "OUT", NetKind::Signal),
        ];
        let block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "REG",
                    &[
                        ("VIN", PinKind::PowerIn, Some(0)),
                        ("EN", PinKind::Input, Some(2)),
                        ("SS", PinKind::Passive, Some(3)),
                        ("COMP", PinKind::Passive, Some(4)),
                    ],
                    &[
                        ("OUT", PinKind::Output, Some(6)),
                        ("GND", PinKind::PowerIn, Some(1)),
                    ],
                ),
                resistor("r22", "R22", "10k", [Some(2), Some(1)]),
                capacitor("c4", "C4", "0.1u", [Some(3), Some(1)]),
                capacitor("c5", "C5", "1.5n", [Some(4), Some(5)]),
                resistor("r1", "R1", "47k", [Some(5), Some(1)]),
                capacitor("c6", "C6", "27p", [Some(4), Some(1)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let out = lay(&block);
        assert!(crate::rules::check(&block, &out.layout).is_empty());
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_crossings"),
            "{:#?}",
            found
                .iter()
                .filter(|f| f.code == "sch_crossings")
                .collect::<Vec<_>>()
        );
    }

    /// A lasting `idiom X kind` re-forms the idiom X anchors: a feedback
    /// resistor held away by a move goes back into the stage's shape, its
    /// hold dropped (round 1, F4).
    #[test]
    fn an_idiom_instruction_reforms_its_stage() {
        use crate::layout::{IdiomChoice, IdiomKind};
        let block = crate::testing::with_stage(seeded(5, 2));
        let first = lay(&block);
        let stage_fine = |b: &Block, o: &Outcome| {
            !crate::lint::lint(b, o)
                .0
                .iter()
                .any(|f| f.code == "sch_topology" && f.refs.iter().any(|r| r == "ua"))
        };
        assert!(stage_fine(&block, &first), "drawn in shape from nothing");
        // RF moved away and held there.
        let mut moved = block.clone();
        moved.previous = Some(first.layout.clone());
        moved.moves.push(Instruction::Edge {
            parts: vec!["rf".into()],
            side: Side::Down,
        });
        let away = lay(&moved);
        assert!(away.layout.placed["rf"].held);
        assert!(!stage_fine(&block, &away), "out of shape once moved");
        let mut b = block.clone();
        b.previous = Some(away.layout.clone());
        b.instructions.insert(
            "i1".into(),
            Instruction::Idiom {
                parts: vec!["ua".into()],
                kind: IdiomChoice::Kind(IdiomKind::OpampStage),
            },
        );
        let out = lay(&b);
        assert_eq!(crate::rules::check(&b, &out.layout), []);
        assert!(stage_fine(&b, &out), "re-formed");
        assert!(!out.layout.placed["rf"].held, "its hold dropped");
    }

    /// `align` takes pins: a connector's pin and an IC's pin on one row,
    /// the first (the lead) staying, the other moving across the row only
    /// (round 1, F3).
    #[test]
    fn align_lines_pins_up() {
        use crate::input::{Net, PinKind};
        use crate::layout::Axis;
        use crate::testing::{connector, ic, net};
        let nets: Vec<Net> = vec![
            net("rail:v", "3V3", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("scl", "SCL", NetKind::Signal),
            net("sda", "SDA", NetKind::Signal),
            net("int", "INT", NetKind::Signal),
        ];
        let mut block = Block {
            parts: vec![
                connector("j1", "J1", &[Some(3), Some(4), Some(2)]),
                ic(
                    "u2",
                    "U2",
                    "SENSOR",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("SCL", PinKind::Input, Some(2)),
                        ("SDA", PinKind::Bidirectional, Some(3)),
                        ("INT", PinKind::Output, Some(4)),
                        ("GND", PinKind::PowerIn, Some(1)),
                    ],
                    &[],
                ),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let prev = lay(&block).layout;
        let u2_was = prev.placed["u2"].clone();
        let j1_was = prev.placed["j1"].clone();
        block.previous = Some(prev);
        let align = Instruction::Align {
            parts: vec!["j1/3".into(), "u2/2".into()],
            axis: Axis::Row,
        };
        block.instructions.insert("i1".into(), align.clone());
        let out = lay(&block);
        assert!(crate::rules::check(&block, &out.layout).is_empty());
        let l = &out.layout;
        let end = |key: &str, pin: usize| {
            let p = block.parts.iter().position(|x| x.key == key).unwrap();
            let x = &l.placed[key];
            Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror)).pins[pin].end
        };
        assert_eq!(
            end("j1", 2)[1],
            end("u2", 1)[1],
            "J1.3 and U2.SCL on one row"
        );
        assert_eq!(l.placed["j1"].at, j1_was.at, "the lead stays");
        assert_eq!(
            l.placed["u2"].at[0], u2_was.at[0],
            "U2 moves across the row only"
        );
        assert!(crate::instr::judge(&block, l, &align).is_ok());
    }

    /// `shift` nudges a part by whole grid steps, held there (round 1, F3).
    #[test]
    fn a_shift_nudges_a_part() {
        let block = seeded(3, 3);
        let prev = lay(&block).layout;
        let key = prev
            .placed
            .keys()
            .find(|k| k.starts_with("rp"))
            .unwrap()
            .clone();
        let was = prev.placed[&key].clone();
        let mut b = block.clone();
        b.previous = Some(prev);
        b.moves.push(Instruction::Shift {
            parts: vec![key.clone()],
            side: Side::Up,
            steps: 2,
        });
        let out = lay(&b);
        let now = &out.layout.placed[&key];
        assert_eq!(now.at, [was.at[0], was.at[1] + 2 * GRID]);
        assert!(now.held);
    }

    /// `align` lines parts up on their own sheet, moving each along the
    /// axis only (round 1, F5: parts thrown 150 to 190 mm and onto other
    /// sheets).
    #[test]
    fn align_keeps_sheets_and_moves_along_its_axis() {
        use crate::input::Net;
        use crate::layout::Axis;
        use crate::testing::{net, resistor};
        let nets: Vec<Net> = vec![
            net("a", "A", NetKind::Signal),
            net("b", "B", NetKind::Signal),
            net("c", "C", NetKind::Signal),
            net("d", "D", NetKind::Signal),
            net("e", "E", NetKind::Signal),
            net("f", "F", NetKind::Signal),
        ];
        let mut block = Block {
            parts: vec![
                resistor("r1", "R1", "1k", [Some(0), Some(1)]),
                resistor("r2", "R2", "1k", [Some(2), Some(3)]),
                resistor("r3", "R3", "1k", [Some(4), Some(5)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        block.instructions.insert(
            "i1".into(),
            Instruction::Sheet {
                parts: vec!["r3".into()],
                sheet: "Second".into(),
            },
        );
        let mut prev = lay(&block).layout;
        let r1 = prev.placed["r1"].clone();
        let r3_sheet = prev.placed["r3"].sheet.clone();
        assert_ne!(r1.sheet, r3_sheet);
        // R2 30 steps below R1 and 40 to its right, on R1's sheet.
        let r2 = prev.placed.get_mut("r2").unwrap();
        r2.sheet = r1.sheet.clone();
        r2.at = [r1.at[0] + 40 * GRID, r1.at[1] - 30 * GRID];
        let r2_was = r2.at;
        block.previous = Some(prev);
        let align = Instruction::Align {
            parts: vec!["r1".into(), "r2".into(), "r3".into()],
            axis: Axis::Row,
        };
        block.instructions.insert("i2".into(), align.clone());
        let out = lay(&block);
        assert!(crate::rules::check(&block, &out.layout).is_empty());
        let l = &out.layout;
        assert_eq!(l.placed["r3"].sheet, r3_sheet, "R3 keeps its sheet");
        assert_eq!(l.placed["r2"].at[0], r2_was[0], "R2 moves only in y");
        assert_ne!(l.placed["r2"].at[1], r2_was[1], "R2 moves onto R1's row");
        let why = crate::instr::judge(&block, l, &align).unwrap_err();
        assert!(why.starts_with("R3 is on sheet Second"), "{why}");
    }

    /// A part added over a previous layout goes beside its host, where
    /// cross-cluster placement would put it from nothing, and nothing kept
    /// moves (an agent's added termination lands by the connector).
    #[test]
    fn a_new_part_is_inserted_beside_its_host() {
        use crate::input::{Net, PinKind};
        use crate::testing::{connector, ic, net, resistor};
        let nets: Vec<Net> = vec![
            net("rail:v", "3V3", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("tdp", "TD+", NetKind::Signal),
            net("tdn", "TD-", NetKind::Signal),
            net("rdp", "RD+", NetKind::Signal),
            net("rdn", "RD-", NetKind::Signal),
        ];
        let block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "PHY",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("NC1", PinKind::Passive, None),
                        ("NC2", PinKind::Passive, None),
                        ("NC3", PinKind::Passive, None),
                        ("NC4", PinKind::Passive, None),
                    ],
                    &[
                        ("TXP", PinKind::Output, Some(2)),
                        ("TXN", PinKind::Output, Some(3)),
                        ("RXP", PinKind::Input, Some(4)),
                        ("RXN", PinKind::Input, Some(5)),
                        ("GND", PinKind::PowerIn, Some(1)),
                    ],
                ),
                connector("j1", "J1", &[Some(2), Some(3), Some(4), Some(5)]),
                resistor("r1", "R1", "100", [Some(2), Some(3)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let before = lay(&block).layout;
        let mut b = block.clone();
        b.parts
            .push(resistor("r2", "R2", "100", [Some(4), Some(5)]));
        b.previous = Some(before.clone());
        let out = lay(&b);
        assert!(crate::rules::check(&b, &out.layout).is_empty());
        for (k, x) in &before.placed {
            assert_eq!(out.layout.placed.get(k), Some(x), "{k} moved");
        }
        let end = |key: &str, pin: usize| {
            let p = b.parts.iter().position(|x| x.key == key).unwrap();
            let x = &out.layout.placed[key];
            Shape::of(&b.parts[p], Place::new(x.at, x.rotation, x.mirror)).pins[pin].end
        };
        // Placed by neighbours' middle, R2 stood 26 steps off; beside its
        // host it stands a little further than from nothing, clear of what
        // the previous layout drew.
        let (a, c) = (end("r2", 0), end("j1", 2));
        let d = (a[0] - c[0]).abs() + (a[1] - c[1]).abs();
        assert!(d <= 20 * GRID, "R2 is {} steps from J1", d / GRID);
    }

    /// The front end's balanced outputs (plan 2.G): a port's net meets a
    /// stack's output pin (labelled, a net a layer) and a clamp's middle
    /// pin. The flag stands on the clamp's pin, wired, and that pin takes
    /// no label of its own: the clamp isn't floating.
    #[test]
    fn a_clamp_hosts_its_ports_flag_beside_a_stack() {
        use crate::input::{Flow, PinKind};
        use crate::testing::{net, port};
        let nets = vec![
            net("rail:p", "+5VA", NetKind::Supply),
            net("rail:n", "-5VA", NetKind::Supply),
            net("out_l", "OUT_L", NetKind::Signal),
            net("out_r", "OUT_R", NetKind::Signal),
        ];
        // The stack: a block symbol whose pin is OUT_L on its front layer
        // and OUT_R behind.
        let stack = crate::shape::stack_symbol(
            "amp",
            "Amp",
            &[("1".into(), "OUT".into(), Flow::Out, vec![Some(2), Some(3)])],
        );
        // The clamp: a BAV99, its signal pin 1 (K) on the left.
        let clamp = crate::testing::ic(
            "d1",
            "D1",
            "BAV99",
            &[("K", PinKind::Passive, Some(2))],
            &[
                ("A1", PinKind::Passive, Some(1)),
                ("A2", PinKind::Passive, Some(0)),
            ],
        );
        let block = Block {
            parts: vec![
                stack,
                clamp,
                port("port:l", "OUT_L", Flow::Out, Some(2)),
                port("port:r", "OUT_R", Flow::Out, Some(3)),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_floating"),
            "{:#?}",
            found
                .iter()
                .filter(|f| f.code == "sch_floating")
                .collect::<Vec<_>>()
        );
        // No label of its own on the clamp's signal pin.
        assert!(
            !out.layout
                .labels
                .get("out_l")
                .into_iter()
                .flatten()
                .any(|l| l.on == "d1/1"),
            "{:?}",
            out.layout.labels.get("out_l")
        );
    }

    /// An in-port whose net reaches one part's pin stands beside that pin,
    /// wired, as an out-port beside its driver does (Ethernet's RMII
    /// flags stood on the sheet's edges, every one labelled).
    #[test]
    fn an_in_port_stands_beside_the_pin_it_feeds() {
        use crate::input::{Flow, Net, PinKind};
        use crate::testing::{ic, net, port};
        let nets: Vec<Net> = vec![
            net("rail:v", "3V3", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("txd0", "TXD0", NetKind::Signal),
        ];
        let block = Block {
            parts: vec![
                ic(
                    "u4",
                    "U4",
                    "PHY",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("TXD0", PinKind::Input, Some(2)),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
                port("port:txd0", "TXD0", Flow::In, Some(2)),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let out = lay(&block);
        assert!(crate::rules::check(&block, &out.layout).is_empty());
        assert!(
            !out.layout.labels.contains_key("txd0"),
            "{:?}",
            out.layout.labels
        );
        assert!(out.layout.routes.contains_key("txd0"));
        let end = |key: &str, pin: usize| {
            let p = block.parts.iter().position(|x| x.key == key).unwrap();
            let x = &out.layout.placed[key];
            Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror)).pins[pin].end
        };
        let (a, b) = (end("port:txd0", 0), end("u4", 1));
        let d = (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
        assert!(
            d <= 8 * GRID,
            "the flag is {} steps from U4's TXD0",
            d / GRID
        );
        assert_eq!(a[1], b[1], "a straight wire");
    }

    /// A net too long to wire whole is labelled for its far leg; its pins
    /// beside each other are still joined by a wire, and no part on it
    /// floats.
    #[test]
    fn a_labelled_nets_near_pins_are_wired() {
        use crate::input::{Net, PinKind};
        use crate::testing::{connector, ic, net, resistor};
        let nets: Vec<Net> = vec![
            net("rail:v", "3V3", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("rx", "RX", NetKind::Signal),
        ];
        let mut block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "PHY",
                    &[("VDD", PinKind::PowerIn, Some(0))],
                    &[
                        ("RXP", PinKind::Input, Some(2)),
                        ("GND", PinKind::PowerIn, Some(1)),
                    ],
                ),
                connector("j1", "J1", &[Some(2)]),
                resistor("r1", "R1", "49.9", [Some(0), Some(2)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        // U1 on the far right edge, J1 and R1 flowed on the left: RX runs
        // past the long threshold and takes labels.
        block.instructions.insert(
            "i1".into(),
            Instruction::Edge {
                parts: vec!["u1".into()],
                side: Side::Right,
            },
        );
        let out = lay(&block);
        assert_eq!(crate::rules::check(&block, &out.layout), []);
        // U1, held alone on the far edge, is on a label (the lint counts
        // it floating: nothing else stands near it). R1 and J1 beside each
        // other are wired.
        let (found, _) = crate::lint::lint(&block, &out);
        let floating: Vec<_> = found
            .iter()
            .filter(|f| f.code == "sch_floating" && !f.refs.iter().any(|r| r == "u1"))
            .collect();
        assert!(floating.is_empty(), "{floating:#?}");
        // RX is still labelled (its far leg), and has wires (the near one).
        assert!(out.layout.labels.contains_key("rx"));
        assert!(out.layout.routes.contains_key("rx"));
    }

    /// A part sharing its nets with a connector and an IC far apart
    /// (Ethernet's terminations, work item 4530) stands beside the part it
    /// shares most nets with, wired to it. The terminations are
    /// differential (across TD+ and TD-, RD+ and RD-), so no pull idiom
    /// takes them to the PHY.
    #[test]
    fn a_loose_part_stands_beside_its_host() {
        use crate::input::{Net, PinKind};
        use crate::testing::{connector, ic, net, resistor};
        let nets: Vec<Net> = vec![
            net("rail:v", "3V3", NetKind::Supply),
            net("rail:gnd", "GND", NetKind::Ground),
            net("tdp", "TD+", NetKind::Signal),
            net("tdn", "TD-", NetKind::Signal),
            net("rdp", "RD+", NetKind::Signal),
            net("rdn", "RD-", NetKind::Signal),
        ];
        let block = Block {
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "PHY",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("NC1", PinKind::Passive, None),
                        ("NC2", PinKind::Passive, None),
                        ("NC3", PinKind::Passive, None),
                        ("NC4", PinKind::Passive, None),
                    ],
                    &[
                        ("TXP", PinKind::Output, Some(2)),
                        ("TXN", PinKind::Output, Some(3)),
                        ("RXP", PinKind::Input, Some(4)),
                        ("RXN", PinKind::Input, Some(5)),
                        ("GND", PinKind::PowerIn, Some(1)),
                    ],
                ),
                connector("j1", "J1", &[Some(2), Some(3), Some(4), Some(5)]),
                resistor("r1", "R1", "100", [Some(2), Some(3)]),
                resistor("r2", "R2", "100", [Some(4), Some(5)]),
            ],
            nets,
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let out = lay(&block);
        assert!(crate::rules::check(&block, &out.layout).is_empty());
        let end = |key: &str, pin: usize| {
            let p = block.parts.iter().position(|x| x.key == key).unwrap();
            let x = &out.layout.placed[key];
            Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror)).pins[pin].end
        };
        // Flowed, R1 stood 45 steps from J1. The first termination takes
        // the spot by J1's pin; the second stands under it.
        for (r, k) in [("r1", 0), ("r2", 2)] {
            let (a, b) = (end(r, 0), end("j1", k));
            let d = (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
            assert!(d <= 16 * GRID, "{r} is {} steps from J1", d / GRID);
        }
        let (found, _) = crate::lint::lint(&block, &out);
        assert!(
            !found.iter().any(|f| f.code == "sch_floating"),
            "{:#?}",
            found
                .iter()
                .filter(|f| f.code == "sch_floating")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_sheet_nothing_stands_on_is_dropped() {
        let b = seeded(3, 3);
        let mut prev = lay(&b).layout;
        prev.sheets.insert(
            "s9".into(),
            crate::layout::Sheet::new("Empty".into(), [0, 0]),
        );
        let mut again = b.clone();
        again.previous = Some(prev);
        let out = lay(&again);
        assert!(
            !out.layout.sheets.contains_key("s9"),
            "{:?}",
            out.layout.sheets
        );
        assert!(!out.layout.sheets.is_empty());
    }

    #[test]
    fn a_net_asked_for_as_wires_drops_the_labels_a_previous_layout_kept() {
        use crate::input::{NetKind, PinKind};
        use crate::layout::{Instruction, NetStyle};
        use crate::testing::{ic, net};
        // U1's Q drives U2's A; laid out with Q labelled, then asked for as
        // wires, lasting or as a move: the labels go and Q is wired.
        let block = Block {
            nets: vec![
                net("rail:v", "3V3", NetKind::Supply),
                net("rail:gnd", "GND", NetKind::Ground),
                net("q", "Q", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "x",
                    &[("VDD", PinKind::PowerIn, Some(0))],
                    &[
                        ("GND", PinKind::PowerIn, Some(1)),
                        ("Q", PinKind::Output, Some(2)),
                    ],
                ),
                ic(
                    "u2",
                    "U2",
                    "y",
                    &[
                        ("VDD", PinKind::PowerIn, Some(0)),
                        ("A", PinKind::Input, Some(2)),
                    ],
                    &[("GND", PinKind::PowerIn, Some(1))],
                ),
            ],
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let mut labelled = block.clone();
        labelled.instructions.insert(
            "i1".into(),
            Instruction::Net {
                net: "q".into(),
                style: NetStyle::Label,
            },
        );
        let before = lay(&labelled).layout;
        assert!(before.labels.contains_key("q"), "{:?}", before.labels);
        let wire = Instruction::Net {
            net: "q".into(),
            style: NetStyle::Wire,
        };
        for lasting in [true, false] {
            let mut b = block.clone();
            b.previous = Some(before.clone());
            if lasting {
                b.instructions.insert("i1".into(), wire.clone());
            } else {
                b.moves = vec![wire.clone()];
            }
            let out = lay(&b);
            assert!(
                !out.layout.labels.contains_key("q"),
                "lasting {lasting}: {:?}",
                out.layout.labels
            );
            assert!(out.layout.routes.contains_key("q"), "lasting {lasting}");
            assert!(crate::rules::check(&b, &out.layout).is_empty());
        }
    }

    #[test]
    fn a_stack_labels_each_layers_net_on_its_one_pin() {
        use crate::input::{Flow, NetKind};
        use crate::testing::{connector, net};
        // Two channels of one block, fed from a connector: IN_A and IN_B
        // differ by layer, VDD is the same rail.
        let nets = vec![
            net("in_a", "IN_A", NetKind::Signal),
            net("in_b", "IN_B", NetKind::Signal),
            net("rail:v", "3V3", NetKind::Supply),
        ];
        let stack = crate::shape::stack_symbol(
            "instance:ch_a",
            "Channel",
            &[
                ("in".into(), "IN".into(), Flow::In, vec![Some(0), Some(1)]),
                (
                    "vdd".into(),
                    "VDD".into(),
                    Flow::Power,
                    vec![Some(2), Some(2)],
                ),
            ],
        );
        let b = Block {
            sheet: [390_000_000, 260_000_000],
            parts: vec![connector("j1", "J1", &[Some(0), Some(1)]), stack],
            nets,
            ..Block::default()
        };
        let out = lay(&b);
        assert_eq!(crate::rules::check(&b, &out.layout), []);
        let on_stack: Vec<&Label> = out
            .layout
            .labels
            .values()
            .flatten()
            .filter(|l| l.on == "instance:ch_a/in")
            .collect();
        assert_eq!(on_stack.len(), 1, "{:?}", out.layout.labels);
        assert_eq!(on_stack[0].text, "IN_A, IN_B");
        // J1's pins take their own nets' labels; nothing is wired.
        assert!(!out.layout.routes.contains_key("in_a"));
        assert!(out.layout.labels["in_b"].iter().any(|l| l.on == "j1/2"));
        // Without the label, IN_B isn't drawn joined to the stack.
        let mut broken = out.layout.clone();
        for list in broken.labels.values_mut() {
            list.retain(|l| l.on != "instance:ch_a/in");
        }
        let rules: Vec<String> = crate::rules::check(&b, &broken)
            .into_iter()
            .map(|v| v.message)
            .collect();
        assert!(
            rules.iter().any(|m| m.contains("in_b in a layer")),
            "{rules:?}"
        );
        // And the SVG says both.
        assert!(crate::svg::draw(&b, &out.layout, &[]).contains(">IN_A, IN_B<"));
        // Asked for as wires, a stacked pin's net is still labelled, and the
        // answer says why.
        let mut asked = b.clone();
        asked.moves = vec![Instruction::Net {
            net: "in_a".into(),
            style: NetStyle::Wire,
        }];
        let out = lay(&asked);
        let note = out
            .notes
            .iter()
            .find(|n| n.code == "net_style_not_held")
            .expect("a note");
        assert!(note.message.contains("stacked"), "{}", note.message);
    }

    #[test]
    fn instructions_hold_and_a_second_layout_keeps_every_bit() {
        let mut b = seeded(2, 3);
        let list = [
            Instruction::Edge {
                parts: vec!["u2".into()],
                side: Side::Right,
            },
            Instruction::Beside {
                parts: vec!["u1".into()],
                of: "u0".into(),
                side: Side::Down,
            },
            Instruction::Sheet {
                parts: vec!["u2".into()],
                sheet: "Logic".into(),
            },
            Instruction::Net {
                net: "rail:gnd".into(),
                style: NetStyle::Label,
            },
            Instruction::Orient {
                parts: vec!["c0".into()],
                rotation: 90,
                mirror: false,
            },
        ];
        for (i, ins) in list.iter().enumerate() {
            b.instructions.insert(format!("i{}", i + 1), ins.clone());
        }
        let out = lay(&b);
        for ins in &list {
            assert!(instr::achieved(&b, &out.layout, ins), "{ins:?}");
        }
        assert_eq!(crate::rules::check(&b, &out.layout), []);
        b.previous = Some(out.layout.clone());
        assert_eq!(lay(&b).layout, out.layout);
    }

    #[test]
    fn moves_are_held_and_kept() {
        let mut b = seeded(1, 2);
        let first = lay(&b).layout;
        b.previous = Some(first.clone());
        b.moves = vec![Instruction::Beside {
            parts: vec!["u1".into()],
            of: "u0".into(),
            side: Side::Down,
        }];
        let moved = lay(&b).layout;
        assert!(moved.placed["u1"].held);
        assert!(instr::achieved(&b, &moved, &b.moves[0]));
        // Untouched parts keep their places.
        for (k, x) in &first.placed {
            if !k.starts_with("u1") && !k.starts_with("c1") && !k.starts_with("rp1") {
                assert_eq!(moved.placed[k].at, x.at, "{k}");
            }
        }
        b.moves.clear();
        b.previous = Some(moved.clone());
        assert_eq!(lay(&b).layout.placed["u1"], moved.placed["u1"]);
    }

    #[test]
    fn a_run_is_cut_at_a_point_on_it() {
        let g = GRID;
        let run = [[0, 0], [4 * g, 0], [4 * g, -3 * g]];
        assert_eq!(
            cut_at(&run, [0, 0], [4 * g, 0]),
            Some(vec![[0, 0], [4 * g, 0]])
        );
        assert_eq!(
            cut_at(&run, [0, 0], [2 * g, 0]),
            Some(vec![[0, 0], [2 * g, 0]])
        );
        assert_eq!(
            cut_at(&run, [4 * g, -3 * g], [4 * g, 0]),
            Some(vec![[4 * g, -3 * g], [4 * g, 0]])
        );
        assert_eq!(cut_at(&run, [0, 0], [g, g]), None);
        assert_eq!(cut_at(&run, [g, 0], [2 * g, 0]), None);
    }

    #[test]
    fn rings_go_out_nearest_first() {
        assert_eq!(ring(0), [(0, 0)]);
        let r1 = ring(1);
        assert_eq!(r1.len(), 8);
        assert_eq!(r1[0], (0, -1));
    }

    #[test]
    fn runs_hold_their_pins_as_one_tree() {
        let g = |x: i64, y: i64| [x * GRID, y * GRID];
        let ends: BTreeSet<Point> = [g(0, 0), g(4, 0), g(2, 3)].into();
        let runs = vec![vec![g(0, 0), g(4, 0)], vec![g(2, 3), g(2, 0)]];
        assert!(holds(&runs, &ends));
        let ends2: BTreeSet<Point> = [g(0, 0), g(4, 0)].into();
        assert!(!holds(&runs, &ends2));
        assert!(!holds(&runs[..1], &ends));
    }

    #[test]
    fn a_net_leaving_its_sheet_takes_one_connector_a_region() {
        let b = crate::testing::leaving_block();
        let out = lay(&b);
        assert!(
            crate::rules::check(&b, &out.layout).is_empty(),
            "{:?}",
            crate::rules::check(&b, &out.layout)
        );
        let away = &out.layout.placed["u2"].sheet;
        assert_ne!(away, &out.layout.placed["u1"].sheet, "u2 on its own sheet");
        let labels = out.layout.labels.get("sig").cloned().unwrap_or_default();
        let here: Vec<&Label> = labels.iter().filter(|l| &l.sheet != away).collect();
        let there: Vec<&Label> = labels.iter().filter(|l| &l.sheet == away).collect();
        // One connector on each sheet: u1's (the part with the most pins)
        // here, u2's there.
        let connectors: Vec<&&Label> = here
            .iter()
            .filter(|l| l.kind == LabelKind::OffSheet)
            .collect();
        assert_eq!(connectors.len(), 1, "{here:?}");
        assert!(connectors[0].on.starts_with("u1/"));
        assert_eq!(
            there
                .iter()
                .filter(|l| l.kind == LabelKind::OffSheet)
                .count(),
            1
        );
        // r1 and r2 are wired to it, or take local labels.
        for r in ["r1", "r2"] {
            let on_label = here
                .iter()
                .any(|l| l.on.starts_with(&format!("{r}/")) && l.kind == LabelKind::Net);
            let wired = out
                .layout
                .routes
                .get("sig")
                .is_some_and(|by| !by.is_empty());
            assert!(on_label || wired, "{r} joins SIG");
        }
        // Nothing on one sheet takes a connector.
        for (key, list) in &out.layout.labels {
            if key != "sig" {
                assert!(list.iter().all(|l| l.kind != LabelKind::OffSheet), "{key}");
            }
        }
    }

    #[test]
    fn a_layout_from_before_off_sheet_connectors_keeps_its_places() {
        // The leaving block laid out once, its connectors turned back
        // into net labels (a format 3 file), then laid out over that.
        let b = crate::testing::leaving_block();
        let first = lay(&b);
        let mut old = first.layout.clone();
        for list in old.labels.values_mut() {
            for l in list.iter_mut().filter(|l| l.kind == LabelKind::OffSheet) {
                l.kind = LabelKind::Net;
            }
        }
        let again = lay(&Block {
            previous: Some(old.clone()),
            ..b.clone()
        });
        assert!(crate::rules::check(&b, &again.layout).is_empty());
        // Every part where it was, and each connector at its old label's
        // place where its box is clear there.
        assert_eq!(again.layout.placed, old.placed);
        let mut connectors = 0;
        for (key, list) in &again.layout.labels {
            for l in list.iter().filter(|l| l.kind == LabelKind::OffSheet) {
                connectors += 1;
                assert!(
                    old.labels[key]
                        .iter()
                        .any(|o| o.on == l.on && o.at == l.at && o.side == l.side),
                    "{key} {} moved",
                    l.on
                );
            }
        }
        assert_eq!(connectors, 2, "a connector on each sheet again");
    }

    #[test]
    fn four_members_between_two_sides_are_drawn_as_a_bus() {
        let mut b = crate::testing::bus_block();
        let out = lay(&b);
        assert!(
            crate::rules::check(&b, &out.layout).is_empty(),
            "{:?}",
            crate::rules::check(&b, &out.layout)
        );
        let drawn = &out.layout.buses.get("bus:q").expect("drawn");
        let (_, bus) = drawn.iter().next().expect("on a sheet");
        assert_eq!(bus.entries.len(), 8, "four members at two ends");
        assert!(!bus.trunk.is_empty());
        // Its members take no labels and no wires of their own.
        for i in 0..4 {
            let k = format!("d{i}");
            assert!(
                out.layout.labels.get(&k).is_none_or(|v| v.is_empty()),
                "{k}"
            );
            assert!(
                out.layout.routes.get(&k).is_none_or(|v| v.is_empty()),
                "{k}"
            );
        }
        // Asked for labels, it isn't drawn.
        b.instructions.insert(
            "i1".into(),
            Instruction::Net {
                net: "bus:q".into(),
                style: NetStyle::Label,
            },
        );
        let out = lay(&b);
        assert!(out.layout.buses.is_empty());
        assert!(crate::rules::check(&b, &out.layout).is_empty());
    }

    #[test]
    fn a_bus_leaving_its_sheet_takes_one_connector() {
        let mut b = crate::testing::bus_block();
        b.instructions.insert(
            "i1".into(),
            Instruction::Sheet {
                parts: vec!["u2".into()],
                sheet: "Away".into(),
            },
        );
        let out = lay(&b);
        assert!(
            crate::rules::check(&b, &out.layout).is_empty(),
            "{:?}",
            crate::rules::check(&b, &out.layout)
        );
        assert_ne!(out.layout.placed["u1"].sheet, out.layout.placed["u2"].sheet);
        let drawn = out.layout.buses.get("bus:q").expect("drawn");
        assert_eq!(drawn.len(), 2, "on both sheets: {:?}", out.notes);
        for bus in drawn.values() {
            assert_eq!(bus.entries.len(), 4);
            let c = bus.connector.as_ref().expect("a connector");
            assert_eq!(c.text, "QSPI_SD[0..3]");
        }
        for i in 0..4 {
            let k = format!("d{i}");
            assert!(
                out.layout.labels.get(&k).is_none_or(|v| v.is_empty()),
                "{k}"
            );
        }
    }

    /// A region moved whole in the file (its origin, one key) carries its
    /// note: laid out again, the note keeps its place in the region.
    #[test]
    fn a_moved_region_keeps_its_note() {
        let mut b = crate::testing::regions_block();
        let first = lay(&b);
        let (rid, _) = first.layout.regions.iter().next().expect("a region");
        let rid = rid.clone();
        b.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: format!("region:{rid}"),
                text: "Fused at 2 A".into(),
            },
        );
        b.previous = Some(first.layout.clone());
        let out = lay(&b);
        let mut file = out.layout.relative();
        file.regions.get_mut(&rid).unwrap().at[0] += 10 * GRID;
        b.previous = Some(file.absolute());
        let again = lay(&b);
        assert!(crate::rules::check(&b, &again.layout).is_empty());
        assert_eq!(again.layout.regions[&rid].at, file.regions[&rid].at);
        assert_eq!(
            again.layout.relative().notes["i9"],
            out.layout.relative().notes["i9"],
            "the note moved with its region"
        );
        let n = &again.layout.notes["i9"];
        assert!(
            again.layout.regions[&rid]
                .frame_on_sheet()
                .contains(&crate::paint::note_box("Fused at 2 A", n.at))
        );
    }

    #[test]
    fn a_note_stands_in_its_region_and_its_frame_holds_it() {
        let mut b = crate::testing::regions_block();
        let first = lay(&b);
        let (rid, _) = first.layout.regions.iter().next().expect("a region");
        b.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: format!("region:{rid}"),
                text: "Fused at 2 A".into(),
            },
        );
        b.notes.insert("i9".into(), "Fused at 2 A".into());
        b.previous = Some(first.layout.clone());
        let out = lay(&b);
        assert!(
            crate::rules::check(&b, &out.layout).is_empty(),
            "{:?}",
            crate::rules::check(&b, &out.layout)
        );
        let placed = &out.layout.notes.get("i9").expect("placed");
        let r = &out.layout.regions[rid];
        assert_eq!(placed.sheet, r.sheet);
        assert!(
            r.frame_on_sheet()
                .contains(&crate::paint::note_box("Fused at 2 A", placed.at))
        );
        // Nothing else moved: the region grew to hold it, or it fit.
        assert_eq!(out.layout.placed, first.layout.placed);
        // On a sheet: inside its drawing area, clear of everything.
        let sheet = r.sheet.clone();
        b.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: sheet,
                text: "Rev A".into(),
            },
        );
        b.notes.insert("i9".into(), "Rev A".into());
        let out = lay(&b);
        assert!(crate::rules::check(&b, &out.layout).is_empty());
        assert!(out.layout.notes.contains_key("i9"));
    }

    #[test]
    fn a_larger_text_moves_only_the_text_that_no_longer_fits() {
        let block = seeded(3, 2);
        let first = lay(&block);
        let key = block
            .parts
            .iter()
            .find(|p| !p.value.is_empty() && first.layout.text.contains_key(&p.key))
            .expect("a part with text")
            .key
            .clone();
        let mut b = block.clone();
        b.previous = Some(first.layout.clone());
        let i = b.part(&key).unwrap();
        b.parts[i].text.size = 1_778_000;
        let out = lay(&b);
        assert_eq!(crate::rules::check(&b, &out.layout), []);
        assert_eq!(out.layout.placed, first.layout.placed, "no part moves");
        for (k, t) in &first.layout.text {
            if *k != key {
                assert_eq!(out.layout.text.get(k), Some(t), "{k}'s text moved");
            }
        }
    }

    #[test]
    fn nets_with_intent_keep_every_rule() {
        for seed in 0..6 {
            let mut b = seeded(seed, 2);
            for (i, n) in b.nets.iter_mut().enumerate() {
                if n.kind == crate::input::NetKind::Signal && i % 2 == 0 {
                    n.intent = "3.3 V · 20 mA".into();
                }
            }
            let out = lay(&b);
            assert_eq!(crate::rules::check(&b, &out.layout), [], "seed {seed}");
            let mut again = b.clone();
            again.previous = Some(out.layout.clone());
            assert_eq!(lay(&again).layout, out.layout, "seed {seed}: kept");
        }
    }

    /// #712: a part's text never stands in front of one of its own pin
    /// ends, where the pin's wire or label will go.
    #[test]
    fn text_stays_out_of_the_room_in_front_of_its_own_pins() {
        use crate::layout::TextField;
        let mut b = Block {
            name: "reg".into(),
            sheet: [277_000_000, 170_000_000],
            ..Block::default()
        };
        b.parts.push(crate::testing::regulator("u1", "U1", "reg"));
        let first = lay(&b);
        b.previous = Some(first.layout.clone());
        b.instructions.insert(
            "i1".into(),
            Instruction::Text {
                parts: vec!["u1".into()],
                field: TextField::Both,
                side: Side::Right,
            },
        );
        let out = lay(&b);
        assert_eq!(crate::rules::check(&b, &out.layout), []);
        let x = &out.layout.placed["u1"];
        let shape = Shape::of(&b.parts[0], Place::new(x.at, x.rotation, x.mirror));
        let fronts = pin_fronts(&shape);
        for (field, line) in b.parts[0].fields() {
            let at = out.layout.text["u1"][field];
            let bx = line.boxed(at);
            assert!(
                fronts.iter().all(|f| !f.overlaps(&bx)),
                "{field} at {at:?} stands in front of a pin: {bx:?}"
            );
        }
    }
}
