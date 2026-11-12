//! Planar faces: a profile's closed loops filled (ryucad 27.C).

use super::Given;
use crate::doc::files::Feature;
use crate::doc::id::Id;
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
