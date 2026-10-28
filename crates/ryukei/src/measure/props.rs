//! What a single entity is: its point, length or area, and the shape of
//! its geometry.

use super::{Entity, Value, length::length};
use crate::Error;
use crate::MeasureError;
use crate::geom::{Curve, Point3, UnitVector3};
use crate::topo::{Body, EdgeId, VertexId};

/// The relative accuracy entity properties aim at.
pub const DEFAULT_TOLERANCE: f64 = 1e-9;

/// An edge's curve, where it has a shape worth naming.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeShape {
    /// Along a line, `direction` from its start to its end.
    Line { direction: UnitVector3 },
    /// Along a circle: `angle` is the arc it spans (2π closed), run
    /// counterclockwise about `axis` from start to end.
    Arc {
        centre: Point3,
        axis: UnitVector3,
        radius: f64,
        angle: f64,
    },
    /// Along an ellipse: `major_axis` is the direction of the major
    /// semi-axis `major`.
    Ellipse {
        centre: Point3,
        axis: UnitVector3,
        major_axis: UnitVector3,
        major: f64,
        minor: f64,
        angle: f64,
    },
    /// A spline: no shape but its length.
    Other,
}

/// A face's surface, where it has a shape worth naming.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaceShape {
    /// A plane: `normal` points out of the solid, `origin` is a point of
    /// the plane.
    Plane {
        origin: Point3,
        normal: UnitVector3,
    },
    /// A cylinder about the line through `origin` along `axis`.
    Cylinder {
        origin: Point3,
        axis: UnitVector3,
        radius: f64,
    },
    /// A cone with its apex at `apex`, widening along `axis` at
    /// `half_angle`.
    Cone {
        apex: Point3,
        axis: UnitVector3,
        half_angle: f64,
    },
    Sphere {
        centre: Point3,
        radius: f64,
    },
    /// A torus about `axis` through `centre`.
    Torus {
        centre: Point3,
        axis: UnitVector3,
        major: f64,
        minor: f64,
    },
    /// A spline, swept surface or pipe: no shape but its area.
    Other,
}

/// What [`properties`] gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Properties {
    Vertex {
        point: Point3,
    },
    Edge {
        length: Value,
        start: Point3,
        end: Point3,
        shape: EdgeShape,
    },
    Face {
        area: Value,
        shape: FaceShape,
    },
    /// A whole body's volume and area, as [`super::mass`] gives them.
    Body {
        volume: Value,
        area: Value,
    },
}

/// The properties of one entity of `body`. Fails with `NotAnEntity` for
/// a dead ID, `Uncertain` where a length or area can't be bounded, and
/// `Unsupported` for a face that doesn't mesh.
pub fn properties(body: &Body, entity: Entity) -> Result<Properties, Error> {
    match entity {
        Entity::Vertex(v) => vertex(body, v),
        Entity::Edge(e) => edge(body, e),
        Entity::Face(f) => super::mass::face_properties(body, f, DEFAULT_TOLERANCE),
        Entity::Body => {
            let m = super::mass::mass(body, 1.0, &super::MassOptions::default())?;
            Ok(Properties::Body {
                volume: m.volume,
                area: m.area,
            })
        }
    }
}

fn vertex(body: &Body, v: VertexId) -> Result<Properties, Error> {
    let v = body.vertex(v).ok_or(MeasureError::NotAnEntity)?;
    Ok(Properties::Vertex { point: v.point })
}

fn edge(body: &Body, e: EdgeId) -> Result<Properties, Error> {
    let edge = body.edge(e).ok_or(MeasureError::NotAnEntity)?;
    let curve = body.curve(edge.curve).ok_or(MeasureError::NotAnEntity)?;
    let point = |v| {
        body.vertex(v)
            .map(|v| v.point)
            .ok_or(MeasureError::NotAnEntity)
    };
    let (start, end) = (point(edge.start)?, point(edge.end)?);
    let [t0, t1] = edge.interval;
    let length = length(curve, edge.interval, DEFAULT_TOLERANCE).ok_or(MeasureError::Uncertain)?;
    let shape = match curve {
        Curve::Line(l) => EdgeShape::Line { direction: l.dir() },
        Curve::Circle(c) => EdgeShape::Arc {
            centre: c.frame().origin(),
            axis: c.frame().z(),
            radius: c.radius(),
            angle: t1 - t0,
        },
        Curve::Ellipse(el) => EdgeShape::Ellipse {
            centre: el.frame().origin(),
            axis: el.frame().z(),
            major_axis: el.frame().x(),
            major: el.major(),
            minor: el.minor(),
            angle: t1 - t0,
        },
        _ => EdgeShape::Other,
    };
    Ok(Properties::Edge {
        length,
        start,
        end,
        shape,
    })
}
