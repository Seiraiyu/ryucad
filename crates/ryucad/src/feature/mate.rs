//! Mates: two connectors of an assembly joined (ryucad 27.C).

use super::Given;
use super::{BodyUse, FeatureKind, Write};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::entity::EntityRef;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::part::part_kind;
use crate::units::Unit;
use serde_json::{Value, json};

pub(crate) const MATE: [&str; 8] = [
    "form", "a", "b", "a_offset", "b_offset", "angle", "distance", "limits",
];

/// A connector as a file holds it, from its text: an entity of an
/// occurrence of assembly `asm`, `<asm>/<path>/<solid>/<name>`, or an
/// occurrence's own frame, `<asm>/<path>`. `param` names the field for
/// errors.
pub(crate) fn connector_json(t: &str, asm: &Id, param: &str) -> Result<Value> {
    let r = EntityRef::parse(t)
        .map_err(|e| e.with("param", param))?
        .in_assembly();
    if r.part != *asm {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{t} is in {}, not assembly {asm}: a connector is on an occurrence of the assembly",
                r.part
            ),
        )
        .with("param", param));
    }
    if r.path.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{t} names no instance: give <assembly>/<instance>[/…]/<solid>/<name>"),
        )
        .with("param", param));
    }
    let at = if r.is_occurrence() {
        json!("origin")
    } else {
        json!({"feature": r.feature, "name": r.name})
    };
    Ok(json!({"instance": r.path, "at": at}))
}

/// A connector read as an occurrence whose path's last step is a datum of
/// the part the rest of it reaches (`<asm>/<instance>/…/<datum>`): a step
/// names an instance while the part before it is an assembly, and a
/// feature once it is a part. Then it is at that datum.
pub(crate) fn datum_at(project: &crate::doc::project::Project, asm: &Id, c: &mut Value) {
    let Some(path) = c["instance"].as_array().cloned() else {
        return;
    };
    let Some((last, steps)) = path.split_last() else {
        return;
    };
    if steps.is_empty() {
        return;
    }
    let mut part = asm.clone();
    for step in steps {
        let Some(step) = step.as_str() else { return };
        // A copy's seed follows it, an instance of the same assembly.
        if step.contains(':') {
            continue;
        }
        let Ok(id) = Id::parse(step, "instance") else {
            return;
        };
        let Some(Ok(Feature::Instance(i))) =
            project.parts.get(&part).and_then(|p| p.features.get(&id))
        else {
            return;
        };
        part = i.part.clone();
    }
    let Some(p) = project.parts.get(&part) else {
        return;
    };
    let datum = last
        .as_str()
        .and_then(|l| Id::parse(l, "feature").ok())
        .filter(|d| matches!(p.features.get(d), Some(Ok(Feature::Datum(_)))));
    if part_kind(p) == PartKind::Part
        && let Some(d) = datum
    {
        c["instance"] = json!(steps);
        c["at"] = json!({"feature": d});
    }
}

