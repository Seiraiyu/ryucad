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
use std::collections::{BTreeMap, BTreeSet};

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
    let region_of = layout.region_of();
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
                // Where it stands and where its connected pins end, so an
                // instruction can be aimed without reading symbols.
                v["box_mm"] = json!([
                    mm(s.body.min[0]),
                    mm(s.body.min[1]),
                    mm(s.body.max[0]),
                    mm(s.body.max[1])
                ]);
                let pins: serde_json::Map<String, Value> = part
                    .pins
                    .iter()
                    .zip(&s.pins)
                    .filter(|(p, _)| p.net.is_some())
                    .map(|(p, e)| {
                        (
                            p.number.clone(),
                            json!({
                                "name": p.name,
                                "at_mm": [mm(e.end[0]), mm(e.end[1])],
                                "side": e.side.as_str(),
                            }),
                        )
                    })
                    .collect();
                if !pins.is_empty() {
                    v["pins"] = Value::Object(pins);
                }
                // A port by its name, as instructions may write it.
                if matches!(part.kind, crate::input::PartKind::Port { .. }) {
                    v["name"] = json!(crate::lint::part_shown(block, *i));
                }
                if x.held {
                    v["held"] = json!(true);
                }
                v["region"] = json!(region_of.get(part.key.as_str()));
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
        // Buses drawn here (plan 2.C): each with its name, members, ends
        // and connector; its members' nets say so.
        let mut buses: Vec<Value> = Vec::new();
        let mut bus_of: BTreeMap<&str, String> = BTreeMap::new();
        for (key, by) in &layout.buses {
            let Some(d) = by.get(sid) else { continue };
            let mut members: Vec<usize> = Vec::new();
            let mut ends: Vec<String> = Vec::new();
            for e in &d.entries {
                if let Some(n) = block.net(&e.net)
                    && !members.contains(&n)
                {
                    members.push(n);
                }
                let part = e.on.rsplit_once('/').map_or(e.on.as_str(), |(p, _)| p);
                let who = block
                    .part(part)
                    .map_or(part.to_owned(), |p| block.parts[p].refdes.clone());
                let end = format!("{who} {}", e.side.as_str());
                if !ends.contains(&end) {
                    ends.push(end);
                }
            }
            let name = block
                .bus(key)
                .map_or_else(|| key.clone(), |b| crate::bus::name(block, b, &members));
            for n in &members {
                bus_of.insert(block.nets[*n].key.as_str(), name.clone());
            }
            buses.push(json!({
                "bus": key,
                "name": name,
                "members": members.iter().map(|n| block.nets[*n].key.clone()).collect::<Vec<_>>(),
                "ends": ends,
                "connector": d.connector.as_ref().map(|c| c.text.clone()),
            }));
        }
        let mut nets = Vec::new();
        for net in &block.nets {
            if let Some(name) = bus_of.get(net.key.as_str()) {
                nets.push(json!({
                    "net": net.key,
                    "name": net.name,
                    "drawn": "bus",
                    "style": "bus",
                    "why": format!("a member of {name}"),
                }));
                continue;
            }
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
            // An off-sheet connector's net: the other sheets it continues
            // on (plan 2.C).
            let connector = layout.labels.get(&net.key).is_some_and(|l| {
                l.iter()
                    .any(|x| &x.sheet == sid && x.kind == crate::layout::LabelKind::OffSheet)
            });
            if connector && let Some(n) = block.net(&net.key) {
                let on: BTreeSet<&String> = block
                    .parts
                    .iter()
                    .filter(|p| p.pins.iter().any(|x| x.nets().contains(&n)))
                    .filter_map(|p| layout.placed.get(&p.key))
                    .map(|x| &x.sheet)
                    .filter(|s| *s != sid)
                    .collect();
                v["continues_on"] = json!(on);
                v["connectors"] = json!(
                    layout.labels[&net.key]
                        .iter()
                        .filter(|x| &x.sheet == sid && x.kind == crate::layout::LabelKind::OffSheet)
                        .count()
                );
            }
            nets.push(v);
        }
        let score = scores.iter().find(|s| &s.sheet == sid);
        let regions: Vec<Value> = layout
            .regions
            .iter()
            .filter(|(_, r)| &r.sheet == sid)
            .map(|(id, r)| {
                let f = r.frame_on_sheet();
                json!({
                    "id": id,
                    "title": r.title,
                    "named": r.named,
                    "parts": r
                        .parts
                        .iter()
                        .map(|k| block.part(k).map_or(k.clone(), |p| crate::lint::part_shown(block, p)))
                        .collect::<Vec<_>>(),
                    "box_mm": [mm(f.min[0]), mm(f.min[1]), mm(f.max[0]), mm(f.max[1])],
                })
            })
            .collect();
        let area = block.area_of(sheet);
        let paper = block.paper_of(sheet);
        let paper = json!({
            "size": sheet.size.clone().or_else(|| paper.map(|p| p.size.clone())),
            "orientation": sheet.orientation.or(paper.map(|p| p.orientation)),
            "pinned": sheet.pinned,
            "area_mm": [mm(area[0]), mm(area[1])],
            "extent": crate::lint::extent(block, layout, sid),
            "fill": crate::lint::fill(block, layout, sid),
        });
        let notes: Vec<Value> = layout
            .notes
            .iter()
            .filter(|(_, n)| &n.sheet == sid)
            .map(|(id, n)| {
                let text = block.note_text(id).unwrap_or("");
                let b = crate::paint::note_box(text, n.at);
                let on = match block.instructions.get(id) {
                    Some(crate::layout::Instruction::Note { on, .. }) => on.clone(),
                    _ => String::new(),
                };
                json!({
                    "instruction": id,
                    "text": text,
                    "on": on,
                    "box_mm": [mm(b.min[0]), mm(b.min[1]), mm(b.max[0]), mm(b.max[1])],
                })
            })
            .collect();
        sheets.push(json!({
            "paper": paper,
            "regions": regions,
            "sheet": sid,
            "name": sheet.name,
            "canvas_mm": [mm(sheet.at[0]), mm(sheet.at[1])],
            "score": score.map(|s| s.score),
            "measures": score,
            "parts": parts,
            "nets": nets,
            "findings": findings.iter().filter(|f| &f.sheet == sid).collect::<Vec<_>>(),
        }));
        if !notes.is_empty()
            && let Some(Value::Object(last)) = sheets.last_mut()
        {
            last.insert("notes".into(), json!(notes));
        }
        if !buses.is_empty()
            && let Some(Value::Object(last)) = sheets.last_mut()
        {
            last.insert("buses".into(), json!(buses));
        }
    }
    let instructions: Vec<Value> = block
        .instructions
        .iter()
        .map(|(id, i)| {
            let verdict = instr::judge(block, layout, i);
            let mut v = json!({
                "id": id,
                "instruction": i,
                "holds": verdict.is_ok(),
            });
            if let Err(why) = verdict {
                v["why"] = json!(why);
            }
            v
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
        "canvas": crate::canvas::order(layout),
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
        assert_eq!(d["canvas"], json!(["s1"]));
        assert_eq!(d["sheets"][0]["canvas_mm"], json!(["0", "0"]));
        let parts = d["sheets"][0]["parts"].as_array().unwrap();
        let u1 = parts.iter().find(|p| p["part"] == "u1").unwrap();
        // Under U0 (its decoupling capacitor sits out beside its pin).
        assert_eq!(u1["neighbours"]["above"], json!("u0"));
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

    #[test]
    fn a_connector_says_where_its_net_continues() {
        let b = crate::testing::leaving_block();
        let out = crate::engine::lay_out(&b).unwrap();
        let (found, scores) = crate::lint::lint(&b, &out);
        let v = describe(&b, &out, &found, &scores);
        let away = &out.layout.placed["u2"].sheet;
        let home = &out.layout.placed["u1"].sheet;
        let sheets = v["sheets"].as_array().unwrap();
        let mut seen = 0;
        for sh in sheets {
            let id = sh["sheet"].as_str().unwrap();
            for n in sh["nets"].as_array().unwrap() {
                if n["net"] == "sig" {
                    let other = if id == home { away } else { home };
                    assert_eq!(n["continues_on"], json!([other]), "{n}");
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, 2);
    }

    #[test]
    fn a_sheet_lists_its_notes() {
        use crate::layout::Instruction;
        let mut b = crate::testing::leaving_block();
        let first = crate::engine::lay_out(&b).unwrap();
        let sid = first.layout.placed["u1"].sheet.clone();
        b.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: sid.clone(),
                text: "Fused at 2 A".into(),
            },
        );
        let out = crate::engine::lay_out(&b).unwrap();
        let (found, scores) = crate::lint::lint(&b, &out);
        let v = describe(&b, &out, &found, &scores);
        let sheet = v["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["sheet"] == sid.as_str())
            .unwrap();
        let notes = sheet["notes"].as_array().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0]["instruction"], "i9");
        assert_eq!(notes[0]["text"], "Fused at 2 A");
        assert_eq!(notes[0]["on"], sid.as_str());
        assert_eq!(notes[0]["box_mm"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_sheet_lists_its_buses_and_their_members_say_so() {
        let b = crate::testing::bus_block();
        let out = crate::engine::lay_out(&b).unwrap();
        let (found, scores) = crate::lint::lint(&b, &out);
        let v = describe(&b, &out, &found, &scores);
        let sheet = &v["sheets"][0];
        assert_eq!(
            sheet["buses"],
            json!([{
                "bus": "bus:q",
                "name": "QSPI_SD[0..3]",
                "members": ["d0", "d1", "d2", "d3"],
                "ends": ["U1 right", "U2 left"],
                "connector": null,
            }])
        );
        let d0 = sheet["nets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["net"] == "d0")
            .unwrap();
        assert_eq!(d0["style"], "bus");
        assert_eq!(d0["why"], "a member of QSPI_SD[0..3]");
    }
}
