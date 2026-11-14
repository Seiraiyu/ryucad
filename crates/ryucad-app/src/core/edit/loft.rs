//! A loft's closing and tangents in the property panel (ryucad's `loft` kind).

use super::sweep::section_text;
use super::*;

/// A loft end's tangent: its direction shown, its weight edited.
#[derive(Clone, Debug, PartialEq)]
pub struct TangentForm {
    /// `normal`, or a reference as text.
    pub direction: String,
    pub weight: Field,
}

/// A loft's form (design 323): its sections, in order, and its tangents'
/// directions shown, never changed here; whether it closes and its
/// tangents' weights edited. Its operation and targets are the form's
/// own, as an extrude's.
#[derive(Clone, Debug, PartialEq)]
pub struct LoftForm {
    /// Each section as one line, in order.
    pub sections: Vec<String>,
    pub closed: Flag,
    pub start: Option<TangentForm>,
    pub end: Option<TangentForm>,
}

impl LoftForm {
    /// From a `feature.show` of a loft. A tangent written without a
    /// weight weighs 1.
    pub(super) fn from_show(v: &Value) -> LoftForm {
        let tangent = |k: &str, name: &str| {
            let t = &v[k];
            t.is_object().then(|| {
                let w = match &t["weight"] {
                    Value::Null => "1".to_owned(),
                    w => text(w),
                };
                TangentForm {
                    direction: text(&t["direction"]),
                    weight: Field::new(name, w),
                }
            })
        };
        LoftForm {
            sections: v["sections"]
                .as_array()
                .into_iter()
                .flatten()
                .map(section_text)
                .collect(),
            closed: Flag {
                name: "closed".to_owned(),
                read: v["closed"] == true,
                chosen: v["closed"] == true,
            },
            start: tangent("start_tangent", "start weight"),
            end: tangent("end_tangent", "end weight"),
        }
    }

    pub(super) fn fields(&self) -> impl Iterator<Item = &Field> {
        self.start.iter().chain(&self.end).map(|t| &t.weight)
    }

    pub fn dirty(&self) -> bool {
        self.fields().any(Field::dirty) || self.closed.dirty()
    }

    pub(super) fn revert(&mut self) {
        for t in self.start.iter_mut().chain(&mut self.end) {
            t.weight.text.clone_from(&t.weight.read);
        }
        self.closed.chosen = self.closed.read;
    }

    /// What changed, as `feature.edit` takes it: whether it closes, and
    /// each weight typed.
    pub(super) fn given(&self, out: &mut FeatureFields) {
        if self.closed.dirty() {
            out.closed = Some(self.closed.chosen);
        }
        if let Some(t) = self.start.as_ref().filter(|t| t.weight.dirty()) {
            out.start_weight = Some(json!(t.weight.text.trim()));
        }
        if let Some(t) = self.end.as_ref().filter(|t| t.weight.dirty()) {
            out.end_weight = Some(json!(t.weight.text.trim()));
        }
    }
}
