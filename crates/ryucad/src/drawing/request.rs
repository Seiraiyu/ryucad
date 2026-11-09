//! The `drawing.*` requests (design 417): list, create, edit, delete,
//! rename and lay out a part's drawings, each write one logged, undoable
//! change guarded by the part's revision; get, describe, render and
//! export one at any revision.

use super::{Built, Placement, SheetBuilt, ViewBuilt, build, output};
use crate::doc::drawing::{DrawingFile, Key, Size, drawing_file};
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryu_session::rpc::Answer;
use serde_json::{Map, Value, json};

fn path(session: &Session, part: &Id, drawing: &Id) -> std::path::PathBuf {
    crate::doc::project::part_dir(session.root(), part).join(drawing_file(drawing))
}

fn not_found(session: &Session, part: &Id, drawing: &str) -> Error {
    let have: Vec<String> = session
        .project()
        .part(part)
        .map(|p| p.drawings.keys().map(Id::to_string).collect())
        .unwrap_or_default();
    Error::new(
        Code::DrawingNotFound,
        format!("{part} has no drawing {drawing}"),
    )
    .with("part", part)
    .with("drawings", have)
}

/// The drawing `drawing` of `part` as its file reads, or why not.
fn file_of(session: &Session, part: &Id, drawing: &str) -> Result<(Id, DrawingFile)> {
    let p = session.project().part(part)?;
    let id = Id::parse(drawing, "drawing")
        .ok()
        .filter(|d| p.drawings.contains_key(d))
        .ok_or_else(|| not_found(session, part, drawing))?;
    match &p.drawings[&id] {
        Ok(f) => Ok((id, f.clone())),
        Err(e) => Err(e.clone()),
    }
}

/// `drawing.list`: a part's drawings, or every part's, each with its
/// sheets and views (or why its file doesn't read), and each listed
/// part's revision.
pub fn list(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let parts: Vec<Id> = match part {
        Some(p) => {
            let id = crate::server::part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut drawings = Vec::new();
    let mut revisions = Map::new();
    for pid in &parts {
        let p = session.project().part(pid)?;
        if part.is_some() || !p.drawings.is_empty() {
            revisions.insert(pid.to_string(), json!(p.revision));
        }
        for (d, f) in &p.drawings {
            drawings.push(match f {
                Ok(f) => json!({
                    "part": pid, "drawing": d, "name": f.name,
                    "sheets": f.sheets.iter().map(|(k, s)| json!({
                        "sheet": k.to_string(), "name": s.name, "size": s.size.as_str()
                    })).collect::<Vec<_>>(),
                    "views": f.views.iter().map(|(k, w)| json!({
                        "view": k.to_string(), "type": w.type_name(), "sheet": w.sheet.to_string()
                    })).collect::<Vec<_>>(),
                }),
                Err(e) => json!({"part": pid, "drawing": d, "error": e}),
            });
        }
    }
    Ok(json!({"drawings": drawings, "revisions": revisions}))
}

/// What `drawing.create` makes.
#[derive(Clone, Debug, Default)]
pub struct Create {
    pub name: Option<String>,
    pub template: Option<String>,
    pub size: Option<String>,
    /// `standard`: a base front view, its top and right (in the
    /// standard's projection, placed as it places them) and an isometric.
    pub views: Option<String>,
}

/// `drawing.create`: a drawing of one sheet (of the standard's default
/// size unless `size` says), with `views: standard` its standard views.
pub fn create(
    session: &mut Session,
    part: &str,
    drawing: &str,
    ask: &Create,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, expected)?;
    let did = crate::doc::drawing::check_id(drawing)?;
    if p.drawings.contains_key(&did) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("{pid} has a drawing {did} already"),
        )
        .with("part", &pid)
        .with("drawing", &did));
    }
    let standard = session.project().manifest.standard();
    let size = match &ask.size {
        None => Size::default_for(standard),
        Some(s) => Size::parse(s).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("{s:?} is no sheet size: iso_a0 to iso_a4, ansi_a to ansi_e"),
            )
            .with("param", "size")
        })?,
    };
    let name = ask.name.clone().unwrap_or_else(|| pid.to_string());
    let mut file = DrawingFile::new(&name, size);
    let mut v = file.to_json();
    if let Some(t) = &ask.template {
        v["sheets"]["s1"]["template"] = json!(t);
    }
    match ask.views.as_deref() {
        None => {}
        Some("standard") => {
            v["views"] = json!({
                "v1": {"type": "base", "sheet": "s1", "orient": "front"},
                "v2": {"type": "projected", "sheet": "s1", "from": "v1", "side": "top"},
                "v3": {"type": "projected", "sheet": "s1", "from": "v1", "side": "right"},
                "v4": {"type": "projected", "sheet": "s1", "from": "v1", "side": "iso"},
            });
        }
        Some(other) => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("views {other:?}: the only set of views is standard"),
            )
            .with("param", "views"));
        }
    }
    file = read_as_param(&v)?;
    write_built(session, &pid, &did, file)
}

