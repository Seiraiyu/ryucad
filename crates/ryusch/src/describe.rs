//! A layout described in relative terms, for an agent to read instead of
//! (or beside) a picture: each sheet's parts with their neighbours by side,
//! their idiom and the parts they line up with; each net drawn as wires or
//! labels; the instructions in force and whether each holds; the lint and
//! the score.

use crate::engine::Outcome;
use crate::geom::{GRID, Place, Rect, mm};
use crate::input::Block;
use crate::instr;
use crate::lint::{Finding, Score};
use crate::shape::Shape;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// How far a neighbour may be and still be named.
const NEAR: i64 = 40 * GRID;

/// How far past one side of a box another lies, if it does.
type Gap<'a> = &'a dyn Fn(&Rect) -> Option<i64>;

/// The nearest part to each side of `me` whose box overlaps it across that
/// side, within reach.
fn neighbours(me: &Rect, others: &[(&str, Rect)]) -> Map<String, Value> {
    let mut out = Map::new();
    let across_x = |o: &Rect| o.min[1] < me.max[1] && me.min[1] < o.max[1];
    let across_y = |o: &Rect| o.min[0] < me.max[0] && me.min[0] < o.max[0];
    let sides: [(&str, Gap); 4] = [
        ("left", &|o: &Rect| {
            (across_x(o) && o.max[0] <= me.min[0]).then(|| me.min[0] - o.max[0])
        }),
        ("right", &|o: &Rect| {
            (across_x(o) && o.min[0] >= me.max[0]).then(|| o.min[0] - me.max[0])
        }),
        ("above", &|o: &Rect| {
            (across_y(o) && o.min[1] >= me.max[1]).then(|| o.min[1] - me.max[1])
        }),
        ("below", &|o: &Rect| {
            (across_y(o) && o.max[1] <= me.min[1]).then(|| me.min[1] - o.max[1])
        }),
    ];
    for (name, gap) in sides {
        let best = others
            .iter()
            .filter_map(|(k, r)| gap(r).filter(|g| *g <= NEAR).map(|g| (g, *k)))
            .min();
        if let Some((_, k)) = best {
            out.insert(name.into(), json!(k));
        }
    }
    out
}

