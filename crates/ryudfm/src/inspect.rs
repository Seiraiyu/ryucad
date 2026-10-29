//! `inspect`: what a board holds, as JSON. With nothing asked, a summary
//! of the board; with `layer`, a page of that layer's features; with
//! `components` or `nets`, a page of those; with `feature`, one feature
//! with its symbol, attributes, net and shape.
//!
//! Lengths are integer nanometres in `_nm` fields and angles millidegrees
//! in `_mdeg` ones. References are the board's (`<step>/<layer>/<index>`,
//! `<step>/comp/<refdes>`, `<step>/net/<name>`), prefixed by the board's ID
//! in a project; `feature` and `after` take them as given.

use crate::codes;
use crate::error::{Code, Error, Result};
use rayon::prelude::*;
use ryu_board::geom::{Dir, Edge, Pen, Place, Point, Rect, Region};
use ryu_board::model::{
    AttrValue, Attributes, Board, Capabilities, Context, FeatureKind, HoleClass, LayerKind,
    Polarity, Ref, Side, StackKind, Step, Symbol, ToolKind,
};
use ryu_board::shape::{Piece, Shape, ShapeError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// How many items a page holds unless asked otherwise.
pub const PAGE: usize = 100;

/// What to inspect.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// The step; the board step by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// A layer whose features to list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// One feature, by reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub components: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nets: bool,
    /// Items per page ([`PAGE`] by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// The page after this item's reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

pub fn capabilities(c: &Capabilities) -> Vec<&'static str> {
    [
        ("geometry", c.geometry),
        ("nets", c.nets),
        ("components", c.components),
        ("stackup", c.stackup),
        ("drill", c.drill),
        ("flex", c.flex),
        ("feature_ids", c.feature_ids),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(n, _)| n)
    .collect()
}

pub fn point(p: Point) -> Value {
    json!([p.x, p.y])
}

pub fn rect(r: Rect) -> Value {
    json!([r.min.x, r.min.y, r.max.x, r.max.y])
}

fn kind_name(k: &LayerKind) -> String {
    match k {
        LayerKind::Signal => "signal",
        LayerKind::PowerGround => "power_ground",
        LayerKind::Mixed => "mixed",
        LayerKind::SolderMask => "solder_mask",
        LayerKind::SolderPaste => "solder_paste",
        LayerKind::SilkScreen => "silk_screen",
        LayerKind::Drill => "drill",
        LayerKind::Rout => "rout",
        LayerKind::Component => "component",
        LayerKind::Dielectric => "dielectric",
        LayerKind::Document => "document",
        LayerKind::ConductivePaste => "conductive_paste",
        LayerKind::Mask => "mask",
        LayerKind::Other(s) => return s.to_ascii_lowercase(),
    }
    .to_owned()
}

fn side(s: Side) -> &'static str {
    match s {
        Side::Top => "top",
        Side::Bottom => "bottom",
        Side::Inner => "inner",
    }
}

fn polarity(p: Polarity) -> &'static str {
    match p {
        Polarity::Positive => "positive",
        Polarity::Negative => "negative",
    }
}

/// An attribute's value: `true` for a flag, a number where its text is
/// one, else the text.
fn attributes(a: &Attributes) -> Value {
    a.iter()
        .map(|(k, v)| {
            let v = match v {
                AttrValue::Flag => json!(true),
                AttrValue::Number(t) => serde_json::from_str::<serde_json::Number>(t)
                    .map(Value::Number)
                    .unwrap_or_else(|_| json!(t)),
                AttrValue::Text(t) => json!(t),
            };
            (k.clone(), v)
        })
        .collect::<Map<String, Value>>()
        .into()
}

fn feature_kind(k: &FeatureKind) -> &'static str {
    match k {
        FeatureKind::Line { .. } => "line",
        FeatureKind::Arc { .. } => "arc",
        FeatureKind::Pad { .. } => "pad",
        FeatureKind::Surface { .. } => "surface",
        FeatureKind::Text(_) => "text",
        FeatureKind::Barcode(_) => "barcode",
    }
}

