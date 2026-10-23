//! B-spline curves and surfaces (ISO 10303-42), simple and complex.
//!
//! A polynomial spline is usually a simple record
//! (`B_SPLINE_CURVE_WITH_KNOTS(…)`), but a rational one can only be a
//! complex instance, whose parts each hold their own entity's attributes:
//! `B_SPLINE_CURVE(degree, …) B_SPLINE_CURVE_WITH_KNOTS(…)
//! RATIONAL_B_SPLINE_CURVE(weights) REPRESENTATION_ITEM(name)`. Parts
//! without attributes (`CURVE()`, `BOUNDED_CURVE()`, …) are optional: some
//! writers leave them out.

use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::{Param, Part};

/// How a spline's knots are given.
#[derive(Clone, Debug, PartialEq)]
pub enum Knots {
    /// Distinct values, each with its multiplicity, and the writer's
    /// `knot_spec` (`.UNSPECIFIED.`, `.PIECEWISE_BEZIER_KNOTS.`, …).
    Explicit {
        multiplicities: Vec<u32>,
        values: Vec<f64>,
        spec: Box<str>,
    },
    /// `BEZIER_…`: one span, clamped.
    Bezier,
    /// `UNIFORM_…`: knots `-degree, …, n` one apart, unclamped.
    Uniform,
    /// `QUASI_UNIFORM_…`: uniform but clamped at both ends.
    QuasiUniform,
}

/// A B-spline curve, possibly rational.
#[derive(Clone, Debug, PartialEq)]
pub struct BSplineCurve {
    pub degree: u32,
    pub control_points: Vec<Id>,
    /// The writer's `curve_form` (`.UNSPECIFIED.`, `.CIRCULAR_ARC.`, …).
    pub form: Box<str>,
    /// `closed_curve`: `None` if unknown.
    pub closed: Option<bool>,
    pub knots: Knots,
    /// One positive weight a control point if rational.
    pub weights: Option<Vec<f64>>,
}

/// A B-spline surface, possibly rational. Control points are rows along
/// `v`, one row a `u` index.
#[derive(Clone, Debug, PartialEq)]
pub struct BSplineSurface {
    pub u_degree: u32,
    pub v_degree: u32,
    pub control_points: Vec<Vec<Id>>,
    pub form: Box<str>,
    pub u_closed: Option<bool>,
    pub v_closed: Option<bool>,
    pub u_knots: Knots,
    pub v_knots: Knots,
    pub weights: Option<Vec<Vec<f64>>>,
}

/// A simple record of a spline curve, or `None`.
pub(super) fn simple_curve(
    id: Id,
    name: &str,
    params: &[Param],
) -> Option<Result<BSplineCurve, ImportError>> {
    let knots = match name {
        "B_SPLINE_CURVE_WITH_KNOTS" => None,
        "BEZIER_CURVE" => Some(Knots::Bezier),
        "UNIFORM_CURVE" => Some(Knots::Uniform),
        "QUASI_UNIFORM_CURVE" => Some(Knots::QuasiUniform),
        _ => return None,
    };
    Some((|| {
        let count = if knots.is_none() { 9 } else { 6 };
        let mut a = Args::new(id, params, count)?;
        a.skip();
        let mut c = curve_attrs(&mut a)?;
        c.knots = match knots {
            Some(k) => k,
            None => explicit(&mut a, 1)?.remove(0),
        };
        check_curve(id, &c)?;
        Ok(c)
    })())
}

/// A simple record of a spline surface, or `None`.
pub(super) fn simple_surface(
    id: Id,
    name: &str,
    params: &[Param],
) -> Option<Result<BSplineSurface, ImportError>> {
    let knots = match name {
        "B_SPLINE_SURFACE_WITH_KNOTS" => None,
        "BEZIER_SURFACE" => Some(Knots::Bezier),
        "UNIFORM_SURFACE" => Some(Knots::Uniform),
        "QUASI_UNIFORM_SURFACE" => Some(Knots::QuasiUniform),
        _ => return None,
    };
    Some((|| {
        let count = if knots.is_none() { 13 } else { 8 };
        let mut a = Args::new(id, params, count)?;
        a.skip();
        let mut s = surface_attrs(&mut a)?;
        match knots {
            Some(k) => {
                s.u_knots = k.clone();
                s.v_knots = k;
            }
            None => {
                let mut uv = explicit(&mut a, 2)?;
                s.v_knots = uv.pop().expect("two directions");
                s.u_knots = uv.pop().expect("two directions");
            }
        }
        check_surface(id, &s)?;
        Ok(s)
    })())
}