/// The drawing `v` holds, a fault in it `invalid_param` at its pointer.
fn read_as_param(v: &Value) -> Result<DrawingFile> {
    DrawingFile::from_json(v).map_err(|e| {
        let mut out = Error::new(Code::InvalidParam, e.message.clone());
        if let Some(p) = e.details.get("pointer") {
            out = out.with("pointer", p.clone());
        }
        out
    })
}

/// Builds `file` in a session over the project as it would be with it
/// written (what a write can find refused before anything is written),
/// writes it with ryucad's placements filled in, and answers as
/// `drawing.get` does with the part's new revision.
fn write_built(session: &mut Session, pid: &Id, did: &Id, file: DrawingFile) -> Result<Value> {
    let at = path(session, pid, did);
    let rel = crate::doc::source::relative(session.root(), &at);
    let bytes = crate::doc::canonical::to_string(&file.to_json()).into_bytes();
    let writes = [(rel.clone(), Some(bytes))].into_iter().collect();
    let mut proposed = session.proposed(&writes)?;
    let built = build(&mut proposed, pid, did)?;
    let file = with_placements(&file, &built);
    let replaced = session.project().part(pid)?.drawings.contains_key(did);
    session.write_json(&at, &file.to_json())?;
    session.reload()?;
    let mut v = summary(&built);
    v["replaced"] = json!(replaced);
    v["revision"] = json!(session.project().part(pid)?.revision);
    Ok(v)
}

/// `file` with the places ryucad chose written in: a view's `at`, or a
/// projected or auxiliary view's `offset`, in sheet millimetres to a
/// thousandth, and a sheet's scale where it chose one.
fn with_placements(file: &DrawingFile, built: &Built) -> DrawingFile {
    let mut v = file.to_json();
    let mm = |x: f64| {
        let r = (x * 1000.0).round() / 1000.0;
        let r = if r == 0.0 { 0.0 } else { r };
        json!(format!("{r} mm"))
    };
    // A scale ryucad chose, as places are: the file is what is drawn.
    for sheet in built.sheets.iter().filter(|s| s.scale_chosen) {
        v["sheets"][sheet.key.to_string()]["scale"] = json!(sheet.scale.to_string());
    }
    for (k, p) in built.placements(file) {
        let at = &mut v["views"][k.to_string()];
        match p {
            Placement::At([x, y]) => at["at"] = json!([mm(x), mm(y)]),
            Placement::Offset(d) => at["offset"] = mm(d),
        }
    }
    DrawingFile::from_json(&v).unwrap_or_else(|_| file.clone())
}