/// A symbol table entry: a standard symbol's parameters (as their debug
/// text, sizes in nanometres), or a user or unknown symbol's name.
fn symbol(s: &Symbol) -> Value {
    match s {
        Symbol::Standard(st) => json!({"standard": format!("{st:?}")}),
        Symbol::User(n) => json!({"user": n}),
        Symbol::Unknown(n) => json!({"unknown": n}),
    }
}

fn edge(e: &Edge) -> Value {
    match e {
        Edge::Segment(s) => json!({"line": {"a_nm": point(s.a), "b_nm": point(s.b)}}),
        Edge::Arc(a) => json!({"arc": {
            "start_nm": point(a.start),
            "end_nm": point(a.end),
            "centre_nm": point(a.centre),
            "dir": if a.dir == Dir::Ccw { "ccw" } else { "cw" },
        }}),
    }
}

fn region(r: &Region) -> Value {
    json!({
        "outer": r.outer.edges().iter().map(edge).collect::<Vec<_>>(),
        "holes": r.holes.iter().map(|h| h.edges().iter().map(edge).collect::<Vec<_>>()).collect::<Vec<_>>(),
    })
}

fn pen(p: Pen) -> Value {
    match p {
        Pen::Round(w) => json!({"round_nm": w}),
        Pen::Square(w) => json!({"square_nm": w}),
    }
}

fn shape(s: &Shape) -> Value {
    let parts: Vec<Value> = s
        .parts
        .iter()
        .map(|p| {
            let piece = match &p.piece {
                Piece::Region(r) => json!({"region": region(r)}),
                Piece::Stroke(st) => json!({"stroke": {
                    "path": st.path.iter().map(edge).collect::<Vec<_>>(),
                    "pen": pen(st.pen),
                }}),
                Piece::Swept { path, pen } => json!({"swept": {
                    "path": edge(path),
                    "pen": pen.iter().map(region).collect::<Vec<_>>(),
                }}),
            };
            json!({"polarity": polarity(p.polarity), "piece": piece})
        })
        .collect();
    let mut v = json!({"parts": parts, "box_nm": s.bounds().map(rect)});
    if let Some(a) = s.approximate {
        v["approximate_nm"] = json!(a);
    }
    if !s.missing.is_empty() {
        v["missing"] = json!(s.missing.iter().collect::<String>());
    }
    if s.skipped {
        v["skipped"] = json!(true);
    }
    v
}

fn shape_error(e: &ShapeError) -> String {
    match e {
        ShapeError::BadParameters(why) => format!("bad parameters: {why}"),
        ShapeError::UnknownSymbol(n) => format!("unknown symbol {n}"),
        ShapeError::SymbolCycle(c) => format!("a symbol cycle: {}", c.join(" -> ")),
        ShapeError::UnknownFont(f) => format!("unknown font {f}"),
        ShapeError::OutOfRange => "a coordinate past 2^40 nm".into(),
    }
}

fn place(p: &Place) -> Value {
    let mut v = json!({"at_nm": point(p.at), "rot_mdeg": p.rot.get(), "mirror": p.mirror});
    if let Some(s) = p.scale {
        v["scale_ppm"] = json!(s);
    }
    v
}

/// A feature as written: its geometry, before any shape.
fn geometry(k: &FeatureKind) -> Value {
    match k {
        FeatureKind::Line { a, b, .. } => json!({"a_nm": point(*a), "b_nm": point(*b)}),
        FeatureKind::Arc { arc, .. } => json!({
            "start_nm": point(arc.start),
            "end_nm": point(arc.end),
            "centre_nm": point(arc.centre),
            "dir": if arc.dir == Dir::Ccw { "ccw" } else { "cw" },
        }),
        FeatureKind::Pad { place: p, .. } => place(p),
        FeatureKind::Surface { regions } => {
            json!({"regions": regions.iter().map(region).collect::<Vec<_>>()})
        }
        FeatureKind::Text(t) => json!({
            "font": t.font,
            "place": place(&t.place),
            "width_nm": t.width,
            "height_nm": t.height,
            "line_width_nm": t.line_width,
            "string": t.string,
        }),
        FeatureKind::Barcode(b) => {
            json!({"place": place(&b.place), "kind": b.kind, "string": b.string})
        }
    }
}

