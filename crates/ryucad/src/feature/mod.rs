//! Feature kinds (ryucad 27.C, design 541): what ryucad does with each
//! feature type a request names, one module per kind, registered in
//! `KINDS`. The file formats stay in `ryucad-doc`, which ryupdm reads.

use crate::doc::files::{Feature, Placement, number};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sweep::RevolveAxis;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::proto::{FeatureFields, PlacementArg};
use crate::rebuild;
use crate::session::Session;
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use serde_json::{Map, Value, json};

/// A placement as output reports it: quantities as written and in meters.
pub(crate) fn placement_report(p: &Placement) -> Value {
    json!({
        "origin": p.origin.iter().map(Quantity::report).collect::<Vec<_>>(),
        "z": p.z.map(number),
        "x": p.x.map(number),
    })
}

/// Applies the parts of `arg` that are given to `p`, its expressions
/// reading `env`.
pub(crate) fn apply_placement(
    p: &mut Placement,
    arg: PlacementArg,
    unit: Unit,
    env: &Env,
) -> Result<()> {
    if let Some(origin) = arg.origin {
        for (i, v) in origin.iter().enumerate() {
            p.origin[i] = Quantity::from_json_in(v, unit, &|n| env.get(n))
                .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
        }
    }
    if let Some(z) = arg.z {
        p.z = z;
    }
    if let Some(x) = arg.x {
        p.x = x;
    }
    // Refuse a placement that could never build, before it is written.
    rebuild::placement(p).map(|_| ())
}

/// What a `feature.add` or `feature.edit` gives besides the feature's
/// place: each part left out keeps its current or default value.
pub(crate) struct Given {
    pub(crate) params: Map<String, Value>,
    pub(crate) placement: Option<PlacementArg>,
    pub(crate) fields: FeatureFields,
}

impl Given {
    /// The names of the fields given.
    pub(crate) fn present(&self) -> Vec<String> {
        let mut out: Vec<String> = self.fields.given().keys().cloned().collect();
        if !self.params.is_empty() {
            out.push("params".into());
        }
        if self.placement.is_some() {
            out.push("placement".into());
        }
        out
    }

    /// Refuses the fields `ty` doesn't take.
    pub(crate) fn only(&self, ty: &str, allowed: &[&str]) -> Result<()> {
        match self
            .present()
            .into_iter()
            .find(|f| !allowed.contains(&f.as_str()))
        {
            None => Ok(()),
            Some(name) => Err(
                Error::new(Code::InvalidParam, format!("a {ty} has no {name}")).with("param", name),
            ),
        }
    }
}

/// Whether body `x` of part `pid`, as a feature at `at` of `list` would
/// see it, is a sheet in the current rebuild: the tip of its chain among
/// the features before `at`. A part no feature of which may make a
/// sheet is never rebuilt for this.
pub(crate) fn source_is_sheet(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    at: usize,
    x: &Id,
) -> bool {
    let Ok(p) = session.project().part(pid).cloned() else {
        return false;
    };
    let may = p
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    if !may {
        return false;
    }
    let solids = Solids::of_order(&p, &list[..at]);
    let Some(tip) = solids.tips().find(|(s, _)| *s == x).map(|(_, t)| t.clone()) else {
        return false;
    };
    let Ok(results) = session.rebuilt(pid) else {
        return false;
    };
    crate::rebuild::Results::result(results, &tip)
        .and_then(|r| r.as_ref().ok())
        .and_then(|o| o.solid(x))
        .is_some_and(|b| b.body.kind() == ryukei::topo::BodyKind::Sheet)
}

/// The solids a feature put at `at` in `order` could act on: founded
/// before it and not consumed.
pub(crate) fn solids_before(p: &Part, order: &[Id], at: usize) -> Vec<Id> {
    Solids::of_order(p, &order[..at])
        .tips()
        .map(|(s, _)| s.clone())
        .collect()
}

