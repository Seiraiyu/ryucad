//! The window's exploded views, apart from egui: the explodes of the
//! assembly it shows (`explode.list`), the one its participant shows
//! (`participant.changed`) and how far through it, where that puts each
//! occurrence ([`ryucad::explode::offsets`], worked out here, so a slider
//! moves the model at once), the lines from where each occurrence is to
//! where it is drawn, and a new step made by dragging selected
//! occurrences along an axis.

use crate::core::gizmo::Arrow;
use crate::core::nav::Projector;
use ryucad::explode::{Offset, Step};
use ryucad::proto::Request;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The explode a participant shows: an assembly's, and how far through.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub part: String,
    pub explode: String,
    pub fraction: f64,
}

impl Shown {
    /// As `participant.changed` gives it; `None` for none.
    pub fn from_json(v: &Value) -> Option<Shown> {
        Some(Shown {
            part: v["part"].as_str()?.to_owned(),
            explode: v["explode"].as_str()?.to_owned(),
            fraction: v["fraction"].as_f64()?,
        })
    }
}

/// One explode as `explode.list` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    /// Its steps as the file writes them.
    pub steps: Value,
    /// As they resolve now.
    pub resolved: Vec<Step>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

/// A step being made: the occurrences it moves (instance paths), the
/// axis (0, 1, 2 for x, y, z) dragged along, how far, and where the drag
/// began.
#[derive(Clone, Debug, PartialEq)]
pub struct Adding {
    pub instances: Vec<String>,
    pub centre: [f64; 3],
    pub axis: Option<usize>,
    pub distance: f64,
    pub from: [f64; 2],
}

/// What the Explode tile and the viewport know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Explodes {
    /// The assembly listed, and its revision when listed.
    pub part: Option<String>,
    pub revision: Option<String>,
    pub list: Vec<Entry>,
    /// What the participant shows.
    pub shown: Option<Shown>,
    /// A fraction the slider holds until the session says it.
    pub sliding: Option<f64>,
    /// A step being dragged out of the selection.
    pub adding: Option<Adding>,
    /// The ID typed for a new explode.
    pub name: String,
}

impl Explodes {
    /// Takes an `explode.list` answer for `part`.
    pub fn listed(&mut self, part: &str, v: &Value) {
        self.part = Some(part.to_owned());
        self.revision = v["revisions"][part].as_str().map(str::to_owned);
        self.list = v["explodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| Entry {
                id: e["explode"].as_str().unwrap_or_default().to_owned(),
                steps: e["steps"].clone(),
                resolved: e["resolved"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Step::from_json)
                    .collect(),
                warnings: e["warnings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w["message"].as_str().map(str::to_owned))
                    .collect(),
                error: e["error"]["message"].as_str().map(str::to_owned),
            })
            .collect();
    }

    /// The explode drawn now, of assembly `part`, and how far through: the
    /// one shown, at the slider's fraction while it is dragged.
    pub fn current(&self, part: &str) -> Option<(&Entry, f64)> {
        let s = self.shown.as_ref().filter(|s| s.part == part)?;
        let e = self.list.iter().find(|e| e.id == s.explode)?;
        Some((e, self.sliding.unwrap_or(s.fraction)))
    }

    /// Where the explode drawn puts each occurrence of `part`, by path,
    /// with a step being made applied after it.
    pub fn offsets(&self, part: &str) -> BTreeMap<String, Offset> {
        let mut out = match self.current(part) {
            Some((e, f)) => ryucad::explode::offsets(&e.resolved, f),
            None => BTreeMap::new(),
        };
        if let Some(a) = &self.adding
            && let Some(step) = a.step()
        {
            for (path, off) in ryucad::explode::offsets(&[step], 1.0) {
                let at = out.entry(path).or_insert(Offset::NONE);
                *at = Offset(off.0.then_after(&at.0));
            }
        }
        out
    }

    /// The lines from where each moved occurrence of `part` is to where
    /// it is drawn, from their middles (`middles`, by path).
    pub fn trails(&self, part: &str, middles: &BTreeMap<String, [f64; 3]>) -> Vec<Vec<[f64; 3]>> {
        let Some((e, f)) = self.current(part) else {
            return Vec::new();
        };
        middles
            .iter()
            .map(|(path, start)| ryucad::explode::trail(&e.resolved, path, *start, f))
            .filter(|t| t.len() > 1)
            .collect()
    }
}

impl Adding {
    /// The step as dragged so far: none until it moves.
    pub fn step(&self) -> Option<Step> {
        let k = self.axis?;
        if self.distance == 0.0 {
            return None;
        }
        let mut t = [0.0; 3];
        t[k] = self.distance;
        Some(Step {
            occurrences: self.instances.clone(),
            translate_m: t,
            rotate: None,
            centre_m: self.centre,
        })
    }

    /// The three arrows to drag along, `length` meters, from the middle
    /// of what moves.
    pub fn arrows(&self, length: f64) -> [Arrow; 3] {
        [0, 1, 2].map(|k| {
            let mut dir = [0.0; 3];
            dir[k] = 1.0;
            Arrow {
                from: self.centre,
                dir,
                length,
            }
        })
    }

    /// Takes hold of an arrow under the pointer at `at`.
    pub fn grab(&mut self, pr: &Projector, size: [f64; 2], at: [f64; 2], length: f64) -> bool {
        let Some(k) = (0..3).find(|k| self.arrows(length)[*k].near(pr, size, at)) else {
            return false;
        };
        self.axis = Some(k);
        self.distance = 0.0;
        self.from = at;
        true
    }

