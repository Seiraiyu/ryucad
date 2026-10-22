//! Curves and surfaces made from others (ISO 10303-42): trims, composites,
//! offsets, polylines, curves on surfaces, swept and trimmed surfaces.

use super::args::{Args, Id};
use super::{Curve, Entity, Surface};
use crate::ImportError;
use crate::part21::Param;

/// One end of a `TRIMMED_CURVE`: a point on the curve, a parameter value,
/// or both (the writer's `master_representation` says which it trusts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trim {
    Point(Id),
    Parameter(f64),
}

/// How a `SURFACE_CURVE` relates to its surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnSurface {
    /// `SURFACE_CURVE`.
    Plain,
    /// `SEAM_CURVE`: one curve, twice on one periodic surface.
    Seam,
    /// `INTERSECTION_CURVE`: where its two surfaces meet.
    Intersection,
}

/// A simple record of a derived curve, an entity of its own (a composite
/// curve's segment), or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Entity, ImportError>> {
    let curve = |c: Result<Curve, ImportError>| Some(c.map(Entity::Curve));
    let surface = |s: Result<Surface, ImportError>| Some(s.map(Entity::Surface));
    match name {
        "TRIMMED_CURVE" => curve(trimmed(id, params)),
        "COMPOSITE_CURVE"
        | "COMPOSITE_CURVE_ON_SURFACE"
        | "BOUNDARY_CURVE"
        | "OUTER_BOUNDARY_CURVE" => curve(composite(id, params)),
        "COMPOSITE_CURVE_SEGMENT" => Some(segment(id, params, 3)),
        "REPARAMETRISED_COMPOSITE_CURVE_SEGMENT" => Some(segment(id, params, 4)),
        "POLYLINE" => curve(polyline(id, params)),
        "OFFSET_CURVE_3D" => curve(offset_curve(id, params, true)),
        "OFFSET_CURVE_2D" => curve(offset_curve(id, params, false)),
        "PCURVE" => curve(pcurve(id, params)),
        "SURFACE_CURVE" => curve(surface_curve(id, params, OnSurface::Plain)),
        "SEAM_CURVE" => curve(surface_curve(id, params, OnSurface::Seam)),
        "INTERSECTION_CURVE" => curve(surface_curve(id, params, OnSurface::Intersection)),
        "SURFACE_OF_LINEAR_EXTRUSION" => surface(extrusion(id, params)),
        "SURFACE_OF_REVOLUTION" => surface(revolution(id, params)),
        "OFFSET_SURFACE" => surface(offset_surface(id, params)),
        "RECTANGULAR_TRIMMED_SURFACE" => surface(rectangular_trimmed(id, params)),
        _ => None,
    }
}

/// `TRIMMED_CURVE(name, basis_curve, trim_1, trim_2, sense_agreement,
/// master_representation)`.
fn trimmed(id: Id, params: &[Param]) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 6)?;
    a.skip();
    let basis = a.id()?;
    let trim_1 = trims(&mut a)?;
    let trim_2 = trims(&mut a)?;
    let sense = a.boolean()?;
    let master = a.enumeration()?.into();
    Ok(Curve::Trimmed {
        basis,
        trim_1,
        trim_2,
        sense,
        master,
    })
}

/// A set of one or two trims: points, or `PARAMETER_VALUE(t)`.
fn trims(a: &mut Args) -> Result<Vec<Trim>, ImportError> {
    let items = a.list()?;
    let trims: Option<Vec<Trim>> = items
        .iter()
        .map(|p| match p {
            Param::Ref(r) => Some(Trim::Point(*r)),
            Param::Typed(name, v) if &**name == "PARAMETER_VALUE" => {
                super::args::real(v).map(Trim::Parameter)
            }
            _ => None,
        })
        .collect();
    match trims {
        Some(t) if (1..=2).contains(&t.len()) => Ok(t),
        _ => Err(a.fault("expected one or two trims: points or PARAMETER_VALUEs")),
    }
}

