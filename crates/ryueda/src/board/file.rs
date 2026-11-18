//! The board's files: `board.json` (kind `ryueda.board`),
//! `placements/<occurrence>.json` (kind `ryueda.placement`) and the routes,
//! `tracks/<id>.json`, `vias/<id>.json` and `holes/<id>.json`, canonical
//! JSON, unknown fields refused. Coordinates are integer nanometres, y up;
//! angles millidegrees counterclockwise.

use crate::id::Id;
use crate::source::join;
use crate::symbol::Point;
use serde::{Deserialize, Serialize};

pub const BOARDS: &str = "boards";
pub const BOARD_FILE: &str = "board.json";
pub const PLACEMENTS: &str = "placements";
pub const BOARD_KIND: &str = "ryueda.board";
pub const PLACEMENT_KIND: &str = "ryueda.placement";
pub const FORMAT: u64 = 1;
/// The newest format read; a file is written at 2 only when it holds
/// what format 1 can't (a stackup, a bottom placement, an inner track, a
/// via's span).
pub const NEWEST: u64 = 2;
pub const TRACKS: &str = "tracks";
pub const VIAS: &str = "vias";
pub const HOLES: &str = "holes";
pub const TRACK_KIND: &str = "ryueda.track";
pub const VIA_KIND: &str = "ryueda.via";
pub const HOLE_KIND: &str = "ryueda.hole";

/// A copper layer by its position in the stackup: the top, the `k`th
/// inner layer from the top (1 or more), or the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CopperLayer {
    Top,
    Inner(u8),
    Bottom,
}

impl CopperLayer {
    /// Its index among `n` copper layers; `None` for an inner layer the
    /// stackup lacks.
    pub fn index(self, n: usize) -> Option<usize> {
        match self {
            CopperLayer::Top => Some(0),
            CopperLayer::Bottom => Some(n.max(2) - 1),
            CopperLayer::Inner(k) => {
                let k = usize::from(k);
                (k >= 1 && k + 1 < n).then_some(k)
            }
        }
    }

    /// The layer at `index` of `n`.
    pub fn at(index: usize, n: usize) -> CopperLayer {
        if index == 0 {
            CopperLayer::Top
        } else if index + 1 >= n {
            CopperLayer::Bottom
        } else {
            CopperLayer::Inner(index as u8)
        }
    }

    /// `top`, `in<k>` or `bottom`: how files and commands name it.
    pub fn short(self) -> String {
        match self {
            CopperLayer::Top => "top".into(),
            CopperLayer::Bottom => "bottom".into(),
            CopperLayer::Inner(k) => format!("in{k}"),
        }
    }

    /// Its shapes layer: `copper.top`, `copper.in1`, `copper.bottom`.
    pub fn name(self) -> String {
        format!("copper.{}", self.short())
    }

    /// `top`, `in1` … `in32`, `bottom`, each with or without `copper.`.
    pub fn parse(text: &str) -> Option<CopperLayer> {
        let t = text.strip_prefix("copper.").unwrap_or(text);
        match t {
            "top" => Some(CopperLayer::Top),
            "bottom" => Some(CopperLayer::Bottom),
            _ => {
                let k = t.strip_prefix("in")?;
                if k.is_empty() || k.starts_with('0') || !k.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let k: u8 = k.parse().ok()?;
                (usize::from(k) + 2 <= super::stackup::MAX_COPPER).then_some(CopperLayer::Inner(k))
            }
        }
    }
}

impl Serialize for CopperLayer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.short())
    }
}

impl<'de> Deserialize<'de> for CopperLayer {
    fn deserialize<D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<CopperLayer, D::Error> {
        let text = String::deserialize(d)?;
        // Files name it bare: `top`, never `copper.top`.
        (!text.starts_with("copper."))
            .then(|| CopperLayer::parse(&text))
            .flatten()
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "{text} isn't a copper layer (top, in1 …, bottom)"
                ))
            })
    }
}

/// A board file as an answer prints it, at the format it is written in.
pub fn value<T: Serialize>(t: &T, kind: &str) -> serde_json::Value {
    let v = serde_json::to_value(t).expect("file types serialize");
    let format = crate::conn::format_of(kind, &v);
    crate::doc::to_value(t, kind, format)
}

/// A track: a polyline of straight segments drawn with a round pen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// The board's local net reference (`sda`, `s1/n1`, `rail:gnd`).
    pub net: String,
    pub layer: CopperLayer,
    pub width_nm: i64,
    pub points: Vec<Point>,
}

/// A plated via: through the board, or blind or buried over a span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Via {
    pub net: String,
    pub at: Point,
    pub drill_nm: i64,
    pub diameter_nm: i64,
    /// The first and last copper layers it joins; the top and the bottom
    /// when absent (a through via).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<CopperLayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<CopperLayer>,
}