/// A mate from `current`'s fields (none for a new one) with what was
/// given laid over them: connectors as text, each keeping its offset
/// unless one is given (`{}` for none); `angle` or `distance` `""` lets it
/// go; a new form keeps only the driven values it takes.
pub(crate) fn mate_from(
    current: Option<&crate::doc::mate::Mate>,
    given: &Given,
    (project, part): (&crate::doc::project::Project, &Id),
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::mate::{Form, Mate};
    let f = &given.fields;
    let mut v = current.map_or_else(|| json!({}), Mate::fields);
    let o = v.as_object_mut().expect("an object");
    match (&f.form, current) {
        (Some(form), _) => {
            let Some(form) = Form::parse(form) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{form:?} isn't a mate: fastened, revolute, slider, cylindrical, planar or ball"),
                )
                .with("param", "form"));
            };
            o.insert("form".into(), json!(form.as_str()));
            if !form.takes_angle() {
                o.remove("angle");
            }
            if !form.takes_distance() {
                o.remove("distance");
            }
            if let Some(Value::Object(l)) = o.get_mut("limits") {
                if !form.takes_angle() {
                    l.remove("angle");
                }
                if !form.takes_distance_limit() {
                    l.remove("distance");
                }
                if l.is_empty() {
                    o.remove("limits");
                }
            }
        }
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a mate names its form: fastened, revolute, slider, cylindrical, planar or ball",
            )
            .with("param", "form"));
        }
        (None, Some(_)) => {}
    }
    for (side, text, offset) in [("a", &f.a, &f.a_offset), ("b", &f.b, &f.b_offset)] {
        if let Some(t) = text {
            let mut c = connector_json(t, part, side)?;
            datum_at(project, part, &mut c);
            if let Some(off) = o.get(side).and_then(|c| c.get("offset")) {
                c["offset"] = off.clone();
            }
            o.insert(side.into(), c);
        }
        if let Some(off) = offset {
            let Some(c) = o.get_mut(side) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{side}_offset moves connector {side}: give {side} too"),
                )
                .with("param", format!("{side}_offset")));
            };
            if *off == json!({}) {
                c.as_object_mut().expect("a connector").remove("offset");
            } else {
                c["offset"] = off.clone();
            }
        }
        if !o.contains_key(side) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a mate joins two connectors: give {side}"),
            )
            .with("param", side));
        }
    }
    for (k, x) in [("angle", &f.angle), ("distance", &f.distance)] {
        match x {
            Some(Value::String(t)) if t.is_empty() => {
                o.remove(k);
            }
            Some(x) => {
                o.insert(k.into(), x.clone());
            }
            None => {}
        }
    }
    if let Some(given) = &f.limits {
        let bad = |what: &str| {
            Error::new(Code::InvalidParam, format!("limits: {what}")).with("param", "limits")
        };
        let Some(given) = given.as_object() else {
            return Err(bad("an object of angle and distance ranges"));
        };
        let mut cur = match o.remove("limits") {
            Some(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        for (value, r) in given {
            if !matches!(value.as_str(), "angle" | "distance") {
                return Err(bad(&format!(
                    "{value:?} isn't a mate's value: angle or distance"
                )));
            }
            match r {
                Value::String(s) if s.is_empty() => {
                    cur.remove(value);
                }
                Value::Object(bounds) => {
                    let mut range = match cur.remove(value) {
                        Some(Value::Object(m)) => m,
                        _ => serde_json::Map::new(),
                    };
                    for (b, x) in bounds {
                        if !matches!(b.as_str(), "min" | "max") {
                            return Err(bad(&format!("a range has min and max, not {b:?}")));
                        }
                        if x.as_str() == Some("") {
                            range.remove(b);
                        } else {
                            range.insert(b.clone(), x.clone());
                        }
                    }
                    if !range.is_empty() {
                        cur.insert(value.clone(), Value::Object(range));
                    }
                }
                _ => return Err(bad("a range is {min, max}, or \"\" to clear it")),
            }
        }
        if !cur.is_empty() {
            o.insert("limits".into(), Value::Object(cur));
        }
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let m = Mate::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Mate(Box::new(m)))
}

/// Mates.
pub(crate) struct Mates;

impl FeatureKind for Mates {
    fn types(&self) -> &'static [&'static str] {
        &["mate"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &MATE
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::NONE
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        mate_from(
            current.and_then(Feature::mate),
            &cx.given,
            (cx.session.project(), cx.part),
            cx.unit,
            cx.env,
        )
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let m = f.mate().expect("a mate");
        let part = &part.id;
        let mut v = m.fields();
        for (side, c) in [("a", &m.a), ("b", &m.b)] {
            v[side]["text"] = json!(crate::part::connector_text(part, c));
        }
        if let Some(q) = &m.angle {
            v["angle"] = q.report();
        }
        if let Some(q) = &m.distance {
            v["distance"] = q.report();
        }
        v
    }

    /// Solved with the whole assembly, over what its instances place.
    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        (
            Err(super::built_elsewhere(
                cx.id,
                f,
                "is solved with its assembly",
            )),
            None,
        )
    }
}
