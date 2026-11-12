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
    /// How each bus is asked to be drawn, by bus index (plan 2.C).
    pub bus_styles: BTreeMap<usize, NetStyle>,
    pub idiom: BTreeMap<usize, IdiomChoice>,
    pub flow: BTreeSet<usize>,
    /// Parts a move of this batch places: held where they go.
    pub moved: BTreeSet<usize>,
    /// Each lasting `region` instruction's title and parts (a cluster's
    /// whole membership following any member), in instruction ID order.
    pub regions: Vec<(String, BTreeSet<usize>)>,
    /// Parts a `region none` instruction leaves to the proposal.
    pub no_region: BTreeSet<usize>,
    /// Networks `near` keeps together off a pin: their parts and the pin.
    pub networks: Vec<(Vec<usize>, (usize, usize))>,
    /// Port flags `near` stands on a pin's row: the flag and the pin.
    pub port_rows: BTreeMap<usize, (usize, usize)>,
    /// Each part's text fields and the side each goes on.
    pub text: BTreeMap<usize, Vec<(crate::layout::TextField, Side)>>,
    /// Each `paper` instruction's paper, by sheet ID or name: pinned.
    pub papers: BTreeMap<String, (String, Option<crate::layout::Orientation>)>,
    pub notes: Vec<Note>,
}

