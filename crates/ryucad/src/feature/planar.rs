//! Planar faces: a profile's closed loops filled (ryucad 27.C).

use super::Given;
use super::{BodyUse, Check, FeatureKind, Write};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::geometry::geom_json;
use serde_json::{Value, json};

/// A planar face from `current`'s fields (none for a new one) with what
/// was given laid over them: its profile's sketch or face, and loops.
pub(crate) fn planar_from(current: Option<Value>, given: &Given, part: &Id) -> Result<Feature> {
    let bad = |param: &str, msg: &str| Error::new(Code::InvalidParam, msg).with("param", param);
    let mut profile = current
        .and_then(|v| v.get("profile").cloned())
        .unwrap_or_else(|| json!({}));
    let f = &given.fields;
    match (&f.sketch, &f.face) {
        (Some(_), Some(_)) => {
            return Err(bad(
                "face",
                "a planar face's profile is a sketch or a planar face, not both",
            ));
        }
        (Some(sk), None) => profile = json!({"sketch": sk}),
        (None, Some(face)) => profile = json!({"face": geom_json(face, part, "/profile/face")?}),
        (None, None) => {}
    }
    if profile.as_object().is_none_or(|o| o.is_empty()) {
        return Err(bad(
            "sketch",
            "a planar face fills closed loops: name a sketch (sketch) or a planar face (face)",
        ));
    }
    match &f.loops {
        Some(l) if l.is_empty() => {
            if let Some(o) = profile.as_object_mut() {
                o.remove("loops");
            }
        }
        Some(l) => profile["loops"] = json!(l),
        None => {}
    }
    let v = json!({"profile": profile});
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = crate::doc::sweep::Planar::from_fields(&mut o)?;
    o.finish()?;
    Ok(Feature::Planar(Box::new(p)))
}

/// Planar faces: a sheet founded from a profile's closed loops.
pub(crate) struct Planars;

impl FeatureKind for Planars {
    fn types(&self) -> &'static [&'static str] {
        &["planar"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &crate::doc::sweep::PLANAR_FIELDS
    }

    fn body(&self, _ty: &str) -> BodyUse {
        BodyUse::FOUNDS
    }

    fn from_given(
        &self,
        _ty: &str,
        current: Option<&Feature>,
        cx: &mut Write<'_>,
    ) -> Result<Feature> {
        let fields = match current {
            None => None,
            Some(Feature::Planar(x)) => Some(x.fields()),
            Some(_) => unreachable!("a planar face"),
        };
        let f = planar_from(fields, &cx.given, cx.part)?;
        if !f.profiles().is_empty() {
            cx.check_profile(&f)?;
        }
        Ok(f)
    }

    fn report(&self, part: &Part, _unit: crate::units::Unit, f: &Feature) -> Value {
        let Feature::Planar(x) = f else {
            unreachable!("a planar face")
        };
        let mut v = x.fields();
        if let crate::doc::sweep::Source::Face(g) = &x.profile.source {
            v["profile"]["face"] = json!(g.text(&part.id));
        }
        v
    }

    fn always_checked(&self, _f: &Feature) -> bool {
        true
    }

    // A planar face's profile is closed loops: its open curves are
    // refused when written (what else fails is rebuild's).
    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        let Feature::Planar(x) = f else {
            unreachable!("a planar face")
        };
        match crate::profile::planar(cx.part, cx.id, x, cx.results) {
            Err(e) if e.code == Code::ProfileInvalid => Err(e),
            _ => Ok(()),
        }
    }
}
