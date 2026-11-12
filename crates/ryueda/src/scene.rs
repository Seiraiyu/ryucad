//! A sheet as a window draws it (design 519, plan 3.B): `ryusch::paint`'s
//! items, each with its owner's index; the owners by reference path, each
//! with its kind, box and a hash of its items; the paper with its border
//! and title block; the sheet's pins and lint findings. Written as JSON
//! text, never a `Value` tree, so a 500-part sheet answers without one.

use crate::design::BlockFiles;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use crate::schem::{self, Laid};
use ryusch::font::{Anchor, Font, Weight};
use ryusch::geom::{Nm, Point, Rect};
use ryusch::paint::{Item, Painted};
use ryusch::style::Rgb;
use ryusch::style::Theme;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

fn moved(at: Point, by: Point) -> Point {
    [at[0] + by[0], at[1] + by[1]]
}

fn point(at: Point, by: Point) -> String {
    serde_json::to_string(&moved(at, by)).expect("integers")
}

fn points(list: &[Point], by: Point) -> String {
    let list: Vec<Point> = list.iter().map(|p| moved(*p, by)).collect();
    serde_json::to_string(&list).expect("integers")
}

/// A box as `[[x0, y0], [x1, y1]]`.
pub fn rect_text(r: Rect, by: Point) -> String {
    format!("[{},{}]", point(r.min, by), point(r.max, by))
}

fn colour(c: Option<Rgb>) -> String {
    c.map_or_else(|| "null".to_owned(), |c| format!("\"{}\"", c.hex()))
}

fn anchor(a: Anchor) -> &'static str {
    match a {
        Anchor::Start => "start",
        Anchor::Middle => "middle",
        Anchor::End => "end",
    }
}

fn font(size: Nm, bold: bool) -> Font {
    let weight = if bold {
        Weight::SemiBold
    } else {
        Weight::Regular
    };
    Font { size, weight }
}

