//! Fillets and chamfers (ryucad 27.C).

use super::Given;
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::reference::Reference;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::units::{Quantity, Unit};
use serde_json::{Value, json};

/// A fillet or chamfer as output reports it: its file's fields, every
/// size as written and in meters (an angle in radians), every edge and
/// face as `<part>/<feature>/<name>`.
pub(crate) fn blend_report(part: &Id, f: &Feature) -> Value {
    use crate::doc::blend::ChamferForm;
    let mut v = match f {
        Feature::Fillet(x) => x.fields(),
        Feature::Chamfer(x) => x.fields(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    let sizes: Vec<Vec<(&str, &Quantity)>> = match f {
        Feature::Fillet(x) => x
            .groups
            .iter()
            .map(|g| vec![("radius", &g.radius)])
            .collect(),
        Feature::Chamfer(x) => x
            .groups
            .iter()
            .map(|g| match &g.form {
                ChamferForm::Equal { d } => vec![("d", d)],
                ChamferForm::Distances { d1, d2, .. } => vec![("d1", d1), ("d2", d2)],
                ChamferForm::DistanceAngle { d, angle, .. } => vec![("d", d), ("angle", angle)],
            })
            .collect(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    for (i, group) in sizes.into_iter().enumerate() {
        for (k, q) in group {
            v["groups"][i][k] = q.report();
        }
    }
    for (role, r) in f.references() {
        let at = format!("/{}", role.replace('.', "/"));
        *v.pointer_mut(&at).expect("its own reference") = json!(r.text(part));
    }
    v
}

pub(crate) const FILLET: [&str; 4] = ["groups", "propagate", "radius", "edges"];

pub(crate) const CHAMFER: [&str; 9] = [
    "groups",
    "propagate",
    "form",
    "d",
    "d1",
    "d2",
    "angle",
    "face",
    "edges",
];

/// A fillet or chamfer (`ty`) from `current`'s fields (none for a new
/// one) with what was given laid over them: its `groups` whole, or its
/// one group's fields (`radius`, or `form`, `d`, `d1`, `d2`, `angle` and
/// `face`; and `edges`), and `propagate`. Edges and faces are given as
/// text and kept as a file holds them; a new form keeps only what it
/// takes.
pub(crate) fn blend_from(
    ty: &str,
    current: Option<&Feature>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::blend::{Chamfer, Fillet};
    let f = &given.fields;
    let reference = |v: &Value, at: &str| -> Result<Value> {
        match v {
            Value::String(t) => Ok(Reference::parse(t, part)
                .map_err(|e| e.with("pointer", at))?
                .to_json()),
            v => Ok(v.clone()),
        }
    };
    let mut v = match current {
        Some(Feature::Fillet(x)) => x.fields(),
        Some(Feature::Chamfer(x)) => x.fields(),
        _ if ty == "chamfer" => json!({"groups": [{"form": "equal"}]}),
        _ => json!({"groups": [{}]}),
    };
    let sizes = [
        ("radius", &f.radius),
        ("d", &f.d),
        ("d1", &f.d1),
        ("d2", &f.d2),
        ("angle", &f.angle),
    ];
    let one = sizes.iter().any(|(_, x)| x.is_some())
        || f.edges.is_some()
        || f.form.is_some()
        || f.face.is_some();
    if let Some(groups) = &f.groups {
        if one {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s groups, or its one group's fields, not both"),
            ));
        }
        let mut groups = groups.clone();
        if let Value::Array(gs) = &mut groups {
            for (i, g) in gs.iter_mut().enumerate() {
                if let Some(Value::Array(es)) = g.get_mut("edges") {
                    for (j, e) in es.iter_mut().enumerate() {
                        *e = reference(e, &format!("/groups/{i}/edges/{j}"))?;
                    }
                }
                if let Some(face) = g.get_mut("face") {
                    *face = reference(face, &format!("/groups/{i}/face"))?;
                }
            }
        }
        v["groups"] = groups;
    } else if one {
        let gs = v["groups"].as_array_mut().expect("groups");
        if gs.len() != 1 {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "this {ty} has {} groups: give them all, as groups",
                    gs.len()
                ),
            )
            .with("param", "groups"));
        }
        let g = gs[0].as_object_mut().expect("a group");
        if let Some(form) = &f.form {
            let takes: &[&str] = match form.as_str() {
                "equal" => &["d", "edges"],
                "distances" => &["d1", "d2", "face", "edges"],
                "distance_angle" => &["d", "angle", "face", "edges"],
                _ => &["edges"],
            };
            g.retain(|k, _| takes.contains(&k.as_str()));
            g.insert("form".into(), json!(form));
        }
        for (k, x) in sizes {
            if let Some(x) = x {
                g.insert(k.into(), x.clone());
            }
        }
        if let Some(face) = &f.face {
            g.insert("face".into(), reference(&json!(face), "/groups/0/face")?);
        }
        if let Some(edges) = &f.edges {
            let edges: Vec<Value> = edges
                .iter()
                .enumerate()
                .map(|(j, e)| reference(&json!(e), &format!("/groups/0/edges/{j}")))
                .collect::<Result<_>>()?;
            g.insert("edges".into(), json!(edges));
        }
    }
    match f.propagate {
        Some(false) => v["propagate"] = json!(false),
        Some(true) => {
            v.as_object_mut().expect("an object").remove("propagate");
        }
        None => {}
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let out = match ty {
        "fillet" => Feature::Fillet(Fillet::from_fields(&mut o, unit, env)?),
        _ => Feature::Chamfer(Chamfer::from_fields(&mut o, unit, env)?),
    };
    o.finish()?;
    Ok(out)
}