/// `drawing.edit`: a batch of ops, all or nothing, with one build to
/// check it. Each op is `{op, …}`:
///
/// - `add` a sheet or a view: `{op: add, kind: sheet|view, id?, value}`,
///   the next key of its kind unless `id` says;
/// - `remove` one: `{op: remove, id}`;
/// - `set` fields: `{op: set, id?, fields: {name: value | null}}` (no
///   `id`: the drawing's own: `name`, `standard`, `projection`, `unit`,
///   `precision`, `fields`);
/// - `move` a view: `{op: move, id, at: [x, y]}` or `{op: move, id,
///   offset}`.
pub fn edit(
    session: &mut Session,
    part: &str,
    drawing: &str,
    ops: &Value,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, expected)?;
    let (did, file) = file_of(session, &pid, drawing)?;
    let mut v = file.to_json();
    let ops = ops.as_array().ok_or_else(|| {
        Error::new(Code::InvalidParam, "ops is a list of operations").with("param", "ops")
    })?;
    for (i, op) in ops.iter().enumerate() {
        apply(&mut v, op, &format!("/ops/{i}"))?;
    }
    let file = read_as_param(&v)?;
    write_built(session, &pid, &did, file)
}

fn bad(at: &str, message: impl Into<String>) -> Error {
    Error::new(Code::InvalidParam, message).with("pointer", at)
}

/// The member group a key's letter names.
fn group(id: &str) -> Option<&'static str> {
    match id.chars().next()? {
        's' => Some("sheets"),
        'v' => Some("views"),
        _ => None,
    }
}

fn apply(v: &mut Value, op: &Value, at: &str) -> Result<()> {
    let name = op["op"].as_str().unwrap_or_default();
    let id = op["id"].as_str();
    match name {
        "add" => {
            let kind = op["kind"].as_str().unwrap_or_default();
            let (letter, g) = match kind {
                "sheet" => ('s', "sheets"),
                "view" => ('v', "views"),
                _ => return Err(bad(&format!("{at}/kind"), "add a sheet or a view")),
            };
            let have: Vec<Key> = v[g]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, _)| Key::parse(k, letter))
                .collect();
            let key = match id {
                Some(k) => {
                    let key = Key::parse(k, letter).ok_or_else(|| {
                        bad(&format!("{at}/id"), format!("a key such as {letter}1"))
                    })?;
                    if have.contains(&key) {
                        return Err(Error::new(
                            Code::DuplicateId,
                            format!("the drawing has a {kind} {key} already"),
                        )
                        .with("pointer", format!("{at}/id")));
                    }
                    key
                }
                None => Key::next(letter, &have),
            };
            if !op["value"].is_object() {
                return Err(bad(&format!("{at}/value"), format!("a {kind}'s fields")));
            }
            v[g][key.to_string()] = op["value"].clone();
        }
        "remove" => {
            let id = id.ok_or_else(|| bad(&format!("{at}/id"), "remove names what it removes"))?;
            let g =
                group(id).ok_or_else(|| bad(&format!("{at}/id"), "a sheet's or a view's key"))?;
            if v[g].as_object_mut().and_then(|m| m.remove(id)).is_none() {
                return Err(
                    Error::new(Code::NotFound, format!("the drawing has no {id}"))
                        .with("pointer", format!("{at}/id")),
                );
            }
        }
        "set" => {
            let fields = op["fields"]
                .as_object()
                .ok_or_else(|| bad(&format!("{at}/fields"), "fields to set, by name"))?;
            let target = match id {
                None => v,
                Some(id) => {
                    let g = group(id)
                        .ok_or_else(|| bad(&format!("{at}/id"), "a sheet's or a view's key"))?;
                    match v[g].get_mut(id) {
                        Some(t) => t,
                        None => {
                            return Err(Error::new(
                                Code::NotFound,
                                format!("the drawing has no {id}"),
                            )
                            .with("pointer", format!("{at}/id")));
                        }
                    }
                }
            };
            for (k, x) in fields {
                if ["format", "kind", "sheets", "views"].contains(&k.as_str()) && id.is_none() {
                    return Err(bad(
                        &format!("{at}/fields/{k}"),
                        format!("{k} isn't set by set"),
                    ));
                }
                match x {
                    Value::Null => {
                        target.as_object_mut().map(|m| m.remove(k));
                    }
                    x => target[k] = x.clone(),
                }
            }
        }
        "move" => {
            let id = id.ok_or_else(|| bad(&format!("{at}/id"), "move names the view it moves"))?;
            let view = v["views"].get_mut(id).ok_or_else(|| {
                Error::new(Code::NotFound, format!("the drawing has no view {id}"))
                    .with("pointer", format!("{at}/id"))
            })?;
            match (op.get("at"), op.get("offset")) {
                (Some(p), None) => view["at"] = p.clone(),
                (None, Some(d)) => view["offset"] = d.clone(),
                _ => return Err(bad(at, "move gives at or offset")),
            }
        }
        _ => {
            return Err(bad(
                &format!("{at}/op"),
                "an op is add, remove, set or move",
            ));
        }
    }
    Ok(())
}