/// One item as a JSON object, moved by `by`, without its owner: a
/// `type` and what SVG draws it from; a text's `width` is the one SVG
/// stretches it to (`textLength`).
pub fn item_body(it: &Item, by: Point) -> String {
    match it {
        Item::Polyline {
            points: p,
            stroke,
            fill,
            width,
            dash,
        } => format!(
            r#"{{"type":"polyline","points":{},"stroke":"{}","fill":{},"width":{width},"dash":{dash}}}"#,
            points(p, by),
            stroke.hex(),
            colour(*fill)
        ),
        Item::Circle {
            center,
            radius,
            stroke,
            fill,
            width,
        } => format!(
            r#"{{"type":"circle","center":{},"radius":{radius},"stroke":{},"fill":{},"width":{width}}}"#,
            point(*center, by),
            colour(*stroke),
            colour(*fill)
        ),
        Item::Arc {
            start,
            mid,
            end,
            stroke,
            width,
        } => format!(
            r#"{{"type":"arc","start":{},"mid":{},"end":{},"stroke":"{}","width":{width}}}"#,
            point(*start, by),
            point(*mid, by),
            point(*end, by),
            stroke.hex()
        ),
        Item::Bezier {
            points: p,
            stroke,
            width,
        } => format!(
            r#"{{"type":"bezier","points":{},"stroke":"{}","width":{width}}}"#,
            points(p, by),
            stroke.hex()
        ),
        Item::Frame {
            rect,
            title_tab,
            stroke,
            paper,
            width,
            frame,
        } => format!(
            r#"{{"type":"frame","rect":{},"title_tab":{},"stroke":"{}","paper":"{}","width":{width},"frame":{frame}}}"#,
            rect_text(*rect, by),
            title_tab.map_or_else(|| "null".to_owned(), |t| rect_text(t, by)),
            stroke.hex(),
            paper.hex()
        ),
        Item::Bus {
            points: p,
            stroke,
            width,
        } => format!(
            r#"{{"type":"bus","points":{},"stroke":"{}","width":{width}}}"#,
            points(p, by),
            stroke.hex()
        ),
        Item::Text {
            text,
            at,
            size,
            anchor: a,
            vertical,
            colour: c,
            bold,
        } => format!(
            r#"{{"type":"text","text":{},"at":{},"size":{size},"anchor":"{}","vertical":{vertical},"colour":"{}","bold":{bold},"width":{}}}"#,
            serde_json::to_string(text).expect("a string"),
            point(*at, by),
            anchor(*a),
            c.hex(),
            ryusch::font::width_in(text, font(*size, *bold)).max(1)
        ),
        Item::Begin(l) => format!(r#"{{"type":"begin","layer":"{}"}}"#, l.key()),
        Item::End => r#"{"type":"end"}"#.to_owned(),
    }
}

/// `r` turned a quarter counterclockwise about `at` (text reading
/// upwards).
fn turned(r: Rect, at: Point) -> Rect {
    let t = |q: Point| [at[0] - (q[1] - at[1]), at[1] + (q[0] - at[0])];
    Rect::new(t(r.min), t(r.max))
}

/// The box of the circle through `a`, `b` and `c` (an arc's), rounded
/// outwards, or `None` when they are on a line. IEEE arithmetic and a
/// square root only: the same on every platform.
fn arc_circle_box(a: Point, b: Point, c: Point) -> Option<Rect> {
    let f = |p: Point| [p[0] as f64, p[1] as f64];
    let (a, b, c) = (f(a), f(b), f(c));
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d == 0.0 {
        return None;
    }
    let sq = |p: [f64; 2]| p[0] * p[0] + p[1] * p[1];
    let ux = (sq(a) * (b[1] - c[1]) + sq(b) * (c[1] - a[1]) + sq(c) * (a[1] - b[1])) / d;
    let uy = (sq(a) * (c[0] - b[0]) + sq(b) * (a[0] - c[0]) + sq(c) * (b[0] - a[0])) / d;
    let r = ((a[0] - ux) * (a[0] - ux) + (a[1] - uy) * (a[1] - uy)).sqrt();
    Some(Rect::new(
        [(ux - r).floor() as Nm, (uy - r).floor() as Nm],
        [(ux + r).ceil() as Nm, (uy + r).ceil() as Nm],
    ))
}

/// What an item covers, half its pen round its lines: a text's box as
/// SVG lays it out, an arc's whole circle's (a little more than the arc,
/// never less), none for a layer's bounds.
pub fn item_box(it: &Item) -> Option<Rect> {
    let half = |w: Nm| w / 2 + 1;
    match it {
        Item::Polyline { points, width, .. } | Item::Bus { points, width, .. } => {
            Rect::of(points.iter().copied()).map(|r| r.grow(half(*width)))
        }
        Item::Circle {
            center,
            radius,
            width,
            ..
        } => Some(Rect::new(*center, *center).grow(radius + half(*width))),
        Item::Arc {
            start,
            mid,
            end,
            width,
            ..
        } => arc_circle_box(*start, *mid, *end)
            .or_else(|| Rect::of([*start, *mid, *end]))
            .map(|r| r.grow(half(*width))),
        Item::Bezier { points, width, .. } => {
            Rect::of(points.iter().copied()).map(|r| r.grow(half(*width)))
        }
        Item::Frame {
            rect,
            title_tab,
            width,
            ..
        } => Some(
            title_tab
                .map_or(*rect, |t| rect.union(t))
                .grow(half(*width)),
        ),
        Item::Text {
            text,
            at,
            size,
            anchor,
            vertical,
            bold,
            ..
        } => {
            let r = ryusch::font::text_box_in(text, font(*size, *bold), *at, *anchor);
            Some(if *vertical { turned(r, *at) } else { r })
        }
        Item::Begin(_) | Item::End => None,
    }
}

/// What layout key `key` draws: a part's key in `parts` is a component;
/// a key without a prefix that isn't a part's is a net's.
fn kind_of(key: &str, parts: &BTreeSet<&str>) -> &'static str {
    match key.split_once(':') {
        Some(("instance", _)) => "instance",
        Some(("port", _)) => "port",
        Some(("rail", _)) => "rail",
        Some(("bus", _)) => "bus",
        Some(("region", _)) => "region",
        Some(("note", _)) => "note",
        Some(("label", _)) => "label",
        _ if parts.contains(key) => "component",
        _ => "net",
    }
}

