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
use crate::vocab::{Names, Said};
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
    let (findings, scores) = ryusch::lint::lint(&laid.built.input, &laid.outcome);
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
) -> Result<Value> {
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
    }
    if !writes.0.is_empty() {
        write::commit(session, "the layout", writes)?;
    }
    let p = session.project();
    for v in &mut out {
        let id = Id::parse(v["block"].as_str().expect("an ID"), "block")?;
        v["revision"] = json!(p.conn.block(&id)?.revision);
    }
    Ok(json!({"blocks": out, "dry_run": dry_run}))
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
            if !subjects.is_empty() {
                let mut f = vec![format!("release {}", subjects.join(" "))];
                f.extend(new.iter().map(|(_, t)| t.clone()));
                out.push(f);
            }
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
    if without.len() < new.len() {
        out.push(without);
    }
    json!(out)
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
) -> Result<Value> {
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
    let mut batch: Vec<(String, String)> = Vec::new();
    for (i, text) in instructions.iter().enumerate() {
        let at = |e: Error| e.with("index", i);
        match names.read(text).map_err(at)? {
            Said::Release(keys) => {
                let gone: Vec<String> = lasting
                    .iter()
                    .filter(|(_, ins)| keys.iter().any(|k| crate::vocab::names(ins, k)))
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in gone {
                    lasting.remove(&id);
                    released.push(id);
                }
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
        match names.read(text).map_err(|e| e.with("move", i))? {
            Said::Do(ins) => {
                move_list.push(ins);
                batch.push((format!("move {}", i + 1), text.clone()));
            }
            Said::Release(_) => {
                return Err(Error::new(
                    Code::InvalidParam,
                    "release is an instruction, not a move",
                )
                .with("move", i));
            }
        }
    }
    let laid = match schem::lay(p, b, Some(&lasting), &move_list, Relayout::None) {
        Ok(l) => l,
        Err(e) if matches!(e.code, Code::InstructionConflict | Code::LayoutRule) => {
            let f = fixes(&names, &before, &batch, &e);
            return Err(e.with("fixes", f));
        }
        Err(e) => return Err(e),
    };
    let (findings, scores) = ryusch::lint::lint(&laid.built.input, &laid.outcome);
    let description =
        ryusch::describe::describe(&laid.built.input, &laid.outcome, &findings, &scores);
    let bid = b.id.clone();
    let listing = listing(&names, &laid);
    let mut v = json!({
        "block": bid,
        "added": added,
        "released": released,
        "instructions": listing,
        "notes": laid.outcome.notes,
        "dry_run": dry_run,
    });
    if dry_run {
        v["description"] = description;
        return Ok(v);
    }
    let mut w = Writes::default();
    w.0.push((
        schem::layout_path(&bid),
        Some(schem::layout_text(&laid.outcome.layout)),
    ));
    write::commit(session, &format!("block {bid}'s layout"), w)?;
    v["revision"] = json!(session.project().conn.block(&bid)?.revision);
    v["scores"] = json!(scores);
    Ok(v)
}

/// Each lasting instruction as text, with whether it holds.
fn listing(names: &Names, laid: &Laid) -> Vec<Value> {
    laid.outcome
        .layout
        .instructions
        .iter()
        .map(|(id, i)| {
            json!({
                "id": id,
                "text": names.text(i),
                "holds": ryusch::instr::achieved(&laid.built.input, &laid.outcome.layout, i),
            })
        })
        .collect()
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
        let laid = schem::lay(p, b, None, &[], Relayout::None)?;
        let (findings, scores) = ryusch::lint::lint(&laid.built.input, &laid.outcome);
        let mut d =
            ryusch::describe::describe(&laid.built.input, &laid.outcome, &findings, &scores);
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
        d["revision"] = json!(b.revision);
        if laid.stale {
            d["stale"] = json!(true);
        }
        if let Some(e) = &laid.invalid {
            d["invalid"] = json!(e);
        }
        out.push(d);
    }
    Ok(json!({"blocks": out}))
}

/// How `render` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Svg,
    Png,
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

/// `render`: a block's canvas (its sheets side by side) or one sheet, as
/// SVG or PNG; bytes after the answer.
pub fn render(
    p: &Project,
    design: Option<&str>,
    block: Option<&str>,
    sheet: Option<&str>,
    format: Format,
    scale: Option<f32>,
    fit: bool,
) -> Result<Answer> {
    let b = blocks(p, design, block)?[0];
    let laid = schem::lay(p, b, None, &[], Relayout::None)?;
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
    let svg = if fit {
        ryusch::svg::draw_fit(&laid.built.input, layout, &refs)
    } else {
        ryusch::svg::draw(&laid.built.input, layout, &refs)
    };
    let mut v = json!({
        "block": b.id,
        "sheets": if which.is_empty() { layout.sheets.keys().cloned().collect() } else { which.clone() },
        "format": format,
        "revision": b.revision,
    });
    if laid.stale {
        v["stale"] = json!(true);
    }
    Ok(match format {
        Format::Svg => Answer::bytes(v, "image/svg+xml", svg.into_bytes()),
        Format::Png => {
            let px = scale.unwrap_or(4.0);
            if !(0.5..=40.0).contains(&px) {
                return Err(Error::new(
                    Code::InvalidParam,
                    "scale is 0.5 to 40 pixels per millimetre",
                )
                .with("param", "scale"));
            }
            v["scale"] = json!(px);
            Answer::bytes(v, "image/png", png(&svg, px)?)
        }
    })
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
        layout(&mut s, None, None, None, false, None).unwrap();
        let path = board.join("blocks/top/layout.json");
        let before = std::fs::read_to_string(&path).unwrap();
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
}