/// `drawing.delete`: a drawing goes; one that doesn't read can too.
pub fn delete(
    session: &mut Session,
    part: &str,
    drawing: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, expected)?;
    let did = Id::parse(drawing, "drawing")
        .ok()
        .filter(|d| p.drawings.contains_key(d))
        .ok_or_else(|| not_found(session, &pid, drawing))?;
    session.remove_file(&path(session, &pid, &did))?;
    session.reload()?;
    let revision = session.project().part(&pid)?.revision.clone();
    Ok(json!({"part": pid, "drawing": did, "revision": revision}))
}

/// `drawing.rename`: a drawing takes a new ID; nothing reads a drawing,
/// so nothing else changes.
pub fn rename(
    session: &mut Session,
    part: &str,
    drawing: &str,
    to: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, expected)?;
    let (did, file) = file_of(session, &pid, drawing)?;
    let new = crate::doc::drawing::check_id(to)?;
    if p.drawings.contains_key(&new) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("{pid} has a drawing {new} already"),
        )
        .with("part", &pid)
        .with("drawing", &new));
    }
    session.write_json(&path(session, &pid, &new), &file.to_json())?;
    session.remove_file(&path(session, &pid, &did))?;
    session.reload()?;
    let revision = session.project().part(&pid)?.revision.clone();
    Ok(json!({"part": pid, "drawing": new, "from": did, "revision": revision}))
}

/// `drawing.layout`: the views laid out again from nothing (every `at`,
/// `offset` and sheet scale taken out and chosen by ryucad), written
/// unless a dry run.
pub fn layout(
    session: &mut Session,
    part: &str,
    drawing: &str,
    dry_run: bool,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, expected)?;
    let (did, file) = file_of(session, &pid, drawing)?;
    let mut v = file.to_json();
    for (_, w) in v["views"].as_object_mut().into_iter().flatten() {
        if let Some(m) = w.as_object_mut() {
            m.remove("at");
            m.remove("offset");
        }
    }
    // A sheet's scale too: chosen again for the views laid out anew.
    for (_, sh) in v["sheets"].as_object_mut().into_iter().flatten() {
        if let Some(m) = sh.as_object_mut() {
            m.remove("scale");
        }
    }
    let fresh = read_as_param(&v)?;
    if !dry_run {
        return write_built(session, &pid, &did, fresh);
    }
    let at = path(session, &pid, &did);
    let rel = crate::doc::source::relative(session.root(), &at);
    let bytes = crate::doc::canonical::to_string(&fresh.to_json()).into_bytes();
    let writes = [(rel, Some(bytes))].into_iter().collect();
    let mut proposed = session.proposed(&writes)?;
    let built = build(&mut proposed, &pid, &did)?;
    let mut out = summary(&built);
    out["dry_run"] = json!(true);
    Ok(out)
}

