//! The schematic's requests: `layout`, `layout.place` (instructions,
//! moves and releases, with fixes when refused), `layout.instructions`,
//! `describe` and `render` (SVG, or PNG by resvg with the bundled font).
//! Writes are one logged, undoable change each, guarded by the block's
//! revision (the project's for a whole design).

use crate::design::BlockFiles;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use crate::proto::Answer;
use crate::schem::{self, Laid};
use crate::server::check_revision;
use crate::session::Session;
use crate::vocab::{Names, Released, Said};
use crate::write::{self, Writes};
use ryusch::input::Relayout;
use ryusch::layout::Instruction;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Inter Regular, the one face of every schematic.
pub const INTER: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");

/// The blocks a request names: one block, else a design's (named, or the
/// project's only one), its root first.
fn blocks<'a>(
    p: &'a Project,
    design: Option<&str>,
    block: Option<&str>,
) -> Result<Vec<&'a BlockFiles>> {
    if let Some(b) = block {
        return Ok(vec![write::block(p, b)?]);
    }
    let d = crate::netlist::one_design(p, design)?;
    let root = crate::netlist::design_root(p, &d)?;
    p.conn
        .reached(&root)
        .into_iter()
        .map(|id| p.conn.block(id))
        .collect()
}

/// A block's summary after a layout: sheets with their scores, notes,
/// findings and whether its file was stale or invalid.
fn summary(p: &Project, b: &BlockFiles, laid: &Laid) -> Value {
    let (findings, scores) =
        ryusch::lint::lint_with(&laid.built.input, &laid.outcome, &crate::lint::prefs(p));
    let sheets: Vec<Value> = laid
        .outcome
        .layout
        .sheets
        .iter()
        .map(|(id, s)| {
            json!({
                "sheet": id,
                "name": s.name,
                "score": scores.iter().find(|x| &x.sheet == id).map(|x| x.score),
            })
        })
        .collect();
    let mut v = json!({
        "block": b.id,
        "sheets": sheets,
        "findings": findings.len(),
        "notes": laid.outcome.notes,
    });
    if laid.stale {
        v["stale"] = json!(true);
    }
    if let Some(e) = &laid.invalid {
        v["invalid"] = json!(e);
    }
    let _ = p;
    v
}