/// A complex instance's parts if they make a spline curve: `None` if
/// there is no `B_SPLINE_CURVE` part.
pub(super) fn complex_curve(id: Id, parts: &[Part]) -> Option<Result<BSplineCurve, ImportError>> {
    let base = part(parts, "B_SPLINE_CURVE")?;
    Some((|| {
        let mut a = Args::new(id, &base.params, 5)?;
        let mut c = curve_attrs(&mut a)?;
        c.knots = knots_of(id, parts, "CURVE", 1)?.remove(0);
        if let Some(r) = part(parts, "RATIONAL_B_SPLINE_CURVE") {
            let mut a = Args::new(id, &r.params, 1)?;
            c.weights = Some(a.reals()?);
        }
        check_curve(id, &c)?;
        Ok(c)
    })())
}

/// A complex instance's parts if they make a spline surface.
pub(super) fn complex_surface(
    id: Id,
    parts: &[Part],
) -> Option<Result<BSplineSurface, ImportError>> {
    let base = part(parts, "B_SPLINE_SURFACE")?;
    Some((|| {
        let mut a = Args::new(id, &base.params, 7)?;
        let mut s = surface_attrs(&mut a)?;
        let mut uv = knots_of(id, parts, "SURFACE", 2)?;
        s.v_knots = uv.pop().expect("two directions");
        s.u_knots = uv.pop().expect("two directions");
        if let Some(r) = part(parts, "RATIONAL_B_SPLINE_SURFACE") {
            let mut a = Args::new(id, &r.params, 1)?;
            let rows = a.list()?;
            let weights: Option<Vec<Vec<f64>>> = rows
                .iter()
                .map(|row| match row {
                    Param::List(items) => items.iter().map(super::args::real).collect(),
                    _ => None,
                })
                .collect();
            s.weights = Some(weights.ok_or_else(|| a.fault("expected rows of weights"))?);
        }
        check_surface(id, &s)?;
        Ok(s)
    })())
}

fn part<'a>(parts: &'a [Part], name: &str) -> Option<&'a Part> {
    parts.iter().find(|p| &*p.name == name)
}

/// The knots a complex spline's parts give, for `dirs` directions.
fn knots_of(id: Id, parts: &[Part], what: &str, dirs: usize) -> Result<Vec<Knots>, ImportError> {
    if let Some(k) = part(parts, &format!("B_SPLINE_{what}_WITH_KNOTS")) {
        let mut a = Args::new(id, &k.params, 2 * dirs + 1)?;
        return explicit(&mut a, dirs);
    }
    let kind = if part(parts, &format!("BEZIER_{what}")).is_some() {
        Knots::Bezier
    } else if part(parts, &format!("UNIFORM_{what}")).is_some() {
        Knots::Uniform
    } else if part(parts, &format!("QUASI_UNIFORM_{what}")).is_some() {
        Knots::QuasiUniform
    } else {
        return Err(ImportError::Malformed {
            instance: id,
            attribute: 0,
            what: "a B-spline needs its knots",
        });
    };
    Ok(vec![kind; dirs])
}

/// `degree, control_points_list, curve_form, closed_curve,
/// self_intersect`, knots to follow.
fn curve_attrs(a: &mut Args) -> Result<BSplineCurve, ImportError> {
    let degree = degree(a)?;
    let control_points = a.ids()?;
    let form = a.enumeration()?.into();
    let closed = a.logical()?;
    a.skip();
    Ok(BSplineCurve {
        degree,
        control_points,
        form,
        closed,
        knots: Knots::Bezier,
        weights: None,
    })
}

/// `u_degree, v_degree, control_points_list, surface_form, u_closed,
/// v_closed, self_intersect`, knots to follow.
fn surface_attrs(a: &mut Args) -> Result<BSplineSurface, ImportError> {
    let u_degree = degree(a)?;
    let v_degree = degree(a)?;
    let rows = a.list()?;
    let control_points: Option<Vec<Vec<Id>>> = rows
        .iter()
        .map(|row| match row {
            Param::List(items) => items
                .iter()
                .map(|p| match p {
                    Param::Ref(r) => Some(*r),
                    _ => None,
                })
                .collect(),
            _ => None,
        })
        .collect();
    let control_points = control_points.ok_or_else(|| a.fault("expected rows of references"))?;
    let form = a.enumeration()?.into();
    let u_closed = a.logical()?;
    let v_closed = a.logical()?;
    a.skip();
    Ok(BSplineSurface {
        u_degree,
        v_degree,
        control_points,
        form,
        u_closed,
        v_closed,
        u_knots: Knots::Bezier,
        v_knots: Knots::Bezier,
        weights: None,
    })
}

fn degree(a: &mut Args) -> Result<u32, ImportError> {
    let d = a.int()?;
    u32::try_from(d)
        .ok()
        .filter(|&d| d >= 1)
        .ok_or_else(|| a.fault("a degree must be a positive integer"))
}