fn rev_text(r: &crate::doc::assembly::Rev) -> Value {
    match r {
        crate::doc::assembly::Rev::Working => json!("working"),
        crate::doc::assembly::Rev::Label(l) => json!({"label": l}),
        crate::doc::assembly::Rev::Foreign { project, label } => {
            json!({"label": label, "project": project})
        }
    }
}

fn view_json(v: &ViewBuilt) -> Value {
    let mut out = json!({
        "view": v.key.to_string(), "type": v.kind, "sheet": v.sheet.to_string(),
        "at_mm": v.at, "placed": v.placed, "scale": v.scale.to_string(),
        "box_mm": v.bounds, "towards": v.towards, "up": v.up,
        "of": v.of, "rev": rev_text(&v.rev),
        "pieces": {"visible": v.counts[0], "hidden": v.counts[1], "uncertain": v.counts[2]},
    });
    if let Some(e) = &v.error {
        out["error"] = json!(e);
    }
    out
}

fn sheet_json(s: &SheetBuilt, items: bool) -> Value {
    let mut out = json!({
        "sheet": s.key.to_string(), "name": s.name, "size": s.size.as_str(),
        "orientation": s.orientation.as_str(), "paper_mm": s.paper,
        "template": s.template, "scale": s.scale.to_string(),
        "views": s.views.iter().map(view_json).collect::<Vec<_>>(),
    });
    if items {
        out["items"] = json!(s.items.iter().map(item_json).collect::<Vec<_>>());
    }
    out
}

fn item_json(it: &ryu_draw::Item) -> Value {
    use ryu_draw::{Item, Segment};
    match it {
        Item::Path { path, stroke, fill } => {
            let segments: Vec<Value> = path
                .segments
                .iter()
                .map(|s| match s {
                    Segment::Line(p) => json!({"line": p}),
                    Segment::Arc { centre, radius, from, sweep } => {
                        json!({"arc": {"centre": centre, "radius": radius, "from": from, "sweep": sweep}})
                    }
                    Segment::Ellipse { centre, a, b, from, sweep } => {
                        json!({"ellipse": {"centre": centre, "a": a, "b": b, "from": from, "sweep": sweep}})
                    }
                    Segment::Bezier { points, weights } => {
                        json!({"bezier": {"points": points, "weights": weights}})
                    }
                })
                .collect();
            let mut v =
                json!({"path": {"start": path.start, "segments": segments, "closed": path.closed}});
            if let Some(s) = stroke {
                v["stroke"] =
                    json!({"colour": s.colour.to_hex(), "width": s.width, "dash": s.dash});
            }
            if let Some(f) = fill {
                v["fill"] = json!(f.to_hex());
            }
            v
        }
        Item::Text {
            text,
            at,
            size,
            angle,
            anchor,
            colour,
        } => json!({
            "text": text, "at": at, "size": size, "angle": angle,
            "anchor": match anchor {
                ryu_draw::Anchor::Start => "start",
                ryu_draw::Anchor::Middle => "middle",
                ryu_draw::Anchor::End => "end",
            },
            "colour": colour.to_hex(),
        }),
    }
}

/// What a write answers: each sheet and view, and the findings.
fn summary(b: &Built) -> Value {
    json!({
        "schema": 1, "part": b.part, "drawing": b.drawing, "name": b.name,
        "standard": b.standard.as_str(), "projection": b.projection.as_str(),
        "sheets": b.sheets.iter().map(|s| sheet_json(s, false)).collect::<Vec<_>>(),
        "findings": b.findings,
    })
}

