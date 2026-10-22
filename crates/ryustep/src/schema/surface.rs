//! Surfaces (ISO 10303-42), as written: lengths in the file's length
//! unit, angles in its plane angle unit.

use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// A surface.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Surface {
    /// `PLANE`: its placement's `xy` plane.
    Plane { position: Id },
    /// `CYLINDRICAL_SURFACE` about its placement's axis.
    Cylinder { position: Id, radius: f64 },
    /// `CONICAL_SURFACE`: `radius` where it crosses the placement's `xy`
    /// plane, opening by `semi_angle` towards the axis's direction.
    Cone {
        position: Id,
        radius: f64,
        semi_angle: f64,
    },
    /// `SPHERICAL_SURFACE` about its placement's origin.
    Sphere { position: Id, radius: f64 },
    /// `TOROIDAL_SURFACE`, or `DEGENERATE_TOROIDAL_SURFACE` (`minor` at
    /// least `major`; `select_outer` keeps the outer part of the
    /// self-intersecting surface).
    Torus {
        position: Id,
        major: f64,
        minor: f64,
        select_outer: Option<bool>,
    },
}

impl Surface {
    pub fn kind(&self) -> &'static str {
        match self {
            Surface::Plane { .. } => "PLANE",
            Surface::Cylinder { .. } => "CYLINDRICAL_SURFACE",
            Surface::Cone { .. } => "CONICAL_SURFACE",
            Surface::Sphere { .. } => "SPHERICAL_SURFACE",
            Surface::Torus {
                select_outer: None, ..
            } => "TOROIDAL_SURFACE",
            Surface::Torus { .. } => "DEGENERATE_TOROIDAL_SURFACE",
        }
    }
}

/// A simple record of a surface entity this crate reads, or `None`.
pub(super) fn simple(id: Id, name: &str, params: &[Param]) -> Option<Result<Surface, ImportError>> {
    let surface = match name {
        "PLANE" => plane(id, params),
        "CYLINDRICAL_SURFACE" => radii(id, params, 1).map(|(position, r)| Surface::Cylinder {
            position,
            radius: r[0],
        }),
        "CONICAL_SURFACE" => cone(id, params),
        "SPHERICAL_SURFACE" => radii(id, params, 1).map(|(position, r)| Surface::Sphere {
            position,
            radius: r[0],
        }),
        "TOROIDAL_SURFACE" => radii(id, params, 2).map(|(position, r)| Surface::Torus {
            position,
            major: r[0],
            minor: r[1],
            select_outer: None,
        }),
        "DEGENERATE_TOROIDAL_SURFACE" => degenerate_torus(id, params),
        _ => return None,
    };
    Some(surface)
}

/// `PLANE(name, position)`.
fn plane(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    a.skip();
    Ok(Surface::Plane { position: a.id()? })
}

/// A placement and `n` positive radii.
fn radii(id: Id, params: &[Param], n: usize) -> Result<(Id, [f64; 2]), ImportError> {
    let mut a = Args::new(id, params, 2 + n)?;
    a.skip();
    let position = a.id()?;
    let mut r = [0.0; 2];
    for x in r.iter_mut().take(n) {
        *x = a.real()?;
        if *x <= 0.0 {
            return Err(a.fault("expected a positive radius"));
        }
    }
    Ok((position, r))
}

/// `CONICAL_SURFACE(name, position, radius, semi_angle)`: the radius may
/// be zero (the apex at the placement), the angle not.
fn cone(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 4)?;
    a.skip();
    let position = a.id()?;
    let radius = a.real()?;
    if radius < 0.0 {
        return Err(a.fault("a cone's radius can't be negative"));
    }
    let semi_angle = a.real()?;
    if semi_angle <= 0.0 {
        return Err(a.fault("a cone's semi-angle must be positive"));
    }
    Ok(Surface::Cone {
        position,
        radius,
        semi_angle,
    })
}

/// `DEGENERATE_TOROIDAL_SURFACE(name, position, major, minor,
/// select_outer)`.
fn degenerate_torus(id: Id, params: &[Param]) -> Result<Surface, ImportError> {
    let mut a = Args::new(id, params, 5)?;
    a.skip();
    let position = a.id()?;
    let mut r = [0.0; 2];
    for x in &mut r {
        *x = a.real()?;
        if *x <= 0.0 {
            return Err(a.fault("expected a positive radius"));
        }
    }
    let select_outer = a.boolean()?;
    Ok(Surface::Torus {
        position,
        major: r[0],
        minor: r[1],
        select_outer: Some(select_outer),
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn elementary_surfaces() {
        let e = entities(
            "#1=PLANE('',#9);#2=CYLINDRICAL_SURFACE('',#9,5.);\
             #3=CONICAL_SURFACE('',#9,6.,0.418879);#4=SPHERICAL_SURFACE('',#9,7.);\
             #5=TOROIDAL_SURFACE('',#9,10.,3.);#6=DEGENERATE_TOROIDAL_SURFACE('',#9,2.,3.,.T.);\
             #7=CONICAL_SURFACE('',#9,0.,-0.1);#8=SPHERICAL_SURFACE('',#9,-1.);",
        );
        assert_eq!(e.surface(0, 1), Ok(&Surface::Plane { position: 9 }));
        assert_eq!(
            e.surface(0, 3),
            Ok(&Surface::Cone {
                position: 9,
                radius: 6.0,
                semi_angle: 0.418879
            })
        );
        assert_eq!(
            e.surface(0, 6),
            Ok(&Surface::Torus {
                position: 9,
                major: 2.0,
                minor: 3.0,
                select_outer: Some(true)
            })
        );
        let kinds: Vec<&str> = (1..=6).map(|k| e.surface(0, k).unwrap().kind()).collect();
        assert_eq!(
            kinds,
            [
                "PLANE",
                "CYLINDRICAL_SURFACE",
                "CONICAL_SURFACE",
                "SPHERICAL_SURFACE",
                "TOROIDAL_SURFACE",
                "DEGENERATE_TOROIDAL_SURFACE"
            ]
        );
        assert!(matches!(
            e.surface(0, 7),
            Err(ImportError::Malformed {
                attribute: 3,
                what: "a cone's semi-angle must be positive",
                ..
            })
        ));
        assert!(e.surface(0, 8).is_err());
        assert_eq!(
            e.surface(0, 9),
            Err(ImportError::DanglingReference {
                instance: 0,
                reference: 9
            })
        );
    }
}
