//! Three versions of a file from bytes, merged as the merge driver merges
//! them. The first byte picks how the rest is read: raw text (three
//! versions split at `0xff` bytes, so anything at all reaches the driver),
//! or a base sketch, part list, parameters file, part's properties,
//! release record, pattern, instance or mate and two sides' edits to it,
//! two bytes an edit; `0xfe` an import feature, `0xff` an instance an
//! import made, `0xfd` the material library, `0xfc` a named view (its
//! sections and explode too), `0xfb` an appearance, `0xfa` an explode,
//! `0xf9` a lock, `0xf8` a manifest's dependencies, `0xf7` a datum
//! plane, `0xf6` a path, `0xf5` a sweep, `0xf4` a shell, `0xf3` a loft, `0xf2` a mirror or split, `0xf1` an annotation (marks on either side, design 423), `0xf0` a base
//! flange or flange, `0xef` a planar face or a surface extrude, `0xee` an
//! unfold or refold, `0xed` a face tweak, `0xec` a thicken or a fill (by the
//! parity of the edit bytes), `0xeb` a draft, `0xea` a split by a tool. Merging must never panic, must give the same bytes twice,
//! must be canonical JSON whose record holds as many conflicts as it says
//! unless it fell back to lines, and readable files never fall back. A
//! side that changed nothing gives the other, an instance's pose never
//! conflicts (it is solver state), an assembly's item numbers conflict
//! only part by part, two sides re-importing different bytes conflict
//! on the file's hash, an explode's steps conflict only as a whole, a
//! lock conflicts only label by label (never in a label's commit or
//! content apart), a manifest's dependencies only entry by entry, and a
//! datum's references only whole, its construction changed on both sides
//! only as the whole file, a path's segments only as the whole list, a
//! sweep's profile only whole, its law only with its direction, a
//! shell's open faces and groups only whole, a loft's sections only
//! as the whole list, a split's plane and flip only together, and a
//! base flange's allowance and relief only whole, and a thicken's
//! source only whole.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::doc::canonical;
use ryucad::merge::{Outcome, bytes, names_in, record};
use serde_json::{Value, json};

/// A square sketch with a length.
fn sketch() -> Value {
    json!({"format": 2, "kind": "ryucad.feature", "type": "sketch", "plane": "xy",
           "entities": {
               "p1": {"type": "point", "m": [0, 0]},
               "p2": {"type": "point", "m": [0.01, 0]},
               "p3": {"type": "point", "m": [0.01, 0.01]},
               "p4": {"type": "point", "m": [0, 0.01]},
               "l1": {"type": "line", "start": "p1", "end": "p2"},
               "l2": {"type": "line", "start": "p2", "end": "p3"},
               "l3": {"type": "line", "start": "p3", "end": "p4"},
               "l4": {"type": "line", "start": "p4", "end": "p1"}},
           "constraints": {"d1": {"type": "length", "a": "l1", "value": "10 mm"}}})
}

