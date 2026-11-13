//! The five primitives: box, cylinder, cone, sphere and torus (ryucad 27.C).

use super::{BodyUse, FeatureKind, Write, apply_placement, new_placement};
use super::{Given, placement_report};
use crate::doc::files::Feature;
use crate::doc::files::{Primitive, PrimitiveType};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::solid::Combine;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Origins, finish, names, op_key, placement};
use ryukei::primitive;
use serde_json::{Map, Value, json};

fn primitive_report(f: &Primitive) -> Value {
    let params: Map<String, Value> = f
        .params
        .iter()
        .map(|(k, q)| (k.clone(), q.report()))
        .collect();
    let mut v = json!({"params": params, "placement": placement_report(&f.placement)});
    f.combine.write(&mut v);
    v
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

/// A cylinder's, cone's or sphere's radius given as the fillet's
/// `radius` field (the CLI's `--radius`) is that parameter (issue #205).
pub(crate) fn radius_param(ty: PrimitiveType, given: &mut Given) -> Result<()> {
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
    Ok(())
}

/// Refuses any other field a primitive doesn't take, saying where its
/// sizes go.
pub(crate) fn refuse_sizes(ty: PrimitiveType, given: &Given) -> Result<()> {
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

/// The five primitives: their sizes are parameters.
pub(crate) struct Primitives;

impl FeatureKind for Primitives {
    fn types(&self) -> &'static [&'static str] {
        &["box", "cylinder", "cone", "sphere", "torus"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &PRIMITIVE
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::WHEN_NEW
    }

    fn take_given(&self, ty: &str, given: &mut Given) -> Result<()> {
        radius_param(PrimitiveType::parse(ty)?, given)
    }

    fn refuse_others(&self, ty: &str, given: &Given) -> Result<()> {
        refuse_sizes(PrimitiveType::parse(ty)?, given)
    }

    fn from_given(
        &self,
        ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let (unit, env) = (cx.unit, cx.env);
        match current {
            None => {
                let ty = PrimitiveType::parse(ty)?;
                let combine = combine_from(&Combine::NEW, &cx.given)?;
                let params = std::mem::take(&mut cx.given.params);
                Ok(Feature::Primitive(Primitive {
                    ty,
                    params: Primitive::params_from_json(ty, &Value::Object(params), unit, env)?,
                    placement: new_placement(
                        cx.given.placement.take().unwrap_or_default(),
                        unit,
                        env,
                    )?,
                    combine,
                }))
            }
            Some(Feature::Primitive(f)) => {
                let mut f = f.clone();
                f.combine = combine_from(&f.combine, &cx.given)?;
                let mut merged: Map<String, Value> = f
                    .params
                    .iter()
                    .map(|(k, q)| (k.clone(), Value::String(q.text())))
                    .collect();
                for (k, v) in std::mem::take(&mut cx.given.params) {
                    if v.is_null() {
                        merged.remove(&k);
                    } else {
                        merged.insert(k, v);
                    }
                }
                f.params = Primitive::params_from_json(f.ty, &Value::Object(merged), unit, env)?;
                apply_placement(
                    &mut f.placement,
                    cx.given.placement.take().unwrap_or_default(),
                    unit,
                    env,
                )?;
                Ok(Feature::Primitive(f))
            }
            Some(_) => unreachable!("a primitive"),
        }
    }

    fn report(&self, _part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let Feature::Primitive(p) = f else {
            unreachable!("a primitive")
        };
        primitive_report(p)
    }

    fn rebuild(&self, cx: &super::RebuildCx<'_>, f: &Feature) -> super::Outcome {
        crate::rebuild::body_step(cx, f)
    }

    fn own_body(
        &self,
        cx: &super::RebuildCx<'_>,
        f: &Feature,
        _targets: &[&crate::rebuild::Built],
    ) -> Result<crate::rebuild::Built> {
        self.alone(&cx.part.id, cx.id, f)
    }

    fn alone(
        &self,
        part: &crate::doc::id::Id,
        id: &crate::doc::id::Id,
        f: &Feature,
    ) -> Result<crate::rebuild::Built> {
        let Feature::Primitive(p) = f else {
            unreachable!("a primitive")
        };
        build_primitive(part, id, p)
    }
}

pub(crate) fn build_primitive(part: &Id, id: &Id, f: &Primitive) -> Result<Built> {
    let key = op_key(part, id);
    let at = placement(&f.placement)?;
    let m = |name| f.meters(name);
    let made = match f.ty {
        PrimitiveType::Box => primitive::block(key, m("x"), m("y"), m("z"), &at),
        PrimitiveType::Cylinder => primitive::cylinder(key, m("radius"), m("height"), &at),
        PrimitiveType::Cone if m("top_radius") > 0.0 => {
            primitive::frustum(key, m("radius"), m("top_radius"), m("height"), &at)
        }
        PrimitiveType::Cone => primitive::cone(key, m("radius"), m("height"), &at),
        PrimitiveType::Sphere => primitive::sphere(key, m("radius"), &at),
        PrimitiveType::Torus => primitive::torus(key, m("major_radius"), m("minor_radius"), &at),
    };
    let made = made.map_err(|e| {
        Error::new(Code::RebuildFailed, format!("{} {id}: {e}", f.ty.as_str()))
            .with("feature", id)
            .with("kernel", format!("{e:?}"))
    })?;
    let origins: Origins = names(&made.0)
        .into_iter()
        .map(|n| (n.to_string(), json!({"feature": id})))
        .collect();
    finish(id, f.ty.as_str(), made, origins)
}