/// `layout`: lays out a design's blocks (or one block) over their layout
/// files, writing those that change; `relayout` (`<block>` or
/// `<block>/<sheet>`) lays out from nothing what it names.
pub fn layout(
    session: &mut Session,
    design: Option<&str>,
    block: Option<&str>,
    relayout: Option<&str>,
    dry_run: bool,
    expected: Option<&str>,
    render: Option<RenderAsk>,
) -> Result<Answer> {
    if render.is_some() && !dry_run {
        return Err(Error::new(
            Code::InvalidParam,
            "a picture is of a dry run: give --dry-run with --render",
        )
        .with("param", "render"));
    }
    if render.is_some() && block.is_none() && relayout.is_none() {
        return Err(Error::new(
            Code::InvalidParam,
            "a picture is of one block: give --block or --relayout",
        )
        .with("param", "render"));
    }
    let p = session.project();
    let list = blocks(p, design, block)?;
    match (block, list.first()) {
        (Some(_), Some(b)) => check_revision(&format!("block {}", b.id), &b.revision, expected)?,
        _ => check_revision("the project", &p.revision, expected)?,
    }
    let fresh: Option<(Id, Option<String>)> = relayout
        .map(|r| -> Result<(Id, Option<String>)> {
            let (b, s) = match r.split_once('/') {
                Some((b, s)) => (b, Some(s.to_owned())),
                None => (r, None),
            };
            Ok((write::block(p, b)?.id.clone(), s))
        })
        .transpose()?;
    // The one block a picture is of: --block, else --relayout's.
    let pictured: Option<Id> = render.and(match block {
        Some(_) => list.first().map(|b| b.id.clone()),
        None => fresh.as_ref().map(|(id, _)| id.clone()),
    });
    let mut picture_of = None;
    let mut writes = Writes::default();
    let mut out = Vec::new();
    for b in &list {
        let how = match &fresh {
            Some((id, None)) if *id == b.id => Relayout::Block,
            Some((id, Some(s))) if *id == b.id => {
                let sheet = match &b.layout {
                    Some(Ok(l)) => l
                        .sheets
                        .iter()
                        .find(|(k, v)| *k == s || v.name == *s)
                        .map(|(k, _)| k.clone()),
                    _ => None,
                }
                .ok_or_else(|| {
                    Error::new(Code::NotFound, format!("block {} has no sheet {s}", b.id))
                        .with("sheet", s)
                })?;
                Relayout::Sheet(sheet)
            }
            _ => Relayout::None,
        };
        let laid = schem::lay(p, b, None, &[], how)?;
        let text = schem::layout_text(&laid.outcome.layout);
        let path = schem::layout_path(&b.id);
        let now = session.snapshot().get(&path).map(|x| x.to_vec());
        let changed = now.as_deref() != Some(text.as_bytes());
        if changed && !dry_run {
            writes.0.push((path, Some(text)));
        }
        let mut v = summary(p, b, &laid);
        v["written"] = json!(changed && !dry_run);
        out.push(v);
        if pictured.as_ref() == Some(&b.id) {
            picture_of = Some(laid);
        }
    }
    if let Some(ask) = render {
        let laid = picture_of.ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                "the block to draw isn't among those laid out: give --block",
            )
            .with("param", "render")
        })?;
        let (mime, bytes, px) = picture(&laid, &[], ask)?;
        let mut v = json!({"blocks": out, "dry_run": dry_run, "format": ask.format});
        if let Some(px) = px {
            v["scale"] = json!(px);
        }
        return Ok(Answer::bytes(v, mime, bytes));
    }
    if !writes.0.is_empty() {
        write::commit(session, "the layout", writes)?;
    }
    let p = session.project();
    for v in &mut out {
        let id = Id::parse(v["block"].as_str().expect("an ID"), "block")?;
        v["revision"] = json!(p.conn.block(&id)?.revision);
    }
    Ok(Answer::json(json!({"blocks": out, "dry_run": dry_run})))
}