/// The keys of `v[group]`, sorted.
fn keys(v: &Value, group: &str) -> Vec<String> {
    v[group]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// One past the largest ID with `prefix`.
fn next(v: &Value, prefix: &str) -> String {
    let n = ["entities", "constraints"]
        .iter()
        .flat_map(|g| keys(v, g))
        .filter_map(|k| k.strip_prefix(prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{}", n + 1)
}

/// Applies edit `(op, arg)` to a sketch, keeping it valid.
fn edit_sketch(v: &mut Value, op: u8, arg: u8) {
    let entities = keys(v, "entities");
    let pick =
        |list: &[String]| (!list.is_empty()).then(|| list[arg as usize % list.len()].clone());
    let kind = |id: &str, v: &Value| v["entities"][id]["type"].as_str().unwrap_or("").to_owned();
    let points: Vec<String> = entities
        .iter()
        .filter(|e| kind(e, v) == "point")
        .cloned()
        .collect();
    let lines: Vec<String> = entities
        .iter()
        .filter(|e| kind(e, v) == "line")
        .cloned()
        .collect();
    let x = f64::from(arg) / 1000.0;
    match op % 7 {
        0 => {
            if let Some(p) = pick(&points) {
                v["entities"][p]["m"] = json!([x, -x]);
            }
        }
        1 => {
            let id = next(v, "p");
            v["entities"][id] = json!({"type": "point", "m": [x, x]});
        }
        2 => {
            if let (Some(a), Some(b)) = (pick(&points), points.first().cloned())
                && a != b
            {
                let id = next(v, "l");
                v["entities"][id] = json!({"type": "line", "start": a, "end": b});
            }
        }
        3 => {
            if let Some(id) = pick(&entities) {
                let mut gone = vec![id];
                while let Some(g) = gone.pop() {
                    for group in ["entities", "constraints"] {
                        if let Some(o) = v[group].as_object_mut() {
                            o.remove(&g);
                            gone.extend(
                                o.iter()
                                    .filter(|(_, m)| names_in(m).contains(&g.as_str()))
                                    .map(|(k, _)| k.clone()),
                            );
                        }
                    }
                }
            }
        }
        4 => {
            if let Some(l) = pick(&lines) {
                let id = next(v, "d");
                v["constraints"][id] =
                    json!({"type": "length", "a": l, "value": format!("{} mm", arg % 50 + 1)});
            }
        }
        5 => {
            if let Some(l) = pick(&lines)
                && let Some(e) = v["entities"][l].as_object_mut()
                && e.remove("construction").is_none()
            {
                e.insert("construction".to_owned(), json!(true));
            }
        }
        _ => v["plane"] = json!(["xy", "yz", "zx"][arg as usize % 3]),
    }
}

/// Applies edit `(op, arg)` to an assembly's part list: its features, or
/// its item numbers (a part numbered, or its number taken away).
fn edit_part(v: &mut Value, op: u8, arg: u8) {
    let mut list: Vec<String> = serde_json::from_value(v["features"].clone()).unwrap_or_default();
    let at = |n: usize| if n == 0 { 0 } else { arg as usize % n };
    let part = format!("q{}", arg % 6);
    match op % 5 {
        3 => {
            v["items"][part.as_str()] = json!(1 + arg % 4);
            return;
        }
        4 => {
            if let Some(items) = v["items"].as_object_mut() {
                items.remove(&part);
            }
            return;
        }
        _ => {}
    }
    match op % 5 {
        0 if !list.is_empty() => {
            let i = at(list.len());
            list.remove(i);
        }
        1 if !list.is_empty() => {
            let f = list.remove(at(list.len()));
            let i = at(list.len() + 1);
            list.insert(i, f);
        }
        _ => {
            let id = format!("f{arg}");
            if !list.contains(&id) {
                let i = at(list.len() + 1);
                list.insert(i, id);
            }
        }
    }
    v["features"] = json!(list);
}

/// Applies edit `(op, arg)` to a parameters file.
fn edit_parameters(v: &mut Value, op: u8, arg: u8) {
    let name = format!("p{}", arg % 8);
    let ps = v["parameters"].as_object_mut().expect("an object");
    match op % 2 {
        0 => {
            ps.remove(&name);
        }
        _ => {
            ps.insert(name, json!(format!("{} mm", op / 2)));
        }
    }
}

/// Applies edit `(op, arg)` to a part's properties: a property removed,
/// set to text, or set to a number.
fn edit_properties(v: &mut Value, op: u8, arg: u8) {
    let names = [
        "number",
        "name",
        "description",
        "material",
        "finish",
        "mass",
    ];
    let name = names[arg as usize % names.len()];
    let o = v.as_object_mut().expect("an object");
    match op % 3 {
        0 => {
            o.remove(name);
        }
        1 => {
            o.insert(name.to_owned(), json!(format!("v{}", op / 3)));
        }
        _ => {
            o.insert(name.to_owned(), json!(op / 3));
        }
    }
}

/// Applies edit `(op, arg)` to a release record: its note, content or
/// properties changed. A record merges as a whole file.
fn edit_record(v: &mut Value, op: u8, arg: u8) {
    match op % 3 {
        0 => v["note"] = json!(format!("note {arg}")),
        1 => v["content"] = json!(format!("{arg:064x}")),
        _ => v["properties"]["number"] = json!(format!("{}", 100 + u32::from(arg))),
    }
}

/// Applies edit `(op, arg)` to a pattern: a seed added or removed, an
/// instance skipped or not, its count or spacing set. Its seeds and skips
/// are arrays, one value each.
fn edit_pattern(v: &mut Value, op: u8, arg: u8) {
    let toggle = |list: &mut Vec<Value>, x: Value| match list.iter().position(|y| *y == x) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(x),
    };
    match op % 4 {
        0 => {
            let mut l = v["features"].as_array().cloned().unwrap_or_default();
            toggle(&mut l, json!(format!("h{}", arg % 4)));
            v["features"] = json!(l);
        }
        1 => {
            let mut l = v["skip"].as_array().cloned().unwrap_or_default();
            toggle(&mut l, json!(1 + arg % 3));
            let o = v.as_object_mut().expect("an object");
            if l.is_empty() {
                o.remove("skip");
            } else {
                o.insert("skip".to_owned(), json!(l));
            }
        }
        2 => v["count"] = json!(format!("{}", 2 + arg % 5)),
        _ => v["spacing"] = json!(format!("{} mm", 1 + arg % 9)),
    }
}

/// Applies edit `(op, arg)` to an instance: its pose moved or turned (as
/// a solver would write it), its revision or its fixing changed.
fn edit_instance(v: &mut Value, op: u8, arg: u8) {
    let x = f64::from(arg) * 1e-3;
    match op % 4 {
        0 => v["pose"]["origin_m"][usize::from(arg % 3)] = json!(x),
        1 => {
            let (c, s) = (1.0 - x * x / 2.0, x);
            v["pose"]["x"] = json!([c, s, 0]);
        }
        2 => {
            v["rev"] = match arg % 3 {
                0 => json!("working"),
                n => json!({"label": if n == 1 { "A" } else { "B" }}),
            }
        }
        _ => v["fixed"] = json!(arg % 2 == 1),
    }
}

/// Applies edit `(op, arg)` to a mate: a connector aimed elsewhere or
/// offset, its form or its angle changed. A connector is one value.
fn edit_mate(v: &mut Value, op: u8, arg: u8) {
    let side = if arg & 1 == 0 { "a" } else { "b" };
    match op % 4 {
        0 => {
            v[side]["at"] = match arg % 3 {
                0 => json!("origin"),
                n => json!({"feature": "slab", "name": format!("{n}").repeat(32)}),
            }
        }
        1 => v[side]["offset"] = json!({"origin": ["0 mm", "0 mm", format!("{} mm", arg % 7)]}),
        2 => v["form"] = json!(["fastened", "revolute", "slider", "ball"][usize::from(arg % 4)]),
        _ => v["angle"] = json!(format!("{} deg", arg % 90)),
    }
}

/// Applies edit `(op, arg)` to an import feature: the file re-imported
/// (a new hash), another body, another product or none, its ordinal, its
/// mode (a reference, else a solid with no field), or its place (a
/// flattened body `arg` millimetres along x by usage `u<arg>`, or none);
/// the format follows, 17 with a place, 13 for a reference, else 5.
fn edit_import(v: &mut Value, op: u8, arg: u8) {
    match op % 6 {
        4 => {
            if arg & 1 == 1 {
                v["mode"] = json!("reference");
            } else {
                v.as_object_mut().unwrap().remove("mode");
            }
        }
        5 => {
            if arg & 1 == 1 {
                v["place"] = json!({
                    "pose": {"origin_m": [f64::from(arg) / 1000.0, 0, 0],
                             "x": [1, 0, 0], "z": [0, 0, 1]},
                    "via": [{"product": {"id": "S-0", "name": "Sub"}, "usage": format!("u{arg}")}],
                });
            } else {
                v.as_object_mut().unwrap().remove("place");
            }
        }
        0 => v["file"]["blake3"] = json!(format!("{arg:064x}")),
        1 => v["body"] = json!(arg % 4),
        2 => match arg % 3 {
            0 => {
                v.as_object_mut().unwrap().remove("product");
            }
            n => v["product"] = json!({"id": format!("P-{n}"), "name": format!("Part {n}")}),
        },
        _ => {
            if v["product"].is_object() {
                match arg % 3 {
                    0 => {
                        v["product"].as_object_mut().unwrap().remove("ordinal");
                    }
                    n => v["product"]["ordinal"] = json!(n),
                }
            }
        }
    }
    v["format"] = json!(if v.get("place").is_some() {
        17
    } else if v.get("mode").is_some() {
        13
    } else {
        5
    });
}

/// Applies edit `(op, arg)` to an instance an import made: its pose (as
/// an update or a solver writes it), its usage, its ordinal, its fixing.
fn edit_imported(v: &mut Value, op: u8, arg: u8) {
    match op % 4 {
        0 => edit_instance(v, 0, arg),
        1 => v["import"]["usage"] = json!(format!("NAUO{}", arg % 3)),
        2 => match arg % 3 {
            0 => {
                v["import"].as_object_mut().unwrap().remove("ordinal");
            }
            n => v["import"]["ordinal"] = json!(n),
        },
        _ => v["fixed"] = json!(arg % 2 == 1),
    }
}

/// Applies edit `(op, arg)` to a material library: a material's
/// density set (adding it), a material removed (the default with it), or
/// the default moved, kept valid.
fn edit_materials(v: &mut Value, op: u8, arg: u8) {
    let name = format!("m{}", arg % 3);
    match op % 3 {
        0 => v["materials"][&name] = json!({"density": format!("{} kg/m^3", u32::from(arg) + 1)}),
        1 => {
            v["materials"].as_object_mut().unwrap().remove(&name);
            if v["default"] == json!(name) {
                v.as_object_mut().unwrap().remove("default");
            }
        }
        _ => {
            if v["materials"].get(&name).is_some() {
                v["default"] = json!(name);
            } else {
                v.as_object_mut().unwrap().remove("default");
            }
        }
    }
}

/// Applies edit `(op, arg)` to a named view: its eye or target moved,
/// its field of view changed, or its projection switched.
fn edit_view(v: &mut Value, op: u8, arg: u8) {
    let x = f64::from(arg) / 1000.0;
    match op % 6 {
        4 => {
            v["sections"] = json!([{"normal": [0, 0, 1], "origin_m": [0, 0, x]}]);
            v["format"] = json!(2);
        }
        5 => {
            v["explode"] = json!({"fraction": f64::from(arg) / 255.0, "id": "apart"});
            v["format"] = json!(2);
        }
        0 => v["eye_m"] = json!([0.1 + x, -0.1, 0.1]),
        1 => v["target_m"] = json!([0, x, 0]),
        2 if v["projection"] == "perspective" => v["fov_deg"] = json!(10 + arg % 90),
        _ => {
            let o = v.as_object_mut().unwrap();
            if o["projection"] == "perspective" {
                o.remove("fov_deg");
                o.insert("projection".into(), json!("orthographic"));
                o.insert("height_m".into(), json!(0.01 + x));
            } else {
                o.remove("height_m");
                o.insert("projection".into(), json!("perspective"));
                o.insert("fov_deg".into(), json!(30));
            }
        }
    }
}

/// Applies edit `(op, arg)` to an explode: a step's lift changed, a step
/// added, the last one taken off (one stays), or a turn given to the
/// first.
fn edit_explode(v: &mut Value, op: u8, arg: u8) {
    let steps = v["steps"].as_array_mut().unwrap();
    let k = usize::from(arg) % steps.len();
    match op % 4 {
        0 => steps[k]["translate"] = json!(["0 mm", "0 mm", format!("{arg} mm")]),
        1 => steps.push(json!({"instances": [format!("i{}", arg % 3)],
                               "translate": ["0 mm", format!("{arg} mm"), "0 mm"]})),
        2 if steps.len() > 1 => {
            steps.pop();
        }
        _ => steps[0]["rotate"] = json!({"angle": format!("{arg} deg"), "axis": [0, 0, 1]}),
    }
}

/// Applies edit `(op, arg)` to an appearance: a face (of four) given a
/// colour, an opacity or its material's colour, a solid's opacity set, an
/// entry taken out, or the part's colour set.
fn edit_appearance(v: &mut Value, op: u8, arg: u8) {
    let face = format!("box/{:032x}", arg % 4);
    let colour = format!("#{:02x}{:02x}{:02x}", arg, arg / 2, 255 - arg);
    let opacity = f64::from(arg % 100 + 1) / 100.0;
    let entry = |v: &mut Value, group: &str, key: &str| -> Value {
        if !v[group].is_object() {
            v[group] = json!({});
        }
        v[group][key].take()
    };
    match op % 6 {
        0 => {
            let mut e = entry(v, "faces", &face);
            if !e.is_object() {
                e = json!({});
            }
            e.as_object_mut().unwrap().remove("material");
            e["colour"] = json!(colour);
            v["faces"][&face] = e;
        }
        1 => {
            let mut e = entry(v, "faces", &face);
            if !e.is_object() {
                e = json!({});
            }
            e["opacity"] = json!(opacity);
            v["faces"][&face] = e;
        }
        2 => {
            let mut e = entry(v, "faces", &face);
            if !e.is_object() {
                e = json!({});
            }
            e.as_object_mut().unwrap().remove("colour");
            e["material"] = json!(true);
            v["faces"][&face] = e;
        }
        3 => v["solids"]["lid"] = json!({"opacity": opacity}),
        4 => {
            if let Some(o) = v["faces"].as_object_mut() {
                o.remove(&face);
            }
        }
        _ => v["part"] = json!({"colour": colour}),
    }
    for group in ["faces", "solids"] {
        if v[group].as_object().is_some_and(|o| o.is_empty()) {
            v.as_object_mut().unwrap().remove(group);
        }
    }
}

/// The project IDs the lock and dependency modes use.
fn project(n: u8) -> String {
    format!("{:032x}", u128::from(n % 3) + 1)
}

/// Applies edit `(op, arg)` to a lock: a label (of four parts, at A or B)
/// locked at a commit, a label taken out, a project's URL moved, or a
/// project added with nothing locked.
fn edit_lock(v: &mut Value, op: u8, arg: u8) {
    let p = project(arg);
    let label = format!("p{}/{}", arg % 4, if arg & 0x10 == 0 { "A" } else { "B" });
    let entry = |v: &mut Value, p: &str| {
        if !v["projects"][p].is_object() {
            v["projects"][p] = json!({"labels": {}, "path": "", "release_branch": "main",
                                      "url": format!("file:///{p}"), "via": []});
        }
    };
    match op % 4 {
        0 => {
            entry(v, &p);
            v["projects"][&p]["labels"][&label] =
                json!({"commit": format!("{:040x}", arg), "content": format!("{:064x}", arg / 3)});
        }
        1 => {
            if let Some(l) = v["projects"][&p]["labels"].as_object_mut() {
                l.remove(&label);
            }
        }
        2 => {
            entry(v, &p);
            v["projects"][&p]["url"] = json!(format!("file:///mirror/{arg}"));
        }
        _ => entry(v, &p),
    }
}

/// Applies edit `(op, arg)` to a manifest's dependencies: one listed (or
/// moved to another URL), taken out, or renamed.
fn edit_dependencies(v: &mut Value, op: u8, arg: u8) {
    let p = project(arg);
    if !v["dependencies"].is_object() {
        v["dependencies"] = json!({});
    }
    match op % 3 {
        0 => {
            v["dependencies"][&p] = json!({"name": format!("d{}", arg % 3), "path": "", "url": format!("file:///{arg}")})
        }
        1 => {
            v["dependencies"].as_object_mut().unwrap().remove(&p);
        }
        _ => {
            if v["dependencies"][&p].is_object() {
                v["dependencies"][&p]["name"] = json!(format!("n{arg}"));
            }
        }
    }
}

/// A datum's edit: its distance, how it is built (offset or parallel), or
/// the plane it starts from (another datum, or a sketch).
fn edit_datum(v: &mut Value, op: u8, arg: u8) {
    match op % 3 {
        0 if v["by"] == "offset" => v["distance"] = json!(format!("{} mm", arg % 40)),
        0 => {}
        1 if v["by"] == "offset" => {
            let o = v.as_object_mut().unwrap();
            o.remove("distance");
            o.insert("by".into(), json!("parallel"));
            o.insert(
                "through".into(),
                json!({"feature": format!("q{}", arg % 3)}),
            );
        }
        1 => {
            let o = v.as_object_mut().unwrap();
            o.remove("through");
            o.insert("by".into(), json!("offset"));
            o.insert("distance".into(), json!(format!("{} mm", arg % 40)));
        }
        _ if arg % 2 == 0 => v["from"] = json!({"feature": format!("d{}", arg % 5)}),
        _ => v["from"] = json!({"entity": format!("p{}", arg % 7), "feature": "s"}),
    }
}

/// A sweep's edit: its twist or scale changed or taken off, its profile
/// narrowed to a loop or moved to another sketch, its law switched
/// (binormal about an axis, or minimal again).
fn edit_sweep(v: &mut Value, op: u8, arg: u8) {
    let o = v.as_object_mut().unwrap();
    match op % 5 {
        0 if arg % 4 == 0 => {
            o.remove("twist");
        }
        0 => {
            o.insert(
                "twist".into(),
                json!(format!("{} deg", 15 * u32::from(arg % 24))),
            );
        }
        1 if arg % 4 == 0 => {
            o.remove("scale");
        }
        1 => {
            o.insert("scale".into(), json!(format!("0.{}", 1 + arg % 9)));
        }
        2 => {
            o.insert(
                "profile".into(),
                json!({"sketch": "s", "loops": [format!("l{}", arg % 4)]}),
            );
        }
        3 => {
            o.insert("profile".into(), json!({"sketch": format!("s{}", arg % 3)}));
        }
        _ if arg % 2 == 0 => {
            o.insert("law".into(), json!("binormal"));
            o.insert(
                "direction".into(),
                json!(["x", "y", "z"][usize::from(arg % 3)]),
            );
        }
        _ => {
            o.insert("law".into(), json!("minimal"));
            o.remove("direction");
        }
    }
}

/// A planar face's or a surface extrude's edit: its profile's sketch or
/// loops, an extrude's length, or the extrude made a solid's again.
fn edit_surface(v: &mut Value, op: u8, arg: u8) {
    let planar = v["type"] == "planar";
    let o = v.as_object_mut().unwrap();
    match op % 3 {
        0 if planar => {
            o.insert("profile".into(), json!({"sketch": format!("s{}", arg % 3)}));
        }
        0 => {
            o.insert("sketch".into(), json!(format!("s{}", arg % 3)));
        }
        1 if planar => {
            o.insert(
                "profile".into(),
                json!({"sketch": "s", "loops": [format!("l{}", arg % 4)]}),
            );
        }
        1 => {
            o.insert(
                "extent".into(),
                json!({"type": "blind", "length": format!("{} mm", 1 + arg % 20)}),
            );
        }
        _ if planar => {
            o.remove("profile");
            o.insert("profile".into(), json!({"sketch": "s"}));
        }
        _ => {
            o.remove("surface");
            o.insert("format".into(), json!(2));
        }
    }
}

/// A loft's edit: a section added, taken off or swapped with the next, a
/// section narrowed to a loop from a point, closed or opened, a tangent
/// set, weighted or taken off.
fn edit_loft(v: &mut Value, op: u8, arg: u8) {
    let o = v.as_object_mut().unwrap();
    let n = o["sections"].as_array().map_or(0, Vec::len);
    match op % 6 {
        0 => {
            let s = o["sections"].as_array_mut().unwrap();
            s.push(json!({"sketch": format!("s{}", arg % 5)}));
        }
        1 if n > 2 => {
            o["sections"]
                .as_array_mut()
                .unwrap()
                .remove(usize::from(arg) % n);
        }
        1 | 2 if n > 1 => {
            let i = usize::from(arg) % (n - 1);
            o["sections"].as_array_mut().unwrap().swap(i, i + 1);
        }
        3 if n > 0 => {
            let i = usize::from(arg) % n;
            o["sections"][i]["loops"] = json!([format!("p{}", arg % 4)]);
        }
        4 if arg % 3 == 0 => {
            o.insert("closed".into(), json!(true));
            o.remove("start_tangent");
            o.remove("end_tangent");
        }
        4 => {
            o.remove("closed");
        }
        _ => {
            let k = if arg % 2 == 0 {
                "start_tangent"
            } else {
                "end_tangent"
            };
            match arg % 4 {
                0 | 1 => {
                    o.insert(k.into(), json!({"direction": "normal"}));
                }
                2 => {
                    o.insert(
                        k.into(),
                        json!({"direction": "z", "weight": format!("{}", 1 + arg % 3)}),
                    );
                }
                _ => {
                    o.remove(k);
                }
            }
        }
    }
}

/// A path's edit: its segments turned round or one taken off, a segment
/// added (a sketch's line, a helix, an edge grown to its chain), or the
/// whole path turned round.
fn edit_path(v: &mut Value, op: u8, arg: u8) {
    let segs = v["segments"].as_array_mut().unwrap();
    match op % 4 {
        0 => segs.reverse(),
        1 if segs.len() > 1 => {
            segs.remove(usize::from(arg) % segs.len());
        }
        1 => {}
        2 => {
            let s = match arg % 3 {
                0 => json!({"entity": format!("l{}", arg % 5), "feature": "s"}),
                1 => json!({"feature": format!("h{}", arg % 3)}),
                _ => json!({"feature": "b", "name": format!("{:032x}", arg), "propagate": true}),
            };
            segs.insert(usize::from(arg) % (segs.len() + 1), s);
        }
        _ if arg % 2 == 0 => v["reverse"] = json!(true),
        _ => {
            v.as_object_mut().unwrap().remove("reverse");
        }
    }
}

/// Edits a draft: its angle, its faces, a face flipped or not, its
/// tangency, its pull reversed, or a new hinge (a neutral plane, or a
/// parting line with its pull).
fn edit_draft(v: &mut Value, op: u8, arg: u8) {
    let face = |a: u8| json!({"feature": "b", "name": format!("{:032x}", a % 5)});
    let o = v.as_object_mut().unwrap();
    match op % 6 {
        0 => {
            o.insert("angle".into(), json!(format!("{} deg", 1 + arg % 6)));
        }
        1 => {
            let mut faces: Vec<Value> = o
                .get("faces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let f = face(arg);
            match faces.iter().position(|x| *x == f) {
                Some(i) if faces.len() > 1 => {
                    faces.remove(i);
                }
                Some(_) => {}
                None => faces.push(f),
            }
            o.insert("faces".into(), json!(faces));
            // A flipped face no longer drafted goes from flip too.
            if let Some(Value::Array(fl)) = o.get_mut("flip") {
                fl.retain(|x| faces.contains(x));
            }
        }
        2 => {
            let first = o.get("faces").and_then(|f| f.get(0)).cloned();
            match (arg % 2, first) {
                (0, Some(f)) => {
                    o.insert("flip".into(), json!([f]));
                }
                _ => {
                    o.remove("flip");
                }
            }
        }
        3 if arg % 2 == 0 => {
            o.insert("tangent".into(), json!(false));
        }
        3 => {
            o.remove("tangent");
        }
        4 if arg % 2 == 0 => {
            o.insert("reverse".into(), json!(true));
        }
        4 => {
            o.remove("reverse");
        }
        _ => {
            for k in ["neutral", "parting", "pull"] {
                o.remove(k);
            }
            if arg % 2 == 0 {
                o.insert(
                    "neutral".into(),
                    json!(["xy", "yz", "zx"][(arg / 2 % 3) as usize]),
                );
            } else {
                o.insert("parting".into(), json!([face(arg.wrapping_add(2))]));
                o.insert(
                    "pull".into(),
                    json!(["x", "y", "z"][(arg / 2 % 3) as usize]),
                );
            }
        }
    }
}

/// Edits a move_face: its distance or angle, its faces, its tangency, or a
/// new `by` with the fields it takes.
fn edit_tweak(v: &mut Value, op: u8, arg: u8) {
    let face = |a: u8| json!({"feature": "b", "name": format!("{:032x}", a % 5)});
    let o = v.as_object_mut().unwrap();
    match op % 5 {
        0 => {
            let k = if o.get("by").and_then(Value::as_str) == Some("rotate") {
                "angle"
            } else {
                "distance"
            };
            let unit = if k == "angle" { "deg" } else { "mm" };
            o.insert(k.into(), json!(format!("{} {unit}", 1 + arg % 6)));
        }
        1 => {
            let mut faces: Vec<Value> = o
                .get("faces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let f = face(arg);
            match faces.iter().position(|x| *x == f) {
                Some(i) if faces.len() > 1 => {
                    faces.remove(i);
                }
                Some(_) => {}
                None => faces.push(f),
            }
            o.insert("faces".into(), json!(faces));
        }
        2 if arg % 2 == 0 => {
            o.insert("tangent".into(), json!(false));
        }
        2 => {
            o.remove("tangent");
        }
        3 => {
            for k in ["direction", "distance", "axis", "angle", "groups"] {
                o.remove(k);
            }
            match arg % 3 {
                0 => {
                    o.insert("by".into(), json!("translate"));
                    o.insert(
                        "direction".into(),
                        json!(["x", "y", "z"][(arg / 3 % 3) as usize]),
                    );
                    o.insert("distance".into(), json!("2 mm"));
                }
                1 => {
                    o.insert("by".into(), json!("rotate"));
                    o.insert("axis".into(), json!("x"));
                    o.insert("angle".into(), json!("3 deg"));
                }
                _ => {
                    o.insert("by".into(), json!("offset"));
                    o.insert("distance".into(), json!("1 mm"));
                }
            }
        }
        _ => {
            if o.get("by").and_then(Value::as_str) == Some("offset") {
                o.insert(
                    "groups".into(),
                    json!([{"distance": format!("{} mm", 1 + arg % 3),
                                                  "faces": [face(arg.wrapping_add(1))]}]),
                );
            }
        }
    }
}

/// A shell's edits on either side: its thickness, its direction, an open
/// face added or taken out, a group's wall, or its groups dropped.
fn edit_shell(v: &mut Value, op: u8, arg: u8) {
    let face = |a: u8| json!({"feature": "b", "name": format!("{:032x}", a % 5)});
    let o = v.as_object_mut().unwrap();
    match op % 5 {
        0 => {
            o.insert("thickness".into(), json!(format!("{} mm", 1 + arg % 4)));
        }
        1 if arg % 2 == 0 => {
            o.insert("direction".into(), json!("outward"));
        }
        1 => {
            o.remove("direction");
        }
        2 => {
            let mut open: Vec<Value> = o
                .get("open")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let f = face(arg);
            match open.iter().position(|x| *x == f) {
                Some(i) => {
                    open.remove(i);
                }
                None => open.push(f),
            }
            if open.is_empty() {
                o.remove("open");
            } else {
                o.insert("open".into(), json!(open));
            }
        }
        3 => {
            o.insert(
                "faces".into(),
                json!([{"thickness": format!("{} mm", 2 + arg % 4),
                        "faces": [face(arg.wrapping_add(1))]}]),
            );
        }
        _ => {
            o.remove("faces");
        }
    }
}

/// A mirror's or split's edits on either side: its plane moved, a
/// split's flip, a split's keep, a mirror's operation, or its targets.
fn edit_mirror(v: &mut Value, op: u8, arg: u8) {
    let o = v.as_object_mut().unwrap();
    let split = o.get("type") == Some(&json!("split"));
    match op % 4 {
        0 => {
            let planes = [
                json!("xy"),
                json!("yz"),
                json!("zx"),
                json!({"feature": "d"}),
            ];
            o.insert("plane".into(), planes[usize::from(arg % 4)].clone());
        }
        1 if split => {
            o.insert("flip".into(), json!(arg % 2 == 0));
        }
        2 if split => {
            if arg % 2 == 0 {
                o.insert("keep".into(), json!("behind"));
            } else {
                o.remove("keep");
            }
        }
        _ if split => {
            o.remove("flip");
        }
        1 | 2 => {
            let ops = ["join", "cut", "intersect"];
            o.insert("operation".into(), json!(ops[usize::from(arg % 3)]));
        }
        _ => {
            let ts = [json!(["b"]), json!(["c"]), json!(["b", "c"])];
            o.insert("targets".into(), ts[usize::from(arg % 3)].clone());
        }
    }
}

/// A sheet metal feature's edits on either side: a base flange's
/// thickness, radius, allowance (a K-factor or a deduction) and relief; a
/// flange's angle, length, setback, edges (one added or taken out) and
/// ends (one set back, given a relief, or taken out).
fn edit_sheet_metal(v: &mut Value, op: u8, arg: u8) {
    let r = |a: u8| json!({"feature": "b", "name": format!("{:032x}", a % 4)});
    let reliefs = [
        json!({"kind": "none"}),
        json!({"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}),
        json!({"kind": "obround", "width": "1 mm", "depth": "1 mm"}),
        json!({"kind": "tear", "width": "0.5 mm"}),
    ];
    let o = v.as_object_mut().unwrap();
    let base = o.get("type") == Some(&json!("base_flange"));
    match (base, op % 4) {
        (true, 0) => {
            o.insert("thickness".into(), json!(format!("{} mm", 1 + arg % 3)));
        }
        (true, 1) => {
            o.insert("radius".into(), json!(format!("{} mm", 1 + arg % 4)));
        }
        (true, 2) if arg % 2 == 0 => {
            o.insert(
                "allowance".into(),
                json!({"k_factor": format!("0.{}", 3 + arg % 5)}),
            );
        }
        (true, 2) => {
            o.insert(
                "allowance".into(),
                json!({"deduction": format!("{} mm", 1 + arg % 3)}),
            );
        }
        (true, _) => {
            o.insert("relief".into(), reliefs[usize::from(arg % 4)].clone());
        }
        (false, 0) => {
            o.insert(
                "angle".into(),
                json!(format!("{} deg", 45 + 15 * (arg % 4))),
            );
        }
        (false, 1) => {
            o.insert("setback".into(), json!(format!("{} mm", arg % 5)));
        }
        (false, 2) => {
            let mut edges: Vec<Value> = o
                .get("edges")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let e = r(arg);
            match edges.iter().position(|x| *x == e) {
                Some(i) if edges.len() > 1 => {
                    edges.remove(i);
                }
                Some(_) => {}
                None => edges.push(e),
            }
            o.insert("edges".into(), json!(edges));
        }
        (false, _) => {
            let mut ends: Vec<Value> = o
                .get("ends")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let v = r(arg / 4);
            match ends.iter().position(|x| x["vertex"] == v) {
                Some(i) => {
                    ends.remove(i);
                }
                None => {
                    ends.push(json!({"vertex": v, "relief": reliefs[usize::from(arg % 4)].clone()}))
                }
            }
            if ends.is_empty() {
                o.remove("ends");
            } else {
                o.insert("ends".into(), json!(ends));
            }
        }
    }
}

/// An unfold's or refold's edits on either side: an unfold's allowance (a
/// K-factor, a deduction or a length), its fixed face, `only` (a bend
/// added or taken out) and `bends` (an override given or taken out); a
/// refold's unfold and `only`.
/// A thicken's source (a sheet, or faces of `b`), front, back or
/// operation, or a fill's sides (one replaced, one added) or tolerance.
fn edit_surface_feature(v: &mut Value, op: u8, arg: u8) {
    let face = |a: u8| json!({"feature": "b", "name": format!("{:032x}", a % 4)});
    let o = v.as_object_mut().unwrap();
    if o.get("type") == Some(&json!("thicken")) {
        match op % 4 {
            0 => {
                let from = match arg % 3 {
                    0 => json!({"sheet": "s"}),
                    1 => json!({"sheet": "t"}),
                    _ => json!({"faces": [face(arg)]}),
                };
                o.insert("from".into(), from);
            }
            1 => {
                o.insert("front".into(), json!(format!("{} mm", 1 + arg % 4)));
            }
            2 => {
                o.insert("back".into(), json!(format!("{} mm", arg % 3)));
            }
            _ => {
                if arg % 2 == 0 {
                    o.insert("operation".into(), json!("join"));
                    o.insert("targets".into(), json!(["b"]));
                } else {
                    o.insert("operation".into(), json!("new"));
                    o.remove("targets");
                }
            }
        }
    } else {
        match op % 3 {
            0 => {
                if let Some(Value::Array(sides)) = o.get_mut("sides") {
                    let i = arg as usize % sides.len();
                    sides[i] =
                        json!({"curve": {"feature": "s", "entity": format!("l{}", 4 + arg % 3)}});
                }
            }
            1 => {
                if let Some(Value::Array(sides)) = o.get_mut("sides") {
                    sides.push(json!({"edge": face(arg)}));
                }
            }
            _ => {
                o.insert("tolerance".into(), json!(format!("0.00{} mm", 1 + arg % 5)));
            }
        }
    }
}

/// A split by a tool: its tool a body (`t`, `u`) or a face of `t`, a
/// plane in its place, its flip or what it keeps.
fn edit_drawing(v: &mut Value, op: u8, arg: u8) {
    let mm = |a: u8| json!(format!("{} mm", u32::from(a) % 200 + 10));
    let o = v.as_object_mut().unwrap();
    match op % 8 {
        // A view moved, or its parent's distance changed.
        0 => {
            o["views"]["v1"]["at"] = json!([mm(arg), mm(arg / 2)]);
        }
        // Only a projected view has an offset (a retyped one is a detail).
        1 => {
            if o["views"]["v2"]["type"] == "projected" {
                o["views"]["v2"]["offset"] = mm(arg);
            }
        }
        // A view seen otherwise, or drawn with its hidden lines.
        2 => {
            let orient = ["front", "top", "right", "back"][usize::from(arg % 4)];
            o["views"]["v1"]["orient"] = json!(orient);
        }
        3 => match arg % 2 {
            0 => o["views"]["v1"]["hidden"] = json!("dashed"),
            _ => {
                o["views"]["v1"].as_object_mut().unwrap().remove("hidden");
            }
        },
        // A view retyped: a detail of its parent.
        4 => {
            o["views"]["v2"] = json!({"type": "detail", "sheet": "s1", "from": "v1",
                "centre": ["0 mm", "0 mm", "0 mm"], "radius": mm(arg / 8), "label": "B"});
        }
        // A dimension's tolerance, place or line.
        5 => {
            let t = match arg % 3 {
                0 => json!({"symmetric": "0.1 mm"}),
                1 => json!({"fit": "H7"}),
                _ => json!({"deviation": ["0.05 mm", "-0.02 mm"]}),
            };
            o["annotations"]["d1"]["tolerance"] = t;
        }
        6 => match arg % 2 {
            0 => o["annotations"]["d1"]["offset"] = mm(arg / 4),
            // Only a note that is there is moved: indexing a removed one
            // would write a note with no type.
            _ => {
                if let Some(n1) = o["annotations"].get_mut("n1") {
                    n1["at"] = json!([mm(arg), mm(arg / 3)]);
                }
            }
        },
        // An annotation added or taken away, the sheet's scale set.
        _ => match arg % 3 {
            0 => {
                o["annotations"].as_object_mut().unwrap().insert(
                    format!("n{}", arg % 5 + 2),
                    json!({"type": "note", "sheet": "s1", "text": "X",
                           "at": [mm(arg), mm(arg)]}),
                );
            }
            1 => {
                o["annotations"].as_object_mut().unwrap().remove("n1");
            }
            _ => o["sheets"]["s1"]["scale"] = json!(["1:1", "1:2", "2:1"][usize::from(arg % 3)]),
        },
    }
}

fn edit_split_tool(v: &mut Value, op: u8, arg: u8) {
    let o = v.as_object_mut().unwrap();
    match op % 4 {
        0 => {
            o.remove("plane");
            let b = if arg % 2 == 0 { "t" } else { "u" };
            o.insert("tool".into(), json!({"body": b}));
        }
        1 => {
            o.remove("plane");
            let face = json!({"feature": "t", "name": format!("{:032x}", arg % 4)});
            o.insert("tool".into(), json!({"faces": [face]}));
        }
        2 => {
            o.remove("tool");
            let p = if arg % 2 == 0 { "xy" } else { "yz" };
            o.insert("plane".into(), json!(p));
        }
        _ => {
            // A file writes flip only when true and keep only when behind.
            match arg % 4 {
                0 => o.insert("flip".into(), json!(true)),
                1 => o.remove("flip"),
                2 => o.insert("keep".into(), json!("behind")),
                _ => o.remove("keep"),
            };
        }
    }
    // A file is written at the lowest format that holds it: 19 for a tool,
    // 15.B's 10 for a plane.
    let format = if o.contains_key("tool") { 19 } else { 10 };
    o.insert("format".into(), json!(format));
}

fn edit_unfold(v: &mut Value, op: u8, arg: u8) {
    let r = |a: u8| json!({"feature": "f", "name": format!("{:032x}", a % 4)});
    let o = v.as_object_mut().unwrap();
    let unfold = o.get("type") == Some(&json!("unfold"));
    let toggle = |o: &mut serde_json::Map<String, Value>,
                  k: &str,
                  x: Value,
                  key: &dyn Fn(&Value) -> Value| {
        let mut list: Vec<Value> = o
            .get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match list.iter().position(|y| key(y) == key(&x)) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push(x),
        }
        if list.is_empty() {
            o.remove(k);
        } else {
            o.insert(k.into(), json!(list));
        }
    };
    match (unfold, op % 4) {
        (true, 0) => {
            let a = match arg % 3 {
                0 => json!({"k_factor": format!("0.{}", 3 + arg % 5)}),
                1 => json!({"deduction": format!("{} mm", 1 + arg % 3)}),
                _ => json!({"allowance": format!("{} mm", 2 + arg % 3)}),
            };
            o.insert("allowance".into(), a);
        }
        (true, 1) => {
            o.insert("fixed".into(), r(arg));
        }
        (true, 2) => {
            let b = json!({"bend": r(arg), "deduction": format!("{} mm", 1 + arg % 3)});
            toggle(o, "bends", b, &|y: &Value| y["bend"].clone());
        }
        (false, 0) => {
            o.insert("unfold".into(), json!(format!("u{}", arg % 3)));
        }
        (_, _) => toggle(o, "only", r(arg), &|y: &Value| y.clone()),
    }
}

fn text(v: &Value) -> Vec<u8> {
    canonical::to_string(v).into_bytes()
}

fn edit_annotation(v: &mut Value, op: u8, arg: u8) {
    let o = v.as_object_mut().unwrap();
    let who = |n: u8| json!({"participant": format!("p{}", n % 3), "kind": "agent"});
    match op % 7 {
        // A reply, numbered one past the largest, as ryucad numbers it.
        0 => {
            let replies = o.get_mut("replies").unwrap().as_object_mut().unwrap();
            let n = replies
                .keys()
                .filter_map(|k| k[1..].parse::<u64>().ok())
                .max()
                .unwrap_or(0);
            replies.insert(
                format!("r{}", n + 1),
                json!({"by": who(arg), "at": "2026-11-05T15:00:00Z", "text": format!("t{arg}")}),
            );
        }
        1 => {
            o.insert("status".into(), json!("in_progress"));
            o.insert("claimed_by".into(), who(arg));
            o.insert("resolution".into(), Value::Null);
        }
        2 => {
            o.insert(
                "status".into(),
                json!(if arg % 2 == 0 { "done" } else { "rejected" }),
            );
            o.insert("claimed_by".into(), Value::Null);
            o.insert(
                "resolution".into(),
                json!({"by": who(arg), "at": "2026-11-05T16:00:00Z",
                                                 "note": format!("n{arg}"), "applied": false}),
            );
        }
        3 => {
            o.insert("comment".into(), json!(format!("c{arg}")));
        }
        // Marks on either side (design 423): a line whose end is the
        // byte, or an arrow and a face stroke, at format 2.
        4 => {
            let x = f64::from(arg) / 1000.0;
            let list = if arg % 2 == 0 {
                json!([{"kind": "line", "in": "view", "xy_m": [[0, 0], [x, 0.001]], "touches": []}])
            } else {
                json!([{"kind": "arrow", "in": "view", "xy_m": [[0, 0], [0.001, x]],
                        "touches": [{"at": "tip", "ref": format!("p/b/{:032x}", arg)}]},
                       {"kind": "stroke", "in": "face", "on": "p/b",
                        "points_m": [[0, 0, 0], [x, 0, 0]], "touches": []}])
            };
            o.insert("format".into(), json!(2));
            o.insert("marks".into(), json!({"part": "p", "list": list}));
        }
        // Marks taken away: format 1 again.
        5 => {
            o.remove("marks");
            o.insert("format".into(), json!(1));
        }
        _ => {
            o.insert(
                "pins".into(),
                json!([{"part": "p", "point_m": [f64::from(arg), 0, 0]}]),
            );
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    // `0xfe` and `0xff` are the import modes (9 and 10), `0xfd` the
    // material library (11), `0xfc` a named view (12), `0xfb` an
    // appearance (13), `0xfa` an explode (14), `0xf9` a lock (15), `0xf8`
    // a manifest's dependencies (16), `0xf7` a datum (17), `0xf6` a path
    // (18), `0xf5` a sweep (19), `0xf4` a shell (20), `0xf3` a loft
    // (21), `0xf2` a mirror or split (22), `0xf1` an annotation (23),
    // `0xf0` a base flange or flange (24), `0xef` a planar face or a
    // surface extrude (25), `0xee` an unfold or refold (26), `0xed` a face
    // tweak (27), `0xec` a thicken or a fill (28), `0xeb` a draft (29),
    // `0xea` a split by a tool (30), `0xe9` a drawing (31); every other
    // byte picks as it did before them.
    let kind = match mode {
        0xe9 => 31,
        0xea => 30,
        0xeb => 29,
        0xec => 28,
        0xed => 27,
        0xee => 26,
        0xef => 25,
        0xf0 => 24,
        0xf1 => 23,
        0xf2 => 22,
        0xf3 => 21,
        0xf4 => 20,
        0xf5 => 19,
        0xf6 => 18,
        0xf7 => 17,
        0xf8 => 16,
        0xf9 => 15,
        0xfa => 14,
        0xfb => 13,
        0xfc => 12,
        0xfd => 11,
        0xfe => 9,
        0xff => 10,
        m => m % 9,
    };
    let (base, ours, theirs, readable) = if kind == 3 {
        let mut parts = rest.splitn(3, |b| *b == 0xff);
        let (b, o, t) = (
            parts.next().unwrap_or(&[]).to_vec(),
            parts.next().unwrap_or(&[]).to_vec(),
            parts.next().unwrap_or(&[]).to_vec(),
        );
        (b, o, t, false)
    } else {
        let base = match kind {
            0 => sketch(),
            1 => json!({"features": ["a", "b", "c", "d"], "format": 3, "kind": "ryucad.part",
                        "part_kind": "assembly", "items": {"q0": 1, "q1": 2}}),
            2 => json!({"format": 1, "kind": "ryucad.parameters",
                        "parameters": {"p0": "1 mm", "p1": "p0 * 2"}}),
            4 => json!({"format": 1, "kind": "ryupdm.properties", "name": "P", "number": "100"}),
            6 => json!({"format": 3, "kind": "ryucad.feature", "type": "pattern", "form": "linear",
                        "features": ["h0"], "direction": "x", "spacing": "3 mm", "count": "3",
                        "targets": ["b"]}),
            7 => json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "bolt",
                        "rev": "working",
                        "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            8 => json!({"format": 4, "kind": "ryucad.feature", "type": "mate", "form": "revolute",
                        "a": {"instance": ["base"], "at": "origin"},
                        "b": {"instance": ["pin"], "at": "origin"}}),
            9 => json!({"format": 5, "kind": "ryucad.feature", "type": "import", "body": 0,
                        "file": {"path": "imports/f.step", "blake3": "0".repeat(64)},
                        "product": {"id": "P-0", "name": "Part 0"}}),
            15 => json!({"format": 1, "kind": "ryucad.lock", "projects": {project(0): {
                         "labels": {"p0/A": {"commit": "0".repeat(40), "content": "0".repeat(64)}},
                         "path": "", "release_branch": "main", "url": "file:///lib", "via": []}}}),
            19 => json!({"format": 8, "kind": "ryucad.feature", "type": "sweep",
                         "profile": {"sketch": "s"}, "path": {"feature": "h0"},
                         "law": "minimal"}),
            21 => json!({"format": 8, "kind": "ryucad.feature", "type": "loft",
                         "sections": [{"sketch": "s0"}, {"sketch": "s1"}, {"sketch": "s2"}],
                         "start_tangent": {"direction": "normal"}}),
            20 => json!({"format": 9, "kind": "ryucad.feature", "type": "shell", "target": "b",
                         "thickness": "2 mm",
                         "open": [{"feature": "b", "name": format!("{:032x}", 1)}],
                         "faces": [{"thickness": "4 mm",
                                    "faces": [{"feature": "b", "name": format!("{:032x}", 2)}]}]}),
            23 => json!({"kind": "ryucad.annotation", "format": 1, "id": "aaaaaaaaaa",
                         "by": {"participant": "dave", "kind": "human"},
                         "at": "2026-11-05T14:02:11Z", "status": "open", "claimed_by": null,
                         "pins": [{"ref": format!("p/b/{:032x}", 1)}],
                         "comment": "move it",
                         "view": {"eye_m": [0.3, -0.3, 0.3], "target_m": [0, 0, 0], "up": [0, 0, 1],
                                  "projection": "perspective", "fov_deg": 40},
                         "change": null,
                         "replies": {"r1": {"by": {"participant": "agent", "kind": "agent"},
                                            "at": "2026-11-05T14:05:00Z", "text": "ok"}},
                         "resolution": null}),
            // A split when the edits leave no odd byte over, else a mirror.
            // A base flange when the edits leave no odd byte over, else a
            // flange.
            24 if rest.len() % 2 == 0 => json!({"format": 11, "kind": "ryucad.feature",
                "type": "base_flange", "sketch": "s", "thickness": "1.5 mm", "radius": "1 mm",
                "allowance": {"k_factor": "0.44"}}),
            24 => json!({"format": 11, "kind": "ryucad.feature", "type": "flange", "target": "b",
                "edges": [{"feature": "b", "name": format!("{:032x}", 1)}], "angle": "90 deg",
                "length": "20 mm"}),
            22 if rest.len() % 2 == 0 => {
                json!({"format": 10, "kind": "ryucad.feature", "type": "split", "target": "b",
                       "plane": {"feature": "d"}})
            }
            27 => json!({"format": 16, "kind": "ryucad.feature", "type": "move_face",
                         "target": "b", "by": "translate", "direction": "z", "distance": "5 mm",
                         "faces": [{"feature": "b", "name": format!("{:032x}", 1)}]}),
            29 => json!({"format": 18, "kind": "ryucad.feature", "type": "draft",
                         "target": "b", "angle": "2 deg", "neutral": "xy",
                         "faces": [{"feature": "b", "name": format!("{:032x}", 1)}]}),
            30 => json!({"format": 19, "kind": "ryucad.feature", "type": "split",
                         "target": "h", "tool": {"body": "t"}}),
            31 => json!({"format": 1, "kind": "ryucad.drawing", "name": "D",
                "sheets": {"s1": {"name": "Sheet 1", "size": "ansi_b"}},
                "views": {"v1": {"type": "base", "sheet": "s1", "orient": "front",
                                 "at": ["100 mm", "100 mm"]},
                          "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top",
                                 "offset": "60 mm"}},
                "annotations": {"d1": {"type": "linear", "view": "v1",
                                       "a": format!("b/{:032x}", 0), "b": format!("b/{:032x}", 1),
                                       "along": "horizontal", "offset": "10 mm"},
                                "n1": {"type": "note", "sheet": "s1", "text": "DEBURR",
                                       "at": ["20 mm", "20 mm"]}}}),
            // A thicken when the edits leave no odd byte over, else a fill.
            28 if rest.len() % 2 == 0 => json!({"format": 15, "kind": "ryucad.feature",
                "type": "thicken", "from": {"sheet": "s"}, "front": "1 mm", "back": "0 mm",
                "operation": "new"}),
            28 => json!({"format": 15, "kind": "ryucad.feature", "type": "fill",
                "sides": [{"curve": {"feature": "s", "entity": "l1"}},
                          {"curve": {"feature": "s", "entity": "l2"}},
                          {"curve": {"feature": "s", "entity": "l3"}}],
                "tolerance": "0.001 mm", "angle": "1 deg"}),
            // An unfold when the edits leave no odd byte over, else a
            // refold.
            26 if rest.len() % 2 == 0 => json!({"format": 14, "kind": "ryucad.feature",
                "type": "unfold", "target": "b", "allowance": {"k_factor": "0.44"},
                "only": [{"feature": "f", "name": format!("{:032x}", 1)}]}),
            26 => json!({"format": 14, "kind": "ryucad.feature", "type": "refold",
                "target": "b", "unfold": "u0"}),
            // A planar face when the edits leave no odd byte over, else a
            // surface extrude.
            25 if rest.len() % 2 == 0 => {
                json!({"format": 12, "kind": "ryucad.feature", "type": "planar",
                       "profile": {"sketch": "s", "loops": ["l1"]}})
            }
            25 => json!({"format": 12, "kind": "ryucad.feature", "type": "extrude",
                         "sketch": "s", "surface": true,
                         "extent": {"type": "blind", "length": "20 mm"}}),
            22 => json!({"format": 10, "kind": "ryucad.feature", "type": "mirror", "solid": "b",
                         "plane": "yz", "operation": "join", "targets": ["b"]}),
            18 => json!({"format": 7, "kind": "ryucad.feature", "type": "path",
                         "segments": [{"entity": "l1", "feature": "s"}, {"feature": "h0"}]}),
            17 => json!({"by": "offset", "distance": "5 mm", "format": 7,
                         "from": {"feature": "d0"}, "kind": "ryucad.feature", "type": "plane"}),
            16 => json!({"default_unit": "mm", "format": 3, "kind": "ryucad.project",
                         "project": "ee".repeat(16),
                         "dependencies": {project(0): {"name": "lib", "path": "",
                                                       "url": "file:///lib"}}}),
            14 => json!({"format": 1, "kind": "ryucad.explode", "steps": [
                {"instances": ["i0"], "translate": ["0 mm", "0 mm", "40 mm"]},
                {"instances": ["i1", "i2/x"], "translate": ["0 mm", "0 mm", "-10 mm"]}
            ]}),
            13 => json!({"faces": {format!("box/{:032x}", 0): {"colour": "#d03030"}},
                         "format": 1, "kind": "ryucad.appearance",
                         "solids": {"lid": {"opacity": 0.5}}}),
            12 => json!({"eye_m": [0.1, -0.1, 0.1], "format": 1, "fov_deg": 30,
                         "kind": "ryucad.view", "projection": "perspective",
                         "target_m": [0, 0, 0], "up": [0, 0, 1]}),
            11 => json!({"default": "m0", "format": 1, "kind": "ryucad.materials",
                         "materials": {"m0": {"density": "7850 kg/m^3"},
                                       "m1": {"colour": "#888888", "density": "2.7 g/cm^3"}}}),
            10 => json!({"format": 5, "kind": "ryucad.feature", "type": "instance", "part": "bolt",
                         "rev": "working", "fixed": true,
                         "import": {"file": "imports/f.step",
                                    "product": {"id": "ASM", "name": "Assembly"},
                                    "usage": "NAUO0"},
                         "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            _ => json!({"built_with": {"commit": "c", "ryucad": "0", "ryukei": "0"},
                        "content": "0".repeat(64), "format": 1, "kind": "ryupdm.release",
                        "label": "A", "part": "p", "project": "0123456789abcdef0123456789abcdef",
                        "properties": {"number": "100"}, "released_by": {"participant": "agent"},
                        "results": {}, "supersedes": null}),
        };
        let edit = |v: &mut Value, op: u8, arg: u8| match kind {
            0 => edit_sketch(v, op, arg),
            1 => edit_part(v, op, arg),
            2 => edit_parameters(v, op, arg),
            4 => edit_properties(v, op, arg),
            6 => edit_pattern(v, op, arg),
            7 => edit_instance(v, op, arg),
            8 => edit_mate(v, op, arg),
            9 => edit_import(v, op, arg),
            10 => edit_imported(v, op, arg),
            11 => edit_materials(v, op, arg),
            12 => edit_view(v, op, arg),
            13 => edit_appearance(v, op, arg),
            14 => edit_explode(v, op, arg),
            15 => edit_lock(v, op, arg),
            16 => edit_dependencies(v, op, arg),
            17 => edit_datum(v, op, arg),
            18 => edit_path(v, op, arg),
            19 => edit_sweep(v, op, arg),
            20 => edit_shell(v, op, arg),
            21 => edit_loft(v, op, arg),
            22 => edit_mirror(v, op, arg),
            23 => edit_annotation(v, op, arg),
            24 => edit_sheet_metal(v, op, arg),
            25 => edit_surface(v, op, arg),
            26 => edit_unfold(v, op, arg),
            27 => edit_tweak(v, op, arg),
            28 => edit_surface_feature(v, op, arg),
            29 => edit_draft(v, op, arg),
            30 => edit_split_tool(v, op, arg),
            31 => edit_drawing(v, op, arg),
            _ => edit_record(v, op, arg),
        };
        // Edits alternate between the sides: the high bit of `op` says which.
        let (mut ours, mut theirs) = (base.clone(), base.clone());
        for pair in rest.chunks_exact(2) {
            let side = if pair[0] & 0x80 == 0 {
                &mut ours
            } else {
                &mut theirs
            };
            edit(side, pair[0] & 0x7f, pair[1]);
        }
        // A side that changed nothing gives the other.
        let (out, outcome) = bytes(&text(&base), &text(&base), &text(&theirs));
        assert_eq!((out, outcome), (text(&theirs), Outcome::Clean));
        // Item numbers merge per part: a conflict is only ever at one.
        if kind == 1 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys().filter(|k| k.starts_with("/items")) {
                assert!(k.len() > "/items/".len(), "items conflicted whole: {v}");
            }
        }
        // Poses are solver state: moved on both sides, no conflict.
        if kind == 7 || kind == 10 {
            let (_, outcome) = bytes(&text(&base), &text(&ours), &text(&theirs));
            if let Outcome::Conflicts(_) = outcome {
                let v: Value =
                    serde_json::from_slice(&bytes(&text(&base), &text(&ours), &text(&theirs)).0)
                        .expect("JSON");
                assert!(
                    record(&v).keys().all(|k| !k.starts_with("/pose")),
                    "a pose conflicted: {v}"
                );
            }
        }
        // A lock's labels and a manifest's dependencies are whole values.
        if kind == 15 || kind == 16 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                let deep = k.split('/').count();
                let whole = if kind == 15 {
                    !(k.contains("/labels/") && deep > 5)
                } else {
                    !(k.starts_with("/dependencies/") && deep > 3)
                };
                assert!(whole, "conflicted inside an entry at {k}: {v}");
            }
        }
        // A sweep's profile is one value, and its law goes with its
        // direction: they conflict together, never apart.
        if kind == 19 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            let rec = record(&v);
            let keys: Vec<&str> = rec.keys().map(String::as_str).collect();
            for k in &keys {
                assert!(
                    ["", "/profile", "/law", "/direction", "/twist", "/scale"].contains(k),
                    "conflicted inside a sweep's value at {k}: {v}"
                );
            }
            assert_eq!(
                keys.contains(&"/law"),
                keys.contains(&"/direction"),
                "a sweep's law conflicted without its direction: {v}"
            );
        }
        // A loft's sections are one sequence: they conflict whole, and
        // nothing conflicts inside them.
        if kind == 21 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    !k.starts_with("/sections/"),
                    "conflicted inside a loft's sections at {k}: {v}"
                );
            }
        }
        // A path's segments are one sequence: they conflict whole.
        if kind == 18 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    ["", "/segments", "/reverse"].contains(&k.as_str()),
                    "conflicted inside a path's segments at {k}: {v}"
                );
            }
        }
        // A shell's open faces and groups are one value each: nothing
        // conflicts inside them.
        if kind == 20 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    ["", "/thickness", "/direction", "/open", "/faces"].contains(&k.as_str()),
                    "conflicted inside a shell's value at {k}: {v}"
                );
            }
        }
        // Every clean merge of annotations reads as one.
        if kind == 23 {
            let (merged, outcome) = bytes(&text(&base), &text(&ours), &text(&theirs));
            if outcome == Outcome::Clean {
                let v: Value = serde_json::from_slice(&merged).expect("JSON");
                ryucad::doc::annotation::Annotation::from_json(&v)
                    .expect("a clean merge of annotations reads");
            }
        }
        // A sheet metal feature's allowance and relief are one value each,
        // never conflicting inside.
        if kind == 24 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    !k.starts_with("/allowance/")
                        && !k.starts_with("/relief/")
                        && !k.starts_with("/ends/")
                        && !k.starts_with("/edges/"),
                    "conflicted inside a sheet metal feature's value at {k}: {v}"
                );
            }
        }
        // A planar face's profile is one value: nothing conflicts inside
        // it; a surface flag is a field of its own.
        if kind == 25 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    !k.starts_with("/profile/"),
                    "conflicted inside a planar's profile at {k}: {v}"
                );
            }
        }
        // An unfold's allowance, `only` and `bends` are whole values.
        if kind == 26 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    !k.starts_with("/allowance/")
                        && !k.starts_with("/only/")
                        && !k.starts_with("/bends/"),
                    "an unfold's {k} conflicted inside: {v}"
                );
            }
        }
        // A thicken's source is one value: never conflicted inside.
        if kind == 28 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    !k.starts_with("/from/") && !k.starts_with("/sides/"),
                    "conflicted inside a thicken's from or a fill's sides at {k}: {v}"
                );
            }
        }
        // A draft's hinge is one value: its fields conflict together or
        // not at all, and its lists are never conflicted inside.
        if kind == 29 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            let rec = record(&v);
            let keys: Vec<&String> = rec.keys().collect();
            let hinge = ["/neutral", "/parting", "/pull", "/reverse"];
            let held = hinge
                .iter()
                .filter(|k| keys.iter().any(|x| x == *k))
                .count();
            assert!(
                held == 0 || held == 4,
                "a draft's hinge conflicted in part: {v}"
            );
            for k in keys {
                assert!(
                    !k.starts_with("/faces/")
                        && !k.starts_with("/groups/")
                        && !k.starts_with("/flip/"),
                    "conflicted inside a draft's list at {k}: {v}"
                );
            }
        }
        // A drawing merges per member and field: a place, a tolerance and
        // a view's orient are one value each, never conflicting inside, a
        // retyped view conflicts whole, and a clean merge reads.
        if kind == 31 {
            let (merged, outcome) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    !k.contains("/at/") && !k.contains("/tolerance/") && !k.contains("/orient/"),
                    "conflicted inside a drawing's value at {k}: {v}"
                );
            }
            if outcome == Outcome::Clean {
                ryucad::doc::drawing::DrawingFile::from_json(&v)
                    .expect("a clean merge of a drawing reads");
            }
        }
        // A split's plane, tool and flip are one value together, never
        // conflicting inside the tool, and a clean merge reads.
        if kind == 30 {
            let (merged, outcome) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    ["", "/plane", "/tool", "/flip", "/keep"].contains(&k.as_str()),
                    "conflicted inside a split's tool at {k}: {v}"
                );
            }
            if outcome == Outcome::Clean {
                ryucad::doc::files::Feature::from_json(
                    &v,
                    ryucad::units::Unit::Mm,
                    &ryucad::expr::Env::empty(),
                )
                .expect("a clean merge of a split reads");
            }
        }
        // A split's plane and flip are one value together; a mirror's
        // fields merge per key, its targets a list.
        if kind == 22 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    ["", "/plane", "/flip", "/keep", "/operation", "/targets"]
                        .contains(&k.as_str()),
                    "conflicted inside a mirror's or split's value at {k}: {v}"
                );
            }
        }
        // A datum's references are whole values, and built another way on
        // both sides it is one value.
        if kind == 17 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            for k in record(&v).keys() {
                assert!(
                    k.split('/').count() <= 2,
                    "conflicted inside a value at {k}: {v}"
                );
            }
            if ours["by"] != base["by"] && theirs["by"] != base["by"] && ours != theirs {
                assert!(
                    record(&v).contains_key(""),
                    "a datum built two ways merged: {v}"
                );
            }
        }
        // An explode's steps are one sequence: they conflict whole.
        if kind == 14 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            assert!(
                record(&v).keys().all(|k| k == "/steps"),
                "an explode conflicted inside its steps: {v}"
            );
        }
        // A place is one value: never a conflict inside it.
        if kind == 9 {
            let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
            let v: Value = serde_json::from_slice(&merged).expect("JSON");
            assert!(
                record(&v).keys().all(|k| !k.starts_with("/place/")),
                "a place conflicted inside itself: {v}"
            );
        }
        // Two re-imports of different bytes conflict on the hash.
        if kind == 9 {
            let hash = |v: &Value| v["file"]["blake3"].clone();
            if hash(&ours) != hash(&base)
                && hash(&theirs) != hash(&base)
                && hash(&ours) != hash(&theirs)
            {
                let (merged, _) = bytes(&text(&base), &text(&ours), &text(&theirs));
                let v: Value = serde_json::from_slice(&merged).expect("JSON");
                assert!(
                    record(&v).keys().any(|k| k.starts_with("/file")),
                    "two re-imports merged: {v}"
                );
            }
        }
        (text(&base), text(&ours), text(&theirs), true)
    };
    let (out, outcome) = bytes(&base, &ours, &theirs);
    assert_eq!(bytes(&base, &ours, &theirs), (out.clone(), outcome));
    match outcome {
        Outcome::Text(_) => assert!(!readable, "readable files fell back to lines"),
        Outcome::Clean | Outcome::Conflicts(_) => {
            let text = std::str::from_utf8(&out).expect("UTF-8");
            assert!(canonical::is_canonical(text), "not canonical: {text}");
            let v: Value = serde_json::from_str(text).expect("JSON");
            let n = record(&v).len();
            match outcome {
                Outcome::Clean => assert!(v.get("conflicts").is_none()),
                _ => assert_eq!(Outcome::Conflicts(n), outcome),
            }
        }
    }
});
