//! Sheet metal as the window shows it (design 346, The app): what a base
//! flange's, flange's, unfold's or refold's rebuild report says, a sheet
//! metal solid's bend table as `sheet.get` gives it (the Part tile's
//! sheet metal section), and what choosing an unfold or a refold paints:
//! an unfold its bend lines (the edges `flat.get` names), a refold the
//! strips it folded (its bends' faces). A flange chosen paints its bends'
//! and flanges' faces by origin, as any feature that makes faces does.

use crate::core::flat::FlatLines;
use ryucad::error::Error;
use serde_json::Value;

/// The kinds of sheet metal feature, as reports name their objects.
pub const KINDS: [&str; 4] = ["base_flange", "flange", "unfold", "refold"];

/// What a sheet metal feature's report says the window needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetInfo {
    /// `base_flange`, `flange`, `unfold` or `refold`.
    pub kind: String,
    /// The solid it acts on (a base flange's own ID).
    pub target: Option<String>,
    /// An unfold's or refold's bends, each by a face of it
    /// (`<part>/<solid>/<name>`); none means every one.
    pub only: Vec<String>,
    /// A refold's unfold.
    pub unfold: Option<String>,
    /// An unfold's flat, once it built: how many bends it unfolded and
    /// whether that was some of them.
    pub flat: Option<(u64, bool)>,
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_owned))
        .collect()
}

impl SheetInfo {
    /// From a feature's rebuild report: its `base_flange`, `flange`,
    /// `unfold` or `refold` object; `None` for any other feature.
    pub fn from_report(f: &Value) -> Option<SheetInfo> {
        let (kind, v) = KINDS
            .iter()
            .find_map(|k| f.get(*k).filter(|v| v.is_object()).map(|v| (*k, v)))?;
        let target = match kind {
            "base_flange" => f["id"].as_str().map(str::to_owned),
            _ => v["target"].as_str().map(str::to_owned),
        };
        let flat = f["flat"].is_object().then(|| {
            (
                f["flat"]["bends"].as_u64().unwrap_or(0),
                f["flat"]["partial"] == true,
            )
        });
        Some(SheetInfo {
            kind: kind.to_owned(),
            target,
            only: texts(&v["only"]),
            unfold: v["unfold"].as_str().map(str::to_owned),
            flat,
        })
    }
}

/// One bend of the table: named by its least inner face, as `unfold`'s
/// `only` and `bends` take it.
#[derive(Clone, Debug, PartialEq)]
pub struct Bend {
    pub bend: String,
    pub inner: Vec<String>,
    pub outer: Vec<String>,
    pub radius_m: f64,
    pub angle_deg: f64,
    /// `up` or `down`.
    pub direction: String,
    /// Its developed length, or why there is none.
    pub developed: Result<f64, String>,
}

impl Bend {
    /// Every face of the bend.
    pub fn faces(&self) -> impl Iterator<Item = &String> {
        self.inner.iter().chain(&self.outer)
    }
}

/// A sheet metal solid's table, as `sheet.get` answers it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetTable {
    pub part: String,
    pub solid: String,
    /// The part revision it was read at.
    pub revision: String,
    pub thickness_m: f64,
    /// `folded`, `flat` or `partly`.
    pub state: String,
    /// The solid's allowance as its founder gives it: `K 0.44`,
    /// `deduction 2.4 mm`; `None` where no base flange founded it.
    pub allowance: Option<String>,
    pub bends: Vec<Bend>,
    /// The bends the unfold in force unfolded, by their inner faces.
    pub unfolded: Vec<String>,
    /// The unfold in force.
    pub unfold: Option<String>,
}

