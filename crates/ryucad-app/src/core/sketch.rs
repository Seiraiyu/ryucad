//! A sketch as the window draws it, read from a `sketch.get` answer: its
//! curves as polylines and its points, each in its solve state, and its
//! dimensions' labels with their values as written.

use ryucad::sketch_view::{ConstraintState, EntityState};
use serde_json::Value;

/// A curve: a polyline in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub entity: String,
    pub construction: bool,
    pub state: EntityState,
    pub points: Vec<[f64; 3]>,
}

/// A point.
#[derive(Clone, Debug, PartialEq)]
pub struct Dot {
    pub entity: String,
    pub state: EntityState,
    pub at: [f64; 3],
}

/// A dimension's label, and its value as written, for editing.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub constraint: String,
    /// `length`, `radius` and so on.
    pub ty: String,
    pub state: ConstraintState,
    pub at: [f64; 3],
    pub text: String,
    pub value: String,
}

/// A sketch as drawn, for the part revision it was read at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    pub revision: String,
    pub curves: Vec<Curve>,
    pub dots: Vec<Dot>,
    pub labels: Vec<Label>,
}

fn point(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

fn constraint_state(s: &str) -> ConstraintState {
    match s {
        "redundant" => ConstraintState::Redundant,
        "conflicting" => ConstraintState::Conflicting,
        _ => ConstraintState::Ok,
    }
}

impl Drawing {
    /// The drawing in a `sketch.get` answer. What it can't read is left
    /// out.
    pub fn from_get(v: &Value) -> Drawing {
        let mut d = Drawing {
            revision: v["revision"].as_str().unwrap_or_default().to_owned(),
            ..Drawing::default()
        };
        for (id, e) in v["entities"].as_object().into_iter().flatten() {
            let state = e["state"]
                .as_str()
                .and_then(EntityState::parse)
                .unwrap_or(EntityState::Under);
            if let Some(at) = point(&e["at"]) {
                d.dots.push(Dot {
                    entity: id.clone(),
                    state,
                    at,
                });
            }
            if let Some(ps) = e["polyline"].as_array() {
                d.curves.push(Curve {
                    entity: id.clone(),
                    construction: e["construction"] == true,
                    state,
                    points: ps.iter().filter_map(point).collect(),
                });
            }
        }
        for (id, c) in v["constraints"].as_object().into_iter().flatten() {
            let Some(at) = point(&c["label"]["at"]) else {
                continue;
            };
            d.labels.push(Label {
                constraint: id.clone(),
                ty: c["type"].as_str().unwrap_or_default().to_owned(),
                state: constraint_state(c["state"].as_str().unwrap_or_default()),
                at,
                text: c["label"]["text"].as_str().unwrap_or_default().to_owned(),
                value: c["value"]["value"].as_str().unwrap_or_default().to_owned(),
            });
        }
        d
    }

    /// Every point it is drawn through: what frames it.
    pub fn points(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.curves
            .iter()
            .flat_map(|c| c.points.iter().copied())
            .chain(self.dots.iter().map(|d| d.at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_drawing_from_sketch_get() {
        let v = json!({
            "revision": "r1",
            "entities": {
                "l1": {"type": "line", "start": "p1", "end": "p2", "state": "well",
                       "polyline": [[0, 0, 0], [1, 0, 0]]},
                "l2": {"type": "line", "construction": true, "start": "p1", "end": "p2",
                       "state": "over", "polyline": [[0, 0, 0], [1, 0, 0]]},
                "p1": {"type": "point", "m": [0, 0], "at": [0, 0, 0], "state": "under"},
                "p2": {"type": "point", "m": [1, 0], "at": [1, 0, 0], "state": "failing"}
            },
            "constraints": {
                "c1": {"type": "horizontal", "a": "l1", "state": "ok"},
                "d1": {"type": "length", "a": "l1", "state": "redundant",
                       "value": {"value": "2 * w", "m": 1.0, "expr": true},
                       "label": {"at": [0.5, 0.1, 0], "text": "2 * w = 1000 mm"}}
            }
        });
        let d = Drawing::from_get(&v);
        assert_eq!(d.revision, "r1");
        assert_eq!(d.curves.len(), 2);
        assert_eq!(d.curves[0].state, EntityState::Well);
        assert!(d.curves[1].construction && !d.curves[0].construction);
        assert_eq!(d.curves[0].points, [[0.0; 3], [1.0, 0.0, 0.0]]);
        assert_eq!(d.dots[1].state, EntityState::Failing);
        assert_eq!(
            d.labels,
            [Label {
                constraint: "d1".into(),
                ty: "length".into(),
                state: ConstraintState::Redundant,
                at: [0.5, 0.1, 0.0],
                text: "2 * w = 1000 mm".into(),
                value: "2 * w".into(),
            }]
        );
        assert_eq!(d.points().count(), 6);
    }
}
