//! Instructions, resolved against the block: what each says of which
//! parts and nets, those whose subject is gone (noted, kept until
//! released), and those that can't hold together (refused, naming each
//! other). Lasting instructions and one-off moves resolve alike; what a
//! move places is held.

use crate::engine::Note;
use crate::geom::{Place, Point, Side};
use crate::input::{Block, NetKind};
use crate::layout::{Axis, IdiomChoice, Instruction, Layout, NetStyle};
use crate::shape::Shape;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Why a layout can't be made: instructions that don't hold together, or
/// one that would break a hard rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    /// `instruction_conflict` or `layout_rule`.
    pub code: &'static str,
    pub message: String,
    /// The instructions concerned (`i3`, or `move 1` for the batch's
    /// first move), and the rule broken.
    pub details: Value,
}

/// A part an `align` lines up, by its body or (`Some`) one of its pins.
pub type Lined = (usize, Option<usize>);

/// What the instructions say, by part and net index.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub orient: BTreeMap<usize, (u16, bool)>,
    /// A sheet by name.
    pub sheet: BTreeMap<usize, String>,
    pub edge: BTreeMap<usize, Side>,
    /// Beside a part, on a side.
    pub beside: BTreeMap<usize, (usize, Side)>,
    /// Near a part, or one of its pins.
    pub near: BTreeMap<usize, (usize, Option<usize>)>,
    /// Parts lined up, each by its body or (`Some`) one of its pins.
    pub align: Vec<(Vec<Lined>, Axis)>,
    /// Moves nudging parts by grid steps.
    pub shift: Vec<(Vec<usize>, Side, u16)>,
    pub groups: Vec<Vec<usize>>,
    pub styles: BTreeMap<usize, NetStyle>,
    pub idiom: BTreeMap<usize, IdiomChoice>,
    pub flow: BTreeSet<usize>,
    /// Parts a move of this batch places: held where they go.
    pub moved: BTreeSet<usize>,
    pub notes: Vec<Note>,
}

/// The parts a subject names: the part keyed so, or every unit of the
/// component keyed so, or a pin's part (`<part key>/<number>`).
pub fn parts_named(block: &Block, subject: &str) -> Vec<usize> {
    if let Some((p, _)) = pin_named(block, subject) {
        return vec![p];
    }
    let mut out: Vec<usize> = block
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.key == subject
                || p.key
                    .strip_prefix(subject)
                    .is_some_and(|rest| rest.starts_with(':') && !subject.contains(':'))
        })
        .map(|(i, _)| i)
        .collect();
    out.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
    out
}

/// A pin `<part key>/<number>`: its part and pin.
pub fn pin_named(block: &Block, text: &str) -> Option<(usize, usize)> {
    let (key, number) = text.rsplit_once('/')?;
    let p = block.part(key)?;
    let k = block.parts[p]
        .pins
        .iter()
        .position(|x| x.number == number)?;
    Some((p, k))
}

fn conflict(a: &str, b: &str, why: &str) -> Refusal {
    Refusal {
        code: "instruction_conflict",
        message: format!("{a} and {b} can't both hold: {why}"),
        details: json!({"instructions": [a, b]}),
    }
}

fn rule(id: &str, rule: &str, why: String) -> Refusal {
    Refusal {
        code: "layout_rule",
        message: format!("{id} would break a rule of readability ({rule}): {why}"),
        details: json!({"instructions": [id], "rule": rule}),
    }
}