impl SheetTable {
    /// From `sheet.get`'s answer, read at part revision `revision`.
    pub fn from_answer(revision: &str, v: &Value) -> SheetTable {
        let allowance = &v["allowance"];
        let allowance = if let Some(k) = allowance["k_factor"].as_f64() {
            Some(format!("K {k}"))
        } else if let Some(d) = allowance["deduction_m"].as_f64() {
            Some(format!("deduction {} mm", d * 1e3))
        } else {
            allowance["allowance_m"]
                .as_f64()
                .map(|a| format!("allowance {} mm", a * 1e3))
        };
        let bends = v["bends"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| Bend {
                bend: b["bend"].as_str().unwrap_or_default().to_owned(),
                inner: texts(&b["faces"]["inner"]),
                outer: texts(&b["faces"]["outer"]),
                radius_m: b["radius_m"].as_f64().unwrap_or(0.0),
                angle_deg: b["angle_deg"].as_f64().unwrap_or(0.0),
                direction: b["direction"].as_str().unwrap_or_default().to_owned(),
                developed: b["developed_m"]
                    .as_f64()
                    .ok_or_else(|| b["developed_note"].as_str().unwrap_or_default().to_owned()),
            })
            .collect();
        SheetTable {
            part: v["part"].as_str().unwrap_or_default().to_owned(),
            solid: v["solid"].as_str().unwrap_or_default().to_owned(),
            revision: revision.to_owned(),
            thickness_m: v["thickness_m"].as_f64().unwrap_or(0.0),
            state: v["state"].as_str().unwrap_or("folded").to_owned(),
            allowance,
            bends,
            unfolded: texts(&v["unfolded"]),
            unfold: v["unfold"].as_str().map(str::to_owned),
        }
    }

    /// The bend a face reference names: one of its inner or outer faces.
    pub fn bend_of(&self, face: &str) -> Option<&Bend> {
        self.bends.iter().find(|b| b.faces().any(|f| f == face))
    }
}

/// What the Part tile's sheet metal section shows: the table of the
/// solid asked for, or why it has none.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Table(SheetTable),
    /// Not sheet metal, or not readable: the section says why, quietly.
    Failed(Error),
}

/// One row of the bend table as the Part tile writes it:
/// `up 90° R1 mm, 2.81 mm developed`.
pub fn bend_text(b: &Bend, unit: ryucad::units::Unit) -> String {
    let len = |m: f64| format!("{} {}", trim(m / unit.factor()), unit.as_str());
    let developed = match &b.developed {
        Ok(d) => format!("{} developed", len(*d)),
        Err(why) if why.is_empty() => "no developed length".to_owned(),
        Err(why) => why.clone(),
    };
    format!(
        "{} {}° R{}, {developed}",
        b.direction,
        trim(b.angle_deg),
        len(b.radius_m)
    )
}