/// `COMPOSITE_CURVE(name, segments, self_intersect)`, and its subtypes
/// with the same attributes.
fn composite(id: Id, params: &[Param]) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    let segments = a.ids()?;
    if segments.is_empty() {
        return Err(a.fault("a composite curve needs a segment"));
    }
    a.skip();
    Ok(Curve::Composite { segments })
}

/// `COMPOSITE_CURVE_SEGMENT(transition, same_sense, parent_curve)`, with
/// `param_length` after it when reparametrised.
fn segment(id: Id, params: &[Param], count: usize) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, count)?;
    a.skip();
    Ok(Entity::Segment {
        same_sense: a.boolean()?,
        parent: a.id()?,
    })
}

/// `POLYLINE(name, points)`.
fn polyline(id: Id, params: &[Param]) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    a.skip();
    let points = a.ids()?;
    if points.len() < 2 {
        return Err(a.fault("a polyline needs two points"));
    }
    Ok(Curve::Polyline { points })
}

/// `OFFSET_CURVE_3D(name, basis_curve, distance, self_intersect,
/// ref_direction)` or `OFFSET_CURVE_2D(name, basis_curve, distance,
/// self_intersect)`.
fn offset_curve(id: Id, params: &[Param], three_d: bool) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, if three_d { 5 } else { 4 })?;
    a.skip();
    let basis = a.id()?;
    let distance = a.real()?;
    a.skip();
    let ref_direction = if three_d { Some(a.id()?) } else { None };
    Ok(Curve::Offset {
        basis,
        distance,
        ref_direction,
    })
}

/// `PCURVE(name, basis_surface, reference_to_curve)`: the curve is the one
/// item of a `DEFINITIONAL_REPRESENTATION` in the surface's parameters.
fn pcurve(id: Id, params: &[Param]) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Curve::Pcurve {
        surface: a.id()?,
        representation: a.id()?,
    })
}

/// `SURFACE_CURVE(name, curve_3d, associated_geometry,
/// master_representation)` and its subtypes.
fn surface_curve(id: Id, params: &[Param], kind: OnSurface) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 4)?;
    a.skip();
    let curve = a.id()?;
    let associated = a.ids()?;
    if !(1..=2).contains(&associated.len()) {
        return Err(a.fault("expected one or two pcurves or surfaces"));
    }
    let master = a.enumeration()?.into();
    Ok(Curve::OnSurface {
        kind,
        curve,
        associated,
        master,
    })
}

/// `SURFACE_OF_LINEAR_EXTRUSION(name, swept_curve, extrusion_axis)`.
fn extrusion(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Surface::Extrusion {
        curve: a.id()?,
        vector: a.id()?,
    })
}

/// `SURFACE_OF_REVOLUTION(name, swept_curve, axis_position)`.
fn revolution(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Surface::Revolution {
        curve: a.id()?,
        axis: a.id()?,
    })
}

/// `OFFSET_SURFACE(name, basis_surface, distance, self_intersect)`.
fn offset_surface(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 4)?;
    a.skip();
    let basis = a.id()?;
    let distance = a.real()?;
    a.skip();
    Ok(Surface::Offset { basis, distance })
}