/// One owner as it is built.
struct Owner {
    reference: String,
    kind: &'static str,
    net: Option<String>,
    boxed: Option<Rect>,
    hash: blake3::Hasher,
}

/// A sheet's items and owners as JSON text: `items` (an array, each
/// `{"owner": n or null, …item}`) and `owners` (an array of `{ref, kind,
/// net, box, hash}`), and each owner's index by its reference path.
pub struct Owned {
    pub items: String,
    pub owners: String,
    pub index: BTreeMap<String, usize>,
}

/// [`Owned`] for block `block`'s painted sheet: first `under` (the
/// paper's border and title block, drawn under everything, owned by no
/// one) moved by `shift` into the sheet's frame, then `painted`'s items
/// with their owners by reference path (`schem::ref_path`). `parts` are
/// the components' part keys. An owner's hash is BLAKE3 over its items'
/// JSON without their owner index, so no other owner moves it.
pub fn owned(
    block: &Id,
    under: &[Item],
    shift: Point,
    painted: &Painted,
    parts: &BTreeSet<&str>,
) -> Owned {
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut owners: Vec<Owner> = Vec::new();
    let mut items = String::from("[");
    let mut first = true;
    let mut put = |items: &mut String, owner: Option<usize>, body: &str| {
        if !first {
            items.push(',');
        }
        first = false;
        let owner = owner.map_or_else(|| "null".to_owned(), |i| i.to_string());
        let _ = write!(items, r#"{{"owner":{owner},{}"#, &body[1..]);
    };
    for it in under {
        put(&mut items, None, &item_body(it, shift));
    }
    for (it, key) in painted.items.iter().zip(&painted.owners) {
        let body = item_body(it, [0, 0]);
        let owner = match key {
            None => None,
            Some(key) => {
                let r = crate::schem::ref_path(block, key);
                let i = match index.get(&r) {
                    Some(i) => *i,
                    None => {
                        let net = key
                            .strip_prefix("label:")
                            .and_then(|rest| rest.rsplit_once('@'))
                            .map(|(net, _)| crate::schem::ref_path(block, net));
                        owners.push(Owner {
                            reference: r.clone(),
                            kind: kind_of(key, parts),
                            net,
                            boxed: None,
                            hash: blake3::Hasher::new(),
                        });
                        index.insert(r, owners.len() - 1);
                        owners.len() - 1
                    }
                };
                let o = &mut owners[i];
                o.hash.update(body.as_bytes());
                o.hash.update(b"\n");
                if let Some(b) = item_box(it) {
                    o.boxed = Some(o.boxed.map_or(b, |x| x.union(b)));
                }
                Some(i)
            }
        };
        put(&mut items, owner, &body);
    }
    items.push(']');
    let list: Vec<String> = owners
        .iter()
        .map(|o| {
            format!(
                r#"{{"ref":{},"kind":"{}","net":{},"box":{},"hash":"{}"}}"#,
                serde_json::to_string(&o.reference).expect("a string"),
                o.kind,
                o.net.as_ref().map_or_else(
                    || "null".to_owned(),
                    |n| serde_json::to_string(n).expect("a string")
                ),
                o.boxed
                    .map_or_else(|| "null".to_owned(), |b| rect_text(b, [0, 0])),
                &o.hash.finalize().to_hex()[..32]
            )
        })
        .collect();
    Owned {
        items,
        owners: format!("[{}]", list.join(",")),
        index,
    }
}

/// What `scene` is asked: a design's root block unless `block`, its
/// first sheet in canvas order unless `sheet` (an ID or a name), a
/// layer's designators (an instance or stack path; the first), a theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    pub design: Option<String>,
    pub block: Option<String>,
    pub sheet: Option<String>,
    pub layer: Option<String>,
    pub theme: Theme,
}

