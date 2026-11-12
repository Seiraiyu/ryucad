//! A sheet as a window draws it (design 519, plan 3.B): `ryusch::paint`'s
//! items, each with its owner's index; the owners by reference path, each
//! with its kind, box and a hash of its items; the paper with its border
//! and title block; the sheet's pins and lint findings. Written as JSON
//! text, never a `Value` tree, so a 500-part sheet answers without one.

use crate::id::Id;
use ryusch::font::{Anchor, Font, Weight};
use ryusch::geom::{Nm, Point, Rect};
use ryusch::paint::{Item, Painted};
use ryusch::style::Rgb;
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
}
