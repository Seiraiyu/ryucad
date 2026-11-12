//! The five primitives: box, cylinder, cone, sphere and torus (ryucad 27.C).

use super::{Given, placement_report};
use crate::doc::files::{Primitive, PrimitiveType};
use crate::doc::solid::Combine;
use crate::error::{Code, Error, Result};
use serde_json::{Map, Value, json};

pub(crate) fn primitive_report(f: &Primitive) -> Value {
    let params: Map<String, Value> = f
        .params
        .iter()
        .map(|(k, q)| (k.clone(), q.report()))
        .collect();
    let mut v = json!({"params": params, "placement": placement_report(&f.placement)});
    f.combine.write(&mut v);
    v
}

/// Reads what a primitive is given: its sizes are parameters, but a
/// cylinder's, cone's or sphere's radius may come as the fillet's
/// `radius` field (the CLI's `--radius`), which is then that parameter
/// (issue #205). Any other field it doesn't take is refused, saying
/// where its sizes go.
pub(crate) fn primitive_fields(ty: PrimitiveType, given: &mut Given) -> Result<()> {
    if ty.required().contains(&"radius")
        && let Some(r) = given.fields.radius.take()
    {
        if given.params.contains_key("radius") {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a {}'s radius is given twice: as radius and as a parameter",
                    ty.as_str()
                ),
            )
            .with("param", "radius"));
        }
        given.params.insert("radius".into(), r);
    }
    given.only(ty.as_str(), &PRIMITIVE).map_err(|mut e| {
        let sizes: Vec<&str> = ty.required().iter().chain(ty.optional()).copied().collect();
        e.message = format!(
            "{}: a {}'s sizes are parameters ({}), given as --param NAME=QUANTITY \
             (params in a request)",
            e.message,
            ty.as_str(),
            sizes.join(", ")
        );
        e
    })
}

/// The fields a primitive, an extrude, a revolve and a boolean take.
pub(crate) const PRIMITIVE: [&str; 4] = ["params", "placement", "operation", "targets"];

/// A primitive's operation and targets: `current`'s, with what was
/// given laid over them (`new` drops the targets).
pub(crate) fn combine_from(current: &Combine, given: &Given) -> Result<Combine> {
    let mut v = json!({});
    current.write(&mut v);
    let f = &given.fields;
    if let Some(op) = &f.operation {
        v["operation"] = json!(op);
        if op == "new" {
            v.as_object_mut().expect("an object").remove("targets");
        }
    }
    if let Some(t) = &f.targets {
        v["targets"] = json!(t);
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let c = Combine::from_fields(&mut o)?;
    o.finish()?;
    Ok(c)
}
