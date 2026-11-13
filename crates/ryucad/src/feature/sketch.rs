//! Sketches: their plane, as a request gives it (ryucad 27.C).

use super::{BodyUse, Check, FeatureKind, Write};
use super::{Given, apply_placement, new_placement, placement_report};
use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::reference::Reference;
use crate::doc::sketch::{OnFace, OriginPlane, Plane, Sketch};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::part::reference_texts;
use crate::units::Unit;
use serde_json::Value;

/// A sketch as output reports it: its file's fields, with every dimension
/// as written and evaluated.
fn sketch_report(part: &Id, s: &Sketch) -> Value {
    let mut v = s.fields();
    if let Plane::Placement(p) = &s.plane {
        v["placement"] = placement_report(p);
    }
    reference_texts(part, s, &mut v);
    for (id, c) in &s.constraints {
        if let Some(q) = &c.value {
            v["constraints"][id.as_str()]["value"] = q.report();
        }
    }
    v
}

/// The fields a sketch takes.
pub(crate) const SKETCH: [&str; 5] = ["plane", "placement", "face", "face_origin", "face_x"];

/// A sketch's plane after an edit: an origin plane, its placement with
/// what was given applied, or a face with its origin and `x`. `current`
/// is `None` for a new sketch.
pub(crate) fn sketch_plane(
    current: Option<&Plane>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Plane> {
    let f = &given.fields;
    let chosen = [
        f.plane.is_some(),
        given.placement.is_some(),
        f.face.is_some(),
    ];
    if chosen.iter().filter(|c| **c).count() > 1 {
        return Err(Error::new(
            Code::Usage,
            "give a sketch one of a plane, a placement and a face",
        ));
    }
    // A face's origin or x, or none: the default.
    use crate::doc::geometry::GeomRef;
    let reference = |s: &Option<String>| -> Result<Option<Option<GeomRef>>> {
        match s.as_deref() {
            None => Ok(None),
            Some("") => Ok(Some(None)),
            Some(t) => GeomRef::parse(t, part).map(|r| Some(Some(r))),
        }
    };
    let (origin, x) = (reference(&f.face_origin)?, reference(&f.face_x)?);
    let plane = match (&f.plane, &given.placement, &f.face) {
        // An origin plane; a datum plane or another sketch; a face.
        (Some(p), _, _) => {
            match GeomRef::parse(p, part).map_err(|e| e.with("pointer", "/plane"))? {
                GeomRef::Plane(o) => Plane::Origin(o),
                g @ GeomRef::Feature(_) => Plane::Datum(g),
                GeomRef::Entity(r) => Plane::Face(OnFace {
                    face: r,
                    origin: None,
                    x: None,
                }),
                _ => {
                    return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a sketch's plane is xy, yz, zx, a datum plane or sketch <part>/<feature>, or a planar face; not {p}"
                    ),
                )
                .with("pointer", "/plane"));
                }
            }
        }
        (_, Some(arg), _) => {
            let mut p = match current.and_then(|c| c.placement(unit)) {
                Some(p) => p,
                None => new_placement(arg.clone(), unit, env)?,
            };
            apply_placement(&mut p, arg.clone(), unit, env)?;
            Plane::Placement(p)
        }
        (_, _, Some(face)) => Plane::Face(OnFace {
            face: Reference::parse(face, part)?,
            origin: None,
            x: None,
        }),
        _ => current.cloned().unwrap_or(Plane::Origin(OriginPlane::Xy)),
    };
    match plane {
        Plane::Face(mut on) => {
            // A new face keeps no old origin or x.
            let old = match current {
                Some(Plane::Face(c)) if f.face.is_none() && f.plane.is_none() => Some(c),
                _ => None,
            };
            on.origin = origin.unwrap_or_else(|| old.and_then(|c| c.origin.clone()));
            on.x = x.unwrap_or_else(|| old.and_then(|c| c.x.clone()));
            Ok(Plane::Face(on))
        }
        p if origin.is_some() || x.is_some() => {
            let _ = p;
            Err(Error::new(
                Code::Usage,
                "face_origin and face_x go with a sketch on a face",
            ))
        }
        p => Ok(p),
    }
}

/// Sketches. Their geometry changes through `sketch.edit`; an edit here
/// moves the plane without changing the solve.
pub(crate) struct Sketches;

impl FeatureKind for Sketches {
    fn types(&self) -> &'static [&'static str] {
        &["sketch"]
    }

    fn fields(&self, _ty: &str) -> &'static [&'static str] {
        &SKETCH
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
        match current {
            None => Ok(Feature::Sketch(Sketch::new(sketch_plane(
                None, &cx.given, cx.part, cx.unit, cx.env,
            )?))),
            Some(Feature::Sketch(s)) => {
                let mut s = s.clone();
                s.plane = sketch_plane(Some(&s.plane), &cx.given, cx.part, cx.unit, cx.env)?;
                Ok(Feature::Sketch(s))
            }
            Some(_) => unreachable!("a sketch"),
        }
    }

    fn report(&self, part: &Part, f: &Feature) -> Value {
        let Feature::Sketch(s) = f else {
            unreachable!("a sketch")
        };
        sketch_report(&part.id, s)
    }

    fn check(&self, cx: &Check<'_>, f: &Feature) -> Result<()> {
        let Feature::Sketch(s) = f else {
            unreachable!("a sketch")
        };
        crate::sketch::context(cx.part, cx.order, cx.id, s, cx.results).map(|_| ())
    }
}