    /// Follows the pointer, now at `at`.
    pub fn drag(&mut self, pr: &Projector, size: [f64; 2], at: [f64; 2], length: f64) {
        if let Some(k) = self.axis {
            self.distance = self.arrows(length)[k].slide(pr, size, self.from, at);
        }
    }

    /// The step as a file writes it, its length in `unit` (`mm`, `in`, …)
    /// and `factor`, meters a unit.
    pub fn written(&self, unit: &str, factor: f64) -> Option<Value> {
        let k = self.axis?;
        if self.distance == 0.0 {
            return None;
        }
        // To a thousandth of the unit: a drag is no more exact than that.
        let d = (self.distance / factor * 1000.0).round() / 1000.0;
        let mut t = vec![json!(format!("0 {unit}")); 3];
        t[k] = json!(format!("{d} {unit}"));
        Some(json!({"instances": self.instances, "translate": t}))
    }
}

/// `explode.save` of `entry`'s steps (or none, for a new explode) with
/// `step` after them.
pub fn save_with(
    part: &str,
    id: &str,
    entry: Option<&Entry>,
    step: Value,
    revision: Option<String>,
) -> Request {
    let mut steps = entry
        .and_then(|e| e.steps.as_array().cloned())
        .unwrap_or_default();
    steps.push(step);
    Request::ExplodeSave {
        part: part.to_owned(),
        explode: id.to_owned(),
        steps: Value::Array(steps),
        expected_revision: revision,
    }
}

/// `explode.save` of `entry`'s steps less the `k`th (from 0).
pub fn save_without(part: &str, entry: &Entry, k: usize, revision: Option<String>) -> Request {
    let mut steps = entry.steps.as_array().cloned().unwrap_or_default();
    if k < steps.len() {
        steps.remove(k);
    }
    Request::ExplodeSave {
        part: part.to_owned(),
        explode: entry.id.clone(),
        steps: Value::Array(steps),
        expected_revision: revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed() -> Explodes {
        let mut x = Explodes::default();
        x.listed(
            "frame",
            &json!({"revisions": {"frame": "r1"}, "explodes": [
                {"part": "frame", "explode": "apart",
                 "steps": [{"instances": ["p1"], "translate": ["0 mm", "0 mm", "20 mm"]}],
                 "resolved": [{"occurrences": ["p1/a", "p1/b"], "translate_m": [0.0, 0.0, 0.02],
                               "centre_m": [0.0, 0.0, 0.0]}],
                 "warnings": [{"code": "explode_unresolved", "message": "step 1 names gone"}]},
                {"part": "frame", "explode": "broken", "error": {"code": "invalid_document",
                                                                 "message": "bad"}}
            ]}),
        );
        x
    }

    #[test]
    fn the_shown_explode_moves_occurrences_as_far_as_the_slider() {
        let mut x = listed();
        assert_eq!(x.revision.as_deref(), Some("r1"));
        assert_eq!(x.list[0].warnings, ["step 1 names gone"]);
        assert_eq!(x.list[1].error.as_deref(), Some("bad"));
        assert!(x.offsets("frame").is_empty(), "nothing shown");
        x.shown = Shown::from_json(&json!({"part": "frame", "explode": "apart", "fraction": 1.0}));
        let all = x.offsets("frame");
        assert_eq!(all["p1/a"].apply([0.0; 3]), [0.0, 0.0, 0.02]);
        x.sliding = Some(0.5);
        assert_eq!(x.offsets("frame")["p1/b"].apply([0.0; 3]), [0.0, 0.0, 0.01]);
        assert!(x.offsets("other").is_empty());
        let middles = BTreeMap::from([
            ("p1/a".to_owned(), [1.0, 0.0, 0.0]),
            ("base".to_owned(), [0.0; 3]),
        ]);
        assert_eq!(
            x.trails("frame", &middles),
            [vec![[1.0, 0.0, 0.0], [1.0, 0.0, 0.01]]]
        );
    }

    #[test]
    fn a_step_is_dragged_out_of_the_selection() {
        use ryucad::view::{Camera, Projection};
        let camera = Camera {
            eye: [0.0, -10.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Orthographic { height: 2.0 },
        };
        let pr = Projector::new(&camera, 1.0, [0.0; 3], 1.0);
        let size = [200.0, 200.0];
        let mut x = listed();
        let mut a = Adding {
            instances: vec!["base".into()],
            centre: [0.0; 3],
            axis: None,
            distance: 0.0,
            from: [0.0; 2],
        };
        assert_eq!(a.step(), None);
        // The z arrow runs up from the middle.
        assert!(a.grab(&pr, size, [100.0, 70.0], 0.5));
        assert_eq!(a.axis, Some(2));
        a.drag(&pr, size, [100.0, 40.0], 0.5);
        assert!((a.distance - 0.3).abs() < 1e-12);
        assert_eq!(
            a.written("mm", 0.001).unwrap(),
            json!({"instances": ["base"], "translate": ["0 mm", "0 mm", "300 mm"]})
        );
        x.adding = Some(a.clone());
        assert!((x.offsets("frame")["base"].apply([0.0; 3])[2] - 0.3).abs() < 1e-12);
        let req = save_with(
            "frame",
            "apart",
            Some(&x.list[0]),
            a.written("mm", 0.001).unwrap(),
            x.revision.clone(),
        );
        assert_eq!(req.params()["steps"].as_array().unwrap().len(), 2);
        assert_eq!(req.params()["expected_revision"], "r1");
        let req = save_without("frame", &x.list[0], 0, None);
        assert_eq!(req.params()["steps"], json!([]));
    }
}