impl Via {
    /// Its span as layer indices of `n`, if both layers exist and `from`
    /// lies above `to`.
    pub fn span(&self, n: usize) -> Option<(usize, usize)> {
        let f = self.from.unwrap_or(CopperLayer::Top).index(n)?;
        let t = self.to.unwrap_or(CopperLayer::Bottom).index(n)?;
        (f < t).then_some((f, t))
    }
}

/// An unplated drilled hole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub at: Point,
    pub drill_nm: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub name: String,
    /// The design it realises, by ID.
    pub design: Id,
    pub preset: String,
    pub outline: Contour,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<Contour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stackup: Option<super::stackup::Stackup>,
}

/// One closed contour: from `start`, each edge to its `to`; closed when
/// the last `to` is `start`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contour {
    pub start: Point,
    pub edges: Vec<Op>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    Line {
        to: Point,
    },
    /// Through `mid`, from the last point to `to`.
    Arc {
        mid: Point,
        to: Point,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub at: Point,
    #[serde(default)]
    pub rotation_mdeg: i64,
}

impl Contour {
    /// A rectangle `w` by `h` with its bottom left at the origin,
    /// counterclockwise.
    pub fn rect(w: i64, h: i64) -> Contour {
        Contour {
            start: [0, 0],
            edges: vec![
                Op::Line { to: [w, 0] },
                Op::Line { to: [w, h] },
                Op::Line { to: [0, h] },
                Op::Line { to: [0, 0] },
            ],
        }
    }

    /// Every point it names.
    pub fn points(&self) -> Vec<Point> {
        let mut out = vec![self.start];
        for e in &self.edges {
            match e {
                Op::Line { to } => out.push(*to),
                Op::Arc { mid, to } => {
                    out.push(*mid);
                    out.push(*to);
                }
            }
        }
        out
    }
}

/// `boards/<board>/board.json`.
pub fn board_path(board: &Id) -> String {
    join(&board_dir(board), BOARD_FILE)
}

/// `boards/<board>`.
pub fn board_dir(board: &Id) -> String {
    join(BOARDS, board.as_str())
}

/// The file name of an occurrence's placement: its instance path and
/// component joined by `.` (`s1.u1`), the design's prefix dropped.
pub fn placement_name(occurrence: &str) -> String {
    occurrence.replace('/', ".")
}

/// The occurrence a placement file's stem names (`s1.u1` to `s1/u1`), if
/// every part is an ID.
pub fn occurrence_of(stem: &str) -> Option<String> {
    let parts: Vec<&str> = stem.split('.').collect();
    parts
        .iter()
        .all(|p| Id::parse(p, "occurrence").is_ok())
        .then(|| parts.join("/"))
}

/// `boards/<board>/placements/<occurrence>.json`.
pub fn placement_path(board: &Id, occurrence: &str) -> String {
    join(
        &join(&board_dir(board), PLACEMENTS),
        &format!("{}.json", placement_name(occurrence)),
    )
}

