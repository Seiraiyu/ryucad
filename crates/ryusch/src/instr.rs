//! Instructions, resolved against the block: what each says of which
//! parts and nets, those whose subject is gone (noted, kept until
//! released), and those that can't hold together (refused, naming each
//! other). Lasting instructions and one-off moves resolve alike; what a
//! move places is held.

use crate::engine::Note;
use crate::geom::{Place, Side};
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
    pub align: Vec<(Vec<usize>, Axis)>,
    pub groups: Vec<Vec<usize>>,
    pub styles: BTreeMap<usize, NetStyle>,
    pub idiom: BTreeMap<usize, IdiomChoice>,
    pub flow: BTreeSet<usize>,
    /// Parts a move of this batch places: held where they go.
    pub moved: BTreeSet<usize>,
    pub notes: Vec<Note>,
}

/// The parts a subject names: the part keyed so, or every unit of the
/// component keyed so.
pub fn parts_named(block: &Block, subject: &str) -> Vec<usize> {
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
            return Err(conflict(
                &other,
                id,
                &format!(
                    "they say different things of {}'s {what}",
                    block.parts[p].key
                ),
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
            Instruction::Align { axis, .. } => {
                if parts.len() >= 2 {
                    r.align.push((parts.clone(), *axis));
                }
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
        Instruction::Align { axis, .. } => {
            let centres: Vec<i64> = parts
                .iter()
                .filter_map(|p| shape(*p))
                .map(|(_, sh)| {
                    let c = sh.body.center();
                    if *axis == Axis::Row { c[1] } else { c[0] }
                })
                .collect();
            centres.len() == parts.len()
                && centres
                    .windows(2)
                    .all(|w| (w[0] - w[1]).abs() <= crate::geom::GRID)
        }
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
}