/// The parts a subject names: the part keyed so, or every unit of the
/// component keyed so, or a pin's part (`<part key>/<number>`).
pub fn parts_named(block: &Block, subject: &str) -> Vec<usize> {
    // A region of the previous layout (`region:<id>`): its parts.
    if let Some(id) = subject.strip_prefix("region:") {
        let mut out: Vec<usize> = block
            .previous
            .as_ref()
            .and_then(|l| l.regions.get(id))
            .map(|r| r.parts.iter().filter_map(|k| block.part(k)).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
        return out;
    }
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
    // Each part's `region` instruction, and `region none`'s.
    let mut region_by: BTreeMap<usize, String> = BTreeMap::new();
    let mut none_by: BTreeMap<usize, String> = BTreeMap::new();
    // Each region's title by ID, and the instruction giving it (plan 3.A).
    let mut titled: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut by_paper: BTreeMap<String, String> = BTreeMap::new();
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
        let mut gone_regions = Vec::new();
        for s in ins.parts() {
            let found = parts_named(block, s);
            if found.is_empty() {
                match s.strip_prefix("region:") {
                    Some(region) => gone_regions.push(region.to_owned()),
                    None => missing.push(s.clone()),
                }
            }
            parts.extend(found);
        }
        if !gone_regions.is_empty() {
            for g in &gone_regions {
                r.notes.push(Note {
                    code: "region_unresolved".into(),
                    message: format!("region {g} is gone: the instruction is kept"),
                    details: json!({"instruction": id, "region": g}),
                });
            }
            continue;
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
            Instruction::Net { net, .. }
                if block.net(net).is_none() && block.bus(net).is_none() =>
            {
                missing.push(net.clone())
            }
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
            Instruction::Title { region, title } => {
                if title.trim().is_empty()
                    || title.chars().count() > crate::layout::TITLE_MAX
                    || title.contains(['\n', '\r'])
                {
                    return Err(rule(
                        &id,
                        "region_title",
                        format!(
                            "a region's title is one line of 1 to {} characters",
                            crate::layout::TITLE_MAX
                        ),
                    ));
                }
                match titled.get(region) {
                    Some((other, t)) if t != title => {
                        return Err(conflict(
                            other,
                            &id,
                            &format!("region {region} can't have two titles"),
                        ));
                    }
                    Some(_) => {}
                    None => {
                        titled.insert(region.clone(), (id.clone(), title.clone()));
                    }
                }
            }
            Instruction::Note { on, text } => {
                if text.trim().is_empty()
                    || text.chars().count() > crate::layout::NOTE_MAX
                    || text.contains(['\n', '\r'])
                {
                    return Err(rule(
                        &id,
                        "note",
                        format!(
                            "a note is one line of 1 to {} characters",
                            crate::layout::NOTE_MAX
                        ),
                    ));
                }
                // On a region or a sheet the previous layout has (from
                // nothing every region is new: the note waits for its
                // region's ID, kept).
                if let Some(prev) = &block.previous {
                    let known = match on.strip_prefix("region:") {
                        Some(r) => prev.regions.contains_key(r),
                        None => {
                            prev.sheets.contains_key(on)
                                || prev.sheets.values().any(|s| &s.name == on)
                        }
                    };
                    if !known {
                        r.notes.push(Note {
                            code: "instruction_unresolved".into(),
                            message: format!(
                                "{id} names {on} which the layout no longer has; it is kept until released"
                            ),
                            details: json!({"instruction": id, "missing": [on]}),
                        });
                    }
                }
            }
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
                match target {
                    // Several parts near a pin of a part of three pins or
                    // more: one network off it.
                    (p, Some(k)) if parts.len() >= 2 && block.parts[p].pins.len() >= 3 => {
                        r.networks.push((parts.clone(), (p, k)))
                    }
                    // A port's flag near a pin: on its row.
                    (p, Some(k))
                        if matches!(
                            block.parts[parts[0]].kind,
                            crate::input::PartKind::Port { .. }
                        ) =>
                    {
                        set(&mut r.near, &mut by, "place", &parts, target, &id, block)?;
                        r.near.remove(&parts[0]);
                        r.port_rows.insert(parts[0], (p, k));
                    }
                    _ => set(&mut r.near, &mut by, "place", &parts, target, &id, block)?,
                }
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
            Instruction::Region { title, .. } => {
                for &p in &parts {
                    if let Some(other) = region_by.get(&p).or(none_by.get(&p)) {
                        return Err(Refusal {
                            code: "instruction_conflict",
                            message: format!(
                                "{} is in two regions: {other} and {id}",
                                block.parts[p].key
                            ),
                            details: json!({"instructions": [other, id]}),
                        });
                    }
                    region_by.insert(p, id.clone());
                }
                r.regions
                    .push((title.clone(), parts.iter().copied().collect()));
            }
            Instruction::RegionNone { .. } => {
                for &p in &parts {
                    if let Some(other) = region_by.get(&p) {
                        return Err(Refusal {
                            code: "instruction_conflict",
                            message: format!(
                                "{} is in two regions: {other} and {id}",
                                block.parts[p].key
                            ),
                            details: json!({"instructions": [other, id]}),
                        });
                    }
                    none_by.insert(p, id.clone());
                }
                r.no_region.extend(parts.iter().copied());
            }
            Instruction::Paper {
                sheet,
                size,
                orientation,
            } => {
                let sizes: Vec<&str> = block.papers.iter().map(|p| p.size.as_str()).collect();
                if !sizes.is_empty() && !sizes.contains(&size.as_str()) {
                    return Err(Refusal {
                        code: "layout_rule",
                        message: format!(
                            "paper {size} isn't one of this project's sizes: {}",
                            sizes.join(", ")
                        ),
                        details: json!({"instructions": [id], "rule": "paper", "sizes": sizes}),
                    });
                }
                let v = (size.clone(), *orientation);
                if let Some(old) = r.papers.get(sheet)
                    && *old != v
                {
                    let other = by_paper[sheet].clone();
                    return Err(conflict(
                        &other,
                        &id,
                        &format!("they say different things of sheet {sheet}'s paper"),
                    ));
                }
                by_paper.insert(sheet.clone(), id.clone());
                r.papers.insert(sheet.clone(), v);
            }
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
            Instruction::Net { net, style } if block.net(net).is_none() => {
                let b = block.bus(net).expect("resolved");
                if !matches!(style, NetStyle::Bus | NetStyle::Label) {
                    return Err(rule(
                        &id,
                        "bus",
                        format!("{net} is a bus: it is drawn as one (bus) or by labels (label)"),
                    ));
                }
                r.bus_styles.insert(b, *style);
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
            Instruction::Text { field, side, .. } => {
                for &p in &parts {
                    let list = r.text.entry(p).or_default();
                    for (f, s) in list.iter() {
                        let overlaps = *f == *field
                            || *f == crate::layout::TextField::Both
                            || *field == crate::layout::TextField::Both;
                        if overlaps && s != side {
                            let other = by.get(&("text", p)).cloned().unwrap_or_else(|| id.clone());
                            return Err(conflict(
                                &other,
                                &id,
                                &format!("both say where {}'s text goes", block.parts[p].key),
                            ));
                        }
                    }
                    list.push((*field, *side));
                    by.insert(("text", p), id.clone());
                }
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
    // An amplifier turned 0 or 180 degrees keeps its op-amp stage, whose
    // template follows its inverting pin's side; at 90 or 270 the stage
    // is drawn as separate parts.
    let amplifier = |p: usize| {
        let pins = &block.parts[p].pins;
        pins.iter().any(crate::topology::out_pin) && pins.iter().any(crate::topology::inv_pin)
    };
    for p in grouped.keys() {
        r.idiom.entry(*p).or_insert(IdiomChoice::None);
    }
    for (p, (rotation, _)) in &r.orient {
        if amplifier(*p) && !grouped.contains_key(p) {
            if rotation % 180 == 0 {
                continue;
            }
            r.notes.push(Note {
                code: "orient_drops_stage".into(),
                message: format!(
                    "{} turned {rotation}°: its op-amp stage is drawn as separate parts; turn it 0 or 180 to keep the stage",
                    crate::lint::who(&block.parts[*p])
                ),
                details: json!({"part": block.parts[*p].key, "rotation": rotation}),
            });
        }
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
    let shape = |p: usize| shape_of(block, layout, p);
    let name = |p: usize| who(&block.parts[p]);
    let parts = subjects(block, ins);
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
                if !on_side(&sh.extent, &ob.extent, *side) {
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
            // A network off a pin stands together: each part within reach
            // of the pin's part or of another part of it that is.
            if parts.len() >= 2 && pin_named(block, to).is_some() {
                let mut near: Vec<Rect> = vec![tb.extent];
                let mut left: Vec<usize> = parts.clone();
                loop {
                    let before = left.len();
                    let mut next = Vec::new();
                    for &p in &left {
                        let (s, sh) = placed(p)?;
                        if s == ts && near.iter().any(|r| gap(&sh.extent, r) <= IDIOM_REACH) {
                            near.push(sh.extent);
                        } else {
                            next.push(p);
                        }
                    }
                    left = next;
                    if left.is_empty() || left.len() == before {
                        break;
                    }
                }
                return match left.first() {
                    None => Ok(()),
                    Some(&p) => Err(format!(
                        "{} isn't within {reach} of {} or the rest of the network",
                        name(p),
                        name(t)
                    )),
                };
            }
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
            let near = IDIOM_REACH;
            for &p in &parts {
                let (sid, sh) = placed(p)?;
                let d = edge_distance(block, layout, &sid, &sh.extent, *side);
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
        Instruction::Title { region, title } => match layout.regions.get(region) {
            Some(g) if g.title == *title => Ok(()),
            Some(g) => Err(format!("region {region} is titled {:?}", g.title)),
            None => Err(format!("there is no region {region}")),
        },
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
                _ if block.bus(net).is_some() => match style {
                    NetStyle::Bus => {
                        format!("{net} is drawn by labels: it can't be drawn as a bus")
                    }
                    _ => format!("{net} is drawn as a bus"),
                },
                NetStyle::Wire => format!("{n} is drawn by labels"),
                NetStyle::Bus => format!("{n} isn't a bus: only a bus is drawn as one"),
                _ => format!("{n} is drawn by wires"),
            })
        }
        Instruction::Text { field, side, .. } => {
            for &p in &parts {
                for (n, at) in text_sides(block, layout, p, *field) {
                    if at != Some(*side) {
                        let place = match at {
                            Some(s) => side_word(s),
                            None => "over",
                        };
                        return Err(format!("{}'s {n} is {place} its body", name(p)));
                    }
                }
            }
            Ok(())
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

/// A part's sheet and shape as `layout` places it.
fn shape_of(block: &Block, layout: &Layout, p: usize) -> Option<(String, Shape)> {
    let part = &block.parts[p];
    layout.placed.get(&part.key).map(|x| {
        (
            x.sheet.clone(),
            Shape::of(part, Place::new(x.at, x.rotation, x.mirror)),
        )
    })
}

/// The parts an instruction names, its `region:` subjects expanded.
fn subjects(block: &Block, ins: &Instruction) -> Vec<usize> {
    ins.parts()
        .iter()
        .flat_map(|s| parts_named(block, s))
        .collect()
}

/// The gap between two boxes, 0 where they overlap.
fn gap(a: &crate::geom::Rect, b: &crate::geom::Rect) -> crate::geom::Nm {
    let dx = (b.min[0] - a.max[0]).max(a.min[0] - b.max[0]).max(0);
    let dy = (b.min[1] - a.max[1]).max(a.min[1] - b.max[1]).max(0);
    dx.max(dy)
}

/// Whether box `a` stands wholly on `side` of box `b`.
fn on_side(a: &crate::geom::Rect, b: &crate::geom::Rect, side: Side) -> bool {
    match side {
        Side::Left => a.max[0] <= b.min[0],
        Side::Right => a.min[0] >= b.max[0],
        Side::Up => a.min[1] >= b.max[1],
        Side::Down => a.max[1] <= b.min[1],
    }
}

/// How far box `r` on sheet `sid` stands from that sheet's `side` edge.
fn edge_distance(
    block: &Block,
    layout: &Layout,
    sid: &str,
    r: &crate::geom::Rect,
    side: Side,
) -> crate::geom::Nm {
    let size = block.area_in(layout, sid);
    match side {
        Side::Left => r.min[0],
        Side::Right => size[0] - r.max[0],
        Side::Up => size[1] - r.max[1],
        Side::Down => r.min[1],
    }
}

/// What an instruction did, as reports say it (design 519).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Held,
    /// Not held, but its parts stand nearer what it asks than before it:
    /// why, with how far is left.
    Pulled(String),
    NotHeld(String),
}

impl Verdict {
    /// `held`, `pulled` or `not_held`.
    pub fn state(&self) -> &'static str {
        match self {
            Verdict::Held => "held",
            Verdict::Pulled(_) => "pulled",
            Verdict::NotHeld(_) => "not_held",
        }
    }

    /// Why it doesn't hold.
    pub fn why(&self) -> Option<&str> {
        match self {
            Verdict::Held => None,
            Verdict::Pulled(w) | Verdict::NotHeld(w) => Some(w),
        }
    }
}

/// How far `ins`'s parts stand past what it asks, by `judge`'s boxes and
/// the idiom reach (0 when it holds by distance); `None` for an
/// instruction not judged by distance (`align`, `sheet`, …) or whose
/// target isn't placed. A part on another sheet, or on the wrong side for
/// `beside`, counts as far.
pub fn shortfall(block: &Block, layout: &Layout, ins: &Instruction) -> Option<crate::geom::Nm> {
    use crate::geom::{Nm, Rect};
    use crate::lint::IDIOM_REACH;
    const FAR: Nm = Nm::MAX / 4;
    let past = |g: Nm| {
        if g >= FAR {
            FAR
        } else {
            (g - IDIOM_REACH).max(0)
        }
    };
    let parts = subjects(block, ins);
    let boxed = |p: usize| shape_of(block, layout, p).map(|(s, sh)| (s, sh.extent));
    // The widest gap a chain from `start` needs to reach every one of
    // `boxes` (the bottleneck of their spanning tree), as `judge` joins a
    // group or a network.
    let chain = |start: Vec<(String, Rect)>, rest: Vec<Option<(String, Rect)>>| -> Nm {
        let mut joined = start;
        let mut left = rest;
        let mut worst = 0;
        while !left.is_empty() {
            let (i, g) = left
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let g = match b {
                        Some((s, r)) => joined
                            .iter()
                            .filter(|(t, _)| t == s)
                            .map(|(_, q)| gap(r, q))
                            .min()
                            .unwrap_or(FAR),
                        None => FAR,
                    };
                    (i, g)
                })
                .min_by_key(|(i, g)| (*g, *i))
                .expect("some left");
            worst = worst.max(g);
            if let Some(b) = left.remove(i) {
                joined.push(b);
            }
        }
        worst
    };
    match ins {
        Instruction::Beside { of, side, .. } => {
            let (os, ob) = boxed(*parts_named(block, of).first()?)?;
            Some(
                parts
                    .iter()
                    .map(|&p| match boxed(p) {
                        Some((s, b)) if s == os && on_side(&b, &ob, *side) => past(gap(&b, &ob)),
                        _ => FAR,
                    })
                    .max()
                    .unwrap_or(0),
            )
        }
        Instruction::Near { to, .. } => {
            let pin = pin_named(block, to);
            let t = pin
                .map(|(p, _)| p)
                .or_else(|| parts_named(block, to).first().copied())?;
            let target = boxed(t)?;
            if parts.len() >= 2 && pin.is_some() {
                let g = chain(vec![target], parts.iter().map(|&p| boxed(p)).collect());
                return Some(past(g));
            }
            Some(
                parts
                    .iter()
                    .map(|&p| match boxed(p) {
                        Some((s, b)) if s == target.0 => past(gap(&b, &target.1)),
                        _ => FAR,
                    })
                    .max()
                    .unwrap_or(0),
            )
        }
        Instruction::Group { .. } => {
            let mut all: Vec<Option<(String, Rect)>> = parts.iter().map(|&p| boxed(p)).collect();
            if all.is_empty() {
                return Some(0);
            }
            let first = all.remove(0);
            Some(match first {
                Some(b) => past(chain(vec![b], all)),
                None => FAR,
            })
        }
        Instruction::Edge { side, .. } => Some(
            parts
                .iter()
                .map(|&p| match boxed(p) {
                    Some((s, b)) => past(edge_distance(block, layout, &s, &b, *side)),
                    None => FAR,
                })
                .max()
                .unwrap_or(0),
        ),
        _ => None,
    }
}

/// `judge`'s verdict on instruction `id`, `pulled` read against the
/// places `Layout::before` keeps for it.
pub fn verdict(block: &Block, layout: &Layout, id: &str, ins: &Instruction) -> Verdict {
    let why = match judge(block, layout, ins) {
        Ok(()) => return Verdict::Held,
        Err(why) => why,
    };
    let was = layout
        .before
        .get(id)
        .filter(|m| !m.is_empty())
        .map(|places| {
            let mut l = layout.clone();
            for (k, p) in places {
                l.placed.insert(k.clone(), p.clone());
            }
            l
        });
    match (
        was.and_then(|l| shortfall(block, &l, ins)),
        shortfall(block, layout, ins),
    ) {
        (Some(then), Some(now)) if now < then => {
            let mm = |v: crate::geom::Nm| format!("{:.1} mm", v as f64 / 1e6);
            // Before it the parts were on another sheet or the wrong side.
            let nearer = if then >= crate::geom::Nm::MAX / 4 {
                "nearer than before it".to_owned()
            } else {
                format!("{} nearer than before it", mm(then - now))
            };
            Verdict::Pulled(format!("{why}; {nearer}, {} short", mm(now)))
        }
        _ => Verdict::NotHeld(why),
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
        // Applied after the parts are placed, by ID.
        Instruction::Title { .. } => true,
        Instruction::Note { .. } => layout
            .instructions
            .iter()
            .any(|(id, i)| i == ins && layout.notes.contains_key(id)),
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
        // Judged as the reports judge them (design 519): one test of
        // holding, so the engine never leaves alone what the report calls
        // not held.
        Instruction::Beside { .. } | Instruction::Group { .. } | Instruction::Edge { .. } => {
            !parts.is_empty() && judge(block, layout, ins).is_ok()
        }
        Instruction::Near { to, .. } if parts.len() == 1 || pin_named(block, to).is_none() => {
            !parts.is_empty() && judge(block, layout, ins).is_ok()
        }
        Instruction::Near { to, .. } => {
            let target = pin_named(block, to)
                .map(|(p, _)| p)
                .or_else(|| parts_named(block, to).first().copied());
            let Some(t) = target.and_then(shape) else {
                return false;
            };
            // A network off a pin: each part near the pin's part or another
            // part of it that is.
            let mut near: Vec<crate::geom::Rect> = vec![t.1.extent];
            let mut left: Vec<usize> = parts.clone();
            loop {
                let before = left.len();
                left.retain(|p| match shape(*p) {
                    Some((s, sh))
                        if s == t.0
                            && near
                                .iter()
                                .any(|r| sh.extent.grow(10 * crate::geom::GRID).overlaps(r)) =>
                    {
                        near.push(sh.extent);
                        false
                    }
                    _ => true,
                });
                if left.is_empty() || left.len() == before {
                    break;
                }
            }
            left.is_empty()
        }
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
        Instruction::Net { net, style } if block.bus(net).is_some() => match style {
            NetStyle::Bus => layout.buses.get(net).is_some_and(|b| !b.is_empty()),
            _ => !layout.buses.contains_key(net),
        },
        Instruction::Net { net, style } => {
            let labelled = layout.labels.contains_key(net);
            let wired = layout.routes.contains_key(net) && !labelled;
            match style {
                NetStyle::Wire => wired,
                NetStyle::Label | NetStyle::Port => labelled,
                // Only a bus is drawn as one.
                NetStyle::Bus => false,
            }
        }
        Instruction::Idiom { .. } | Instruction::Flow { .. } => true,
        Instruction::Region { title, .. } => {
            let keys: Vec<&str> = parts.iter().map(|p| block.parts[*p].key.as_str()).collect();
            !keys.is_empty()
                && layout.regions.values().any(|g| {
                    &g.title == title && keys.iter().all(|k| g.parts.iter().any(|x| x == k))
                })
        }
        Instruction::RegionNone { .. } => parts.iter().all(|p| {
            !layout
                .regions
                .values()
                .any(|g| g.named && g.parts.contains(&block.parts[*p].key))
        }),
        Instruction::Paper { sheet, size, .. } => layout
            .sheets
            .iter()
            .find(|(id, s)| *id == sheet || &s.name == sheet)
            .is_some_and(|(_, s)| s.pinned && s.size.as_deref() == Some(size.as_str())),
        Instruction::Text { field, side, .. } => all(&|p| {
            text_sides(block, layout, p, *field)
                .iter()
                .all(|(_, at)| *at == Some(*side))
        }),
    }
}

/// Which side of part `p`'s body each of its text fields `field` names
/// lies wholly on (`None`: over it, or not placed), field by field.
pub fn text_sides(
    block: &Block,
    layout: &Layout,
    p: usize,
    field: crate::layout::TextField,
) -> Vec<(&'static str, Option<Side>)> {
    let part = &block.parts[p];
    let body = layout
        .placed
        .get(&part.key)
        .map(|x| Shape::of(part, Place::new(x.at, x.rotation, x.mirror)).body);
    let texts = layout.text.get(&part.key);
    part.fields()
        .into_iter()
        .filter(|(n, l)| field.covers(n) && !l.text.is_empty())
        .map(|(n, l)| {
            let side = match (body, texts.and_then(|f| f.get(n))) {
                (Some(body), Some(at)) => {
                    let b = l.boxed(*at);
                    if b.max[0] <= body.min[0] {
                        Some(Side::Left)
                    } else if b.min[0] >= body.max[0] {
                        Some(Side::Right)
                    } else if b.min[1] >= body.max[1] {
                        Some(Side::Up)
                    } else if b.max[1] <= body.min[1] {
                        Some(Side::Down)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            (n, side)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two `text` instructions saying different sides for one part's field
    /// conflict; `text` holds where the field's box lies on that side.
    #[test]
    fn text_instructions_resolve_and_conflict() {
        use crate::layout::TextField;
        let mut block = crate::testing::seeded(3, 1);
        let key = block.parts[1].key.clone();
        block.instructions.insert(
            "i1".into(),
            Instruction::Text {
                parts: vec![key.clone()],
                field: TextField::Refdes,
                side: Side::Left,
            },
        );
        let r = resolve(&block).unwrap();
        assert_eq!(r.text.get(&1), Some(&vec![(TextField::Refdes, Side::Left)]));
        block.instructions.insert(
            "i2".into(),
            Instruction::Text {
                parts: vec![key],
                field: TextField::Both,
                side: Side::Right,
            },
        );
        let e = resolve(&block).unwrap_err();
        assert_eq!(e.code, "instruction_conflict", "{e:?}");
    }
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

    /// U1 at (100, 100) steps and R1 at (100, 180): above it, on the side
    /// but out of reach, with `beside r1 u1 above`.
    fn beside_far() -> (Block, Layout, Instruction) {
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
        for (k, at) in [
            ("u1", [100 * GRID, 100 * GRID]),
            ("r1", [100 * GRID, 180 * GRID]),
        ] {
            layout.placed.insert(
                k.into(),
                Placed {
                    sheet: "s1".into(),
                    at,
                    rotation: 0,
                    mirror: false,
                    held: false,
                },
            );
        }
        let beside = Instruction::Beside {
            parts: vec!["r1".into()],
            of: "u1".into(),
            side: Side::Up,
        };
        (block, layout, beside)
    }

    /// Design 519: held within the reach; pulled when not held but
    /// nearer than before it (`Layout::before`); else not held.
    #[test]
    fn a_verdict_is_held_pulled_or_not_held() {
        use crate::geom::GRID;
        let (block, mut layout, ins) = beside_far();
        let id = "i1";
        // 80 steps away, no before: not held.
        assert!(matches!(
            verdict(&block, &layout, id, &ins),
            Verdict::NotHeld(_)
        ));
        assert_eq!(verdict(&block, &layout, id, &ins).state(), "not_held");
        // It stood 120 steps away before: pulled, saying how far is left.
        let mut was = layout.placed["r1"].clone();
        was.at[1] = 220 * GRID;
        layout
            .before
            .entry(id.into())
            .or_default()
            .insert("r1".into(), was);
        let v = verdict(&block, &layout, id, &ins);
        let Verdict::Pulled(why) = &v else {
            panic!("{v:?}")
        };
        assert!(
            why.contains("beside means within 20.3 mm") && why.contains("nearer"),
            "{why}"
        );
        assert_eq!(v.state(), "pulled");
        // Within the reach: held.
        layout.placed.get_mut("r1").unwrap().at[1] = 110 * GRID;
        assert_eq!(verdict(&block, &layout, id, &ins), Verdict::Held);
        assert_eq!(shortfall(&block, &layout, &ins), Some(0));
    }

    /// The engine and the report agree (design 519): what `judge` calls
    /// not held, `achieved` doesn't call done, for every instruction
    /// judged by distance.
    #[test]
    fn the_engine_and_the_report_agree_on_holding() {
        let (block, layout, ins) = beside_far();
        assert!(judge(&block, &layout, &ins).is_err());
        assert!(
            !achieved(&block, &layout, &ins),
            "beside 80 steps away is not done"
        );
        for seed in 1..=16 {
            let b = crate::testing::seeded(seed, 2);
            let out = crate::engine::lay_out(&b).unwrap();
            let keys: Vec<String> = b.parts.iter().take(3).map(|p| p.key.clone()).collect();
            for ins in [
                Instruction::Group {
                    parts: keys.clone(),
                },
                Instruction::Near {
                    parts: keys[..1].to_vec(),
                    to: keys[2].clone(),
                },
                Instruction::Edge {
                    parts: keys[..1].to_vec(),
                    side: Side::Left,
                },
                Instruction::Beside {
                    parts: keys[..1].to_vec(),
                    of: keys[1].clone(),
                    side: Side::Right,
                },
            ] {
                assert_eq!(
                    achieved(&b, &out.layout, &ins),
                    judge(&b, &out.layout, &ins).is_ok(),
                    "seed {seed}: {ins:?}"
                );
            }
        }
    }

    #[test]
    fn judge_measures_as_the_lint_does() {
        use crate::geom::GRID;
        use crate::layout::Placed;
        let (block, mut layout, beside) = beside_far();
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
        assert!(
            !achieved(&block, &layout, &beside),
            "the engine's test is the report's (design 519)"
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

    #[test]
    fn a_note_is_one_line_of_at_most_120_characters() {
        let mut b = crate::testing::leaving_block();
        b.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: "s1".into(),
                text: "Rev A".into(),
            },
        );
        assert!(resolve(&b).is_ok());
        for text in ["".to_owned(), "x".repeat(121), "two\nlines".to_owned()] {
            b.instructions.insert(
                "i9".into(),
                Instruction::Note {
                    on: "s1".into(),
                    text,
                },
            );
            assert_eq!(resolve(&b).unwrap_err().code, "layout_rule");
        }
    }
}