/// The fixes for a refusal of `batch`: batches that would hold.
fn fixes(
    names: &Names,
    existing: &BTreeMap<String, Instruction>,
    batch: &[(String, String)],
    e: &Error,
) -> Value {
    let ids: Vec<String> = e.details["instructions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<Vec<String>> = Vec::new();
    let new: Vec<&(String, String)> = batch.iter().collect();
    // The batch without what is refused.
    let without: Vec<String> = new
        .iter()
        .filter(|(id, _)| !ids.contains(id))
        .map(|(_, t)| t.clone())
        .collect();
    // Replacing an existing instruction: release its subjects first.
    for id in &ids {
        if let Some(old) = existing.get(id) {
            let subjects: Vec<String> = old.parts().iter().map(|k| names.part_text(k)).collect();
            // By its subjects where it has some, else (a net's style) by
            // its ID.
            let mut f = vec![if subjects.is_empty() {
                format!("release {id}")
            } else {
                format!("release {}", subjects.join(" "))
            }];
            f.extend(new.iter().map(|(_, t)| t.clone()));
            out.push(f);
        }
    }
    if e.code == Code::LayoutRule {
        for (id, t) in &new {
            if ids.contains(id) && t.starts_with("net ") && t.ends_with(" port") {
                let mut f = without.clone();
                f.push(format!("{} label", t.trim_end_matches(" port")));
                out.push(f);
            }
        }
    }
    // A conflict inside the batch: each of its instructions alone.
    let mine: Vec<&String> = ids
        .iter()
        .filter(|id| new.iter().any(|(b, _)| b == *id))
        .collect();
    if mine.len() > 1 {
        for keep in &mine {
            out.push(
                new.iter()
                    .filter(|(id, _)| id == *keep || !ids.contains(id))
                    .map(|(_, t)| t.clone())
                    .collect(),
            );
        }
    }
    if without.len() < new.len() {
        out.push(without);
    }
    // An empty batch is never a fix.
    out.retain(|f| !f.is_empty());
    distinct(&mut out);
    json!(out)
}

/// Drops every fix equal to one before it, keeping the first's place.
fn distinct(fixes: &mut Vec<Vec<String>>) {
    let mut seen = std::collections::BTreeSet::new();
    fixes.retain(|f| seen.insert(f.clone()));
}

/// `layout.place`: a batch of instructions (lasting, by text), moves
/// (applied once, their results held) and releases, all or nothing.
pub fn place(
    session: &mut Session,
    block: &str,
    instructions: &[String],
    moves: &[String],
    dry_run: bool,
    expected: Option<&str>,
    render: Option<RenderAsk>,
) -> Result<Answer> {
    if render.is_some() && !dry_run {
        return Err(Error::new(
            Code::InvalidParam,
            "a picture is of a dry run: give --dry-run with --render",
        )
        .with("param", "render"));
    }
    let p = session.project();
    let b = write::block(p, block)?;
    check_revision(&format!("block {}", b.id), &b.revision, expected)?;
    let built = schem::build(p, b)?;
    let names = Names {
        p,
        b,
        built: &built,
    };
    let mut lasting: BTreeMap<String, Instruction> = match &b.layout {
        Some(Ok(l)) => l.instructions.clone(),
        _ => BTreeMap::new(),
    };
    let before = lasting.clone();
    let mut added = Vec::new();
    let mut released = Vec::new();
    // Sheets renamed: (index, by ID or name, new name).
    let mut renames: Vec<(usize, String, String)> = Vec::new();
    // What the released instructions placed, laid out again.
    let mut replace: Vec<String> = Vec::new();
    let mut batch: Vec<(String, String)> = Vec::new();
    for (i, text) in instructions.iter().enumerate() {
        let at = |e: Error| e.with("index", i).with("text", text.as_str());
        match names.read(text).map_err(at)? {
            Said::Release(said) => {
                for r in &said {
                    let gone: Vec<String> = lasting
                        .iter()
                        .filter(|(id, ins)| match r {
                            Released::Part(k) => crate::vocab::names(ins, k),
                            Released::Id(i) => *id == i,
                            Released::Net(n) => {
                                matches!(ins, Instruction::Net { net, .. } if net == n)
                            }
                        })
                        .map(|(id, _)| id.clone())
                        .collect();
                    if gone.is_empty() && !matches!(r, Released::Part(_)) {
                        let what = match r {
                            Released::Id(i) => format!("instruction {i}"),
                            Released::Net(n) => format!("instruction on net {}", names.net_text(n)),
                            Released::Part(_) => unreachable!(),
                        };
                        return Err(Error::new(
                            Code::NotFound,
                            format!("block {} has no {what} to release", b.id),
                        )
                        .with("index", i)
                        .with("text", text.as_str()));
                    }
                    for id in gone {
                        if let Some(ins) = lasting.remove(&id) {
                            replace.extend(ins.parts().iter().cloned());
                        }
                        released.push(id);
                    }
                }
            }
            Said::RenameSheet { from, to } => renames.push((i, from, to)),
            Said::Do(Instruction::Shift { .. }) => {
                return Err(
                    Error::new(Code::InvalidParam, "shift is a move: give it with --move")
                        .with("index", i)
                        .with("text", text.as_str()),
                );
            }
            Said::Do(ins) => {
                // Repeating an instruction changes nothing.
                if let Some((id, _)) = lasting.iter().find(|(_, x)| **x == ins) {
                    batch.push((id.clone(), text.clone()));
                    continue;
                }
                let id = ryusch::layout::next_id(lasting.keys());
                lasting.insert(id.clone(), ins);
                added.push(id.clone());
                batch.push((id, text.clone()));
            }
        }
    }
    let mut move_list = Vec::new();
    for (i, text) in moves.iter().enumerate() {
        match names
            .read(text)
            .map_err(|e| e.with("move", i).with("text", text.as_str()))?
        {
            Said::Do(ins) => {
                move_list.push(ins);
                batch.push((format!("move {}", i + 1), text.clone()));
            }
            Said::Release(_) | Said::RenameSheet { .. } => {
                return Err(Error::new(
                    Code::InvalidParam,
                    "release and sheet rename are instructions, not moves",
                )
                .with("move", i));
            }
        }
    }
    // Renamed sheets: the stored layout's sheet takes its new name, and
    // lasting sheet instructions follow it.
    let renamed_block;
    let b = if renames.is_empty() {
        b
    } else {
        let Some(Ok(mut l)) = b.layout.clone() else {
            return Err(Error::new(
                Code::NotFound,
                format!("block {} has no layout yet", b.id),
            ));
        };
        for (i, from, to) in &renames {
            let Some(id) = l
                .sheets
                .iter()
                .find(|(id, x)| *id == from || x.name == *from)
                .map(|(id, _)| id.clone())
            else {
                return Err(Error::new(
                    Code::NotFound,
                    format!("block {} has no sheet {from}", b.id),
                )
                .with("index", *i)
                .with("sheet", from.as_str()));
            };
            let old =
                std::mem::replace(&mut l.sheets.get_mut(&id).expect("found").name, to.clone());
            for ins in lasting.values_mut() {
                if let Instruction::Sheet { sheet, .. } = ins
                    && *sheet == old
                {
                    *sheet = to.clone();
                }
            }
        }
        let mut c = b.clone();
        c.layout = Some(Ok(l));
        renamed_block = c;
        &renamed_block
    };
    replace.sort();
    replace.dedup();
    let relayout = if replace.is_empty() {
        Relayout::None
    } else {
        Relayout::Parts(replace)
    };
    let laid = match schem::lay(p, b, Some(&lasting), &move_list, relayout) {
        Ok(l) => l,
        Err(e) if matches!(e.code, Code::InstructionConflict | Code::LayoutRule) => {
            let f = fixes(&names, &before, &batch, &e);
            return Err(e.with("fixes", f));
        }
        Err(e) => return Err(e),
    };
    // A layout that breaks a hard rule is never written (the next read
    // would lay the block out from nothing, losing every earlier move).
    let broken = ryusch::rules::check(&laid.built.input, &laid.outcome.layout);
    if let Some(first) = broken.first() {
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let mut fixes: Vec<Vec<String>> = Vec::new();
        if texts.len() > 1 {
            fixes.push(texts[..texts.len() - 1].to_vec());
            fixes.extend(texts.iter().map(|t| vec![t.clone()]));
        }
        fixes.retain(|f| !f.is_empty());
        distinct(&mut fixes);
        return Err(Error::new(
            Code::LayoutRule,
            format!(
                "this layout would break a hard rule ({}): {}",
                first.rule, first.message
            ),
        )
        .with(
            "broken",
            json!(broken
                .iter()
                .map(|v| json!({"rule": v.rule, "sheet": v.sheet, "message": v.message, "refs": v.refs}))
                .collect::<Vec<_>>()),
        )
        .with("fixes", json!(fixes)));
    }
    let bid = b.id.clone();
    let mut listing = listing(&names, &laid);
    // What changed nothing says so: a lasting instruction added in this
    // batch, a move.
    let prev = match &b.layout {
        Some(Ok(l)) => Some(l),
        _ => None,
    };
    for v in &mut listing {
        let id = v["id"].as_str().unwrap_or_default().to_owned();
        if added.contains(&id)
            && let Some(ins) = lasting.get(&id)
            && unchanged(&laid.built.input, prev, &laid.outcome.layout, ins)
        {
            v["unchanged"] = json!(true);
        }
    }
    let mut notes = serde_json::to_value(&laid.outcome.notes).unwrap_or(json!([]));
    for (i, ins) in move_list.iter().enumerate() {
        if unchanged(&laid.built.input, prev, &laid.outcome.layout, ins) {
            let mut n = json!({
                "code": "move_unchanged",
                "message": format!("move {} ({}) changed nothing", i + 1, names.text(ins)),
                "details": {"move": i + 1},
            });
            if let Err(why) = ryusch::instr::judge(&laid.built.input, &laid.outcome.layout, ins) {
                n["details"]["why"] = json!(why);
            }
            if let Some(a) = notes.as_array_mut() {
                a.push(n);
            }
        }
    }
    // A batch that costs a sheet more than 10 points says so, naming the
    // findings that grew (round 1, F5: moves scattered what they touched).
    if let Ok(was) = schem::lay(p, b, None, &[], Relayout::None) {
        let prefs = crate::lint::prefs(p);
        let (f0, s0) = ryusch::lint::lint_with(&was.built.input, &was.outcome, &prefs);
        let (f1, s1) = ryusch::lint::lint_with(&laid.built.input, &laid.outcome, &prefs);
        for after in &s1 {
            let Some(before) = s0.iter().find(|x| x.sheet == after.sheet) else {
                continue;
            };
            if i64::from(before.score) - i64::from(after.score) <= 10 {
                continue;
            }
            let points = |f: &[ryusch::lint::Finding], code: &str| -> i64 {
                f.iter()
                    .filter(|x| x.sheet == after.sheet && x.code == code)
                    .map(|x| x.points)
                    .sum()
            };
            let mut codes: Vec<&str> = f1.iter().map(|x| x.code).collect();
            codes.sort_unstable();
            codes.dedup();
            let grew: Vec<String> = codes
                .into_iter()
                .filter_map(|c| {
                    let (a, z) = (points(&f0, c), points(&f1, c));
                    (z > a).then(|| format!("{c} {a} -> {z}"))
                })
                .collect();
            if let Some(a) = notes.as_array_mut() {
                a.push(json!({
                    "code": "move_costly",
                    "message": format!(
                        "sheet {} scores {} after this, from {} ({})",
                        after.sheet,
                        after.score,
                        before.score,
                        grew.join(", ")
                    ),
                    "details": {"sheet": after.sheet, "before": before.score, "after": after.score},
                }));
            }
        }
    }
    let mut v = json!({
        "block": bid,
        "added": added,
        "released": released,
        "instructions": listing,
        "notes": notes,
        "dry_run": dry_run,
    });
    // One shape with describe's: the block's description and its scores,
    // written or not.
    let mut laid = laid;
    let mut description = block_description(p, b, &mut laid, None)?;
    let scores: Vec<Value> = description["sheets"]
        .as_array()
        .map(|a| a.iter().map(|s| s["measures"].clone()).collect())
        .unwrap_or_default();
    v["scores"] = json!(scores);
    if dry_run {
        v["description"] = json!({"blocks": [description]});
        if let Some(ask) = render {
            let (mime, bytes, px) = picture(&laid, &[], ask)?;
            v["format"] = json!(ask.format);
            if let Some(px) = px {
                v["scale"] = json!(px);
            }
            return Ok(Answer::bytes(v, mime, bytes));
        }
        return Ok(Answer::json(v));
    }
    let mut w = Writes::default();
    w.0.push((
        schem::layout_path(&bid),
        Some(schem::layout_text(&laid.outcome.layout)),
    ));
    write::commit(session, &format!("block {bid}'s layout"), w)?;
    v["revision"] = json!(session.project().conn.block(&bid)?.revision);
    description["revision"] = v["revision"].clone();
    v["description"] = json!({"blocks": [description]});
    Ok(Answer::json(v))
}

/// Each lasting instruction as text, with whether it holds as the lint
/// sees the drawing, and why not.
fn listing(names: &Names, laid: &Laid) -> Vec<Value> {
    laid.outcome
        .layout
        .instructions
        .iter()
        .map(|(id, i)| {
            let verdict = ryusch::instr::judge(&laid.built.input, &laid.outcome.layout, i);
            let mut v = json!({
                "id": id,
                "text": names.text(i),
                "holds": verdict.is_ok(),
            });
            if let Err(why) = verdict {
                v["why"] = json!(why);
            }
            v
        })
        .collect()
}

/// Whether what `ins` speaks of is drawn as it was: its parts' places, or
/// a net's wires and labels.
fn unchanged(
    block: &ryusch::input::Block,
    before: Option<&ryusch::layout::Layout>,
    after: &ryusch::layout::Layout,
    ins: &Instruction,
) -> bool {
    let Some(before) = before else {
        return false;
    };
    if let Instruction::Net { net, .. } = ins {
        return before.routes.get(net) == after.routes.get(net)
            && before.labels.get(net) == after.labels.get(net);
    }
    ins.parts()
        .iter()
        .flat_map(|s| ryusch::instr::parts_named(block, s))
        .all(|p| {
            let k = &block.parts[p].key;
            before
                .placed
                .get(k)
                .map(|x| (&x.sheet, x.at, x.rotation, x.mirror))
                == after
                    .placed
                    .get(k)
                    .map(|x| (&x.sheet, x.at, x.rotation, x.mirror))
        })
}

/// `layout.instructions`: a block's lasting instructions, as text.
pub fn instructions(p: &Project, block: &str) -> Result<Value> {
    let b = write::block(p, block)?;
    let laid = schem::lay(p, b, None, &[], Relayout::None)?;
    let names = Names {
        p,
        b,
        built: &laid.built,
    };
    Ok(json!({"block": b.id, "instructions": listing(&names, &laid), "revision": b.revision}))
}

/// `describe`: each block's sheets in relative terms, its idioms and
/// instructions, lint and scores; one sheet (by ID or name) when asked.
pub fn describe(
    p: &Project,
    design: Option<&str>,
    block: Option<&str>,
    sheet: Option<&str>,
) -> Result<Value> {
    let mut out = Vec::new();
    for b in blocks(p, design, block)? {
        let mut laid = schem::lay(p, b, None, &[], Relayout::None)?;
        out.push(block_description(p, b, &mut laid, sheet)?);
    }
    Ok(json!({"blocks": out}))
}

/// One block's description as `describe` gives it, from a layout: the
/// one shape `describe` and `place` answer in.
fn block_description(
    p: &Project,
    b: &BlockFiles,
    laid: &mut Laid,
    sheet: Option<&str>,
) -> Result<Value> {
    let (layers, shown) = show_layer(p, b, laid, None)?;
    let (findings, scores) =
        ryusch::lint::lint_with(&laid.built.input, &laid.outcome, &crate::lint::prefs(p));
    let mut d = ryusch::describe::describe(&laid.built.input, &laid.outcome, &findings, &scores);
    let names = Names {
        p,
        b,
        built: &laid.built,
    };
    if let Some(list) = d["instructions"].as_array_mut() {
        for v in list {
            let id = v["id"].as_str().unwrap_or_default().to_owned();
            if let Some(i) = laid.outcome.layout.instructions.get(&id) {
                v["text"] = json!(names.text(i));
            }
        }
    }
    if let Some(s) = sheet {
        let sheets = d["sheets"].as_array().cloned().unwrap_or_default();
        let kept: Vec<Value> = sheets
            .into_iter()
            .filter(|x| x["sheet"] == s || x["name"] == s)
            .collect();
        if kept.is_empty() {
            return Err(
                Error::new(Code::NotFound, format!("block {} has no sheet {s}", b.id))
                    .with("sheet", s),
            );
        }
        d["sheets"] = json!(kept);
    }
    d["block_id"] = json!(b.id);
    if layers.len() > 1 {
        d["layers"] = json!(layers.iter().map(schem::Layer::to_json).collect::<Vec<_>>());
    }
    if let Some(l) = shown.filter(|_| layers.len() > 1) {
        d["layer"] = l.to_json();
    }
    d["revision"] = json!(b.revision);
    if laid.stale {
        d["stale"] = json!(true);
    }
    if let Some(e) = &laid.invalid {
        d["invalid"] = json!(e);
    }
    Ok(d)
}

/// How `render` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Svg,
    Png,
}