/// Records `v` for each of `parts` in `map`, refusing where an earlier
/// instruction set another value for one of them.
fn set<T: Clone + PartialEq>(
    map: &mut BTreeMap<usize, T>,
    by: &mut BTreeMap<(&'static str, usize), String>,
    what: &'static str,
    parts: &[usize],
    v: T,
    id: &str,
    block: &Block,
) -> Result<(), Refusal> {
    for &p in parts {
        if let Some(old) = map.get(&p)
            && *old != v
        {
            let other = by[&(what, p)].clone();
            // A style is a net's; everything else a part's.
            let of = if what == "style" {
                &block.nets[p].name
            } else {
                &block.parts[p].key
            };
            return Err(conflict(
                &other,
                id,
                &format!("they say different things of {of}'s {what}"),
            ));
        }
        map.insert(p, v.clone());
        by.insert((what, p), id.to_owned());
    }
    Ok(())
}

/// Resolves the block's lasting instructions (by ID) and moves (`move 1`,
/// `move 2`, …), in that order.
pub fn resolve(block: &Block) -> Result<Resolved, Refusal> {
    let mut r = Resolved::default();
    let mut by: BTreeMap<(&'static str, usize), String> = BTreeMap::new();
    let mut grouped: BTreeMap<usize, String> = BTreeMap::new();
    // Edge, beside and near each say where a part goes: one at most.
    let mut placed_by: BTreeMap<usize, String> = BTreeMap::new();
    let all: Vec<(String, &Instruction, bool)> = block
        .instructions
        .iter()
        .map(|(id, i)| (id.clone(), i, false))
        .chain(
            block
                .moves
                .iter()
                .enumerate()
                .map(|(n, i)| (format!("move {}", n + 1), i, true)),
        )
        .collect();
    for (id, ins, is_move) in all {
        // Each subject must name something; one that doesn't is noted and
        // the instruction set aside.
        let mut missing = Vec::new();
        let mut parts: Vec<usize> = Vec::new();
        for s in ins.parts() {
            let found = parts_named(block, s);
            if found.is_empty() {
                missing.push(s.clone());
            }
            parts.extend(found);
        }
        match ins {
            Instruction::Beside { of, .. } if parts_named(block, of).is_empty() => {
                missing.push(of.clone())
            }
            Instruction::Near { to, .. }
                if pin_named(block, to).is_none() && parts_named(block, to).is_empty() =>
            {
                missing.push(to.clone())
            }
            Instruction::Net { net, .. } if block.net(net).is_none() => missing.push(net.clone()),
            _ => {}
        }
        if !missing.is_empty() {
            r.notes.push(Note {
                code: "instruction_unresolved".into(),
                message: format!(
                    "{id} names {} which the block no longer has; it is kept until released",
                    missing.join(", ")
                ),
                details: json!({"instruction": id, "missing": missing}),
            });
            continue;
        }
        if is_move {
            r.moved.extend(parts.iter().copied());
        }
        if matches!(
            ins,
            Instruction::Edge { .. } | Instruction::Beside { .. } | Instruction::Near { .. }
        ) {
            for &p in &parts {
                if let Some(other) = placed_by.get(&p) {
                    return Err(conflict(
                        other,
                        &id,
                        &format!("both say where {} goes", block.parts[p].key),
                    ));
                }
                placed_by.insert(p, id.clone());
            }
        }
        match ins {
            Instruction::Orient {
                rotation, mirror, ..
            } => {
                if rotation % 90 != 0 {
                    return Err(rule(
                        &id,
                        "upright",
                        format!("{rotation}° isn't a quarter turn"),
                    ));
                }
                set(
                    &mut r.orient,
                    &mut by,
                    "orientation",
                    &parts,
                    (rotation % 360, *mirror),
                    &id,
                    block,
                )?;
            }
            Instruction::Sheet { sheet, .. } => {
                set(
                    &mut r.sheet,
                    &mut by,
                    "sheet",
                    &parts,
                    sheet.clone(),
                    &id,
                    block,
                )?;
            }
            Instruction::Edge { side, .. } => {
                set(&mut r.edge, &mut by, "place", &parts, *side, &id, block)?;
            }
            Instruction::Beside { of, side, .. } => {
                let of_parts = parts_named(block, of);
                if parts.iter().any(|p| of_parts.contains(p)) {
                    return Err(rule(
                        &id,
                        "overlap",
                        format!("{of} can't sit beside itself"),
                    ));
                }
                set(
                    &mut r.beside,
                    &mut by,
                    "place",
                    &parts,
                    (of_parts[0], *side),
                    &id,
                    block,
                )?;
            }
            Instruction::Near { to, .. } => {
                let target = match pin_named(block, to) {
                    Some((p, k)) => (p, Some(k)),
                    None => (parts_named(block, to)[0], None),
                };
                if parts.contains(&target.0) {
                    return Err(rule(&id, "overlap", format!("{to} can't sit near itself")));
                }
                set(&mut r.near, &mut by, "place", &parts, target, &id, block)?;
            }
            Instruction::Align {
                axis,
                parts: subjects,
            } => {
                // A pin subject lines up by that pin; a component's units
                // each by their bodies.
                let mut list: Vec<(usize, Option<usize>)> = Vec::new();
                for s in subjects {
                    match pin_named(block, s) {
                        Some((p, k)) => list.push((p, Some(k))),
                        None => list.extend(parts_named(block, s).into_iter().map(|p| (p, None))),
                    }
                }
                if list.len() >= 2 {
                    r.align.push((list, *axis));
                }
            }
            Instruction::Shift { side, steps, .. } => {
                if !is_move {
                    return Err(rule(
                        &id,
                        "shift",
                        "shift is a move: a lasting one would nudge again on every layout".into(),
                    ));
                }
                if *steps == 0 || *steps > 40 {
                    return Err(rule(
                        &id,
                        "shift",
                        format!("{steps} isn't 1 to 40 grid steps"),
                    ));
                }
                r.shift.push((parts.clone(), *side, *steps));
            }
            // plan 2.A Task 7 (regions) and Task 16 (paper).
            Instruction::Region { .. }
            | Instruction::RegionNone { .. }
            | Instruction::Paper { .. } => {}
            Instruction::Group { .. } => {
                for &p in &parts {
                    if let Some(other) = grouped.get(&p) {
                        return Err(conflict(
                            other,
                            &id,
                            &format!("{} can't be in two groups", block.parts[p].key),
                        ));
                    }
                    grouped.insert(p, id.clone());
                }
                r.groups.push(parts.clone());
            }
            Instruction::Net { net, style } => {
                let n = block.net(net).expect("resolved");
                if *style == NetStyle::Port && block.nets[n].kind == NetKind::Signal {
                    return Err(rule(
                        &id,
                        "ports",
                        format!("{net} is a signal; power ports draw rails"),
                    ));
                }
                set(&mut r.styles, &mut by, "style", &[n], *style, &id, block)?;
            }
            Instruction::Idiom { kind, .. } => {
                set(&mut r.idiom, &mut by, "idiom", &parts, *kind, &id, block)?;
            }
            Instruction::Flow { .. } => {
                r.flow.extend(parts.iter().copied());
            }
        }
    }
    // Grouped and oriented parts draw no idiom (its shape would place and
    // turn them), unless an instruction asks for one.
    for p in grouped.keys().chain(r.orient.keys()) {
        r.idiom.entry(*p).or_insert(IdiomChoice::None);
    }
    Ok(r)
}

/// Whether an instruction holds in `layout` as the lint sees the drawing,
/// and if not, why: `beside`, `near` and `group` within the idiom reach
/// (`lint::IDIOM_REACH`), `align` on the lint's lines (`lint::part_lines`).
/// Reports use this; the engine keeps `achieved` for what it moves.
pub fn judge(block: &Block, layout: &Layout, ins: &Instruction) -> Result<(), String> {
    use crate::geom::{GRID, Nm, Rect};
    use crate::lint::{IDIOM_REACH, part_lines, who};
    let mm = |v: Nm| format!("{:.1} mm", v as f64 / 1e6);
    let reach = mm(IDIOM_REACH);
    let shape = |p: usize| {
        let part = &block.parts[p];
        layout.placed.get(&part.key).map(|x| {
            (
                x.sheet.clone(),
                Shape::of(part, Place::new(x.at, x.rotation, x.mirror)),
            )
        })
    };
    let name = |p: usize| who(&block.parts[p]);
    let parts: Vec<usize> = ins
        .parts()
        .iter()
        .flat_map(|s| parts_named(block, s))
        .collect();
    // The gap between two boxes, 0 where they overlap.
    let gap = |a: &Rect, b: &Rect| -> Nm {
        let dx = (b.min[0] - a.max[0]).max(a.min[0] - b.max[0]).max(0);
        let dy = (b.min[1] - a.max[1]).max(a.min[1] - b.max[1]).max(0);
        dx.max(dy)
    };
    let placed = |p: usize| shape(p).ok_or_else(|| format!("{} isn't placed", name(p)));
    match ins {
        Instruction::Beside { of, side, .. } => {
            let o = parts_named(block, of)
                .first()
                .copied()
                .ok_or_else(|| format!("{of} names nothing"))?;
            let (os, ob) = placed(o)?;
            for &p in &parts {
                let (s, sh) = placed(p)?;
                if s != os {
                    return Err(format!("{} is on another sheet than {}", name(p), name(o)));
                }
                let on_side = match side {
                    Side::Left => sh.extent.max[0] <= ob.extent.min[0],
                    Side::Right => sh.extent.min[0] >= ob.extent.max[0],
                    Side::Up => sh.extent.min[1] >= ob.extent.max[1],
                    Side::Down => sh.extent.max[1] <= ob.extent.min[1],
                };
                if !on_side {
                    return Err(format!(
                        "{} isn't {} {}",
                        name(p),
                        side_word(*side),
                        name(o)
                    ));
                }
                let g = gap(&sh.extent, &ob.extent);
                if g > IDIOM_REACH {
                    return Err(format!(
                        "{} is {} from {}; beside means within {reach}",
                        name(p),
                        mm(g),
                        name(o)
                    ));
                }
            }
            Ok(())
        }
        Instruction::Near { to, .. } => {
            let t = pin_named(block, to)
                .map(|(p, _)| p)
                .or_else(|| parts_named(block, to).first().copied())
                .ok_or_else(|| format!("{to} names nothing"))?;
            let (ts, tb) = placed(t)?;
            for &p in &parts {
                let (s, sh) = placed(p)?;
                if s != ts {
                    return Err(format!("{} is on another sheet than {}", name(p), name(t)));
                }
                let g = gap(&sh.extent, &tb.extent);
                if g > IDIOM_REACH {
                    return Err(format!(
                        "{} is {} from {}; near means within {reach}",
                        name(p),
                        mm(g),
                        name(t)
                    ));
                }
            }
            Ok(())
        }
        Instruction::Shift { .. } => Ok(()),
        Instruction::Align {
            axis,
            parts: subjects,
        } if subjects.iter().any(|s| pin_named(block, s).is_some()) => {
            let on = |v: Point| if *axis == Axis::Row { v[1] } else { v[0] };
            let mut ends = Vec::new();
            for s in subjects {
                let (p, k) = pin_named(block, s)
                    .ok_or_else(|| format!("{s}: a pin-level align lines up pins only"))?;
                let (sheet, sh) = placed(p)?;
                ends.push((p, k, sheet, on(sh.pins[k].end)));
            }
            let (p0, k0, s0, v0) = ends[0].clone();
            let pin_name = |p: usize, k: usize| {
                let pin = &block.parts[p].pins[k];
                let n = if pin.name.is_empty() {
                    &pin.number
                } else {
                    &pin.name
                };
                format!("{}.{n}", name(p))
            };
            for (p, k, s, v) in &ends[1..] {
                if *s != s0 {
                    return Err(format!(
                        "{} is on another sheet than {}; align lines pins up on one sheet",
                        pin_name(*p, *k),
                        pin_name(p0, k0)
                    ));
                }
                if *v != v0 {
                    return Err(format!(
                        "{} is {} off {}'s {}",
                        pin_name(*p, *k),
                        mm((v - v0).abs()),
                        pin_name(p0, k0),
                        if *axis == Axis::Row { "row" } else { "column" }
                    ));
                }
            }
            Ok(())
        }
        Instruction::Align { axis, .. } => {
            let mut lines = Vec::new();
            for &p in &parts {
                let (s, sh) = placed(p)?;
                let (xs, ys) = part_lines(&block.parts[p], &sh);
                lines.push((p, s, if *axis == Axis::Row { ys } else { xs }));
            }
            let Some((first, s0, l0)) = lines.first().cloned() else {
                return Err("it names no part".into());
            };
            // Align lines parts up on one sheet: it never moves a part to
            // another.
            if let Some((p, s, _)) = lines.iter().find(|(_, s, _)| *s != s0) {
                let sheet = |id: &str| {
                    layout
                        .sheets
                        .get(id)
                        .map_or(id.to_owned(), |x| x.name.clone())
                };
                return Err(format!(
                    "{} is on sheet {}; align lines parts up on one sheet",
                    name(*p),
                    sheet(s)
                ));
            }
            let near = |a: Nm, b: Nm| (a - b).abs() <= GRID / 2;
            // A line of the first part every other part stands on.
            let shared = l0.iter().any(|v| {
                lines
                    .iter()
                    .all(|(_, s, l)| *s == s0 && l.iter().any(|w| near(*v, *w)))
            });
            if shared {
                return Ok(());
            }
            let off = lines
                .iter()
                .filter(|(_, _, l)| !l0.iter().any(|v| l.iter().any(|w| near(*v, *w))))
                .map(|(p, _, l)| {
                    let d = l0
                        .iter()
                        .flat_map(|v| l.iter().map(move |w| (v - w).abs()))
                        .min()
                        .unwrap_or(0);
                    format!("{} is {} off", name(*p), mm(d))
                })
                .collect::<Vec<_>>();
            Err(format!(
                "align judges body middles (and a two-pin part's axis), as sch_out_of_line does: {} {}'s {}",
                off.join(", "),
                name(first),
                if *axis == Axis::Row { "row" } else { "column" }
            ))
        }
        Instruction::Group { .. } => {
            // One connected cluster within the reach.
            let mut boxes = Vec::new();
            for &p in &parts {
                boxes.push((p, placed(p)?));
            }
            let mut joined = vec![false; boxes.len()];
            if !boxes.is_empty() {
                joined[0] = true;
            }
            loop {
                let mut grew = false;
                for i in 0..boxes.len() {
                    if joined[i] {
                        continue;
                    }
                    if (0..boxes.len()).any(|j| {
                        joined[j]
                            && boxes[j].1.0 == boxes[i].1.0
                            && gap(&boxes[j].1.1.extent, &boxes[i].1.1.extent) <= IDIOM_REACH
                    }) {
                        joined[i] = true;
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            match boxes.iter().zip(&joined).find(|(_, j)| !**j) {
                None => Ok(()),
                Some(((p, _), _)) => Err(format!(
                    "{} stands apart from the rest; a group's parts lie within {reach} of each other",
                    name(*p)
                )),
            }
        }
        Instruction::Edge { side, .. } => {
            let near = 16 * GRID;
            for &p in &parts {
                let (_, sh) = placed(p)?;
                let d = match side {
                    Side::Left => sh.extent.min[0],
                    Side::Right => block.sheet[0] - sh.extent.max[0],
                    Side::Up => block.sheet[1] - sh.extent.max[1],
                    Side::Down => sh.extent.min[1],
                };
                if d > near {
                    return Err(format!(
                        "{} is {} from the sheet's {} edge; edge means within {}",
                        name(p),
                        mm(d),
                        match side {
                            Side::Left => "left",
                            Side::Right => "right",
                            Side::Up => "top",
                            Side::Down => "bottom",
                        },
                        mm(near)
                    ));
                }
            }
            Ok(())
        }
        Instruction::Sheet { sheet, .. } => {
            for &p in &parts {
                let (s, _) = placed(p)?;
                let n = layout.sheets.get(&s).map_or(s.clone(), |x| x.name.clone());
                if &n != sheet {
                    return Err(format!("{} is on {n}", name(p)));
                }
            }
            Ok(())
        }
        Instruction::Orient {
            rotation, mirror, ..
        } => {
            for &p in &parts {
                let x = layout
                    .placed
                    .get(&block.parts[p].key)
                    .ok_or_else(|| format!("{} isn't placed", name(p)))?;
                if x.rotation != *rotation % 360 || x.mirror != *mirror {
                    return Err(format!(
                        "{} is turned {}{}",
                        name(p),
                        x.rotation,
                        if x.mirror { " mirrored" } else { "" }
                    ));
                }
            }
            Ok(())
        }
        Instruction::Net { net, style } => {
            if achieved(block, layout, ins) {
                return Ok(());
            }
            let n = block
                .net(net)
                .map_or(net.clone(), |n| block.nets[n].name.clone());
            Err(match style {
                NetStyle::Wire => format!("{n} is drawn by labels"),
                _ => format!("{n} is drawn by wires"),
            })
        }
        _ => {
            if achieved(block, layout, ins) {
                Ok(())
            } else {
                Err("it doesn't hold where the parts stand".into())
            }
        }
    }
}

fn side_word(s: Side) -> &'static str {
    match s {
        Side::Left => "left of",
        Side::Right => "right of",
        Side::Up => "above",
        Side::Down => "below",
    }
}

/// Whether a laid out instruction holds: what `describe` reports.
pub fn achieved(block: &Block, layout: &Layout, ins: &Instruction) -> bool {
    let shape = |p: usize| {
        let part = &block.parts[p];
        layout.placed.get(&part.key).map(|x| {
            (
                x.sheet.clone(),
                Shape::of(part, Place::new(x.at, x.rotation, x.mirror)),
            )
        })
    };
    let parts: Vec<usize> = ins
        .parts()
        .iter()
        .flat_map(|s| parts_named(block, s))
        .collect();
    let all = |f: &dyn Fn(usize) -> bool| !parts.is_empty() && parts.iter().all(|p| f(*p));
    match ins {
        Instruction::Sheet { sheet, .. } => all(&|p| {
            shape(p).is_some_and(|(s, _)| layout.sheets.get(&s).is_some_and(|x| &x.name == sheet))
        }),
        Instruction::Orient {
            rotation, mirror, ..
        } => all(&|p| {
            layout
                .placed
                .get(&block.parts[p].key)
                .is_some_and(|x| x.rotation == *rotation % 360 && x.mirror == *mirror)
        }),
        Instruction::Beside { of, side, .. } => {
            let Some(o) = parts_named(block, of).first().copied().and_then(shape) else {
                return false;
            };
            all(&|p| {
                shape(p).is_some_and(|(s, sh)| {
                    s == o.0
                        && match side {
                            Side::Left => sh.extent.max[0] <= o.1.extent.min[0],
                            Side::Right => sh.extent.min[0] >= o.1.extent.max[0],
                            Side::Up => sh.extent.min[1] >= o.1.extent.max[1],
                            Side::Down => sh.extent.max[1] <= o.1.extent.min[1],
                        }
                })
            })
        }
        Instruction::Near { to, .. } => {
            let target = pin_named(block, to)
                .map(|(p, _)| p)
                .or_else(|| parts_named(block, to).first().copied());
            let Some(t) = target.and_then(shape) else {
                return false;
            };
            all(&|p| {
                shape(p).is_some_and(|(s, sh)| {
                    s == t.0 && sh.extent.grow(10 * crate::geom::GRID).overlaps(&t.1.extent)
                })
            })
        }
        Instruction::Edge { side, .. } => all(&|p| {
            shape(p).is_some_and(|(_, sh)| {
                let near = 16 * crate::geom::GRID;
                match side {
                    Side::Left => sh.extent.min[0] <= near,
                    Side::Right => sh.extent.max[0] >= block.sheet[0] - near,
                    Side::Up => sh.extent.max[1] >= block.sheet[1] - near,
                    Side::Down => sh.extent.min[1] <= near,
                }
            })
        }),
        Instruction::Align {
            axis,
            parts: subjects,
        } => {
            // Pins line up exactly, by their ends; bodies by their middles,
            // within a step.
            let pins: Vec<(usize, usize)> = subjects
                .iter()
                .filter_map(|s| pin_named(block, s))
                .collect();
            let on = |v: Point| if *axis == Axis::Row { v[1] } else { v[0] };
            if !pins.is_empty() {
                let ends: Vec<i64> = pins
                    .iter()
                    .filter_map(|(p, k)| shape(*p).map(|(_, sh)| on(sh.pins[*k].end)))
                    .collect();
                return ends.len() == pins.len() && ends.windows(2).all(|w| w[0] == w[1]);
            }
            let centres: Vec<i64> = parts
                .iter()
                .filter_map(|p| shape(*p))
                .map(|(_, sh)| on(sh.body.center()))
                .collect();
            centres.len() == parts.len()
                && centres
                    .windows(2)
                    .all(|w| (w[0] - w[1]).abs() <= crate::geom::GRID)
        }
        // A move: done once it is applied.
        Instruction::Shift { .. } => true,
        Instruction::Group { .. } => {
            let boxes: Vec<_> = parts.iter().filter_map(|p| shape(*p)).collect();
            boxes.len() == parts.len()
                && boxes.windows(2).all(|w| {
                    w[0].0 == w[1].0
                        && w[0]
                            .1
                            .extent
                            .grow(8 * crate::geom::GRID)
                            .overlaps(&w[1].1.extent)
                })
        }
        Instruction::Net { net, style } => {
            let labelled = layout.labels.contains_key(net);
            let wired = layout.routes.contains_key(net) && !labelled;
            match style {
                NetStyle::Wire => wired,
                NetStyle::Label | NetStyle::Port => labelled,
            }
        }
        Instruction::Idiom { .. } | Instruction::Flow { .. } => true,
        // plan 2.A Task 7 (regions) and Task 16 (paper).
        Instruction::Region { .. } | Instruction::RegionNone { .. } | Instruction::Paper { .. } => {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::seeded;

    #[test]
    fn subjects_resolve_and_conflicts_name_each_other() {
        let mut b = seeded(1, 2);
        b.instructions.insert(
            "i1".into(),
            Instruction::Edge {
                parts: vec!["u0".into()],
                side: Side::Right,
            },
        );
        b.instructions.insert(
            "i2".into(),
            Instruction::Sheet {
                parts: vec!["gone".into()],
                sheet: "Power".into(),
            },
        );
        let r = resolve(&b).unwrap();
        assert_eq!(r.edge.len(), 1);
        assert_eq!(r.notes[0].code, "instruction_unresolved");
        b.moves.push(Instruction::Beside {
            parts: vec!["u0".into()],
            of: "u1".into(),
            side: Side::Left,
        });
        let e = resolve(&b).unwrap_err();
        assert_eq!(e.code, "instruction_conflict");
        assert_eq!(e.details["instructions"], json!(["i1", "move 1"]));
        b.moves = vec![Instruction::Net {
            net: "in0_0".into(),
            style: NetStyle::Port,
        }];
        assert_eq!(resolve(&b).unwrap_err().code, "layout_rule");
    }

    #[test]
    fn judge_measures_as_the_lint_does() {
        use crate::geom::GRID;
        use crate::input::{NetKind, PinKind};
        use crate::layout::{Instruction, Placed};
        use crate::testing::{ic, net, resistor};
        let block = Block {
            nets: vec![
                net("rail:v", "3V3", NetKind::Supply),
                net("a", "A", NetKind::Signal),
            ],
            parts: vec![
                ic(
                    "u1",
                    "U1",
                    "x",
                    &[("VDD", PinKind::PowerIn, Some(0))],
                    &[("A", PinKind::Output, Some(1))],
                ),
                resistor("r1", "R1", "1k", [Some(1), Some(0)]),
            ],
            sheet: [400_000_000, 277_000_000],
            ..Block::default()
        };
        let mut layout = Layout::default();
        layout.sheets.insert(
            "s1".into(),
            crate::layout::Sheet::new("Sheet 1".into(), [0, 0]),
        );
        let put = |l: &mut Layout, k: &str, at: [i64; 2]| {
            l.placed.insert(
                k.into(),
                Placed {
                    sheet: "s1".into(),
                    at,
                    rotation: 0,
                    mirror: false,
                    held: false,
                },
            );
        };
        put(&mut layout, "u1", [100 * GRID, 100 * GRID]);
        // R1 above U1, far: on the side but out of reach.
        put(&mut layout, "r1", [100 * GRID, 180 * GRID]);
        let beside = Instruction::Beside {
            parts: vec!["r1".into()],
            of: "u1".into(),
            side: Side::Up,
        };
        assert!(
            achieved(&block, &layout, &beside),
            "the engine's test: the side"
        );
        let why = judge(&block, &layout, &beside).unwrap_err();
        assert!(why.contains("beside means within"), "{why}");
        // Close: it holds.
        put(&mut layout, "r1", [100 * GRID, 110 * GRID]);
        assert_eq!(judge(&block, &layout, &beside), Ok(()));
        // align judges body middles: off by a step it doesn't hold.
        let align = Instruction::Align {
            parts: vec!["u1".into(), "r1".into()],
            axis: Axis::Column,
        };
        let r = judge(&block, &layout, &align);
        let x = |k: &str| {
            let p = block.part(k).unwrap();
            let pl = &layout.placed[k];
            Shape::of(&block.parts[p], Place::new(pl.at, pl.rotation, pl.mirror))
                .body
                .center()[0]
        };
        assert_eq!(r.is_ok(), (x("u1") - x("r1")).abs() <= GRID / 2, "{r:?}");
    }
}