/// `multiplicities…, knots…, knot_spec` for `dirs` directions: all the
/// multiplicity lists, then all the knot lists.
fn explicit(a: &mut Args, dirs: usize) -> Result<Vec<Knots>, ImportError> {
    let mut mults = Vec::with_capacity(dirs);
    for _ in 0..dirs {
        let list = a.list()?;
        let m: Option<Vec<u32>> = list
            .iter()
            .map(|p| match p {
                Param::Int(n) => u32::try_from(*n).ok().filter(|&n| n >= 1),
                _ => None,
            })
            .collect();
        mults.push(m.ok_or_else(|| a.fault("multiplicities must be positive integers"))?);
    }
    let mut values = Vec::with_capacity(dirs);
    for m in &mults {
        let v = a.reals()?;
        if v.len() != m.len() {
            return Err(a.fault("one multiplicity a knot"));
        }
        values.push(v);
    }
    let spec: Box<str> = a.enumeration()?.into();
    Ok(mults
        .into_iter()
        .zip(values)
        .map(|(multiplicities, values)| Knots::Explicit {
            multiplicities,
            values,
            spec: spec.clone(),
        })
        .collect())
}

fn check_curve(id: Id, c: &BSplineCurve) -> Result<(), ImportError> {
    let fault = |what| ImportError::Malformed {
        instance: id,
        attribute: 0,
        what,
    };
    if c.control_points.len() < 2 {
        return Err(fault("a B-spline needs at least two control points"));
    }
    if let Some(w) = &c.weights {
        if w.len() != c.control_points.len() {
            return Err(fault("one weight a control point"));
        }
        if w.iter().any(|&w| w <= 0.0) {
            return Err(fault("weights must be positive"));
        }
    }
    Ok(())
}