/// A picture asked of a dry run (`place --dry-run --render FILE`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RenderAsk {
    pub format: Format,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fit: bool,
}

/// A layout drawn as `render` draws it: the bytes, their type, and the
/// scale used (PNG).
fn picture(
    laid: &Laid,
    refs: &[&str],
    ask: RenderAsk,
) -> Result<(&'static str, Vec<u8>, Option<f32>)> {
    let layout = &laid.outcome.layout;
    let svg = if ask.fit {
        ryusch::svg::draw_fit(&laid.built.input, layout, refs)
    } else {
        ryusch::svg::draw(&laid.built.input, layout, refs)
    };
    Ok(match ask.format {
        Format::Svg => ("image/svg+xml", svg.into_bytes(), None),
        Format::Png => {
            let px = ask.scale.unwrap_or(4.0);
            if !(0.5..=40.0).contains(&px) {
                return Err(Error::new(
                    Code::InvalidParam,
                    "scale is 0.5 to 40 pixels per millimetre",
                )
                .with("param", "scale"));
            }
            ("image/png", png(&svg, px)?, Some(px))
        }
    })
}

/// SVG to PNG at `px_per_mm`, with Inter alone for text.
pub fn png(svg: &str, px_per_mm: f32) -> Result<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options {
        font_family: "Inter".into(),
        ..usvg::Options::default()
    };
    std::sync::Arc::make_mut(&mut opt.fontdb).load_font_data(INTER.to_vec());
    let tree = usvg::Tree::from_str(svg, &opt)
        .map_err(|e| Error::new(Code::Internal, format!("the SVG doesn't read: {e}")))?;
    // usvg reads millimetres at 96 per inch.
    let k = px_per_mm / (96.0 / 25.4);
    let size = tree.size();
    let (w, h) = (
        (size.width() * k).ceil() as u32,
        (size.height() * k).ceil() as u32,
    );
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 200_000_000 {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{w}x{h} pixels is no image to make; choose another scale"),
        )
        .with("param", "scale"));
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("a size checked above");
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(k, k),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|e| Error::new(Code::Internal, format!("PNG: {e}")))
}