/// `boards/<board>/<folder>/<id>.json`, `folder` one of [`TRACKS`],
/// [`VIAS`], [`HOLES`].
pub fn route_path(board: &Id, folder: &str, id: &Id) -> String {
    join(&join(&board_dir(board), folder), &format!("{id}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc;
    use serde_json::json;

    #[test]
    fn a_board_reads_and_writes_canonically() {
        let v = json!({
            "kind": BOARD_KIND, "format": 1, "name": "Main", "design": "board",
            "preset": "jlc-2l",
            "outline": {"start": [0, 0], "edges": [
                {"type": "line", "to": [10, 0]},
                {"type": "arc", "mid": [15, 5], "to": [10, 10]},
                {"type": "line", "to": [0, 10]},
                {"type": "line", "to": [0, 0]}]}
        });
        let b: Board =
            doc::from_value(v.clone(), "boards/b/board.json", BOARD_KIND, FORMAT).unwrap();
        assert_eq!(b.outline.edges.len(), 4);
        assert_eq!(doc::to_value(&b, BOARD_KIND, FORMAT), v);
    }

    #[test]
    fn copper_layers_are_named_by_position() {
        for (text, layer, at4) in [
            ("top", CopperLayer::Top, 0),
            ("in1", CopperLayer::Inner(1), 1),
            ("in2", CopperLayer::Inner(2), 2),
            ("bottom", CopperLayer::Bottom, 3),
        ] {
            assert_eq!(CopperLayer::parse(text), Some(layer));
            assert_eq!(CopperLayer::parse(&format!("copper.{text}")), Some(layer));
            assert_eq!(layer.index(4), Some(at4));
            assert_eq!(layer.name(), format!("copper.{text}"));
            assert_eq!(serde_json::to_value(layer).unwrap(), json!(text));
            assert_eq!(
                serde_json::from_value::<CopperLayer>(json!(text)).unwrap(),
                layer
            );
            assert_eq!(CopperLayer::at(at4, 4), layer);
        }
        assert_eq!(CopperLayer::Inner(1).index(2), None);
        assert_eq!(CopperLayer::Inner(3).index(4), None);
        assert_eq!(CopperLayer::Bottom.index(2), Some(1));
        for bad in ["in0", "in", "in01", "in33", "inner", "IN1"] {
            assert_eq!(CopperLayer::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn files_keep_format_1_until_they_hold_something_new() {
        let t = |layer: &str| json!({"net": "a", "layer": layer, "width_nm": 1, "points": [[0, 0], [1, 1]]});
        assert_eq!(crate::conn::format_of(TRACK_KIND, &t("top")), 1);
        assert_eq!(crate::conn::format_of(TRACK_KIND, &t("in1")), 2);
        assert_eq!(
            crate::conn::format_of(VIA_KIND, &json!({"net": "a", "at": [0, 0]})),
            1
        );
        assert_eq!(
            crate::conn::format_of(VIA_KIND, &json!({"net": "a", "at": [0, 0], "to": "in1"})),
            2
        );
        assert_eq!(crate::conn::format_of(BOARD_KIND, &json!({"name": "b"})), 1);
        assert_eq!(
            crate::conn::format_of(BOARD_KIND, &json!({"name": "b", "stackup": {}})),
            2
        );
        assert_eq!(
            crate::conn::format_of(PLACEMENT_KIND, &json!({"at": [0, 0]})),
            1
        );
        assert_eq!(
            crate::conn::format_of(PLACEMENT_KIND, &json!({"at": [0, 0], "side": "bottom"})),
            2
        );
        assert_eq!(crate::conn::format_of(HOLE_KIND, &json!({"at": [0, 0]})), 1);
        assert_eq!(crate::merge::newest(TRACK_KIND), Some(2));
        assert_eq!(crate::merge::newest(HOLE_KIND), Some(1));
    }

    #[test]
    fn unknown_fields_are_refused() {
        let v = json!({"kind": PLACEMENT_KIND, "format": 1, "at": [0, 0], "side": "top"});
        let e = doc::from_value::<Placement>(v, "p.json", PLACEMENT_KIND, FORMAT).unwrap_err();
        assert_eq!(e.code, crate::error::Code::InvalidDocument);
    }

    #[test]
    fn placement_names_round_trip() {
        assert_eq!(placement_name("s1/u1"), "s1.u1");
        assert_eq!(occurrence_of("s1.u1").as_deref(), Some("s1/u1"));
        assert_eq!(occurrence_of("r1").as_deref(), Some("r1"));
        assert_eq!(occurrence_of("S1.u1"), None);
        assert_eq!(occurrence_of("s1..u1"), None);
        let b = Id::parse("b", "board").unwrap();
        assert_eq!(
            placement_path(&b, "s1/u1"),
            "boards/b/placements/s1.u1.json"
        );
        assert_eq!(board_path(&b), "boards/b/board.json");
    }

    #[test]
    fn routes_read_and_write_canonically() {
        let t = json!({"kind": TRACK_KIND, "format": 1, "net": "rail:gnd", "layer": "bottom",
                       "width_nm": 250_000, "points": [[0, 0], [1_000_000, 0], [1_000_000, 2_000_000]]});
        let track: Track =
            doc::from_value(t.clone(), "boards/b/tracks/t1.json", TRACK_KIND, FORMAT).unwrap();
        assert_eq!(track.layer, CopperLayer::Bottom);
        assert_eq!(track.layer.index(2), Some(1));
        assert_eq!(track.layer.name(), "copper.bottom");
        assert_eq!(doc::to_value(&track, TRACK_KIND, FORMAT), t);
        let v = json!({"kind": VIA_KIND, "format": 1, "net": "sda", "at": [5, 6],
                       "drill_nm": 300_000, "diameter_nm": 600_000});
        let via: Via =
            doc::from_value(v.clone(), "boards/b/vias/v1.json", VIA_KIND, FORMAT).unwrap();
        assert_eq!(doc::to_value(&via, VIA_KIND, FORMAT), v);
        let h = json!({"kind": HOLE_KIND, "format": 1, "at": [5, 6], "drill_nm": 3_200_000});
        let hole: Hole =
            doc::from_value(h.clone(), "boards/b/holes/h1.json", HOLE_KIND, FORMAT).unwrap();
        assert_eq!(doc::to_value(&hole, HOLE_KIND, FORMAT), h);
        let bad = json!({"kind": TRACK_KIND, "format": 1, "net": "x", "layer": "in0",
                         "width_nm": 1, "points": [[0, 0], [1, 1]]});
        assert!(doc::from_value::<Track>(bad, "t.json", TRACK_KIND, FORMAT).is_err());
        let b = Id::parse("b", "board").unwrap();
        let t1 = Id::parse("t1", "track").unwrap();
        assert_eq!(route_path(&b, TRACKS, &t1), "boards/b/tracks/t1.json");
    }
}
