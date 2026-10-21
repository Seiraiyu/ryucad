//! Editing a feature's parameters and placement: the property panel's form.
//! Values stay as written (quantities with their units). A save is an
//! ordinary `feature.edit` with the revision the form was read at, so a
//! human's stale edit fails with `conflict` like an agent's; the form then
//! reloads and lists what changed instead of overwriting it.

use ryucad::error::Error;
use ryucad::proto::{PlacementArg, Request};
use serde_json::{Map, Value, json};

/// One editable value: its name, as read, and as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub read: String,
    pub text: String,
}

impl Field {
    pub fn new(name: &str, read: String) -> Field {
        Field {
            name: name.to_owned(),
            text: read.clone(),
            read,
        }
    }

    pub fn dirty(&self) -> bool {
        self.text.trim() != self.read
    }
}

/// A value someone else changed while the form was being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub name: String,
    /// What it is now.
    pub theirs: String,
    /// What this window tried to save.
    pub yours: String,
}

/// The form for one feature.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub part: String,
    pub feature: String,
    pub ty: String,
    /// The part revision the values were read at.
    pub revision: String,
    pub params: Vec<Field>,
    /// The origin's three quantities, then the z and x axes' six numbers.
    pub origin: [Field; 3],
    pub z: [Field; 3],
    pub x: [Field; 3],
    /// What changed under a save that conflicted.
    pub changed: Vec<Changed>,
    /// Why the last save failed, other than a conflict.
    pub error: Option<Error>,
    /// A save is on its way.
    pub saving: bool,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        // A quantity as reported: `{"value", "m"}`.
        Value::Object(o) => o.get("value").map_or_else(String::new, text),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn triple(names: [&str; 3], v: &Value) -> [Field; 3] {
    [0, 1, 2].map(|i| Field::new(names[i], text(&v[i])))
}

impl Form {
    /// The form for a `feature.show` answer.
    pub fn from_show(part: &str, v: &Value) -> Form {
        let params = v["params"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, q)| Field::new(k, text(q)))
            .collect();
        let p = &v["placement"];
        Form {
            part: part.to_owned(),
            feature: text(&v["id"]),
            ty: text(&v["type"]),
            revision: text(&v["revision"]),
            params,
            origin: triple(["origin x", "origin y", "origin z"], &p["origin"]),
            z: triple(["z x", "z y", "z z"], &p["z"]),
            x: triple(["x x", "x y", "x z"], &p["x"]),
            changed: Vec::new(),
            error: None,
            saving: false,
        }
    }

    fn fields(&self) -> impl Iterator<Item = &Field> {
        self.params
            .iter()
            .chain(&self.origin)
            .chain(&self.z)
            .chain(&self.x)
    }

    /// Whether anything was typed.
    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty)
    }

    /// The `feature.edit` that saves what was typed, guarded by the
    /// revision the form was read at.
    pub fn request(&self) -> Result<Request, Error> {
        let params: Map<String, Value> = self
            .params
            .iter()
            .filter(|f| f.dirty())
            .map(|f| (f.name.clone(), json!(f.text.trim())))
            .collect();
        let quantities = |fs: &[Field; 3]| fs.clone().map(|f| json!(f.text.trim()));
        let numbers = |fs: &[Field; 3]| -> Result<[f64; 3], Error> {
            let mut out = [0.0; 3];
            for (o, f) in out.iter_mut().zip(fs) {
                *o = f.text.trim().parse().map_err(|_| {
                    Error::new(
                        ryucad::error::Code::InvalidPlacement,
                        format!("{} must be a number, not {:?}", f.name, f.text),
                    )
                })?;
            }
            Ok(out)
        };
        let any = |fs: &[Field; 3]| fs.iter().any(Field::dirty);
        let placement = PlacementArg {
            origin: any(&self.origin).then(|| quantities(&self.origin)),
            z: if any(&self.z) {
                Some(numbers(&self.z)?)
            } else {
                None
            },
            x: if any(&self.x) {
                Some(numbers(&self.x)?)
            } else {
                None
            },
        };
        let moved = placement != PlacementArg::default();
        Ok(Request::FeatureEdit {
            part: self.part.clone(),
            id: self.feature.clone(),
            params,
            placement: moved.then_some(placement),
            fields: Default::default(),
            expected_revision: Some(self.revision.clone()),
        })
    }

    /// The form read again after a conflict: the new values, with what
    /// changed under what was typed listed.
    pub fn reloaded(&self, v: &Value) -> Form {
        let mut fresh = Form::from_show(&self.part, v);
        fresh.changed = self
            .fields()
            .filter(|f| f.dirty())
            .filter_map(|mine| {
                let now = fresh.fields().find(|f| f.name == mine.name)?;
                (now.read != mine.read).then(|| Changed {
                    name: mine.name.clone(),
                    theirs: now.read.clone(),
                    yours: mine.text.trim().to_owned(),
                })
            })
            .collect();
        fresh
    }
}

/// The form for one of a sketch's dimensions: its value as written,
/// saved with `sketch.edit` against the revision it was read at.
#[derive(Clone, Debug, PartialEq)]
pub struct DimForm {
    pub part: String,
    pub sketch: String,
    pub constraint: String,
    /// `length`, `radius` and so on.
    pub ty: String,
    /// The part revision the value was read at.
    pub revision: String,
    pub value: Field,
    /// What changed under a save that conflicted.
    pub changed: Option<Changed>,
    /// Why the last save failed, other than a conflict.
    pub error: Option<Error>,
    /// A save is on its way.
    pub saving: bool,
}