/// `sheet_not_found`: block `block` has no sheet `s`; its sheets listed.
pub fn sheet_not_found(block: &Id, s: &str, sheets: &[(String, String)]) -> Error {
    Error::new(
        Code::SheetNotFound,
        format!("block {block} has no sheet {s}"),
    )
    .with("block", block)
    .with("sheet", s)
    .with("sheets", sheet_list(sheets))
}

fn sheet_list(sheets: &[(String, String)]) -> Vec<Value> {
    sheets
        .iter()
        .map(|(k, n)| json!({"sheet": k, "name": n}))
        .collect()
}

/// A layout's sheets in canvas order, `(ID, name)`.
pub fn sheets_of(layout: &ryusch::layout::Layout) -> Vec<(String, String)> {
    ryusch::canvas::order(layout)
        .into_iter()
        .map(|k| {
            let name = layout.sheets[&k].name.clone();
            (k, name)
        })
        .collect()
}

/// The sheet `asked` names (an ID or a name), else the first.
pub fn sheet_named(block: &Id, sheets: &[(String, String)], asked: Option<&str>) -> Result<String> {
    match asked {
        Some(s) => sheets
            .iter()
            .find(|(k, n)| k == s || n == s)
            .map(|(k, _)| k.clone())
            .ok_or_else(|| sheet_not_found(block, s, sheets)),
        None => sheets
            .first()
            .map(|(k, _)| k.clone())
            .ok_or_else(|| sheet_not_found(block, "", sheets)),
    }
}

/// The blocks block `b`'s drawing reads: itself, those it places (their
/// symbols' ports and names) and those that place it along every layer
/// (their designator formats).
fn drawn_from(p: &Project, b: &BlockFiles) -> BTreeSet<String> {
    let mut out = BTreeSet::from([b.id.to_string()]);
    out.extend(
        b.instances
            .values()
            .filter_map(|i| i.item.as_ref().ok())
            .map(|i| i.of.to_string()),
    );
    for l in schem::layers(p, &b.id) {
        let Some(root) = p
            .conn
            .designs
            .get(&l.design)
            .and_then(|d| d.item.as_ref().ok())
            .map(|d| d.root.clone())
        else {
            continue;
        };
        let mut at = root;
        for iid in &l.path {
            out.insert(at.to_string());
            match p
                .conn
                .blocks
                .get(&at)
                .and_then(|x| x.instances.get(iid))
                .and_then(|i| i.item.as_ref().ok())
            {
                Some(i) => at = i.of.clone(),
                None => break,
            }
        }
    }
    out
}

/// The revisions block `b`'s scene is drawn from, as `project.changed`
/// names them: `project`, `style`, `standards`, every `lib/<alias>`, and
/// `block/<id>` and `layout/<id>` of each block [`drawn_from`] gives. A
/// window draws a sheet again only when one of these moves.
pub fn revisions(p: &Project, b: &BlockFiles) -> BTreeMap<String, Value> {
    let blocks = drawn_from(p, b);
    crate::server::revisions(p)
        .into_iter()
        .filter(|(k, _)| {
            matches!(k.as_str(), "project" | "style" | "standards")
                || k.starts_with("lib/")
                || k.strip_prefix("block/")
                    .or_else(|| k.strip_prefix("layout/"))
                    .is_some_and(|id| blocks.contains(id))
        })
        .collect()
}

/// One token for [`revisions`]: BLAKE3 of their canonical JSON, 32 hex.
pub fn revision(revs: &BTreeMap<String, Value>) -> String {
    let text = crate::canonical::to_string(&json!(revs));
    blake3::hash(text.as_bytes()).to_hex()[..32].to_owned()
}

/// A pin as the scene lists it.
#[derive(Serialize)]
struct Pin<'a> {
    #[serde(rename = "ref")]
    reference: String,
    owner: Option<usize>,
    at: Point,
    base: Point,
    number: &'a str,
    name: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    net: Option<String>,
    nc: bool,
}

