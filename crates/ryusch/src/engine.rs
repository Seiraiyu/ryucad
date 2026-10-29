//! Laying a block out: keep what the previous layout holds where it still
//! fits, place what is new (by flow from nothing, else into free space
//! near its neighbours), then on each sheet draw rails and label nets,
//! route the rest, and put each part's text in the first free place.
//! Each step works in a fixed order (part keys, net keys), never a hash's.

use crate::cluster::Cluster;
use crate::geom::{GRID, Nm, Place, Point, Rect, Side};
use crate::input::{Block, NetKind, PartKind, Relayout};
use crate::label;
use crate::layout::{Label, LabelKind, Layout, NetStyle, Placed, Sheet};
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
const LONG_SHARE: (Nm, Nm) = (3, 5);

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

/// Each part alone, turned as `turn` says.
fn singles(block: &Block, parts: &[usize], turn: &dyn Fn(usize) -> (u16, bool)) -> Vec<Cluster> {
    parts
        .iter()
        .map(|&p| {
            let (r, m) = turn(p);
            let place = Place::new([0, 0], r, m);
            let o = grid_origin(&block.parts[p], &place, [0, 0]);
            Cluster::single(p, Place::new(o, r, m))
        })
        .collect()
}

/// Lays `block` out.
pub fn lay_out(block: &Block) -> Outcome {
    let mut st = State::new(block);
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
    // What stays where it was.
    keep(&mut st, prev, fresh_sheet);
    let new: Vec<usize> = (0..block.parts.len())
        .filter(|p| st.place[*p].is_none())
        .collect();
    let turn = |p: usize| -> (u16, bool) {
        prev.and_then(|l| l.placed.get(&block.parts[p].key))
            .map_or_else(|| default_turn(block, p), |x| (x.rotation, x.mirror))
    };
    let clusters = singles(block, &new, &turn);
    let kept_any = st.place.iter().any(Option::is_some);
    if kept_any {
        insert(&mut st, prev, &clusters);
    } else {
        let all: Vec<usize> = (0..clusters.len()).collect();
        flow_onto_sheets(&mut st, &clusters, all, fresh_sheet);
    }
    draw(st, prev)
}

/// Places `which` clusters by flow on a fresh sheet (the one being laid
/// out again, else a new one), and what doesn't fit on the next.
fn flow_onto_sheets(
    st: &mut State,
    clusters: &[Cluster],
    mut which: Vec<usize>,
    sheet: Option<&str>,
) {
    let block = st.block;
    let mut first = true;
    while !which.is_empty() {
        let s = match (first, sheet) {
            (true, Some(id)) => st.sheet_id(id, id),
            _ => st.new_sheet(None),
        };
        first = false;
        let placed = place::flow(block, clusters, &which, block.sheet, &Hints::default());
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
/// ones first, then in key order).
fn keep(st: &mut State, prev: Option<&Layout>, fresh: Option<&str>) {
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
        if Some(x.sheet.as_str()) == fresh {
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

/// Places new clusters among kept parts: on the sheet most of its
/// neighbours are on, at the free spot nearest them, clear of every
/// part, wire, label and text the previous layout drew.
fn insert(st: &mut State, prev: Option<&Layout>, clusters: &[Cluster]) {
    let block = st.block;
    if st.sheets.is_empty() {
        st.new_sheet(None);
    }
    // What the previous layout drew, per sheet: wires, labels and text.
    let mut drawn: BTreeMap<String, Vec<Rect>> = BTreeMap::new();
    if let Some(prev) = prev {
        for sheets in prev.routes.values() {
            for (s, runs) in sheets {
                for run in runs {
                    for w in run.windows(2) {
                        drawn
                            .entry(s.clone())
                            .or_default()
                            .push(Rect::new(w[0], w[1]).grow(GRID / 2));
                    }
                }
            }
        }
        for labels in prev.labels.values() {
            for l in labels {
                drawn
                    .entry(l.sheet.clone())
                    .or_default()
                    .push(Rect::new(l.at, l.at).grow(3 * GRID));
            }
        }
        for (key, fields) in &prev.text {
            let Some(x) = prev.placed.get(key) else {
                continue;
            };
            for at in fields.values() {
                drawn
                    .entry(x.sheet.clone())
                    .or_default()
                    .push(Rect::new(*at, [at[0] + 6 * GRID, at[1] + GRID]).grow(GRID / 2));
            }
        }
    }
    let a = area(block).grow(-MARGIN);
    for c in clusters {
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
        let (s, target) = match by_sheet
            .iter()
            .max_by_key(|(s, v)| (v.len(), usize::MAX - **s))
        {
            Some((s, pts)) => {
                let n = pts.len() as Nm;
                let sum = pts.iter().fold([0, 0], |a, p| [a[0] + p[0], a[1] + p[1]]);
                (*s, [sum[0] / n, sum[1] / n])
            }
            None => (0, a.center()),
        };
        let others: Vec<Rect> = (0..block.parts.len())
            .filter(|p| st.place[*p].is_some_and(|(x, _, _)| x == s))
            .filter_map(|p| st.shape(p).map(|sh| sh.extent))
            .collect();
        let sheet_id = st.sheets[s].0.clone();
        let blocked = drawn.get(&sheet_id).cloned().unwrap_or_default();
        let e0 = c.room(block, [0, 0]);
        let fits = |o: Point| {
            let e = e0.shift(o);
            a.contains(&e)
                && !others.iter().any(|r| r.overlaps(&e.grow(SPACING)))
                && !blocked.iter().any(|r| r.overlaps(&e.grow(GRID)))
        };
        let want = [
            route::point_of(route::node_of([target[0] - e0.center()[0], 0]))[0],
            route::point_of(route::node_of([0, target[1] - e0.center()[1]]))[1],
        ];
        let max = (block.sheet[0].max(block.sheet[1]) / GRID) as i64;
        let mut found = None;
        'rings: for r in 0..=max {
            for (dx, dy) in ring(r) {
                let o = [want[0] + dx * GRID, want[1] + dy * GRID];
                if fits(o) {
                    found = Some((s, o));
                    break 'rings;
                }
            }
        }
        let (s, o) = match found {
            Some(f) => f,
            None => {
                let s = st.new_sheet(None);
                let name = block.parts[c.first()].key.clone();
                st.notes.push(Note {
                    code: "sheet_full".into(),
                    message: format!(
                        "sheet {sheet_id} has no room for {name}; it goes on sheet {}",
                        st.sheets[s].0
                    ),
                    details: json!({"sheet": sheet_id, "parts": [name], "to": st.sheets[s].0}),
                });
                (s, [MARGIN - e0.min[0], block.sheet[1] - MARGIN - e0.max[1]])
            }
        };
        for (p, place) in at(c, o) {
            st.place[p] = Some((s, place, false));
        }
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
fn draw(st: State, prev: Option<&Layout>) -> Outcome {
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
            .map(|(n, list)| (*n, style(block, *n, list.len(), net_sheets[n].len())))
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
        let path = f.search(net, from, side)?;
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