/// `RECTANGULAR_TRIMMED_SURFACE(name, basis_surface, u1, u2, v1, v2,
/// usense, vsense)`.
fn rectangular_trimmed(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 8)?;
    a.skip();
    let basis = a.id()?;
    let u = [a.real()?, a.real()?];
    let v = [a.real()?, a.real()?];
    Ok(Surface::RectangularTrimmed {
        basis,
        u,
        v,
        u_sense: a.boolean()?,
        v_sense: a.boolean()?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn curves() {
        let e = entities(
            "#1=TRIMMED_CURVE('',#9,(#7,PARAMETER_VALUE(0.)),(PARAMETER_VALUE(1.5)),.T.,\
             .PARAMETER.);#2=COMPOSITE_CURVE('',(#3,#4),.F.);\
             #3=COMPOSITE_CURVE_SEGMENT(.CONTINUOUS.,.T.,#9);\
             #4=REPARAMETRISED_COMPOSITE_CURVE_SEGMENT(.DISCONTINUOUS.,.F.,#9,2.);\
             #5=POLYLINE('',(#7,#8));#6=OFFSET_CURVE_3D('',#9,-1.5,.U.,#10);\
             #11=OFFSET_CURVE_2D('',#9,2.,.F.);#12=PCURVE('',#13,#14);\
             #15=SEAM_CURVE('',#9,(#12,#16),.PCURVE_S1.);\
             #17=TRIMMED_CURVE('',#9,(),(#7),.T.,.CARTESIAN.);\
             #18=POLYLINE('',(#7));#19=SURFACE_CURVE('',#9,(),.CURVE_3D.);",
        );
        assert_eq!(
            e.curve(0, 1),
            Ok(&Curve::Trimmed {
                basis: 9,
                trim_1: vec![Trim::Point(7), Trim::Parameter(0.0)],
                trim_2: vec![Trim::Parameter(1.5)],
                sense: true,
                master: "PARAMETER".into(),
            })
        );
        assert_eq!(
            e.curve(0, 2),
            Ok(&Curve::Composite {
                segments: vec![3, 4]
            })
        );
        assert_eq!(
            e.segment(0, 4),
            Ok((false, 9)),
            "reparametrised segments read as plain ones"
        );
        assert_eq!(e.curve(0, 5).unwrap().kind(), "POLYLINE");
        assert_eq!(
            e.curve(0, 6),
            Ok(&Curve::Offset {
                basis: 9,
                distance: -1.5,
                ref_direction: Some(10)
            })
        );
        assert_eq!(e.curve(0, 11).unwrap().kind(), "OFFSET_CURVE_2D");
        assert_eq!(
            e.curve(0, 12),
            Ok(&Curve::Pcurve {
                surface: 13,
                representation: 14
            })
        );
        assert_eq!(
            e.curve(0, 15),
            Ok(&Curve::OnSurface {
                kind: OnSurface::Seam,
                curve: 9,
                associated: vec![12, 16],
                master: "PCURVE_S1".into()
            })
        );
        assert_eq!(e.curve(0, 15).unwrap().kind(), "SEAM_CURVE");
        for bad in [17, 18, 19] {
            assert!(
                matches!(e.curve(0, bad), Err(ImportError::Malformed { .. })),
                "#{bad}"
            );
        }
        assert!(matches!(
            e.curve(0, 3),
            Err(ImportError::WrongEntity { .. })
        ));
    }

    #[test]
    fn surfaces() {
        let e = entities(
            "#1=SURFACE_OF_LINEAR_EXTRUSION('',#9,#10);#2=SURFACE_OF_REVOLUTION('',#9,#11);\
             #3=OFFSET_SURFACE('',#12,1.,.F.);\
             #4=RECTANGULAR_TRIMMED_SURFACE('',#12,0.,6.5,-1.,1.,.T.,.F.);",
        );
        assert_eq!(
            e.surface(0, 1),
            Ok(&Surface::Extrusion {
                curve: 9,
                vector: 10
            })
        );
        assert_eq!(
            e.surface(0, 2),
            Ok(&Surface::Revolution { curve: 9, axis: 11 })
        );
        assert_eq!(
            e.surface(0, 3),
            Ok(&Surface::Offset {
                basis: 12,
                distance: 1.0
            })
        );
        assert_eq!(
            e.surface(0, 4),
            Ok(&Surface::RectangularTrimmed {
                basis: 12,
                u: [0.0, 6.5],
                v: [-1.0, 1.0],
                u_sense: true,
                v_sense: false
            })
        );
        let kinds: Vec<&str> = (1..=4).map(|k| e.surface(0, k).unwrap().kind()).collect();
        assert_eq!(
            kinds,
            [
                "SURFACE_OF_LINEAR_EXTRUSION",
                "SURFACE_OF_REVOLUTION",
                "OFFSET_SURFACE",
                "RECTANGULAR_TRIMMED_SURFACE"
            ]
        );
    }
}