fn pin_kind(k: ryusch::input::PinKind) -> &'static str {
    use ryusch::input::PinKind as K;
    match k {
        K::Input => "input",
        K::Output => "output",
        K::Bidirectional => "bidirectional",
        K::TriState => "tri_state",
        K::Passive => "passive",
        K::Free => "free",
        K::Unspecified => "unspecified",
        K::PowerIn => "power_in",
        K::PowerOut => "power_out",
        K::OpenCollector => "open_collector",
        K::OpenEmitter => "open_emitter",
        K::NoConnect => "no_connect",
    }
}

/// Every component pin on sheet `sid`: its reference path, where wires
/// meet it and where it meets the body, and its net's reference path.
fn pins<'a>(
    block: &Id,
    laid: &'a Laid,
    sid: &str,
    index: &BTreeMap<String, usize>,
) -> Vec<Pin<'a>> {
    let layout = &laid.outcome.layout;
    let input = &laid.built.input;
    let mut out = Vec::new();
    for (part, of) in input.parts.iter().zip(&laid.built.of) {
        let schem::Of::Unit { component, .. } = of else {
            continue;
        };
        let Some(x) = layout.placed.get(&part.key).filter(|x| x.sheet == sid) else {
            continue;
        };
        let shape =
            ryusch::shape::Shape::of(part, ryusch::geom::Place::new(x.at, x.rotation, x.mirror));
        let owner_ref = format!("{block}/{component}");
        let owner = index.get(&owner_ref).copied();
        for (pin, at) in part.pins.iter().zip(&shape.pins) {
            out.push(Pin {
                reference: format!("{owner_ref}/{}", pin.number),
                owner,
                at: at.end,
                base: at.base,
                number: &pin.number,
                name: &pin.name,
                kind: pin_kind(pin.kind),
                net: pin
                    .net
                    .and_then(|n| input.nets.get(n))
                    .map(|n| schem::ref_path(block, &n.key)),
                nc: pin.nc,
            });
        }
    }
    out
}

/// A finding as the scene lists it: the sheet's lint, its references as
/// reference paths.
#[derive(Serialize)]
struct Finding<'a> {
    code: &'a str,
    severity: ryusch::lint::Severity,
    message: &'a str,
    refs: Vec<String>,
}

/// What the scene says besides its items and owners.
#[derive(Serialize)]
struct Head<'a> {
    block: &'a Id,
    design: Option<String>,
    layer: Option<Value>,
    layers: Vec<Value>,
    sheet: &'a str,
    name: &'a str,
    sheets: Vec<Value>,
    theme: Theme,
    revision: String,
    revisions: BTreeMap<String, Value>,
    paper: Value,
    score: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    invalid: Option<&'a Error>,
    findings: Vec<Finding<'a>>,
    pins: Vec<Pin<'a>>,
}