fn symbol_index(k: &FeatureKind) -> Option<u32> {
    match k {
        FeatureKind::Line { symbol, .. }
        | FeatureKind::Arc { symbol, .. }
        | FeatureKind::Pad { symbol, .. } => Some(*symbol),
        _ => None,
    }
}

/// The board as an inspection sees it: which step, and how references
/// are written.
struct At<'a> {
    b: &'a Board,
    prefix: Option<&'a str>,
    step: usize,
}

impl At<'_> {
    fn step(&self) -> &Step {
        &self.b.steps[self.step]
    }

    fn reference(&self, r: Ref) -> String {
        match self.prefix {
            Some(p) => format!("{p}/{r}"),
            None => r.to_string(),
        }
    }

    /// A reference as given, less the board's prefix.
    fn parse(&self, text: &str, what: &str) -> Result<Ref> {
        let bad = || {
            Error::new(
                Code::InvalidParam,
                format!("{text:?} isn't a reference on this board"),
            )
            .with("param", what)
        };
        let rest = match self.prefix {
            Some(p) => text
                .strip_prefix(p)
                .and_then(|r| r.strip_prefix('/'))
                .ok_or_else(bad)?,
            None => text,
        };
        rest.parse::<Ref>().map_err(|_| bad())
    }

    fn feature_ref(&self, layer: usize, index: usize) -> String {
        self.reference(Ref::Feature {
            step: self.step().name.clone(),
            layer: self.b.layers[layer].name.clone(),
            index,
        })
    }

    fn net_name(&self, net: Option<u32>) -> Option<&str> {
        net.and_then(|n| self.step().nets.get(n as usize))
            .map(|n| n.name.as_str())
    }
}

/// The layer named `name`, or `not_found` naming the layers there are.
fn layer_index(b: &Board, name: &str) -> Result<usize> {
    b.layer(name).ok_or_else(|| {
        Error::new(Code::NotFound, format!("the board has no layer {name}"))
            .with("layer", name)
            .with(
                "layers",
                b.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            )
    })
}

/// The step named `name`, or the board step.
pub fn step_index(b: &Board, name: Option<&str>) -> Result<usize> {
    match name {
        None => Ok(b.board_step),
        Some(n) => b.step(n).ok_or_else(|| {
            Error::new(Code::NotFound, format!("the board has no step {n}"))
                .with("step", n)
                .with(
                    "steps",
                    b.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                )
        }),
    }
}

/// Inspects board `b`; `prefix` is its ID in a project.
pub fn inspect(b: &Board, prefix: Option<&str>, args: &Args) -> Result<Value> {
    let asked = [
        args.layer.is_some(),
        args.feature.is_some(),
        args.components,
        args.nets,
    ]
    .iter()
    .filter(|x| **x)
    .count();
    if asked > 1 {
        return Err(Error::new(
            Code::InvalidParam,
            "ask for one of a layer, a feature, components or nets",
        ));
    }
    if args.limit == Some(0) {
        return Err(
            Error::new(Code::InvalidParam, "a page holds at least one item").with("param", "limit"),
        );
    }
    let at = At {
        b,
        prefix,
        step: step_index(b, args.step.as_deref())?,
    };
    let mut v = if let Some(f) = &args.feature {
        feature(&at, f)?
    } else if let Some(l) = &args.layer {
        features(&at, layer_index(b, l)?, args)?
    } else if args.components {
        components(&at, args)?
    } else if args.nets {
        nets(&at, args)?
    } else {
        summary(&at)
    };
    v["board"] = json!(prefix);
    Ok(v)
}

/// A page of `items` (each with its reference) after `after`.
fn page(
    at: &At,
    items: Vec<(String, Value)>,
    args: &Args,
    what: &str,
) -> Result<(Vec<Value>, Option<String>, usize)> {
    let total = items.len();
    let start = match &args.after {
        None => 0,
        Some(a) => {
            at.parse(a, "after")?;
            items
                .iter()
                .position(|(r, _)| r == a)
                .map(|i| i + 1)
                .ok_or_else(|| {
                    Error::new(Code::NotFound, format!("no {what} {a} to start after"))
                        .with("after", a)
                })?
        }
    };
    let limit = args.limit.unwrap_or(PAGE);
    let page: Vec<(String, Value)> = items.into_iter().skip(start).take(limit).collect();
    let next = (start + page.len() < total)
        .then(|| page.last().map(|(r, _)| r.clone()))
        .flatten();
    Ok((page.into_iter().map(|(_, v)| v).collect(), next, total))
}

