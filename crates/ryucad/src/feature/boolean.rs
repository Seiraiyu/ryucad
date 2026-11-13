//! Booleans of whole solids (ryucad 27.C).

use super::Given;
use super::{BodyUse, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::solid::Boolean;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Made, Results, ToolBody, apply, read, solid_key};
use crate::solid::Solids;
use ryukei::boolean::BooleanOp;
use ryukei::name::ToolKey;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

pub(crate) const BOOLEAN: [&str; 4] = ["op", "target", "targets", "tools"];

/// A boolean from `current`'s fields (none for a new one) with what was
/// given laid over them. Its target may come as `target`, or as the one
/// entry of `targets`.
pub(crate) fn boolean_from(current: Option<&Boolean>, given: &Given) -> Result<Boolean> {
    let mut v = current.map_or_else(|| json!({}), Boolean::fields);
    let f = &given.fields;
    let target = match (&f.target, &f.targets) {
        (Some(_), Some(_)) => {
            return Err(Error::new(Code::Usage, "give a boolean's target once"));
        }
        (Some(t), None) => Some(t.clone()),
        (None, Some(ts)) => match ts.as_slice() {
            [t] => Some(t.clone()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a boolean has one target, not {}", ts.len()),
                )
                .with("param", "targets"));
            }
        },
        (None, None) => None,
    };
    if let Some(op) = &f.op {
        v["op"] = json!(op);
    }
    if let Some(t) = target {
        v["target"] = json!(t);
    }
    if let Some(t) = &f.tools {
        v["tools"] = json!(t);
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let b = Boolean::from_fields(&mut o)?;
    o.finish()?;
    Ok(b)
}

/// Booleans of whole solids.
pub(crate) struct Booleans;

impl FeatureKind for Booleans {
    fn types(&self) -> &'static [&'static str] {
        &["boolean"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &BOOLEAN
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
        Ok(Feature::Boolean(boolean_from(
            current.and_then(Feature::boolean),
            &cx.given,
        )?))
    }

    fn report(&self, _part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        f.boolean().expect("a boolean").fields()
    }
}

/// A `boolean` feature: its target as it sees it, with its tools as it
/// sees them, each placed under a key from its founder.
pub(crate) fn build_boolean(
    part: &Part,
    solids: &Solids,
    id: &Id,
    f: &Feature,
    b: &crate::doc::solid::Boolean,
    before: &dyn Results,
    run: &crate::rebuild::Run,
) -> Result<Made> {
    let target = read(part, solids, id, f, "target", &b.target, before)?;
    let tools: Vec<(&Id, &Built)> = b
        .tools
        .iter()
        .map(|t| Ok((t, read(part, solids, id, f, "tools", t, before)?)))
        .collect::<Result<_>>()?;
    let keys: Vec<ToolKey> = b.tools.iter().map(|t| solid_key(&part.id, id, t)).collect();
    let tools: Vec<ToolBody> = tools
        .iter()
        .zip(keys)
        .map(|((name, built), key)| ToolBody::whole(key, built, name))
        .collect();
    let kop = match b.op {
        crate::doc::solid::BooleanOp::Unite => BooleanOp::Unite,
        crate::doc::solid::BooleanOp::Subtract => BooleanOp::Subtract,
        crate::doc::solid::BooleanOp::Intersect => BooleanOp::Intersect,
    };
    let built = apply(
        part,
        id,
        "boolean",
        (kop, b.op.as_str()),
        &b.target,
        target,
        &tools,
        run,
    )?;
    Ok(Made::new(vec![(b.target.clone(), Arc::new(built))]))
}