/// The description of a laid out block: `lint` gives its findings and
/// scores.
pub fn describe(block: &Block, out: &Outcome, findings: &[Finding], scores: &[Score]) -> Value {
    let layout = &out.layout;
    let idiom_of: BTreeMap<usize, (&str, Option<String>)> = out
        .idioms
        .iter()
        .flat_map(|i| {
            let anchor = i.anchor.map(|(p, _)| block.parts[p].key.clone());
            i.parts
                .iter()
                .map(move |p| (*p, (i.kind.as_str(), anchor.clone())))
        })
        .collect();
    let mut sheets = Vec::new();
    for (sid, sheet) in &layout.sheets {
        let placed: Vec<(usize, Shape)> = block
            .parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let x = layout.placed.get(&p.key).filter(|x| &x.sheet == sid)?;
                Some((i, Shape::of(p, Place::new(x.at, x.rotation, x.mirror))))
            })
            .collect();
        let boxes: Vec<(&str, Rect)> = placed
            .iter()
            .map(|(i, s)| (block.parts[*i].key.as_str(), s.extent))
            .collect();
        let parts: Vec<Value> = placed
            .iter()
            .map(|(i, s)| {
                let part = &block.parts[*i];
                let x = &layout.placed[&part.key];
                let others: Vec<(&str, Rect)> = boxes
                    .iter()
                    .filter(|(k, _)| *k != part.key)
                    .cloned()
                    .collect();
                let c = s.body.center();
                let line = |axis: usize| -> Vec<&str> {
                    placed
                        .iter()
                        .filter(|(j, t)| {
                            *j != *i && (t.body.center()[axis] - c[axis]).abs() <= GRID / 2
                        })
                        .map(|(j, _)| block.parts[*j].key.as_str())
                        .collect()
                };
                let mut v = json!({
                    "part": part.key,
                    "refdes": part.refdes,
                    "at_mm": [mm(x.at[0]), mm(x.at[1])],
                    "rotation": x.rotation,
                    "neighbours": neighbours(&s.extent, &others),
                });
                if x.mirror {
                    v["mirror"] = json!(true);
                }
                if x.held {
                    v["held"] = json!(true);
                }
                if !part.value.is_empty() {
                    v["value"] = json!(part.value);
                }
                if let Some((kind, anchor)) = idiom_of.get(i) {
                    v["idiom"] = json!(kind);
                    if let Some(a) = anchor {
                        v["serves"] = json!(a);
                    }
                }
                let row = line(1);
                if !row.is_empty() {
                    v["row"] = json!(row);
                }
                let column = line(0);
                if !column.is_empty() {
                    v["column"] = json!(column);
                }
                v
            })
            .collect();
        let mut nets = Vec::new();
        for net in &block.nets {
            let wired = layout
                .routes
                .get(&net.key)
                .is_some_and(|r| r.contains_key(sid));
            let labels = layout
                .labels
                .get(&net.key)
                .map_or(0, |l| l.iter().filter(|x| &x.sheet == sid).count());
            if !wired && labels == 0 {
                continue;
            }
            let fell_back = out.fallbacks.get(sid).is_some_and(|f| f.contains(&net.key));
            let mut v = json!({
                "net": net.key,
                "name": net.name,
                "drawn": if labels > 0 { if net.kind == crate::input::NetKind::Signal { "labels" } else { "ports" } } else { "wires" },
            });
            if labels > 0 {
                v["labels"] = json!(labels);
            }
            if fell_back {
                v["why"] = json!("its wires would run too long or cross too many");
            }
            nets.push(v);
        }
        let score = scores.iter().find(|s| &s.sheet == sid);
        sheets.push(json!({
            "sheet": sid,
            "name": sheet.name,
            "score": score.map(|s| s.score),
            "measures": score,
            "parts": parts,
            "nets": nets,
            "findings": findings.iter().filter(|f| &f.sheet == sid).collect::<Vec<_>>(),
        }));
    }
    let instructions: Vec<Value> = block
        .instructions
        .iter()
        .map(|(id, i)| {
            json!({
                "id": id,
                "instruction": i,
                "holds": instr::achieved(block, layout, i),
            })
        })
        .collect();
    let idioms: Vec<Value> = out
        .idioms
        .iter()
        .map(|i| {
            let mut v = json!({
                "kind": i.kind.as_str(),
                "parts": i.parts.iter().map(|p| &block.parts[*p].key).collect::<Vec<_>>(),
            });
            if let Some((p, k)) = i.anchor {
                v["serves"] = json!(format!(
                    "{}/{}",
                    block.parts[p].key, block.parts[p].pins[k].number
                ));
            }
            if let Some(n) = &i.note {
                v["note"] = json!(n);
            }
            v
        })
        .collect();
    json!({
        "block": block.name,
        "sheets": sheets,
        "idioms": idioms,
        "instructions": instructions,
        "notes": out.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::seeded;

    #[test]
    fn a_description_names_neighbours_idioms_and_instructions() {
        let mut b = seeded(1, 2);
        b.instructions.insert(
            "i1".into(),
            crate::layout::Instruction::Beside {
                parts: vec!["u1".into()],
                of: "u0".into(),
                side: crate::geom::Side::Down,
            },
        );
        let out = crate::engine::lay_out(&b).unwrap();
        let (f, s) = crate::lint::lint(&b, &out);
        let d = describe(&b, &out, &f, &s);
        assert_eq!(d["instructions"][0]["holds"], json!(true));
        let parts = d["sheets"][0]["parts"].as_array().unwrap();
        let u1 = parts.iter().find(|p| p["part"] == "u1").unwrap();
        // Under U0's own decoupling capacitor, which sits under U0.
        assert_eq!(u1["neighbours"]["above"], json!("c0"));
        let c0 = parts.iter().find(|p| p["part"] == "c0").unwrap();
        assert_eq!(
            (&c0["idiom"], &c0["serves"]),
            (&json!("decoupling"), &json!("u0"))
        );
        assert_eq!(
            d,
            describe(&b, &crate::engine::lay_out(&b).unwrap(), &f, &s)
        );
    }
}