fn summary(at: &At) -> Value {
    let b = at.b;
    let step = at.step();
    let layers: Vec<usize> = (0..b.layers.len()).collect();
    b.build_shapes(at.step, &layers);
    let layer_json: Vec<Value> = b
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let f = &step.layers[i];
            let mut counts = Map::new();
            for k in ["line", "pad", "arc", "surface", "text", "barcode"] {
                counts.insert(k.into(), json!(0));
            }
            for feat in f.features() {
                let k = feature_kind(&feat.kind);
                counts[k] = json!(counts[k].as_u64().unwrap_or(0) + 1);
            }
            counts.insert("total".into(), json!(f.features().len()));
            let shapes = b.layer_shapes(at.step, i);
            let unshaped = shapes.iter().filter(|s| s.is_err()).count();
            // In parallel: a union is the same in any order.
            let bounds = shapes
                .par_iter()
                .filter_map(|s| s.as_ref().ok().and_then(Shape::bounds))
                .reduce_with(Rect::union);
            let (mut standard, mut user, mut unknown) = (0, Vec::new(), Vec::new());
            for s in f.symbols() {
                match s {
                    Symbol::Standard(_) => standard += 1,
                    Symbol::User(n) => user.push(n.as_str()),
                    Symbol::Unknown(n) => unknown.push(n.as_str()),
                }
            }
            let mut v = json!({
                "name": l.name,
                "row": l.row,
                "context": if l.context == Context::Board { "board" } else { "misc" },
                "kind": kind_name(&l.kind),
                "polarity": polarity(l.polarity),
                "side": l.side.map(side),
                "features": counts,
                "box_nm": bounds.map(rect),
                "symbols": {"standard": standard, "user": user, "unknown": unknown},
                "unshaped": unshaped,
            });
            if let Some(s) = l.span {
                v["span"] = json!({"start": b.layers[s.start].name, "end": b.layers[s.end].name});
            }
            v
        })
        .collect();
    let drills: Vec<Value> = b
        .drills
        .iter()
        .map(|d| {
            json!({
                "layer": b.layers[d.layer].name,
                "class": b.hole_class(d).map(|c| match c {
                    HoleClass::Through => "through",
                    HoleClass::Blind => "blind",
                    HoleClass::Buried => "buried",
                }),
                "back_drill": d.back_drill,
                "tools": d.tools.iter().map(|t| json!({
                    "number": t.number,
                    "kind": match t.kind {
                        ToolKind::Plated => "plated",
                        ToolKind::NonPlated => "non_plated",
                        ToolKind::Via => "via",
                    },
                    "finished_nm": t.finished,
                    "drilled_nm": t.drilled,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let stackup = b.stackup.as_ref().map(|s| {
        s.layers
            .iter()
            .map(|l| {
                json!({
                    "name": l.name,
                    "kind": if l.kind == StackKind::Copper { "copper" } else { "dielectric" },
                    "material": l.material,
                    "thickness_nm": l.thickness,
                    "properties": attributes(&l.properties),
                })
            })
            .collect::<Vec<_>>()
    });
    json!({
        "name": b.name,
        "source": {
            "format": b.source.format,
            "version": b.source.version,
            "application": b.source.application,
            "units": b.source.units,
            "created": b.source.created,
            "saved": b.source.saved,
        },
        "step": step.name,
        "board_step": b.steps[b.board_step].name,
        "steps": b.steps.iter().map(|s| json!({
            "name": s.name,
            "datum_nm": point(s.datum),
            "profile_box_nm": s.profile.as_ref().map(|p| rect(p.bounds())),
            "repeats": s.repeats.iter().map(|r| json!({
                "step": r.step,
                "at_nm": point(r.at),
                "dx_nm": r.dx,
                "dy_nm": r.dy,
                "nx": r.nx,
                "ny": r.ny,
                "rot_mdeg": r.rot.get(),
                "mirror": r.mirror,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "layers": layer_json,
        "drills": drills,
        "components": step.components.len(),
        "packages": step.packages.len(),
        "nets": step.nets.len(),
        "stackup": stackup,
        "profile_box_nm": step.profile.as_ref().map(|p| rect(p.bounds())),
        "capabilities": capabilities(&b.capabilities),
        "attributes": attributes(&b.attributes),
        "warnings": b.warnings.iter().map(codes::notice).collect::<Vec<_>>(),
        "faults": b.faults.iter().map(codes::notice).collect::<Vec<_>>(),
    })
}

fn features(at: &At, layer: usize, args: &Args) -> Result<Value> {
    let f = &at.step().layers[layer];
    let shapes = at.b.layer_shapes(at.step, layer);
    let items: Vec<(String, Value)> = f
        .features()
        .iter()
        .enumerate()
        .map(|(i, feat)| {
            let r = at.feature_ref(layer, i);
            let mut v = json!({
                "ref": r,
                "kind": feature_kind(&feat.kind),
                "polarity": polarity(feat.polarity),
                "box_nm": shapes[i].as_ref().ok().and_then(Shape::bounds).map(rect),
            });
            if let Some(s) = symbol_index(&feat.kind).and_then(|s| f.symbols().get(s as usize)) {
                v["symbol"] = symbol(s);
            }
            if let Some(n) = at.net_name(feat.net) {
                v["net"] = json!(n);
            }
            if let Err(e) = &shapes[i] {
                v["shape_error"] = json!(shape_error(e));
            }
            (r, v)
        })
        .collect();
    let (items, next, total) = page(at, items, args, "feature")?;
    Ok(json!({
        "step": at.step().name,
        "layer": at.b.layers[layer].name,
        "total": total,
        "features": items,
        "next": next,
    }))
}

fn feature(at: &At, text: &str) -> Result<Value> {
    let Ref::Feature { step, layer, index } = at.parse(text, "feature")? else {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{text} isn't a feature's reference"),
        )
        .with("param", "feature"));
    };
    let s = step_index(at.b, Some(&step))?;
    let l = layer_index(at.b, &layer)?;
    let at = At { step: s, ..*at };
    let f = &at.step().layers[l];
    let feat = f.features().get(index).ok_or_else(|| {
        Error::new(
            Code::NotFound,
            format!(
                "layer {layer} has {} features; no {index}",
                f.features().len()
            ),
        )
        .with("feature", text)
    })?;
    let mut v = json!({
        "ref": at.feature_ref(l, index),
        "step": step,
        "layer": layer,
        "index": index,
        "kind": feature_kind(&feat.kind),
        "polarity": polarity(feat.polarity),
        "dcode": feat.dcode,
        "id": feat.id,
        "net": at.net_name(feat.net),
        "attributes": attributes(&feat.attributes),
        "geometry": geometry(&feat.kind),
    });
    if let Some(s) = symbol_index(&feat.kind).and_then(|s| f.symbols().get(s as usize)) {
        v["symbol"] = symbol(s);
    }
    match at.b.feature_shape(s, l, index) {
        Ok(sh) => v["shape"] = shape(sh),
        Err(e) => v["shape_error"] = json!(shape_error(e)),
    }
    Ok(v)
}

fn components(at: &At, args: &Args) -> Result<Value> {
    let step = at.step();
    let items: Vec<(String, Value)> = step
        .components
        .iter()
        .map(|c| {
            let r = at.reference(Ref::Component {
                step: step.name.clone(),
                refdes: c.refdes.clone(),
            });
            let v = json!({
                "ref": r,
                "refdes": c.refdes,
                "package": step.packages.get(c.package as usize).map(|p| p.name.as_str()),
                "part": c.part,
                "at_nm": point(c.at),
                "rot_mdeg": c.rot.get(),
                "mirror": c.mirror,
                "side": side(c.side),
                "properties": c.properties,
                "attributes": attributes(&c.attributes),
                "pins": c.pins.iter().map(|p| json!({
                    "name": p.name,
                    "at_nm": point(p.at),
                    "rot_mdeg": p.rot.get(),
                    "net": at.net_name(p.net),
                })).collect::<Vec<_>>(),
            });
            (r, v)
        })
        .collect();
    let (items, next, total) = page(at, items, args, "component")?;
    Ok(json!({"step": step.name, "total": total, "components": items, "next": next}))
}

fn nets(at: &At, args: &Args) -> Result<Value> {
    let step = at.step();
    let items: Vec<(String, Value)> = step
        .nets
        .iter()
        .map(|n| {
            let r = at.reference(Ref::Net {
                step: step.name.clone(),
                name: n.name.clone(),
            });
            let v = json!({
                "ref": r,
                "name": n.name,
                "features": n.features.len(),
                "pins": n.pins.iter().map(|p| {
                    let c = &step.components[p.component];
                    json!({"component": c.refdes, "pin": c.pins[p.pin].name})
                }).collect::<Vec<_>>(),
                "cad_points": n.cad.len(),
            });
            (r, v)
        })
        .collect();
    let (items, next, total) = page(at, items, args, "net")?;
    Ok(json!({"step": step.name, "total": total, "nets": items, "next": next}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Board {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean.tgz");
        crate::odb::read_path(&path, &Default::default()).unwrap()
    }

    #[test]
    fn a_summary_of_the_board() {
        let b = minimal();
        let v = inspect(&b, None, &Args::default()).unwrap();
        assert_eq!(v["board"], Value::Null);
        assert_eq!(v["step"], json!("pcb"));
        assert_eq!(v["layers"].as_array().unwrap().len(), 23);
        assert_eq!(v["components"], json!(11));
        assert_eq!(
            v["capabilities"],
            json!(["geometry", "nets", "components", "stackup", "drill"])
        );
        let total: u64 = v["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["features"]["total"].as_u64().unwrap())
            .sum();
        assert_eq!(total, 258 + 72 + 1);
        assert!(v["profile_box_nm"].is_array());
    }

    #[test]
    fn pages_follow_on() {
        let b = minimal();
        let args = Args {
            components: true,
            limit: Some(4),
            ..Default::default()
        };
        let p1 = inspect(&b, Some("mc"), &args).unwrap();
        assert_eq!(p1["total"], json!(11));
        assert_eq!(p1["components"].as_array().unwrap().len(), 4);
        let next = p1["next"].as_str().unwrap().to_owned();
        assert!(next.starts_with("mc/pcb/comp/"), "{next}");
        let mut seen = 4;
        let mut after = Some(next);
        while let Some(a) = after {
            let p = inspect(
                &b,
                Some("mc"),
                &Args {
                    after: Some(a),
                    ..args.clone()
                },
            )
            .unwrap();
            seen += p["components"].as_array().unwrap().len();
            after = p["next"].as_str().map(str::to_owned);
        }
        assert_eq!(seen, 11);
        let e = inspect(
            &b,
            Some("mc"),
            &Args {
                after: Some("pcb/comp/U1".into()),
                ..args
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn one_feature_with_its_shape() {
        let b = minimal();
        let step = &b.steps[b.board_step];
        let layer = (0..b.layers.len())
            .find(|&i| {
                step.layers[i]
                    .features()
                    .iter()
                    .any(|f| matches!(f.kind, FeatureKind::Pad { .. }))
            })
            .unwrap();
        let name = b.layers[layer].name.clone();
        let list = inspect(
            &b,
            None,
            &Args {
                layer: Some(name.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let first_pad = list["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["kind"] == "pad")
            .unwrap()["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let f = inspect(
            &b,
            None,
            &Args {
                feature: Some(first_pad.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(f["ref"], json!(first_pad));
        assert!(f["symbol"]["standard"].is_string(), "{f}");
        assert!(
            f["shape"]["parts"]
                .as_array()
                .is_some_and(|p| !p.is_empty())
        );
        assert!(f["geometry"]["at_nm"].is_array());
        let e = inspect(
            &b,
            None,
            &Args {
                feature: Some(format!("pcb/{name}/99999")),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = inspect(
            &b,
            None,
            &Args {
                layer: Some("nope".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        assert!(
            inspect(
                &b,
                None,
                &Args {
                    nets: true,
                    components: true,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
