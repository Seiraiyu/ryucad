//! A sheet against a revision (design 518, plan 4.B): what `render
//! --compare` and `output --compare` paint. Each owner of the working
//! sheet is marked added or changed by its object's file (a component, an
//! instance, a port, a net) and its layout keys, moved when only its
//! place changed; what the revision drew and the working files don't is
//! ghosted from the revision's own painting. Keys are layout keys
//! (`ryusch::paint`'s owners), turned into reference paths only in SVG.

use crate::design::BlockFiles;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use crate::schem::{self, Laid, part_of};
use ryu_session::rpc::Answer;
use ryusch::diff::Change;
use ryusch::paint::{Mark, Marks};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A loaded file's JSON, to compare.
fn json_of<T: serde::Serialize>(l: &crate::library::Loaded<T>) -> Option<Value> {
    l.item
        .as_ref()
        .ok()
        .map(|x| serde_json::to_value(x).expect("files serialize"))
}

/// The objects of block `block` whose file is new or differs, by the key
/// their parts carry: `<component>`, `instance:<id>`, `port:<id>`; and
/// its nets likewise, by net key.
fn objects(
    now: &Project,
    then: &Project,
    block: &Id,
) -> (BTreeMap<String, Mark>, BTreeMap<String, Mark>) {
    let (mut parts, mut nets) = (BTreeMap::new(), BTreeMap::new());
    let (a, Some(b)) = (then.conn.blocks.get(block), now.conn.blocks.get(block)) else {
        return (parts, nets);
    };
    let judge = |old: Option<Option<Value>>, new: Option<Value>| match old {
        None => Some(Mark::Added),
        Some(o) if o != new => Some(Mark::Changed),
        _ => None,
    };
    for (id, c) in &b.components {
        let old = a.map(|a| a.components.get(id).map(json_of)).unwrap_or(None);
        if let Some(m) = judge(old, json_of(c)) {
            parts.insert(id.to_string(), m);
        }
    }
    for (id, i) in &b.instances {
        let old = a.map(|a| a.instances.get(id).map(json_of)).unwrap_or(None);
        if let Some(m) = judge(old, json_of(i)) {
            parts.insert(format!("instance:{id}"), m);
        }
    }
    let ports = |x: Option<&BlockFiles>| -> BTreeMap<String, Value> {
        x.and_then(|x| x.block.item.as_ref().ok())
            .map(|x| {
                x.ports
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.to_string(),
                            serde_json::to_value(v).expect("ports serialize"),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (pa, pb) = (ports(a), ports(Some(b)));
    for (id, v) in &pb {
        if let Some(m) = judge(pa.get(id).map(|x| Some(x.clone())), Some(v.clone())) {
            parts.insert(format!("port:{id}"), m);
        }
    }
    for (id, n) in &b.nets {
        let old = a.map(|a| a.nets.get(id).map(json_of)).unwrap_or(None);
        if let Some(m) = judge(old, json_of(n)) {
            nets.insert(id.to_string(), m);
        }
    }
    (parts, nets)
}

/// Block `block`'s marks by sheet: its working layout `now` against the
/// revision's `then` (none when the revision hasn't the block), `rev` as
/// the strip names it.
pub fn marks(
    now_p: &Project,
    now: &Laid,
    then_p: &Project,
    then: Option<&Laid>,
    block: &Id,
    rev: &str,
) -> BTreeMap<String, Marks> {
    let (parts, nets) = objects(now_p, then_p, block);
    let empty = ryusch::layout::Layout::default();
    let old = then.map_or(&empty, |t| &t.outcome.layout);
    let new = &now.outcome.layout;
    let mut out: BTreeMap<String, Marks> = new
        .sheets
        .keys()
        .map(|s| (s.clone(), Marks::default()))
        .collect();
    let mark = |out: &mut BTreeMap<String, Marks>, sheet: &str, owner: String, m: Mark| {
        if let Some(x) = out.get_mut(sheet) {
            x.marks.entry(owner).or_insert(m);
        }
    };
    let ghost = |out: &mut BTreeMap<String, Marks>, sheet: &str, owner: String| {
        if let Some(x) = out.get_mut(sheet) {
            x.ghosts.insert(owner);
        }
    };
    // Parts whose file is new or differs.
    for part in &now.built.input.parts {
        if let (Some(at), Some(m)) = (new.placed.get(&part.key), parts.get(part_of(&part.key))) {
            mark(&mut out, &at.sheet, part.key.clone(), *m);
        }
    }
    // Nets whose file is new or differs: their wires and labels.
    for (net, m) in &nets {
        for sheet in new.routes.get(net).into_iter().flat_map(|s| s.keys()) {
            mark(&mut out, sheet, net.clone(), *m);
        }
        for l in new.labels.get(net).into_iter().flatten() {
            mark(&mut out, &l.sheet, format!("label:{net}@{}", l.on), *m);
        }
    }
    // The layouts, by key.
    for e in ryusch::diff::layouts(old, new) {
        let Some(sheet) = e.sheet.clone() else {
            continue;
        };
        let k = e.key.key.clone();
        let (owner, wires) = match e.key.section {
            "placed" | "text" => (k.clone(), false),
            "labels" => (format!("label:{}", k.replacen('|', "@", 1)), false),
            "routes" => (k.split('|').next().unwrap_or_default().to_owned(), true),
            "buses" => (k.split('|').next().unwrap_or_default().to_owned(), true),
            "regions" => (format!("region:{k}"), false),
            "notes" => (format!("note:{k}"), false),
            _ => continue,
        };
        match e.change {
            Change::Added => mark(&mut out, &sheet, owner, Mark::Added),
            Change::Removed => ghost(&mut out, &sheet, owner),
            Change::Moved => {
                if let (Some(a), Some(b)) = (old.placed.get(&k), new.placed.get(&k)) {
                    mark(
                        &mut out,
                        &sheet,
                        owner,
                        Mark::Moved {
                            from: a.at,
                            to: b.at,
                        },
                    );
                }
            }
            Change::Changed => {
                mark(&mut out, &sheet, owner.clone(), Mark::Changed);
                if wires {
                    // The old wires, ghosted under the new.
                    ghost(&mut out, &sheet, owner.clone());
                }
                if e.key.section == "placed"
                    && let Some(a) = old.placed.get(&k)
                    && a.sheet != sheet
                {
                    ghost(&mut out, &a.sheet, owner);
                }
            }
        }
    }
    for m in out.values_mut() {
        if m.is_empty() {
            continue;
        }
        let n = |f: &dyn Fn(&Mark) -> bool| m.marks.values().filter(|x| f(x)).count();
        let (added, moved) = (
            n(&|x| *x == Mark::Added),
            n(&|x| matches!(x, Mark::Moved { .. })),
        );
        let changed = m.marks.len() - added - moved;
        m.strip = format!(
            "Against {rev}: {added} added, {changed} changed, {moved} moved, {} removed",
            m.ghosts
                .iter()
                .filter(|g| !m.marks.contains_key(*g))
                .count()
        );
    }
    out
}

/// Block `b` of the revision's project `then`, laid out, with the
/// designators of the layer the working drawing shows (`layer`'s design
/// and path), or `None` when the revision hasn't the block or it doesn't
/// lay out there.
pub fn laid_then(then: &Project, b: &Id, layer: Option<&schem::Layer>) -> Option<Laid> {
    let tb = then.conn.blocks.get(b)?;
    let mut l = schem::lay(then, tb, None, &[], ryusch::input::Relayout::None).ok()?;
    let all = schem::layers(then, b);
    let same = layer
        .and_then(|x| {
            all.iter()
                .find(|t| t.design == x.design && t.path == x.path)
        })
        .or(all.first());
    if let Some(t) = same {
        l.built.designators(&t.refdes);
    }
    Some(l)
}

/// `render --compare`: block `block`'s canvas (or one sheet) from the
/// working files `now`, marked against `then` (`rev` naming `commit`).
#[allow(clippy::too_many_arguments)]
pub fn render(
    now: &Project,
    then: &Project,
    rev: &str,
    commit: &str,
    design: Option<&str>,
    block: Option<&str>,
    sheet: Option<&str>,
    layer: Option<&str>,
    format: crate::schemcmd::Format,
    scale: Option<f32>,
    fit: bool,
    theme: ryusch::style::Theme,
) -> Result<Answer> {
    let b = crate::schemcmd::blocks(now, design, block)?[0];
    let mut laid = schem::lay(now, b, None, &[], ryusch::input::Relayout::None)?;
    let (_, shown) = crate::schemcmd::show_layer(now, b, &mut laid, layer)?;
    let old = laid_then(then, &b.id, shown.as_ref());
    let marks = marks(now, &laid, then, old.as_ref(), &b.id, rev);
    let layout = &laid.outcome.layout;
    let which: Vec<String> = match sheet {
        Some(s) => vec![
            layout
                .sheets
                .iter()
                .find(|(k, v)| *k == s || v.name == s)
                .map(|(k, _)| k.clone())
                .ok_or_else(|| {
                    Error::new(Code::NotFound, format!("block {} has no sheet {s}", b.id))
                        .with("sheet", s)
                })?,
        ],
        None => Vec::new(),
    };
    let refs: Vec<&str> = which.iter().map(String::as_str).collect();
    let looks = crate::schemcmd::looks(now, b, &laid.built, layout, theme);
    let then_looks = match (&old, then.conn.blocks.get(&b.id)) {
        (Some(o), Some(tb)) => crate::schemcmd::looks(then, tb, &o.built, &o.outcome.layout, theme),
        _ => BTreeMap::new(),
    };
    let none = Marks::default();
    let to_ref = |k: &str| schem::ref_path(&b.id, k);
    let svg = ryusch::svg::draw_with(
        &laid.built.input,
        layout,
        &refs,
        fit,
        &|sid| looks.get(sid).cloned().unwrap_or_default(),
        &|sid, look| {
            let now = ryusch::paint::sheet_in(&laid.built.input, layout, sid, look);
            let was = old
                .as_ref()
                .filter(|o| o.outcome.layout.sheets.contains_key(sid))
                .map(|o| {
                    ryusch::paint::sheet_in(
                        &o.built.input,
                        &o.outcome.layout,
                        sid,
                        &then_looks.get(sid).cloned().unwrap_or_default(),
                    )
                });
            ryusch::paint::compare(&now, was.as_ref(), marks.get(sid).unwrap_or(&none), look)
        },
        Some(&to_ref as &dyn Fn(&str) -> String),
    );
    let counts: BTreeMap<&String, &String> = marks
        .iter()
        .filter(|(_, m)| !m.is_empty())
        .map(|(s, m)| (s, &m.strip))
        .collect();
    let mut v = json!({
        "block": b.id,
        "sheets": if which.is_empty() { layout.sheets.keys().cloned().collect() } else { which.clone() },
        "format": format,
        "theme": theme,
        "compare": {"rev": rev, "commit": commit},
        "changed": counts,
    });
    if let Some(l) = &shown {
        v["layer"] = l.to_json();
    }
    Ok(match format {
        crate::schemcmd::Format::Svg => Answer::bytes(v, "image/svg+xml", svg.into_bytes()),
        crate::schemcmd::Format::Png => {
            let px = scale.unwrap_or(4.0);
            if !(0.5..=40.0).contains(&px) {
                return Err(Error::new(
                    Code::InvalidParam,
                    "scale is 0.5 to 40 pixels per millimetre",
                )
                .with("param", "scale"));
            }
            v["scale"] = json!(px);
            Answer::bytes(v, "image/png", crate::schemcmd::png(&svg, px)?)
        }
    })
}