/// The layer of block `b` drawn: the one `layer` names (its instance
/// path or stack path, `<design>:` before either), else the first; `None`
/// for a block no design places. Its designators go on `laid`'s parts.
fn show_layer(
    p: &Project,
    b: &BlockFiles,
    laid: &mut Laid,
    layer: Option<&str>,
) -> Result<(Vec<schem::Layer>, Option<schem::Layer>)> {
    let all = schem::layers(p, &b.id);
    let chosen = match layer {
        Some(t) => Some(all.iter().find(|l| l.named(t)).cloned().ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("block {} has no layer {t} in any design", b.id),
            )
            .with("layer", t)
            .with(
                "layers",
                all.iter().map(schem::Layer::to_json).collect::<Vec<_>>(),
            )
        })?),
        None => all.first().cloned(),
    };
    if let Some(l) = &chosen {
        laid.built.designators(&l.refdes);
    }
    Ok((all, chosen))
}

/// `render`: a block's canvas (its sheets side by side) or one sheet, as
/// SVG or PNG; bytes after the answer. A block a design places several
/// times (a stack's layers) draws one occurrence's designators: `layer`'s,
/// else the first's; the answer lists them all.
#[allow(clippy::too_many_arguments)]
pub fn render(
    p: &Project,
    design: Option<&str>,
    block: Option<&str>,
    sheet: Option<&str>,
    layer: Option<&str>,
    format: Format,
    scale: Option<f32>,
    fit: bool,
) -> Result<Answer> {
    let b = blocks(p, design, block)?[0];
    let mut laid = schem::lay(p, b, None, &[], Relayout::None)?;
    let (layers, shown) = show_layer(p, b, &mut laid, layer)?;
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
    let mut v = json!({
        "block": b.id,
        "sheets": if which.is_empty() { layout.sheets.keys().cloned().collect() } else { which.clone() },
        "format": format,
        "revision": b.revision,
        "layers": layers.iter().map(schem::Layer::to_json).collect::<Vec<_>>(),
    });
    if let Some(l) = &shown {
        v["layer"] = l.to_json();
    }
    if laid.stale {
        v["stale"] = json!(true);
    }
    let (mime, bytes, px) = picture(&laid, &refs, RenderAsk { format, scale, fit })?;
    if let Some(px) = px {
        v["scale"] = json!(px);
    }
    Ok(Answer::bytes(v, mime, bytes))
}

