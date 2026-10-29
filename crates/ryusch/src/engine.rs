//! Laying a block out: keep what the previous layout holds where it still
//! fits, place what is new (by flow from nothing, else into free space
//! near its neighbours), then on each sheet draw rails and label nets,
//! route the rest, and put each part's text in the first free place.
//! Each step works in a fixed order (part keys, net keys), never a hash's.

use crate::cluster::Cluster;
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
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Space kept between parts placed into a layout that has others.
pub const SPACING: Nm = 2 * GRID;
/// Between sheets on a block's canvas.
pub const SHEET_GAP: Nm = 10 * GRID;
/// A net whose wires would run longer than this share of a sheet's width
/// takes labels.
const LONG_SHARE: (Nm, Nm) = (1, 4);
/// A connection crossing more wires than this takes labels instead.
const CROWDED: usize = 2;

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
}

/// Work in progress: each part's sheet and place.
pub(crate) struct State<'a> {
    pub block: &'a Block,
    pub sheets: Vec<(String, Sheet)>,
    pub place: Vec<Option<(usize, Place, bool)>>,
    pub notes: Vec<Note>,
}

impl<'a> State<'a> {
    pub fn new(block: &'a Block) -> State<'a> {
        State {
            block,
            sheets: Vec::new(),
            place: vec![None; block.parts.len()],
            notes: Vec::new(),
        }
    }

    /// The sheet with ID `id`, or a new one of that ID.
    pub fn sheet_id(&mut self, id: &str, name: &str) -> usize {
        if let Some(i) = self.sheets.iter().position(|(s, _)| s == id) {
            return i;
        }
        let at = [
            self.sheets.len() as Nm * (self.block.sheet[0] + SHEET_GAP),
            0,
        ];
        self.sheets.push((
            id.to_owned(),
            Sheet {
                name: name.to_owned(),
                at,
            },
        ));
        self.sheets.len() - 1
    }

    /// A new sheet, its ID one past the largest `s<n>`.
    pub fn new_sheet(&mut self, name: Option<&str>) -> usize {
        let n = self
            .sheets
            .iter()
            .filter_map(|(id, _)| id.strip_prefix('s')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("s{n}");
        let name = name.map_or_else(|| format!("Sheet {n}"), str::to_owned);
        self.sheet_id(&id, &name)
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

/// The area a sheet's contents stay inside.
fn area(block: &Block) -> Rect {
    Rect::new([0, 0], block.sheet)
}

/// A cluster's members' places at `origin`, on the grid.
fn at(c: &Cluster, origin: Point) -> Vec<(usize, Place)> {
    c.places(origin)
}

/// How a part is turned when nothing says otherwise: a connector faces
/// into the sheet from its left edge, so its pins point right.
pub fn default_turn(block: &Block, p: usize) -> (u16, bool) {
    let part = &block.parts[p];
    if crate::cluster::is_connector(block, p) {
        let left = part.pins.iter().filter(|x| x.side == Side::Left).count();
        if left * 2 > part.pins.len() {
            return (0, true);
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
    }
}

/// Lays `block` out, or refuses when its instructions can't hold.
pub fn lay_out(block: &Block) -> Result<Outcome, instr::Refusal> {
    let r = instr::resolve(block)?;
    let mut st = State::new(block);
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
            st.sheet_id(id, &s.name);
        }
    }
    // What stays where it was: not what this batch moves, nor what a
    // lasting instruction says should be elsewhere.
    let mut unkeep = r.moved.clone();
    if let Some(prev) = prev {
        for ins in block.instructions.values() {
            if positional(ins) && !instr::achieved(block, prev, ins) {
                for s in ins.parts() {
                    unkeep.extend(instr::parts_named(block, s));
                }
            }
        }
    }
    keep(&mut st, prev, fresh_sheet, &unkeep);
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
    let idioms = idiom::find(block, &r.idiom);
    let mut clusters = idiom::clusters(block, &idioms, singles);
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
    let mut hints = Hints::default();
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
        if !fresh.is_empty() || by_name.is_empty() {
            flow_onto_sheets(
                &mut st,
                &clusters,
                fresh,
                Target::Fresh(fresh_sheet),
                &hints,
            );
        }
        for (name, list) in by_name {
            flow_onto_sheets(&mut st, &clusters, list, Target::Named(&name), &hints);
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
        insert(&mut st, prev, &clusters, &fresh, &mixed, &r, &hints);
    }
    relate(&mut st, prev, &clusters, &r);
    for p in &r.moved {
        if let Some((s, place, _)) = st.place[*p] {
            st.place[*p] = Some((s, place, true));
        }
    }
    let mut out = draw(st, prev, &r.styles);
    out.idioms = idioms;
    out.flow = r.flow.iter().map(|p| block.parts[*p].key.clone()).collect();
    Ok(out)
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
fn relate(st: &mut State, prev: Option<&Layout>, clusters: &[Cluster], r: &instr::Resolved) {
    let drawn = drawn(prev);
    // What moves with `p`: its whole cluster if it leads one, else itself.
    let movers = |p: usize| -> Vec<usize> {
        clusters
            .iter()
            .find(|c| c.first() == p)
            .map_or_else(|| vec![p], |c| c.members.iter().map(|(q, _)| *q).collect())
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
    // Moves `list` so its room's middle lands nearest `centre`.
    let lay = |st: &mut State, list: &[usize], centre: Point| {
        let Some((s, now)) = room(st, list) else {
            return;
        };
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
        match spot(st, s, e0, want, SPACING, &blocked) {
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
        let (Some((_, target)), Some((_, mine))) = (room(st, &movers(of)), room(st, &list)) else {
            continue;
        };
        let (c, half) = (target.center(), [mine.width() / 2, mine.height() / 2]);
        let centre = match side {
            Side::Left => [target.min[0] - SPACING - half[0], c[1]],
            Side::Right => [target.max[0] + SPACING + half[0], c[1]],
            Side::Up => [c[0], target.max[1] + SPACING + half[1]],
            Side::Down => [c[0], target.min[1] - SPACING - half[1]],
        };
        lay(st, &list, centre);
    }
    for (&p, &(to, pin)) in &r.near {
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
        let (Some((_, target)), Some((_, mine))) = (room(st, &[to]), room(st, &list)) else {
            continue;
        };
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
        lay(st, &list, centre);
    }
    for (list, axis) in &r.align {
        if holds(
            st,
            Instruction::Align {
                parts: list.iter().map(|p| key(*p)).collect(),
                axis: *axis,
            },
        ) {
            continue;
        }
        let Some(lead) = list.first().and_then(|l| st.shape(*l)) else {
            continue;
        };
        let lc = lead.body.center();
        for &p in &list[1..] {
            let moving = movers(p);
            let (Some(sh), Some((_, mine))) = (st.shape(p), room(st, &moving)) else {
                continue;
            };
            let c = sh.body.center();
            let shift = match axis {
                crate::layout::Axis::Row => [0, lc[1] - c[1]],
                crate::layout::Axis::Column => [lc[0] - c[0], 0],
            };
            let m = mine.center();
            lay(st, &moving, [m[0] + shift[0], m[1] + shift[1]]);
        }
    }
}

/// Where `flow_onto_sheets` starts.
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
        let placed = place::flow(block, clusters, &which, block.sheet, hints);
        for (c, o) in &placed.origins {
            for (p, place) in at(&clusters[*c], *o) {
                st.place[p] = Some((s, place, false));
            }
        }
        if placed.overflow.len() == which.len() {
            // Nothing fits even alone: put the first where it may.
            let c = which.remove(0);
            for (p, place) in at(&clusters[c], [MARGIN, block.sheet[1] - MARGIN]) {
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
    let a = area(block);
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
        if !a.contains(&shape.extent) {
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
    let block = st.block;
    let a = area(block).grow(-MARGIN);
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
    let max = block.sheet[0].max(block.sheet[1]) / GRID;
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
/// that), on their sheet; a new cluster on the sheet most of its
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
        let blocked = drawn.get(&sid).cloned().unwrap_or_default();
        for &(p, place) in &c.members {
            if st.place[p].is_some() {
                continue;
            }
            let at0 = Place::new([0, 0], place.rotation, place.mirror);
            let e0 = Shape::of(&block.parts[p], at0).extent;
            match spot(st, s, e0, place.at, GRID, &blocked) {
                Some(o) => {
                    st.place[p] = Some((s, Place::new(o, place.rotation, place.mirror), false))
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
        let a = area(block).grow(-MARGIN);
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
            Some((s, pts)) if pts.is_empty() => (*s, a.center()),
            Some((s, pts)) => {
                let n = pts.len() as Nm;
                let sum = pts.iter().fold([0, 0], |a, p| [a[0] + p[0], a[1] + p[1]]);
                (*s, [sum[0] / n, sum[1] / n])
            }
            None => (0, a.center()),
        };
        let sid = st.sheets[s].0.clone();
        let blocked = drawn.get(&sid).cloned().unwrap_or_default();
        let e0 = c.room(block, [0, 0]);
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
        crate::geom::snap(block.sheet[1] - MARGIN - e0.max[1]),
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

/// Draws every sheet: labels, wires and text.
fn draw(st: State, prev: Option<&Layout>, asked: &BTreeMap<usize, NetStyle>) -> Outcome {
    let block = st.block;
    let mut layout = Layout::default();
    let mut fallbacks: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, s) in &st.sheets {
        layout.sheets.insert(id.clone(), s.clone());
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
    // Which sheets each net is on.
    let mut net_sheets: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (p, part) in block.parts.iter().enumerate() {
        let Some((s, _, _)) = st.place[p] else {
            continue;
        };
        for pin in &part.pins {
            if let Some(n) = pin.net
                && !pin.nc
            {
                net_sheets.entry(n).or_default().insert(s);
            }
        }
    }
    for s in 0..st.sheets.len() {
        let sid = st.sheets[s].0.clone();
        let parts = st.on_sheet(s);
        let mut field = Field::new(block.sheet);
        let mut pins_of: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
        for &p in &parts {
            let shape = st.shape(p).expect("on the sheet");
            field.block_body(shape.body);
            for (k, pin) in block.parts[p].pins.iter().enumerate() {
                let at = shape.pins[k];
                let net = pin.net.filter(|_| !pin.nc);
                field.pin(at.base, at.end, at.side, net.unwrap_or(NO_NET));
                if let Some(n) = net {
                    pins_of.entry(n).or_default().push((p, k));
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
        let long = block.sheet[0] * LONG_SHARE.0 / LONG_SHARE.1;
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
        let base = field;
        let (field_done, labels, routes) = loop {
            let mut field = base.clone();
            let mut labels: BTreeMap<usize, Vec<Label>> = BTreeMap::new();
            let mut routes: BTreeMap<usize, Vec<Vec<Point>>> = BTreeMap::new();
            // Labels first, the previous ones where they still fit.
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
                    want_label.push((*n, p, k, kind));
                }
            }
            place_labels(
                block,
                &st,
                &mut field,
                &want_label,
                &prev_labels,
                &runs_before,
                &sid,
                &mut labels,
                &mut routes,
            );
            // Wires: nets the previous layout drew that still hold, then
            // the rest, shortest first.
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
                break (field, labels, routes);
            }
            instead.extend(failed);
        };
        let mut field = field_done;
        if !instead.is_empty() {
            fallbacks
                .entry(sid.clone())
                .or_default()
                .extend(instead.iter().map(|n| block.nets[*n].key.clone()));
        }
        // Text.
        for &p in &parts {
            let part = &block.parts[p];
            if matches!(part.kind, PartKind::Port { .. }) {
                continue;
            }
            let lines: Vec<(&str, &str)> = [
                ("refdes", part.refdes.as_str()),
                ("value", part.value.as_str()),
            ]
            .into_iter()
            .filter(|(_, t)| !t.is_empty())
            .collect();
            if lines.is_empty() {
                continue;
            }
            let texts: Vec<&str> = lines.iter().map(|(_, t)| *t).collect();
            let kept = prev
                .and_then(|l| l.text.get(&part.key))
                .and_then(|f| f.get(lines[0].0))
                .copied();
            let body = st.shape(p).expect("placed").body;
            let fits = |at: Point| text::boxes(&texts, at).iter().all(|b| field.clear(*b));
            let at = kept
                .filter(|k| fits(*k))
                .or_else(|| {
                    text::candidates(body, &texts)
                        .into_iter()
                        .find(|c| fits(*c))
                })
                .unwrap_or_else(|| text::candidates(body, &texts)[0]);
            for b in text::boxes(&texts, at) {
                field.block(b);
            }
            let mut fields = BTreeMap::new();
            for (i, (name, _)) in lines.iter().enumerate() {
                fields.insert((*name).to_owned(), [at[0], at[1] - text::LINE * i as Nm]);
            }
            layout.text.insert(part.key.clone(), fields);
        }
        for (n, runs) in routes {
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
    Outcome {
        layout,
        notes: st.notes,
        fallbacks,
        idioms: Vec::new(),
        flow: BTreeSet::new(),
    }
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
    let mut trial = field.clone();
    for r in runs {
        if trial.mark(net, r).is_err() {
            return false;
        }
    }
    *field = trial;
    true
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
        if f.wired(from, net) {
            joined.push(from);
            continue;
        }
        let path = f.search(net, from, side)?;
        if f.crossings(net, &path) > CROWDED {
            return None;
        }
        let run = route::run_of(&path);
        total += route::length(&run);
        if total > long {
            return None;
        }
        f.mark(net, &run).ok()?;
        joined.push(from);
        runs.push(run);
    }
    Some((f, runs))
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
    prev_labels: &dyn Fn(usize) -> Vec<Label>,
    prev_runs: &BTreeMap<usize, Vec<Vec<Point>>>,
    sid: &str,
    labels: &mut BTreeMap<usize, Vec<Label>>,
    routes: &mut BTreeMap<usize, Vec<Vec<Point>>>,
) {
    for &(n, p, k, kind) in want {
        let part = &block.parts[p];
        let on = format!("{}/{}", part.key, part.pins[k].number);
        let pin = st.shape(p).expect("placed").pins[k];
        let name = &block.nets[n].name;
        let fits = |field: &Field, stub: &[Point], at: Point, side: Side| -> bool {
            if !field.clear(label::label_box(kind, name, at, side)) {
                return false;
            }
            stub.is_empty() || field.clone().mark(n, stub).is_ok()
        };
        // The previous label, its stub one of the net's previous runs.
        let before = prev_labels(n)
            .into_iter()
            .find(|l| l.on == on && l.kind == kind);
        let mut chosen: Option<(Vec<Point>, Point, Side)> = None;
        if let Some(l) = before {
            let stub = if l.at == pin.end {
                Some(Vec::new())
            } else {
                prev_runs
                    .get(&n)
                    .and_then(|runs| {
                        runs.iter()
                            .find(|r| r.first() == Some(&pin.end) && r.last() == Some(&l.at))
                    })
                    .cloned()
            };
            if let Some(stub) = stub
                && fits(field, &stub, l.at, l.side)
            {
                chosen = Some((stub, l.at, l.side));
            }
        }
        let cands = label::candidates(kind, pin.end, pin.side);
        let (stub, at, side) = chosen
            .or_else(|| {
                cands
                    .iter()
                    .find(|(s, a, side)| fits(field, s, *a, *side))
                    .cloned()
            })
            .unwrap_or_else(|| cands[0].clone());
        if !stub.is_empty() && field.mark(n, &stub).is_ok() {
            routes.entry(n).or_default().push(stub);
        }
        field.block(label::label_box(kind, name, at, side));
        labels.entry(n).or_default().push(Label {
            sheet: sid.to_owned(),
            at,
            side,
            kind,
            on,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::seeded;

    fn lay(b: &Block) -> Outcome {
        lay_out(b).unwrap()
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
}
