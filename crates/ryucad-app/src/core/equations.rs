//! The Equations tile's table: every parameter of the project and of
//! each part, and every expression of each part's features (sketch
//! dimensions among them), from `equations.list`. Each row edits in
//! place and is written where it lives, one request per row, against the
//! revision the table was read at: a parameter through `param.set`, a
//! feature's field through `feature.edit` by its pointer, a dimension
//! through `sketch.edit`. A write that conflicts reads the table again
//! and says what changed under what was typed, as the forms do.

use crate::core::edit::{Changed, Field};
use ryucad::error::Error;
use ryucad::proto::Request;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Where a row lives, and so how it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// A parameter: the project's (`part` none) or a part's.
    Parameter { part: Option<String>, name: String },
    /// A feature's expression field, by JSON pointer.
    Field {
        part: String,
        feature: String,
        pointer: String,
    },
    /// A sketch's dimension: its constraint.
    Dimension {
        part: String,
        sketch: String,
        constraint: String,
    },
}

impl Place {
    /// The part it belongs to, none for a project parameter.
    pub fn part(&self) -> Option<&str> {
        match self {
            Place::Parameter { part, .. } => part.as_deref(),
            Place::Field { part, .. } | Place::Dimension { part, .. } => Some(part),
        }
    }

    /// The feature it belongs to, if any.
    pub fn feature(&self) -> Option<&str> {
        match self {
            Place::Parameter { .. } => None,
            Place::Field { feature, .. } => Some(feature),
            Place::Dimension { sketch, .. } => Some(sketch),
        }
    }

    /// Where it is, for people: `project`, `p`, `p/b /params/x`,
    /// `p/s d1`.
    pub fn label(&self) -> String {
        match self {
            Place::Parameter { part: None, .. } => "project".into(),
            Place::Parameter { part: Some(p), .. } => p.clone(),
            Place::Field {
                part,
                feature,
                pointer,
            } => format!("{part}/{feature} {pointer}"),
            Place::Dimension {
                part,
                sketch,
                constraint,
            } => format!("{part}/{sketch} {constraint}"),
        }
    }

    /// Its name: a parameter's, else where it is in its feature.
    pub fn name(&self) -> &str {
        match self {
            Place::Parameter { name, .. } => name,
            Place::Field { pointer, .. } => pointer,
            Place::Dimension { constraint, .. } => constraint,
        }
    }
}

/// One row: where it lives, its expression as written (editable), its
/// value for people, what it reads, and why it has no value.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub place: Place,
    pub field: Field,
    pub value: String,
    pub reads: Vec<String>,
    pub error: Option<String>,
}

/// Which rows the table shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    /// The project's parameters.
    Project,
    /// A part's rows.
    Part(String),
    /// A feature's rows.
    Feature(String, String),
}

impl Filter {
    pub fn keeps(&self, place: &Place) -> bool {
        match self {
            Filter::All => true,
            Filter::Project => place.part().is_none(),
            Filter::Part(p) => place.part() == Some(p),
            Filter::Feature(p, f) => place.part() == Some(p) && place.feature() == Some(f),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Filter::All => "Everything".into(),
            Filter::Project => "Project parameters".into(),
            Filter::Part(p) => format!("Part {p}"),
            Filter::Feature(p, f) => format!("Feature {p}/{f}"),
        }
    }
}

/// The table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Equations {
    pub rows: Vec<Row>,
    /// The project's revision and each part's, as read.
    pub project_revision: String,
    pub revisions: BTreeMap<String, String>,
    pub filter: Filter,
    /// A parameter being added: in the project (`None`) or a part, its
    /// name and expression.
    pub new_part: Option<String>,
    pub new_name: String,
    pub new_value: String,
    /// What changed under a write that conflicted.
    pub changed: Vec<Changed>,
    /// Why the last write failed, other than a conflict.
    pub error: Option<Error>,
    /// The row being written (an index), or the new parameter (`None`
    /// inside `Some`).
    pub saving: Option<Option<usize>>,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