impl DimForm {
    /// The form for `constraint` in a `sketch.get` answer, if it is there
    /// and has a value.
    pub fn from_get(part: &str, sketch: &str, constraint: &str, v: &Value) -> Option<DimForm> {
        let c = v["constraints"].get(constraint)?;
        let value = c["value"]["value"].as_str()?;
        Some(DimForm {
            part: part.to_owned(),
            sketch: sketch.to_owned(),
            constraint: constraint.to_owned(),
            ty: text(&c["type"]),
            revision: text(&v["revision"]),
            value: Field::new(constraint, value.to_owned()),
            changed: None,
            error: None,
            saving: false,
        })
    }

    pub fn dirty(&self) -> bool {
        self.value.dirty()
    }

    /// The `sketch.edit` that sets what was typed, guarded by the revision
    /// the form was read at.
    pub fn request(&self) -> Request {
        Request::SketchEdit {
            part: self.part.clone(),
            sketch: self.sketch.clone(),
            ops: vec![json!({"op": "set", "id": self.constraint,
                             "value": self.value.text.trim()})],
            expected_revision: Some(self.revision.clone()),
        }
    }

    /// The form read again after a conflict, saying what changed under
    /// what was typed; `None` if the dimension is gone.
    pub fn reloaded(&self, v: &Value) -> Option<DimForm> {
        let mut fresh = DimForm::from_get(&self.part, &self.sketch, &self.constraint, v)?;
        if self.dirty() && fresh.value.read != self.value.read {
            fresh.changed = Some(Changed {
                name: self.constraint.clone(),
                theirs: fresh.value.read.clone(),
                yours: self.value.text.trim().to_owned(),
            });
        }
        Some(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn got(width: &str, revision: &str) -> Value {
        json!({"revision": revision, "constraints": {
            "d1": {"type": "length", "a": "l1", "value": {"value": width, "m": 0.02}},
            "c1": {"type": "horizontal", "a": "l1"}}})
    }

    #[test]
    fn a_dimension_saves_through_sketch_edit() {
        let mut f = DimForm::from_get("p", "s", "d1", &got("width", "r1")).unwrap();
        assert_eq!((f.ty.as_str(), f.value.read.as_str()), ("length", "width"));
        assert!(DimForm::from_get("p", "s", "c1", &got("width", "r1")).is_none());
        assert!(DimForm::from_get("p", "s", "d9", &got("width", "r1")).is_none());
        f.value.text = " 2 * width ".into();
        assert!(f.dirty());
        let Request::SketchEdit {
            ops,
            expected_revision,
            ..
        } = f.request()
        else {
            panic!()
        };
        assert_eq!(
            ops,
            [json!({"op": "set", "id": "d1", "value": "2 * width"})]
        );
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        // Changed underneath: what it is now, and what was typed.
        let again = f.reloaded(&got("25 mm", "r2")).unwrap();
        assert_eq!(again.value.text, "25 mm");
        assert_eq!(
            again.changed,
            Some(Changed {
                name: "d1".into(),
                theirs: "25 mm".into(),
                yours: "2 * width".into()
            })
        );
    }

    fn shown(radius: &str, revision: &str) -> Value {
        json!({
            "id": "boss", "type": "cylinder", "revision": revision,
            "params": {"radius": {"value": radius, "m": 0.003}, "height": {"value": "8 mm", "m": 0.008}},
            "placement": {"origin": [{"value": "0 mm", "m": 0.0}, {"value": "0 mm", "m": 0.0}, {"value": "5 mm", "m": 0.005}],
                          "z": [0, 0, 1], "x": [1, 0, 0]},
        })
    }

    #[test]
    fn a_save_sends_only_what_was_typed() {
        let mut f = Form::from_show("p", &shown("3 mm", "r1"));
        assert_eq!(f.params[1].read, "3 mm");
        assert_eq!(f.origin[2].read, "5 mm");
        assert_eq!(f.z[2].read, "1");
        assert!(!f.dirty());
        f.params[1].text = " 4 mm ".into();
        f.origin[0].text = "1 in".into();
        assert!(f.dirty());
        let Request::FeatureEdit {
            params,
            placement,
            expected_revision,
            ..
        } = f.request().unwrap()
        else {
            panic!()
        };
        assert_eq!(
            params,
            json!({"radius": "4 mm"}).as_object().unwrap().clone()
        );
        let p = placement.unwrap();
        assert_eq!(
            p.origin,
            Some([json!("1 in"), json!("0 mm"), json!("5 mm")])
        );
        assert_eq!((p.z, p.x), (None, None));
        assert_eq!(expected_revision.as_deref(), Some("r1"));
        f.z[0].text = "up".into();
        assert!(f.request().is_err());
    }

    #[test]
    fn a_conflict_lists_what_changed_underneath() {
        let mut f = Form::from_show("p", &shown("3 mm", "r1"));
        f.params[1].text = "4 mm".into();
        f.params[0].text = "9 mm".into();
        // Meanwhile an agent set the radius to 5 mm; the height is as it was.
        let again = f.reloaded(&shown("5 mm", "r2"));
        assert_eq!(again.revision, "r2");
        assert_eq!(again.params[1].text, "5 mm");
        assert_eq!(
            again.changed,
            [Changed {
                name: "radius".into(),
                theirs: "5 mm".into(),
                yours: "4 mm".into()
            }]
        );
    }
}
