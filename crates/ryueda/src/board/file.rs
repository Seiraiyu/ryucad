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
pub const TRACKS: &str = "tracks";
pub const VIAS: &str = "vias";
pub const HOLES: &str = "holes";
pub const TRACK_KIND: &str = "ryueda.track";
pub const VIA_KIND: &str = "ryueda.via";
pub const HOLE_KIND: &str = "ryueda.hole";

/// A copper layer of a two-layer board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopperLayer {
    Top,
    Bottom,
}

impl CopperLayer {
    /// Its index in the copper layout's layers.
    pub fn index(self) -> usize {
        match self {
            CopperLayer::Top => 0,
            CopperLayer::Bottom => 1,
        }
    }

    /// Its shapes layer: `copper.top` or `copper.bottom`.
    pub fn name(self) -> &'static str {
        match self {
            CopperLayer::Top => "copper.top",
            CopperLayer::Bottom => "copper.bottom",
        }
    }

    /// `top`, `bottom`, `copper.top` or `copper.bottom`.
    pub fn parse(text: &str) -> Option<CopperLayer> {
        match text {
            "top" | "copper.top" => Some(CopperLayer::Top),
            "bottom" | "copper.bottom" => Some(CopperLayer::Bottom),
            _ => None,
        }
    }
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

/// A plated through via.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Via {
    pub net: String,
    pub at: Point,
    pub drill_nm: i64,
    pub diameter_nm: i64,
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
        assert_eq!(track.layer.index(), 1);
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
        let bad = json!({"kind": TRACK_KIND, "format": 1, "net": "x", "layer": "inner",
                         "width_nm": 1, "points": [[0, 0], [1, 1]]});
        assert!(doc::from_value::<Track>(bad, "t.json", TRACK_KIND, FORMAT).is_err());
        let b = Id::parse("b", "board").unwrap();
        let t1 = Id::parse("t1", "track").unwrap();
        assert_eq!(route_path(&b, TRACKS, &t1), "boards/b/tracks/t1.json");
    }
}