/// The layout writes a connectivity write needs: every laid out block
/// whose drawing it changes (its parts, pins, nets or paper) laid out
/// again over its file. A file stale from outside stays as it is.
pub fn keep_current(before: &Project, after: &Project) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for (id, b) in &after.conn.blocks {
        let Some(Ok(_)) = &b.layout else { continue };
        let Ok(now) = schem::build(after, b) else {
            continue;
        };
        let changed = match before.conn.blocks.get(id) {
            Some(old) => schem::build(before, old).map_or(true, |x| x.input != now.input),
            None => true,
        };
        if !changed {
            continue;
        }
        let Ok(laid) = schem::lay(after, b, None, &[], Relayout::None) else {
            continue;
        };
        if laid.invalid.is_none() {
            out.push((
                schem::layout_path(id),
                Some(schem::layout_text(&laid.outcome.layout)),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{design_in, library_in, manifest};

    #[test]
    fn writes_keep_laid_out_blocks_current() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let mut s = Session::open(&board).unwrap();
        layout(&mut s, None, None, None, false, None, None).unwrap();
        let path = board.join("blocks/top/layout.json");
        let before = std::fs::read_to_string(&path).unwrap();
        let revs = crate::server::revisions(s.project());
        let layout_rev = revs["layout/top"].clone();
        assert!(layout_rev.is_string(), "{revs:?}");
        crate::partcmd::component_add(
            &mut s,
            "top",
            Some("r9"),
            &crate::partcmd::ComponentFields {
                of: Some("r_100k".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_ne!(before, after);
        assert!(after.contains("\"r9\""), "{after}");
        // A viewer hears which block's layout moved.
        assert_ne!(
            crate::server::revisions(s.project())["layout/top"],
            layout_rev
        );
        // Connecting it draws it joined, in the same change.
        crate::partcmd::wire(
            &mut s,
            "top/r9/1",
            crate::partcmd::Wire::To {
                net: "sda",
                new: false,
            },
            None,
        )
        .unwrap();
        let p = s.project();
        let b = p.conn.block(&Id::parse("top", "b").unwrap()).unwrap();
        let laid = schem::lay(p, b, None, &[], Relayout::None).unwrap();
        assert!(laid.invalid.is_none() && !laid.stale, "{:?}", laid.invalid);
    }

    #[test]
    fn a_stacked_blocks_layers_draw_their_own_designators() {
        let (_dir, p) = crate::netlist::tests::project(&[]);
        let svg = |layer: Option<&str>| {
            let a = render(&p, None, Some("s"), None, layer, Format::Svg, None, false).unwrap();
            (String::from_utf8(a.binary.unwrap()).unwrap(), a.result)
        };
        let (first, v) = svg(None);
        assert!(first.contains(">U1_A<"), "{first}");
        assert_eq!(v["layer"]["path"], "ch_a");
        assert_eq!(v["layers"].as_array().unwrap().len(), 2);
        let (second, v) = svg(Some("B"));
        assert!(second.contains(">U1_B<") && !second.contains(">U1_A<"));
        assert_eq!(v["layer"]["stack_path"], "B");
        // The drawing is the same but for the text and its measure.
        let plain = |svg: &str| -> String {
            svg.split("textLength=\"")
                .enumerate()
                .map(|(i, x)| {
                    if i == 0 {
                        x
                    } else {
                        x.split_once('"').map_or(x, |(_, r)| r)
                    }
                })
                .collect()
        };
        assert_eq!(plain(&first.replace("U1_A", "U1_B")), plain(&second));
        let e = render(
            &p,
            None,
            Some("s"),
            None,
            Some("C"),
            Format::Svg,
            None,
            false,
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        assert_eq!(e.details["layers"].as_array().unwrap().len(), 2);
        // The parent draws the two as one stack.
        let (top, _) = {
            let a = render(&p, None, Some("top"), None, None, Format::Svg, None, false).unwrap();
            (String::from_utf8(a.binary.unwrap()).unwrap(), a.result)
        };
        assert!(
            top.contains(">×2<") && top.contains(">ch_a, ch_b<"),
            "{top}"
        );
    }
}