/// One sheet of a block as a window draws it (design 519), as JSON text:
/// laid out in memory exactly as `render` lays it out (nothing written),
/// a layer's designators, the sheet's look in `ask.theme`; `items` (the
/// paper's border, zone marks and title block first, then the sheet's,
/// in the sheet's frame, nanometres, y up), `owners`, `pins`, the sheet's
/// lint `findings` and `score`, the `paper` (`size`, its `rect` in the
/// sheet's frame, `colour`, `canvas`), and the `revisions` it is drawn
/// from with their `revision`.
pub fn scene(p: &Project, ask: &Ask) -> Result<String> {
    let b = crate::schemcmd::blocks(p, ask.design.as_deref(), ask.block.as_deref())?[0];
    let mut laid = schem::lay(p, b, None, &[], ryusch::input::Relayout::None)?;
    let (layers, shown) = crate::schemcmd::show_layer(p, b, &mut laid, ask.layer.as_deref())?;
    let layout = &laid.outcome.layout;
    let sheets = sheets_of(layout);
    let sid = sheet_named(&b.id, &sheets, ask.sheet.as_deref())?;
    let place = sheets.iter().position(|(k, _)| *k == sid).unwrap_or(0);
    let name = sheets[place].1.as_str();
    let looks = crate::schemcmd::looks(p, b, &laid.built, layout, ask.theme);
    let look = looks.get(&sid).cloned().unwrap_or_default();
    let painted = ryusch::paint::sheet_in(&laid.built.input, layout, &sid, &look);
    let parts: BTreeSet<&str> = laid
        .built
        .input
        .parts
        .iter()
        .zip(&laid.built.of)
        .filter(|(_, of)| matches!(of, schem::Of::Unit { .. }))
        .map(|(part, _)| part.key.as_str())
        .collect();
    // The paper as the PDF pages it, in the sheet's frame.
    let area = laid.built.input.area_in(layout, &sid);
    let paper = [
        area[0] + 2 * schem::BORDER,
        area[1] + 2 * schem::BORDER + schem::TITLE_BAND,
    ];
    let origin = [schem::BORDER, schem::BORDER + schem::TITLE_BAND];
    let design = shown.as_ref().map(|l| l.design.to_string());
    let design_name = shown
        .as_ref()
        .and_then(|l| p.conn.designs.get(&l.design))
        .and_then(|d| d.item.as_ref().ok())
        .map(|d| d.name.clone())
        .or_else(|| design.clone())
        .unwrap_or_default();
    let title = crate::output::title_block(
        p,
        &crate::cascade::Layers::of(p),
        b,
        &design_name,
        shown.as_ref().map_or("", |l| l.stack_path.as_str()),
        name,
        sheets.len(),
        format!("{}/{}", place + 1, sheets.len()),
    );
    let under = crate::pdf::paper_items(paper, &title, &look);
    let owned = owned(&b.id, &under, [-origin[0], -origin[1]], &painted, &parts);
    let (found, scores) =
        ryusch::lint::lint_with(&laid.built.input, &laid.outcome, &crate::lint::prefs(p));
    let revs = revisions(p, b);
    let head = Head {
        block: &b.id,
        design,
        layer: shown.as_ref().map(schem::Layer::to_json),
        layers: layers.iter().map(schem::Layer::to_json).collect(),
        sheet: &sid,
        name,
        sheets: sheet_list(&sheets),
        theme: ask.theme,
        revision: revision(&revs),
        revisions: revs,
        paper: json!({
            "size": paper,
            "rect": [[-origin[0], -origin[1]], [paper[0] - origin[0], paper[1] - origin[1]]],
            "colour": look.role(ryusch::style::Role::Paper).hex(),
            "canvas": look.role(ryusch::style::Role::Canvas).hex(),
        }),
        score: scores.iter().find(|s| s.sheet == sid).map(|s| s.score),
        stale: laid.stale,
        invalid: laid.invalid.as_ref(),
        findings: found
            .iter()
            .filter(|f| f.sheet == sid)
            .map(|f| Finding {
                code: f.code,
                severity: f.severity,
                message: &f.message,
                refs: f.refs.iter().map(|k| schem::ref_path(&b.id, k)).collect(),
            })
            .collect(),
        pins: pins(&b.id, &laid, &sid, &owned.index),
    };
    let mut text = serde_json::to_string(&head)
        .map_err(|e| Error::new(Code::Internal, format!("a scene's head: {e}")))?;
    text.pop();
    let _ = write!(
        text,
        r#","items":{},"owners":{}}}"#,
        owned.items, owned.owners
    );
    Ok(text)
}