/// Refuses a sweep or a hole that names what isn't there: its sketch must
/// be a sketch listed before it in `order`, with the entities it names,
/// its axis a line and a hole's points points. Whether the loops make a
/// profile only rebuild can tell.
pub(crate) fn check_sweep(part: &Part, order: &[Id], id: &Id, f: &Feature) -> Result<()> {
    // A loft's sections: each sketch listed before it, with the entities
    // its loops name. How they match only its rebuild check tells.
    if let Feature::Loft(l) = f {
        for (k, s) in l.sections.iter().enumerate() {
            let Some(sketch) = s.sketch() else { continue };
            check_sketch(part, order, id, sketch)?;
            let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
                continue;
            };
            for (i, e) in s.loops.iter().enumerate() {
                if !sk.entities.contains_key(e) {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/sections/{k}/loops/{i}")),
                    );
                }
            }
        }
        return Ok(());
    }
    let sketch = f.profiles()[0];
    check_sketch(part, order, id, sketch)?;
    let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
        return Ok(());
    };
    // A hole's points must be points of its sketch; whether it has any
    // to drill when it lists none only rebuild can tell.
    if let Feature::Hole(h) = f {
        for (i, e) in h.points.iter().enumerate() {
            match sk.entities.get(e).map(|x| &x.geometry) {
                None => {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/points/{i}")),
                    );
                }
                Some(crate::doc::sketch::Geometry::Point { .. }) => {}
                Some(g) => {
                    return Err(Error::new(
                        Code::ReferenceWrongKind,
                        format!("a hole drills at points; {e} is a {}", g.type_name()),
                    )
                    .with("entity", e)
                    .with("pointer", format!("/points/{i}")));
                }
            }
        }
        return Ok(());
    }
    let (loops, axis) = match f {
        Feature::Extrude(e) => (&e.loops, None),
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Line(l) => (&r.loops, Some(l)),
            RevolveAxis::Ref(_) => (&r.loops, None),
        },
        Feature::Sweep(w) => (&w.profile.loops, None),
        Feature::Planar(x) => (&x.profile.loops, None),
        Feature::BaseFlange(b) => (&b.loops, None),
        _ => unreachable!("a sweep"),
    };
    let at = if f.sweep().is_some() || matches!(f, Feature::Planar(_)) {
        "/profile"
    } else {
        ""
    };
    for (field, e) in loops
        .iter()
        .map(|e| ("loops", e))
        .chain(axis.map(|a| ("axis", a)))
    {
        match sk.entities.get(e).map(|x| &x.geometry) {
            None => {
                return Err(
                    Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                        .with("sketch", sketch)
                        .with("entity", e)
                        .with("pointer", format!("{at}/{field}")),
                );
            }
            Some(g)
                if field == "axis" && !matches!(g, crate::doc::sketch::Geometry::Line { .. }) =>
            {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("an axis is a line; {e} is a {}", g.type_name()),
                )
                .with("entity", e)
                .with("pointer", "/axis"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Refuses a sketch that isn't a sketch listed before `id` in `order`.
pub(crate) fn check_sketch(part: &Part, order: &[Id], id: &Id, sketch: &Id) -> Result<()> {
    let at = |f: &Id| order.iter().position(|x| x == f);
    match (at(sketch), at(id)) {
        (None, _) => {
            return Err(Error::new(
                Code::NotFound,
                format!("part {} has no feature {sketch} to sweep", part.id),
            )
            .with("sketch", sketch));
        }
        (Some(s), Some(me)) if s >= me => {
            return Err(Error::new(
                Code::ReferenceForward,
                format!("{id} would sweep {sketch}, which comes after it"),
            )
            .with("feature", id)
            .with("sketch", sketch));
        }
        _ => {}
    }
    match part.features.get(sketch) {
        Some(Ok(f)) if f.sketch().is_none() => Err(Error::new(
            Code::ReferenceWrongKind,
            format!("{sketch} is a {}, not a sketch", f.type_name()),
        )
        .with("sketch", sketch)),
        _ => Ok(()),
    }
}

/// A new feature's placement from what was given. With z given and x left
/// out, x defaults to world X, or to world Y when z lies along X; either
/// way the file records it.
pub(crate) fn new_placement(arg: PlacementArg, unit: Unit, env: &Env) -> Result<Placement> {
    let mut placed = Placement::identity(unit);
    let along_x = arg.x.is_none()
        && arg.z.is_some_and(|z| {
            let mut p = Placement::identity(unit);
            p.z = z;
            rebuild::placement(&p).is_err_and(|e| e.details["kernel"] == "Degenerate")
        });
    if along_x {
        placed.x = [0.0, 1.0, 0.0];
    }
    apply_placement(&mut placed, arg, unit, env)?;
    Ok(placed)
}

/// Refuses an up-to target (`role` in feature `id`) that isn't a plane
/// (a planar face, a datum plane) or a point (a vertex, a datum point) now.
pub(crate) fn check_up_to(
    p: &Part,
    order: &[Id],
    id: &Id,
    role: &str,
    target: &crate::doc::geometry::GeomRef,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    use crate::geometry::{At, Want, resolve};
    let at = At {
        part: p,
        order,
        owner: id,
        results,
    };
    resolve(&at, role, target, &[Want::Point, Want::Plane]).map(|_| ())
}
