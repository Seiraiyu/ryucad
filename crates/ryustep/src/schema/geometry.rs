//! Points, directions, vectors and axis placements (ISO 10303-42).

use super::Entity;
use super::args::{Args, Id};
use crate::ImportError;
use crate::part21::Param;

/// One to three coordinates: a `CARTESIAN_POINT`'s or a `DIRECTION`'s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    xyz: [f64; 3],
    dim: u8,
}

impl Point {
    /// The coordinates as written: one, two or three.
    pub fn coords(&self) -> &[f64] {
        &self.xyz[..usize::from(self.dim)]
    }

    /// How many coordinates.
    pub fn dim(&self) -> usize {
        usize::from(self.dim)
    }
}

/// An axis placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// `AXIS1_PLACEMENT`: a point and an axis (`z` if unset).
    Axis1 { location: Id, axis: Option<Id> },
    /// `AXIS2_PLACEMENT_2D`: a point and an `x` direction (the first axis
    /// if unset).
    Axis2d {
        location: Id,
        ref_direction: Option<Id>,
    },
    /// `AXIS2_PLACEMENT_3D`: a point, an axis (`z` if unset) and an
    /// approximate `x` direction (`x`, made perpendicular, if unset).
    Axis3d {
        location: Id,
        axis: Option<Id>,
        ref_direction: Option<Id>,
    },
}

impl Placement {
    pub fn kind(&self) -> &'static str {
        match self {
            Placement::Axis1 { .. } => "AXIS1_PLACEMENT",
            Placement::Axis2d { .. } => "AXIS2_PLACEMENT_2D",
            Placement::Axis3d { .. } => "AXIS2_PLACEMENT_3D",
        }
    }

    pub fn location(&self) -> Id {
        match *self {
            Placement::Axis1 { location, .. }
            | Placement::Axis2d { location, .. }
            | Placement::Axis3d { location, .. } => location,
        }
    }
}

/// `CARTESIAN_POINT(name, coordinates)` or `DIRECTION(name, ratios)`.
pub(super) fn point(id: Id, params: &[Param]) -> Result<Point, ImportError> {
    let mut a = Args::new(id, params, 2)?;
    a.skip();
    let c = a.reals()?;
    if c.is_empty() || c.len() > 3 {
        return Err(a.fault("expected 1 to 3 coordinates"));
    }
    if c.iter().any(|x| !x.is_finite()) {
        return Err(a.fault("expected finite coordinates"));
    }
    let mut xyz = [0.0; 3];
    xyz[..c.len()].copy_from_slice(&c);
    Ok(Point {
        xyz,
        dim: c.len() as u8,
    })
}

/// `VECTOR(name, orientation, magnitude)`.
pub(super) fn vector(id: Id, params: &[Param]) -> Result<Entity, ImportError> {
    let mut a = Args::new(id, params, 3)?;
    a.skip();
    let orientation = a.id()?;
    let magnitude = a.real()?;
    if magnitude < 0.0 {
        return Err(a.fault("a vector's magnitude can't be negative"));
    }
    Ok(Entity::Vector {
        orientation,
        magnitude,
    })
}

pub(super) fn placement(id: Id, name: &str, params: &[Param]) -> Result<Placement, ImportError> {
    match name {
        "AXIS1_PLACEMENT" => {
            let mut a = Args::new(id, params, 3)?;
            a.skip();
            Ok(Placement::Axis1 {
                location: a.id()?,
                axis: a.opt_id()?,
            })
        }
        "AXIS2_PLACEMENT_2D" => {
            let mut a = Args::new(id, params, 3)?;
            a.skip();
            Ok(Placement::Axis2d {
                location: a.id()?,
                ref_direction: a.opt_id()?,
            })
        }
        _ => {
            let mut a = Args::new(id, params, 4)?;
            a.skip();
            Ok(Placement::Axis3d {
                location: a.id()?,
                axis: a.opt_id()?,
                ref_direction: a.opt_id()?,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::entities;
    use super::*;

    #[test]
    fn points_and_vectors() {
        let e = entities(
            "#1=CARTESIAN_POINT('p',(1.5,-2));#2=CARTESIAN_POINT('',());\
             #3=DIRECTION('',(0.,0.,1.));#4=VECTOR('',#3,25.4);#5=VECTOR('',#3,-1.);\
             #6=CARTESIAN_POINT('',(1.,$));#7=CARTESIAN_POINT('');",
        );
        let p = e.point(0, 1).unwrap();
        assert_eq!(p.coords(), &[1.5, -2.0]);
        assert_eq!(p.dim(), 2);
        assert!(matches!(
            e.get(0, 2),
            Err(ImportError::Malformed { attribute: 1, .. })
        ));
        assert_eq!(
            e.get(0, 4),
            Ok(&Entity::Vector {
                orientation: 3,
                magnitude: 25.4
            })
        );
        assert!(matches!(
            e.get(0, 5),
            Err(ImportError::Malformed {
                what: "a vector's magnitude can't be negative",
                ..
            })
        ));
        assert!(matches!(
            e.get(0, 6),
            Err(ImportError::Malformed {
                what: "expected a list of reals",
                ..
            })
        ));
        assert!(matches!(
            e.get(0, 7),
            Err(ImportError::Malformed {
                what: "wrong number of attributes",
                ..
            })
        ));
    }

    #[test]
    fn placements() {
        let e = entities(
            "#1=AXIS2_PLACEMENT_3D('',#2,#3,$);#4=AXIS1_PLACEMENT('',#2,$);\
             #5=AXIS2_PLACEMENT_2D('',#2,#3);#6=AXIS2_PLACEMENT_3D('',$,#3,#3);",
        );
        assert_eq!(
            e.placement(0, 1),
            Ok(&Placement::Axis3d {
                location: 2,
                axis: Some(3),
                ref_direction: None
            })
        );
        assert_eq!(
            e.placement(0, 4),
            Ok(&Placement::Axis1 {
                location: 2,
                axis: None
            })
        );
        assert_eq!(e.placement(0, 5).unwrap().kind(), "AXIS2_PLACEMENT_2D");
        assert_eq!(e.placement(0, 5).unwrap().location(), 2);
        assert!(e.placement(0, 6).is_err());
    }
}