/// `drawing.get`: the built drawing at the session's revision: each
/// sheet's views with their boxes and frames, and with `items` every
/// item's exact geometry.
pub fn get(
    session: &mut Session,
    part: &str,
    drawing: &str,
    sheet: Option<&str>,
    items: bool,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let (did, _) = file_of(session, &pid, drawing)?;
    let b = build(session, &pid, &did)?;
    let sheets: Vec<Value> = match sheet {
        Some(k) => vec![sheet_json(b.sheet(Some(k))?, items)],
        None => b.sheets.iter().map(|s| sheet_json(s, items)).collect(),
    };
    let mut v = summary(&b);
    v["sheets"] = json!(sheets);
    v["revision"] = json!(session.project().part(&pid)?.revision);
    Ok(v)
}

fn side_of(a: &ViewBuilt, b: &ViewBuilt) -> &'static str {
    let (dx, dy) = (b.at[0] - a.at[0], b.at[1] - a.at[1]);
    if dx.abs() >= dy.abs() {
        if dx > 0.0 { "right of" } else { "left of" }
    } else if dy > 0.0 {
        "above"
    } else {
        "below"
    }
}

/// `drawing.describe`: each sheet in words, for an agent: its paper and
/// scale, each view, what it shows and where it stands, what lies next to
/// it, and the findings.
pub fn describe(
    session: &mut Session,
    part: &str,
    drawing: &str,
    sheet: Option<&str>,
) -> Result<Value> {
    let pid = crate::server::part_id(part)?;
    let (did, _) = file_of(session, &pid, drawing)?;
    let b = build(session, &pid, &did)?;
    let wanted: Vec<&SheetBuilt> = match sheet {
        Some(k) => vec![b.sheet(Some(k))?],
        None => b.sheets.iter().collect(),
    };
    let sheets: Vec<Value> = wanted
        .iter()
        .map(|s| {
            let views: Vec<Value> = s
                .views
                .iter()
                .map(|v| {
                    let w = v.bounds[1][0] - v.bounds[0][0];
                    let h = v.bounds[1][1] - v.bounds[0][1];
                    let mut text = format!(
                        "{} ({}) of {} at {}, {:.1} × {:.1} mm, centred at ({:.1}, {:.1}) mm",
                        v.key, v.kind, v.of, v.scale, w, h, v.at[0], v.at[1]
                    );
                    if let Some(e) = &v.error {
                        text.push_str(&format!("; failed: {}", e.message));
                    } else {
                        text.push_str(&format!(
                            "; {} visible, {} hidden and {} uncertain pieces",
                            v.counts[0], v.counts[1], v.counts[2]
                        ));
                    }
                    let near: Vec<String> = s
                        .views
                        .iter()
                        .filter(|o| o.key != v.key)
                        .map(|o| format!("{} is {} it", o.key, side_of(v, o)))
                        .collect();
                    json!({"view": v.key.to_string(), "text": text, "near": near})
                })
                .collect();
            json!({
                "sheet": s.key.to_string(),
                "text": format!(
                    "{} ({}), {} {}, {:.1} × {:.1} mm at {}, template {}",
                    s.key, s.name, s.size.as_str(), s.orientation.as_str(),
                    s.paper[0], s.paper[1], s.scale, s.template
                ),
                "views": views,
            })
        })
        .collect();
    Ok(json!({
        "schema": 1, "part": pid, "drawing": did, "name": b.name,
        "standard": b.standard.as_str(), "projection": b.projection.as_str(),
        "sheets": sheets, "findings": b.findings,
    }))
}