impl Equations {
    /// The table for an `equations.list` answer, values shown in `unit`.
    pub fn from_list(v: &Value, unit: &str) -> Equations {
        let rows = v["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let part = r["part"].as_str().map(str::to_owned);
                let place = match r["what"].as_str()? {
                    "parameter" => Place::Parameter {
                        part,
                        name: text(&r["name"]),
                    },
                    "field" => Place::Field {
                        part: part?,
                        feature: text(&r["feature"]),
                        pointer: text(&r["pointer"]),
                    },
                    "dimension" => Place::Dimension {
                        part: part?,
                        sketch: text(&r["feature"]),
                        constraint: text(&r["constraint"]),
                    },
                    _ => return None,
                };
                Some(Row {
                    field: Field::new(place.name(), text(&r["text"])),
                    place,
                    value: crate::core::params::value(&r["value"], unit),
                    reads: r["reads"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(text)
                        .collect(),
                    error: r.get("error").map(|e| text(&e["message"])),
                })
            })
            .collect();
        Equations {
            rows,
            project_revision: text(&v["revisions"]["project"]),
            revisions: v["revisions"]["parts"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, r)| (k.clone(), text(r)))
                .collect(),
            ..Equations::default()
        }
    }

    /// The rows the filter keeps, by index.
    pub fn shown(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|i| self.filter.keeps(&self.rows[*i].place))
            .collect()
    }

    /// The filters to choose from: everything, the project's parameters,
    /// each part, each feature, in the table's order.
    pub fn filters(&self) -> Vec<Filter> {
        let mut out = vec![Filter::All, Filter::Project];
        for r in &self.rows {
            let more = [
                r.place.part().map(|p| Filter::Part(p.to_owned())),
                r.place
                    .part()
                    .zip(r.place.feature())
                    .map(|(p, f)| Filter::Feature(p.to_owned(), f.to_owned())),
            ];
            for f in more.into_iter().flatten() {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        out
    }

    /// Whether anything was typed and not written.
    pub fn dirty(&self) -> bool {
        self.rows.iter().any(|r| r.field.dirty())
            || !self.new_name.trim().is_empty()
            || !self.new_value.trim().is_empty()
    }

    fn revision(&self, part: Option<&str>) -> Option<String> {
        Some(match part {
            None => self.project_revision.clone(),
            Some(p) => self.revisions.get(p)?.clone(),
        })
    }

    /// The request that writes row `i` as typed, guarded by the revision
    /// it was read at; `None` when it isn't changed.
    pub fn request(&self, i: usize) -> Option<Request> {
        let row = self.rows.get(i)?;
        if !row.field.dirty() {
            return None;
        }
        let typed = row.field.text.trim().to_owned();
        let expected_revision = self.revision(row.place.part());
        Some(match &row.place {
            Place::Parameter { part, name } => Request::ParamSet {
                part: part.clone(),
                scope: part.is_none().then_some(ryucad::param::Scope::Project),
                parameters: Map::from_iter([(name.clone(), json!(typed))]),
                expected_revision,
            },
            Place::Field {
                part,
                feature,
                pointer,
            } => Request::FeatureEdit {
                part: part.clone(),
                id: feature.clone(),
                params: Map::new(),
                placement: None,
                fields: Default::default(),
                expressions: Map::from_iter([(pointer.clone(), json!(typed))]),
                expected_revision,
            },
            Place::Dimension {
                part,
                sketch,
                constraint,
            } => Request::SketchEdit {
                part: part.clone(),
                sketch: sketch.clone(),
                ops: vec![json!({"op": "set", "id": constraint, "value": typed})],
                expected_revision,
            },
        })
    }

    /// The `param.set` that adds the new parameter, in the project or
    /// its part; `None` until it has a name and a value.
    pub fn add_request(&self) -> Option<Request> {
        let (name, value) = (self.new_name.trim(), self.new_value.trim());
        if name.is_empty() || value.is_empty() {
            return None;
        }
        Some(Request::ParamSet {
            part: self.new_part.clone(),
            scope: self
                .new_part
                .is_none()
                .then_some(ryucad::param::Scope::Project),
            parameters: Map::from_iter([(name.to_owned(), json!(value))]),
            expected_revision: self.revision(self.new_part.as_deref()),
        })
    }

    /// The table read again (`v`), keeping the filter, what is being
    /// added and what was typed into rows still there; with `conflicted`,
    /// listing each typed row whose expression changed underneath.
    pub fn reloaded(&self, v: &Value, unit: &str, conflicted: bool) -> Equations {
        let mut fresh = Equations::from_list(v, unit);
        fresh.filter = self.filter.clone();
        fresh.new_part.clone_from(&self.new_part);
        fresh.new_name.clone_from(&self.new_name);
        fresh.new_value.clone_from(&self.new_value);
        for mine in self.rows.iter().filter(|r| r.field.dirty()) {
            let now = fresh.rows.iter_mut().find(|r| r.place == mine.place);
            let theirs = now
                .as_ref()
                .map_or("(deleted)".to_owned(), |r| r.field.read.clone());
            if conflicted && theirs != mine.field.read {
                fresh.changed.push(Changed {
                    name: mine.place.label(),
                    theirs,
                    yours: mine.field.text.trim().to_owned(),
                });
            } else if let Some(r) = now {
                // Not written yet: what was typed stays.
                r.field.text.clone_from(&mine.field.text);
            }
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(wall: &str, rev: &str) -> Value {
        json!({
            "rows": [
                {"scope": "project", "what": "parameter", "name": "wall", "text": wall,
                 "value": {"kind": "length", "m": 0.004}, "reads": []},
                {"scope": "part", "part": "p", "what": "parameter", "name": "r",
                 "text": "wall / 2", "value": {"kind": "length", "m": 0.002},
                 "reads": ["wall"]},
                {"scope": "part", "part": "p", "what": "field", "feature": "b",
                 "type": "box", "pointer": "/params/x", "text": "wall * 3",
                 "value": {"kind": "length", "m": 0.012}, "reads": ["wall"]},
                {"scope": "part", "part": "p", "what": "dimension", "feature": "s",
                 "type": "sketch", "constraint": "d1", "pointer": "/constraints/d1/value",
                 "text": "gap", "reads": ["gap"],
                 "error": {"code": "unknown_parameter", "message": "no parameter gap"}},
            ],
            "revisions": {"project": rev, "parts": {"p": "rp"}},
        })
    }

    #[test]
    fn rows_read_and_filter() {
        let e = Equations::from_list(&listed("4 mm", "r1"), "mm");
        let rows: Vec<(String, &str, &str)> = e
            .rows
            .iter()
            .map(|r| (r.place.label(), r.field.read.as_str(), r.value.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("project".to_owned(), "4 mm", "4 mm"),
                ("p".to_owned(), "wall / 2", "2 mm"),
                ("p/b /params/x".to_owned(), "wall * 3", "12 mm"),
                ("p/s d1".to_owned(), "gap", ""),
            ]
        );
        assert_eq!(e.rows[3].error.as_deref(), Some("no parameter gap"));
        assert_eq!(
            e.filters(),
            [
                Filter::All,
                Filter::Project,
                Filter::Part("p".into()),
                Filter::Feature("p".into(), "b".into()),
                Filter::Feature("p".into(), "s".into()),
            ]
        );
        let mut f = e.clone();
        f.filter = Filter::Project;
        assert_eq!(f.shown(), [0]);
        f.filter = Filter::Part("p".into());
        assert_eq!(f.shown(), [1, 2, 3]);
        f.filter = Filter::Feature("p".into(), "s".into());
        assert_eq!(f.shown(), [3]);
    }

    #[test]
    fn each_row_is_written_where_it_lives() {
        let mut e = Equations::from_list(&listed("4 mm", "r1"), "mm");
        assert!(e.request(0).is_none());
        for (i, typed) in [(0, "5 mm"), (1, "wall"), (2, "wall * 2"), (3, "3 mm")] {
            e.rows[i].field.text = format!(" {typed} ");
        }
        assert!(e.dirty());
        let Some(Request::ParamSet {
            part: None,
            scope: Some(ryucad::param::Scope::Project),
            parameters,
            expected_revision,
        }) = e.request(0)
        else {
            panic!("{:?}", e.request(0))
        };
        assert_eq!(Value::Object(parameters), json!({"wall": "5 mm"}));
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        let Some(Request::ParamSet {
            part: Some(p),
            expected_revision,
            ..
        }) = e.request(1)
        else {
            panic!()
        };
        assert_eq!(
            (p.as_str(), expected_revision.as_deref()),
            ("p", Some("rp"))
        );
        let Some(Request::FeatureEdit {
            id, expressions, ..
        }) = e.request(2)
        else {
            panic!()
        };
        assert_eq!(id, "b");
        assert_eq!(Value::Object(expressions), json!({"/params/x": "wall * 2"}));
        let Some(Request::SketchEdit { sketch, ops, .. }) = e.request(3) else {
            panic!()
        };
        assert_eq!(sketch, "s");
        assert_eq!(ops, [json!({"op": "set", "id": "d1", "value": "3 mm"})]);
        // A new parameter, in the project or a part.
        assert!(e.add_request().is_none());
        e.new_name = "gap".into();
        e.new_value = "1 mm".into();
        let Some(Request::ParamSet { scope, .. }) = e.add_request() else {
            panic!()
        };
        assert_eq!(scope, Some(ryucad::param::Scope::Project));
        e.new_part = Some("p".into());
        let Some(Request::ParamSet { part, scope, .. }) = e.add_request() else {
            panic!()
        };
        assert_eq!((part.as_deref(), scope), (Some("p"), None));
    }

    #[test]
    fn a_conflict_says_what_changed_under_what_was_typed() {
        let mut e = Equations::from_list(&listed("4 mm", "r1"), "mm");
        e.filter = Filter::Project;
        e.rows[0].field.text = "5 mm".into();
        e.rows[2].field.text = "wall * 2".into();
        let again = e.reloaded(&listed("6 mm", "r2"), "mm", true);
        assert_eq!(
            again.changed,
            [Changed {
                name: "project".into(),
                theirs: "6 mm".into(),
                yours: "5 mm".into()
            }]
        );
        assert_eq!(again.rows[0].field.text, "6 mm");
        // What didn't change underneath keeps what was typed.
        assert_eq!(again.rows[2].field.text, "wall * 2");
        assert_eq!(again.filter, Filter::Project);
        assert_eq!(again.project_revision, "r2");
    }
}