/// `scene` as the session answers it: a scene drawn before from the same
/// revisions comes from the session's [`SCENES`](crate::session::SCENES),
/// else it is drawn and kept.
pub fn answer(session: &mut crate::session::Session, ask: &Ask) -> Result<crate::proto::Answer> {
    use ryu_session::rpc::Raw;
    let key = {
        let p = session.project();
        let b = crate::schemcmd::blocks(p, ask.design.as_deref(), ask.block.as_deref())?[0];
        format!("{ask:?}\u{0}{}", revision(&revisions(p, b)))
    };
    let text = match session.scene(&key) {
        Some(t) => t,
        None => {
            let t = std::sync::Arc::new(scene(session.project(), ask)?);
            session.keep_scene(key, t.clone());
            t
        }
    };
    Ok(crate::proto::Answer::raw(Raw::Result(
        text.as_ref().clone(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryusch::paint::Layer;
    use serde_json::{Value, json};

    fn line(x: Nm) -> Item {
        Item::Polyline {
            points: vec![[x, 0], [x, 1_000_000]],
            stroke: Rgb(0, 0, 0),
            fill: None,
            width: 150_000,
            dash: false,
        }
    }

    fn painted(last: Item) -> Painted {
        Painted {
            items: vec![
                Item::Begin(Layer::Frames),
                line(0),
                Item::End,
                line(1),
                line(2),
                last,
            ],
            owners: vec![
                None,
                Some("region:r1".into()),
                None,
                Some("u1:1".into()),
                Some("u1:2".into()),
                Some("sda".into()),
            ],
            ..Painted::default()
        }
    }

    #[test]
    fn every_item_names_its_owner_and_owners_hash_their_own_items() {
        let block = Id::parse("top", "block").unwrap();
        let parts: BTreeSet<&str> = ["u1:1", "u1:2"].into();
        let under = [line(-5)];
        let o = owned(&block, &under, [10, 20], &painted(line(3)), &parts);
        let items: Value = serde_json::from_str(&o.items).unwrap();
        let owners: Value = serde_json::from_str(&o.owners).unwrap();
        let of: Vec<Value> = items
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["owner"].clone())
            .collect();
        assert_eq!(
            of,
            [
                json!(null),
                json!(null),
                json!(0),
                json!(null),
                json!(1),
                json!(1),
                json!(2)
            ]
        );
        // The paper's items come first, moved into the sheet's frame.
        assert_eq!(items[0]["points"], json!([[5, 20], [5, 1_000_020]]));
        assert_eq!(
            items[1],
            json!({"owner": null, "type": "begin", "layer": "frames"})
        );
        let refs: Vec<&str> = owners
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["ref"].as_str().unwrap())
            .collect();
        assert_eq!(refs, ["top/region:r1", "top/u1", "top/sda"]);
        let kinds: Vec<&str> = owners
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["region", "component", "net"]);
        // A unit of several is one owner: both units' lines, half a pen round.
        assert_eq!(
            owners[1]["box"],
            json!([[-75_000, -75_001], [75_003, 1_075_001]])
        );
        assert_eq!(o.index["top/u1"], 1);
        // Another line for sda moves sda's hash alone.
        let again = owned(&block, &under, [10, 20], &painted(line(4)), &parts);
        let after: Value = serde_json::from_str(&again.owners).unwrap();
        for i in 0..2 {
            assert_eq!(after[i]["hash"], owners[i]["hash"], "{i}");
        }
        assert_ne!(after[2]["hash"], owners[2]["hash"]);
        assert_eq!(owners[2]["hash"].as_str().unwrap().len(), 32);
    }

    #[test]
    fn labels_name_their_net_and_text_is_measured() {
        let block = Id::parse("top", "block").unwrap();
        let text = Item::Text {
            text: "SDA".into(),
            at: [0, 0],
            size: 1_270_000,
            anchor: Anchor::Start,
            vertical: true,
            colour: Rgb(1, 2, 3),
            bold: false,
        };
        let p = Painted {
            items: vec![text],
            owners: vec![Some("label:sda@u1:2/5".into())],
            ..Painted::default()
        };
        let o = owned(&block, &[], [0, 0], &p, &BTreeSet::new());
        let owners: Value = serde_json::from_str(&o.owners).unwrap();
        assert_eq!(owners[0]["ref"], "top/label:sda@u1/5");
        assert_eq!(owners[0]["kind"], "label");
        assert_eq!(owners[0]["net"], "top/sda");
        let items: Value = serde_json::from_str(&o.items).unwrap();
        let w = ryusch::font::width_in(
            "SDA",
            Font {
                size: 1_270_000,
                weight: Weight::Regular,
            },
        );
        assert_eq!(items[0]["width"], json!(w));
        assert_eq!(items[0]["colour"], "#010203");
        // Reading upwards: its box stands on its baseline's start.
        let b = &owners[0]["box"];
        assert!(b[1][1].as_i64().unwrap() >= w, "{b}");
        assert!(b[0][0].as_i64().unwrap() < 0, "{b}");
    }

    #[test]
    fn a_projects_sheet_is_a_whole_scene() {
        use crate::project::tests::{design_in, library_in, manifest};
        use crate::project::{Project, ProjectKind};
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let p = Project::load(&board).unwrap();
        let ask = Ask {
            block: Some("top".into()),
            ..Ask::default()
        };
        let text = scene(&p, &ask).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["block"], "top");
        assert_eq!(v["theme"], "light");
        assert_eq!(v["sheet"], v["sheets"][0]["sheet"]);
        let owners = v["owners"].as_array().unwrap();
        let items = v["items"].as_array().unwrap();
        for it in items {
            if let Some(i) = it["owner"].as_u64() {
                assert!((i as usize) < owners.len(), "{it}");
            }
        }
        // The paper's items come first and belong to no one.
        assert!(items[0]["owner"].is_null() && items[0]["type"] == "polyline");
        let refs: Vec<&str> = owners.iter().map(|o| o["ref"].as_str().unwrap()).collect();
        assert!(refs.contains(&"top/r1"), "{refs:?}");
        let pin = v["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["ref"] == "top/r1/1")
            .unwrap();
        assert_eq!(pin["net"], "top/sda");
        assert_eq!(
            owners[pin["owner"].as_u64().unwrap() as usize]["ref"],
            "top/r1"
        );
        assert!(v["revisions"]["block/top"].is_string());
        assert_eq!(v["revision"].as_str().unwrap().len(), 32);
        // The same files, the same text; dark another paper.
        assert_eq!(scene(&p, &ask).unwrap(), text);
        let dark = scene(
            &p,
            &Ask {
                theme: Theme::Dark,
                ..ask.clone()
            },
        )
        .unwrap();
        let d: Value = serde_json::from_str(&dark).unwrap();
        assert_ne!(d["paper"]["colour"], v["paper"]["colour"]);
        assert_eq!(d["owners"].as_array().unwrap().len(), owners.len());
        // A sheet the block hasn't.
        let e = scene(
            &p,
            &Ask {
                sheet: Some("nope".into()),
                ..ask
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::SheetNotFound);
        assert_eq!(e.details["sheets"], v["sheets"]);
    }

    #[test]
    fn a_session_keeps_scenes_by_what_they_were_drawn_from() {
        use crate::project::ProjectKind;
        use crate::project::tests::{design_in, library_in, manifest};
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let mut s = crate::session::Session::open(&board).unwrap();
        let ask = Ask {
            block: Some("top".into()),
            ..Ask::default()
        };
        let text = |a: crate::proto::Answer| match a.raw {
            Some(ryu_session::rpc::Raw::Result(t)) => t,
            other => panic!("{other:?}"),
        };
        let one = text(answer(&mut s, &ask).unwrap());
        assert_eq!(text(answer(&mut s, &ask).unwrap()), one);
        assert_eq!(s.scenes.len(), 1, "drawn once");
        let r1 = board.join("blocks/top/components/r1.json");
        let mut c: Value = serde_json::from_slice(&std::fs::read(&r1).unwrap()).unwrap();
        c["value"] = json!("22k");
        std::fs::write(&r1, crate::canonical::to_string(&c)).unwrap();
        s.refresh_changes().unwrap();
        let two = text(answer(&mut s, &ask).unwrap());
        assert_ne!(two, one);
        assert_eq!(s.scenes.len(), 2);
        assert!(two.contains("22k"));
    }
}