/// A number to four decimals, trailing zeros dropped.
fn trim(x: f64) -> String {
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// What choosing a sheet metal feature paints beyond the faces it made:
/// faces and edges by reference, in the chosen colour. An unfold its bend
/// lines: the edges of the flat's bend lines, when the flat shown is its
/// own, else of the bends it names (every one when it names none) in the
/// flat `flat.get` lays out of the solid as shown. A refold the faces of
/// the bends it folded: those it names, else those its unfold names,
/// else every bend the table lists.
pub fn painted(
    id: &str,
    info: &SheetInfo,
    unfold_only: &[String],
    table: Option<&SheetTable>,
    flat: Option<&FlatLines>,
) -> (Vec<String>, Vec<String>) {
    let mut faces = Vec::new();
    let mut edges = Vec::new();
    // A bend line's bend is named by its least inner face; a feature's
    // `only` by any face of it.
    let names = |only: &[String], bend: &str| {
        only.is_empty()
            || only.iter().any(|f| {
                f == bend
                    || table
                        .and_then(|t| t.bend_of(f))
                        .is_some_and(|b| b.bend == bend)
            })
    };
    match info.kind.as_str() {
        "unfold" => {
            if let Some(f) = flat {
                let own = f.unfold.as_deref() == Some(id);
                for l in &f.lines {
                    if own || names(&info.only, &l.bend) {
                        edges.extend(l.edges.iter().cloned());
                    }
                }
            }
        }
        "refold" => {
            let only = if info.only.is_empty() {
                unfold_only
            } else {
                &info.only
            };
            for b in table.into_iter().flat_map(|t| &t.bends) {
                if names(only, &b.bend) {
                    faces.extend(b.faces().cloned());
                }
            }
        }
        _ => {}
    }
    (faces, edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table() -> SheetTable {
        SheetTable::from_answer(
            "r1",
            &json!({"schema": 1, "part": "p", "solid": "plate", "thickness_m": 0.0015,
                "tolerance_m": 1e-7, "state": "folded", "tree": true,
                "allowance": {"k_factor": 0.44}, "allowance_from": "plate",
                "flanges": [],
                "bends": [
                  {"bend": "p/plate/a1", "faces": {"inner": ["p/plate/a1"], "outer": ["p/plate/a2"]},
                   "radius_m": 0.001, "angle_rad": std::f64::consts::FRAC_PI_2, "angle_deg": 90.0,
                   "direction": "up", "flanges": ["p/plate/x", "p/plate/y"],
                   "developed_m": 0.00256},
                  {"bend": "p/plate/b1", "faces": {"inner": ["p/plate/b1"], "outer": ["p/plate/b2"]},
                   "radius_m": 0.001, "angle_rad": std::f64::consts::PI, "angle_deg": 180.0,
                   "direction": "down", "flanges": ["p/plate/x", "p/plate/z"],
                   "developed_m": null, "developed_note": "a deduction has no hem"}]}),
        )
    }

    #[test]
    fn a_sheet_report_reads_its_kind_target_and_bends() {
        let u = SheetInfo::from_report(&json!({"id": "u", "type": "unfold",
            "unfold": {"target": "plate", "only": ["p/plate/a2"]},
            "flat": {"bends": 1, "partial": true, "box_m": [[0, 0], [1, 1]]}}))
        .unwrap();
        assert_eq!(u.kind, "unfold");
        assert_eq!(u.target.as_deref(), Some("plate"));
        assert_eq!(u.only, ["p/plate/a2"]);
        assert_eq!(u.flat, Some((1, true)));
        let b = SheetInfo::from_report(&json!({"id": "plate", "type": "base_flange",
            "base_flange": {"sketch": "s"}}))
        .unwrap();
        assert_eq!(b.target.as_deref(), Some("plate"));
        let r = SheetInfo::from_report(&json!({"id": "r", "refold": {"target": "plate",
            "unfold": "u"}}))
        .unwrap();
        assert_eq!(r.unfold.as_deref(), Some("u"));
        assert!(SheetInfo::from_report(&json!({"id": "s", "shell": {}})).is_none());
    }

    #[test]
    fn the_table_names_each_bend_by_any_face_and_writes_its_row() {
        let t = table();
        assert_eq!(
            (t.thickness_m, t.state.as_str(), t.allowance.as_deref()),
            (0.0015, "folded", Some("K 0.44"))
        );
        assert_eq!(t.bend_of("p/plate/a2").unwrap().bend, "p/plate/a1");
        assert!(t.bend_of("p/plate/x").is_none());
        let mm = ryucad::units::Unit::Mm;
        assert_eq!(
            bend_text(&t.bends[0], mm),
            "up 90° R1 mm, 2.56 mm developed"
        );
        assert_eq!(
            bend_text(&t.bends[1], mm),
            "down 180° R1 mm, a deduction has no hem"
        );
    }

    #[test]
    fn an_unfold_paints_its_bend_lines_and_a_refold_its_strips() {
        let t = table();
        let flat = FlatLines::from_answer(&json!({"part": "p", "solid": "plate",
            "from": "whole",
            "frame": {"origin_m": [0, 0, 0], "x": [1, 0, 0], "y": [0, 1, 0], "z": [0, 0, 1]},
            "bend_lines": [
              {"bend": "p/plate/a1", "from_mm": [0, 0], "to_mm": [10, 0], "direction": "up",
               "note": "UP 90° R1 K0.44", "edges": ["p/plate/e1", "p/plate/e2"]},
              {"bend": "p/plate/b1", "from_mm": [0, 5], "to_mm": [10, 5], "direction": "down",
               "note": "DOWN 180° R1", "edges": ["p/plate/e3"]}]}));
        let unfold = SheetInfo {
            kind: "unfold".into(),
            only: vec!["p/plate/a2".into()],
            ..SheetInfo::default()
        };
        // Folded: the bends it names, by any of their faces.
        assert_eq!(
            painted("u", &unfold, &[], Some(&t), Some(&flat)),
            (
                vec![],
                vec!["p/plate/e1".to_owned(), "p/plate/e2".to_owned()]
            )
        );
        // Its own flat in force: every bend line it laid.
        let mine = FlatLines {
            unfold: Some("u".into()),
            ..flat.clone()
        };
        assert_eq!(painted("u", &unfold, &[], Some(&t), Some(&mine)).1.len(), 3);
        // A refold naming nothing folds what its unfold named.
        let refold = SheetInfo {
            kind: "refold".into(),
            ..SheetInfo::default()
        };
        assert_eq!(
            painted("r", &refold, &["p/plate/a1".to_owned()], Some(&t), None).0,
            ["p/plate/a1", "p/plate/a2"]
        );
        assert_eq!(painted("r", &refold, &[], Some(&t), None).0.len(), 4);
    }
}