fn check_surface(id: Id, s: &BSplineSurface) -> Result<(), ImportError> {
    let fault = |what| ImportError::Malformed {
        instance: id,
        attribute: 0,
        what,
    };
    let cols = s.control_points.first().map_or(0, Vec::len);
    if s.control_points.len() < 2 || cols < 2 {
        return Err(fault(
            "a B-spline surface needs at least two rows and columns",
        ));
    }
    if s.control_points.iter().any(|r| r.len() != cols) {
        return Err(fault("control point rows must be equally long"));
    }
    if let Some(w) = &s.weights {
        if w.len() != s.control_points.len() || w.iter().any(|r| r.len() != cols) {
            return Err(fault("one weight a control point"));
        }
        if w.iter().flatten().any(|&w| w <= 0.0) {
            return Err(fault("weights must be positive"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::super::{Curve, Surface};
    use super::*;

    fn curve(e: &super::super::Entities, id: Id) -> BSplineCurve {
        match e.curve(0, id) {
            Ok(Curve::BSpline(c)) => c.clone(),
            r => panic!("#{id}: {r:?}"),
        }
    }

    fn surface(e: &super::super::Entities, id: Id) -> BSplineSurface {
        match e.surface(0, id) {
            Ok(Surface::BSpline(s)) => s.clone(),
            r => panic!("#{id}: {r:?}"),
        }
    }

    #[test]
    fn simple_curves() {
        let e = entities(
            "#1=B_SPLINE_CURVE_WITH_KNOTS('',3,(#10,#11,#12,#13),.UNSPECIFIED.,.F.,.U.,\
             (4,4),(0.,1.),.PIECEWISE_BEZIER_KNOTS.);\
             #2=BEZIER_CURVE('',2,(#10,#11,#12),.UNSPECIFIED.,.U.,.F.);\
             #3=QUASI_UNIFORM_CURVE('',1,(#10,#11,#12),.POLYLINE_FORM.,.F.,.F.);",
        );
        assert_eq!(
            curve(&e, 1),
            BSplineCurve {
                degree: 3,
                control_points: vec![10, 11, 12, 13],
                form: "UNSPECIFIED".into(),
                closed: Some(false),
                knots: Knots::Explicit {
                    multiplicities: vec![4, 4],
                    values: vec![0.0, 1.0],
                    spec: "PIECEWISE_BEZIER_KNOTS".into(),
                },
                weights: None,
            }
        );
        let b = curve(&e, 2);
        assert_eq!((b.degree, b.closed, b.knots), (2, None, Knots::Bezier));
        assert_eq!(curve(&e, 3).knots, Knots::QuasiUniform);
        assert_eq!(e.curve(0, 3).unwrap().kind(), "B_SPLINE_CURVE");
    }

    #[test]
    fn rational_curves_from_complex_instances() {
        // As OCCT writes them, and with the attribute-less parts left out.
        let e = entities(
            "#1=( BOUNDED_CURVE() B_SPLINE_CURVE(2,(#26,#27,#28),.UNSPECIFIED.,.F.,.F.) \
             B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.PIECEWISE_BEZIER_KNOTS.) CURVE() \
             GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE((1.,0.5,1.)) \
             REPRESENTATION_ITEM('') );\
             #2=(B_SPLINE_CURVE(2,(#26,#27,#28),.CIRCULAR_ARC.,.F.,.F.)BEZIER_CURVE()\
             RATIONAL_B_SPLINE_CURVE((1.,0.5,1.)));\
             #3=(B_SPLINE_CURVE(2,(#26,#27,#28),.UNSPECIFIED.,.F.,.F.)\
             RATIONAL_B_SPLINE_CURVE((1.,0.5,1.)));\
             #4=(B_SPLINE_CURVE(2,(#26,#27,#28),.UNSPECIFIED.,.F.,.F.)BEZIER_CURVE()\
             RATIONAL_B_SPLINE_CURVE((1.,0.,1.)));",
        );
        let c = curve(&e, 1);
        assert_eq!(c.weights, Some(vec![1.0, 0.5, 1.0]));
        assert_eq!(
            c.knots,
            Knots::Explicit {
                multiplicities: vec![3, 3],
                values: vec![0.0, 1.0],
                spec: "PIECEWISE_BEZIER_KNOTS".into()
            }
        );
        let c = curve(&e, 2);
        assert_eq!((c.knots, &*c.form), (Knots::Bezier, "CIRCULAR_ARC"));
        assert!(matches!(
            e.curve(0, 3),
            Err(ImportError::Malformed {
                what: "a B-spline needs its knots",
                ..
            })
        ));
        assert!(matches!(
            e.curve(0, 4),
            Err(ImportError::Malformed {
                what: "weights must be positive",
                ..
            })
        ));
    }

    #[test]
    fn surfaces() {
        let e = entities(
            "#1=B_SPLINE_SURFACE_WITH_KNOTS('',2,1,((#10,#11),(#12,#13),(#14,#15)),\
             .UNSPECIFIED.,.T.,.F.,.F.,(3),(2),(0.),(1.),.UNSPECIFIED.);\
             #2=( BOUNDED_SURFACE() B_SPLINE_SURFACE(1,1,((#10,#11),(#12,#13)),\
             .UNSPECIFIED.,.F.,.F.,.F.) B_SPLINE_SURFACE_WITH_KNOTS((2),(2),(0.),(5.),\
             .UNSPECIFIED.) GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_SURFACE(\
             ((1.,2.),(1.,0.5))) REPRESENTATION_ITEM('') SURFACE() );\
             #3=BEZIER_SURFACE('',1,1,((#10,#11),(#12,#13)),.PLANE_SURF.,.F.,.F.,.F.);\
             #4=BEZIER_SURFACE('',1,1,((#10,#11),(#12)),.PLANE_SURF.,.F.,.F.,.F.);\
             #5=B_SPLINE_SURFACE_WITH_KNOTS('',1,1,((#10,#11),(#12,#13)),\
             .UNSPECIFIED.,.F.,.F.,.F.,(2),(2,1),(0.),(1.),.UNSPECIFIED.);\
             #6=B_SPLINE_SURFACE_WITH_KNOTS('',0,1,((#10,#11),(#12,#13)),\
             .UNSPECIFIED.,.F.,.F.,.F.,(2),(2),(0.),(1.),.UNSPECIFIED.);",
        );
        let s = surface(&e, 1);
        assert_eq!((s.u_degree, s.v_degree), (2, 1));
        assert_eq!(s.control_points[2], vec![14, 15]);
        assert_eq!(s.u_closed, Some(true));
        assert_eq!(
            s.v_knots,
            Knots::Explicit {
                multiplicities: vec![2],
                values: vec![1.0],
                spec: "UNSPECIFIED".into()
            }
        );
        let s = surface(&e, 2);
        assert_eq!(s.weights, Some(vec![vec![1.0, 2.0], vec![1.0, 0.5]]));
        assert_eq!(
            s.v_knots,
            Knots::Explicit {
                multiplicities: vec![2],
                values: vec![5.0],
                spec: "UNSPECIFIED".into()
            }
        );
        assert_eq!(surface(&e, 3).u_knots, Knots::Bezier);
        assert_eq!(e.surface(0, 3).unwrap().kind(), "B_SPLINE_SURFACE");
        assert!(matches!(
            e.surface(0, 4),
            Err(ImportError::Malformed {
                what: "control point rows must be equally long",
                ..
            })
        ));
        assert!(matches!(
            e.surface(0, 5),
            Err(ImportError::Malformed {
                what: "one multiplicity a knot",
                ..
            })
        ));
        assert!(matches!(
            e.surface(0, 6),
            Err(ImportError::Malformed {
                attribute: 1,
                what: "a degree must be a positive integer",
                ..
            })
        ));
    }
}
