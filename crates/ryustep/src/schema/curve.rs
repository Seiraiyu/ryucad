//! Curves (ISO 10303-42), as written: lengths in the file's length unit,
//! angles in its plane angle unit.

use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// A curve in two or three dimensions.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Curve {
    /// `LINE`: a point and a `VECTOR`, whose length is a unit of the line's
    /// parameter.
    Line { point: Id, vector: Id },
    /// `CIRCLE`, about its placement's axis from its `x` direction.
    Circle { position: Id, radius: f64 },
    /// `ELLIPSE`: `semi_axis_1` along the placement's `x` direction.
    Ellipse {
        position: Id,
        semi_axis_1: f64,
        semi_axis_2: f64,
    },
    /// `PARABOLA`.
    Parabola { position: Id, focal_dist: f64 },
    /// `HYPERBOLA`.
    Hyperbola {
        position: Id,
        semi_axis: f64,
        semi_imag_axis: f64,
    },
}

impl Curve {
    pub fn kind(&self) -> &'static str {
        match self {
            Curve::Line { .. } => "LINE",
            Curve::Circle { .. } => "CIRCLE",
            Curve::Ellipse { .. } => "ELLIPSE",
            Curve::Parabola { .. } => "PARABOLA",
            Curve::Hyperbola { .. } => "HYPERBOLA",
        }
    }
}

/// A simple record of a curve entity this crate reads, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Curve, ImportError>> {
    let curve = match name {
        "LINE" => line(id, params),
        "CIRCLE" => conic(id, params, 1).map(|(position, r)| Curve::Circle {
            position,
            radius: r[0],
        }),
        "ELLIPSE" => conic(id, params, 2).map(|(position, r)| Curve::Ellipse {
            position,
            semi_axis_1: r[0],
            semi_axis_2: r[1],
        }),
        "PARABOLA" => parabola(id, params),
        "HYPERBOLA" => conic(id, params, 2).map(|(position, r)| Curve::Hyperbola {
            position,
            semi_axis: r[0],
            semi_imag_axis: r[1],
        }),
        _ => return None,
    };
    Some(curve)
}

/// `LINE(name, pnt, dir)`.
fn line(id: Id, params: &[Param]) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    Ok(Curve::Line {
        point: a.id()?,
        vector: a.id()?,
    })
}

/// A conic with `n` positive lengths after its placement.
fn conic(id: Id, params: &[Param], n: usize) -> Result<(Id, [f64; 2]), ImportError> {
    let mut a = Args::new(id, params, 2 + n)?;
    a.skip();
    let position = a.id()?;
    let mut r = [0.0; 2];
    for x in r.iter_mut().take(n) {
        *x = a.real()?;
        if *x <= 0.0 {
            return Err(a.fault("expected a positive length"));
        }
    }
    Ok((position, r))
}

/// `PARABOLA(name, position, focal_dist)`: the focal distance may have
/// either sign, but not be zero.
fn parabola(id: Id, params: &[Param]) -> Result<Curve, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    let position = a.id()?;
    let focal_dist = a.real()?;
    if focal_dist == 0.0 {
        return Err(a.fault("a parabola's focal distance can't be zero"));
    }
    Ok(Curve::Parabola {
        position,
        focal_dist,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn analytic_curves() {
        let e = entities(
            "#1=LINE('',#2,#3);#4=CIRCLE('',#5,2.5);#6=ELLIPSE('',#5,8.,3.);\
             #7=PARABOLA('',#5,-0.5);#8=HYPERBOLA('',#5,1.,2.);#9=CIRCLE('',#5,0.);\
             #10=PARABOLA('',#5,0.);#11=ELLIPSE('',#5,8.);",
        );
        assert_eq!(
            e.curve(0, 1),
            Ok(&Curve::Line {
                point: 2,
                vector: 3
            })
        );
        assert_eq!(
            e.curve(0, 4),
            Ok(&Curve::Circle {
                position: 5,
                radius: 2.5
            })
        );
        assert_eq!(
            e.curve(0, 6),
            Ok(&Curve::Ellipse {
                position: 5,
                semi_axis_1: 8.0,
                semi_axis_2: 3.0
            })
        );
        assert_eq!(e.curve(0, 7).unwrap().kind(), "PARABOLA");
        assert_eq!(e.curve(0, 8).unwrap().kind(), "HYPERBOLA");
        assert!(matches!(
            e.curve(0, 9),
            Err(ImportError::Malformed {
                what: "expected a positive length",
                ..
            })
        ));
        assert!(e.curve(0, 10).is_err());
        assert!(e.curve(0, 11).is_err());
        assert_eq!(
            e.curve(0, 2),
            Err(ImportError::DanglingReference {
                instance: 0,
                reference: 2
            })
        );
    }
}