/// `drawing.render`: a sheet as PNG (the default) or SVG, the bytes after
/// the answer.
pub fn render(
    session: &mut Session,
    part: &str,
    drawing: &str,
    sheet: Option<&str>,
    format: Option<&str>,
    scale: Option<f32>,
) -> Result<Answer> {
    let pid = crate::server::part_id(part)?;
    let (did, _) = file_of(session, &pid, drawing)?;
    let b = build(session, &pid, &did)?;
    let s = b.sheet(sheet)?;
    let result = json!({
        "part": pid, "drawing": did, "sheet": s.key.to_string(),
        "paper_mm": s.paper, "findings": b.findings,
    });
    match format.unwrap_or("png") {
        "png" => {
            let px = scale.unwrap_or(output::PX_PER_MM);
            let bytes = output::sheet_png(s, px)?;
            let mut result = result;
            result["px_per_mm"] = json!(px);
            Ok(Answer::bytes(result, "image/png", bytes))
        }
        "svg" if scale.is_none() => Ok(Answer::bytes(
            result,
            "image/svg+xml",
            output::sheet_svg(s).into_bytes(),
        )),
        "svg" => Err(
            Error::new(Code::InvalidParam, "an SVG has no pixels to scale").with("param", "scale"),
        ),
        f => Err(Error::new(
            Code::InvalidParam,
            format!("{f:?}: a sheet renders as png or svg"),
        )
        .with("param", "format")),
    }
}

/// `drawing.export`: every sheet as one PDF, or one sheet as SVG, written
/// by the daemon to `out` (never a project file) as `export` writes, or
/// following the answer without one.
pub fn export(
    session: &mut Session,
    part: &str,
    drawing: &str,
    sheet: Option<&str>,
    format: Option<&str>,
    out: Option<&str>,
) -> Result<Answer> {
    let pid = crate::server::part_id(part)?;
    let format = match (format, out) {
        (Some(f), _) => f.to_owned(),
        (None, Some(o)) => std::path::Path::new(o)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default(),
        (None, None) => "pdf".to_owned(),
    };
    let out = out
        .map(|o| crate::export::out_path(o, session.root()))
        .transpose()?;
    let (did, _) = file_of(session, &pid, drawing)?;
    let b = build(session, &pid, &did)?;
    let job = session.job.clone();
    let mut result = json!({"part": pid, "drawing": did, "format": format, "findings": b.findings});
    match format.as_str() {
        "pdf" => {
            if sheet.is_some() {
                return Err(Error::new(Code::InvalidParam, "a PDF holds every sheet")
                    .with("param", "sheet"));
            }
            result["sheets"] = json!(b.sheets.len());
            let bytes = output::pdf(&b);
            crate::export::deliver(&job, result, out.as_deref(), "application/pdf", |w| {
                w.write_all(&bytes)
            })
        }
        "svg" => {
            let s = b.sheet(sheet)?;
            result["sheet"] = json!(s.key.to_string());
            let text = output::sheet_svg(s);
            crate::export::deliver(&job, result, out.as_deref(), "image/svg+xml", |w| {
                w.write_all(text.as_bytes())
            })
        }
        f => Err(Error::new(
            Code::InvalidParam,
            format!("{f:?}: a drawing exports as pdf or svg (DXF is plan 9.B's)"),
        )
        .with("param", "format")),
    }
}

/// `check`'s findings of `part`'s drawings, `source: drawing`: each that
/// doesn't build (a template not found, a view that can't look where it
/// says), each view that fails, each with uncertain pieces.
pub fn findings(session: &mut Session, part: &Id) -> Vec<Value> {
    let Ok(p) = session.project().part(part) else {
        return Vec::new();
    };
    // One whose file doesn't read is doctor's `invalid_document`.
    let ids: Vec<Id> = p
        .drawings
        .iter()
        .filter(|(_, f)| f.is_ok())
        .map(|(d, _)| d.clone())
        .collect();
    let mut out = Vec::new();
    for d in ids {
        let at = format!("parts/{part}/{}", drawing_file(&d));
        match build(session, part, &d) {
            Ok(b) => {
                for mut f in b.findings {
                    f["source"] = json!("drawing");
                    f["part"] = json!(part);
                    f["drawing"] = json!(d);
                    out.push(f);
                }
            }
            Err(e) => out.push(json!({
                "code": e.code, "severity": "error", "source": "drawing",
                "part": part, "drawing": d, "message": e.message,
                "details": {"path": at, "error": e},
            })),
        }
    }
    out
}
