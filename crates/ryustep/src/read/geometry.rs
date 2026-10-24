//! STEP's curves and surfaces as ryukei's, in meters and radians.
//!
//! Each entity becomes the ryukei type that is exact for it: placements
//! frames (kept bit for bit where their axes are already unit and
//! perpendicular, as files ryukei writes have them), lines, circles,
//! ellipses, planes, cylinders, cones, spheres and tori; B-splines of
//! every knot form as NURBS (see [`super::spline`]); a surface of linear
//! extrusion of a planar curve an extrusion, and of revolution of a curve
//! in a half-plane through its axis a revolution. A trimmed curve, a
//! rectangular trimmed surface, a curve on a surface and a composite of
//! one segment are their basis: an edge's or a face's own bounds trim it.
//! A polyline is a degree 1 spline on its length. An offset of a plane,
//! cylinder, cone, sphere or torus, or of a line or circle, is exact;
//! any other offset is [`ImportError::NotBounded`].
//!
//! Lengths are multiplied by the context's meters per length unit and
//! angles by its radians per angle unit, once each, when an entity is
//! made.

use super::spline::Raw;
use crate::ImportError;
use crate::schema::{
    BSplineCurve, BSplineSurface, Curve as SCurve, Entities, Entity, Id, Knots, Placement,
    Surface as SSurface,
};
use ryukei::geom::{
    Circle, Circle2, Cone, Curve, Curve2, Cylinder, Ellipse, Ellipse2, Extrusion, Frame, Frame2,
    KnotVector, Line, Line2, NurbsCurve, NurbsCurve2, NurbsSurface, Plane, Point2, Point3,
    Revolution, Sphere, Surface, Torus, UnitVector3, Vector2, Vector3,
};
use std::sync::Arc;

/// Converts geometry of one representation context.
pub(crate) struct Geo<'a> {
    pub(crate) e: &'a Entities,
    /// Meters per length unit.
    pub(crate) length: f64,
    /// Radians per angle unit.
    pub(crate) angle: f64,
}

/// A kernel refusal of `instance`'s geometry.
pub(crate) fn invalid(instance: Id, error: ryukei::Error) -> ImportError {
    match error {
        ryukei::Error::OutsideSizeBox => ImportError::OutsideSizeBox { instance },
        error => ImportError::InvalidGeometry { instance, error },
    }
}

fn unsupported(e: &Entities, id: Id) -> ImportError {
    let name = e.get(id, id).map_or("?", |x| x.kind()).into();
    ImportError::UnsupportedEntity { instance: id, name }
}

impl Geo<'_> {
    /// A `CARTESIAN_POINT` in three dimensions, in meters.
    pub(crate) fn point(&self, id: Id) -> Result<Point3, ImportError> {
        let c = self.e.point(id, id)?.coords();
        if c.len() != 3 {
            return Err(ImportError::WrongEntity {
                instance: id,
                expected: "a point in three dimensions",
            });
        }
        let l = self.length;
        Point3::try_new(c[0] * l, c[1] * l, c[2] * l).map_err(|e| invalid(id, e))
    }

    /// A `DIRECTION` in three dimensions, as written.
    fn vector(&self, id: Id) -> Result<Vector3, ImportError> {
        let c = self.e.direction(id, id)?.coords();
        if c.len() != 3 {
            return Err(ImportError::WrongEntity {
                instance: id,
                expected: "a direction in three dimensions",
            });
        }
        Ok(Vector3::new(c[0], c[1], c[2]))
    }

    /// A direction of unit length: kept bit for bit when it already is,
    /// else normalized.
    fn unit(&self, id: Id) -> Result<UnitVector3, ImportError> {
        let v = self.vector(id)?;
        match UnitVector3::exact(v) {
            Some(u) => Ok(u),
            None => v
                .normalize()
                .ok_or_else(|| invalid(id, ryukei::Error::Degenerate)),
        }
    }

    /// An `AXIS2_PLACEMENT_3D` as a frame. Missing axes are `z` and `x`.
    pub(crate) fn frame(&self, id: Id) -> Result<Frame, ImportError> {
        let Placement::Axis3d {
            location,
            axis,
            ref_direction,
        } = *self.e.placement(id, id)?
        else {
            return Err(crate::schema::wrong(id, "an AXIS2_PLACEMENT_3D"));
        };
        let origin = self.point(location)?;
        let z = match axis {
            Some(a) => self.unit(a)?,
            None => UnitVector3::Z,
        };
        let x = match ref_direction {
            Some(r) => self.unit(r)?,
            None => UnitVector3::X,
        };
        Frame::from_axes(origin, z, x)
            .or_else(|_| Frame::new(origin, z.get(), x.get()))
            .map_err(|e| invalid(id, e))
    }

    /// An `AXIS1_PLACEMENT`: its point and direction.
    fn axis(&self, id: Id) -> Result<(Point3, UnitVector3), ImportError> {
        let Placement::Axis1 { location, axis } = *self.e.placement(id, id)? else {
            return Err(crate::schema::wrong(id, "an AXIS1_PLACEMENT"));
        };
        let dir = match axis {
            Some(a) => self.unit(a)?,
            None => UnitVector3::Z,
        };
        Ok((self.point(location)?, dir))
    }

    /// A length, in meters.
    fn len(&self, x: f64) -> f64 {
        x * self.length
    }

    /// A curve in three dimensions.
    pub(crate) fn curve(&self, id: Id) -> Result<Curve, ImportError> {
        let fail = |e| invalid(id, e);
        match self.e.curve(id, id)? {
            SCurve::Line { point, vector } => {
                let p = self.point(*point)?;
                let Entity::Vector { orientation, .. } = self.e.get(id, *vector)? else {
                    return Err(crate::schema::wrong(*vector, "a VECTOR"));
                };
                let d = self.unit(*orientation)?;
                Line::from_unit(p, d).map(Curve::Line).map_err(fail)
            }
            SCurve::Circle { position, radius } => {
                Circle::new(self.frame(*position)?, self.len(*radius))
                    .map(Curve::Circle)
                    .map_err(fail)
            }
            SCurve::Ellipse {
                position,
                semi_axis_1,
                semi_axis_2,
            } => {
                let f = self.frame(*position)?;
                let (a, b) = (self.len(*semi_axis_1), self.len(*semi_axis_2));
                // ryukei's major axis is its frame's x: turn the frame a
                // quarter where the file's first axis is the shorter.
                let (f, a, b) = if a >= b {
                    (f, a, b)
                } else {
                    let turned = Frame::new(f.origin(), f.z().get(), f.y().get()).map_err(fail)?;
                    (turned, b, a)
                };
                Ellipse::new(f, a, b).map(Curve::Ellipse).map_err(fail)
            }
            SCurve::BSpline(b) => self.nurbs_curve(id, b).map(|n| Curve::Nurbs(Arc::new(n))),
            SCurve::Trimmed { basis, .. } => self.curve(*basis),
            SCurve::OnSurface { curve, .. } => self.curve(*curve),
            SCurve::Composite { segments } if segments.len() == 1 => {
                let (_, parent) = self.e.segment(id, segments[0])?;
                self.curve(parent)
            }
            SCurve::Polyline { points } => self.polyline(id, points),
            SCurve::Offset {
                basis,
                distance,
                ref_direction: Some(r),
            } => self.offset_curve(id, *basis, self.len(*distance), *r),
            SCurve::Offset { .. } => Err(ImportError::NotBounded { instance: id }),
            _ => Err(unsupported(self.e, id)),
        }
    }

    /// A polyline as a degree 1 spline on its length.
    fn polyline(&self, id: Id, points: &[Id]) -> Result<Curve, ImportError> {
        let pts: Vec<Point3> = points
            .iter()
            .map(|&p| self.point(p))
            .collect::<Result<_, _>>()?;
        let mut knots = vec![0.0];
        for w in pts.windows(2) {
            let d = w[0].distance(w[1]);
            knots.push(knots.last().expect("a knot") + d);
        }
        let mut mults = vec![1; knots.len()];
        mults[0] = 2;
        *mults.last_mut().expect("a knot") = 2;
        let kv = KnotVector::clamped(1, &knots, &mults).map_err(|e| invalid(id, e))?;
        NurbsCurve::new(kv, pts, None)
            .map(|n| Curve::Nurbs(Arc::new(n)))
            .map_err(|e| invalid(id, e))
    }

    /// `OFFSET_CURVE_3D`: the curve moved `d` along `T × ref`, exact for
    /// a line and for a circle about `ref` (or against it).
    fn offset_curve(&self, id: Id, basis: Id, d: f64, r: Id) -> Result<Curve, ImportError> {
        let fail = |e| invalid(id, e);
        let r = self.unit(r)?;
        match self.curve(basis)? {
            Curve::Line(l) => {
                let side = l.dir().get().cross(r.get());
                let side = side
                    .normalize()
                    .ok_or(ImportError::NotBounded { instance: id })?;
                Line::from_unit(l.origin() + side.get() * d, l.dir())
                    .map(Curve::Line)
                    .map_err(fail)
            }
            Curve::Circle(c) => {
                // T × axis is the outward radial direction.
                let along = c.frame().z().dot(r);
                if (along.abs() - 1.0).abs() > 1e-12 {
                    return Err(ImportError::NotBounded { instance: id });
                }
                Circle::new(*c.frame(), c.radius() + d * along.signum())
                    .map(Curve::Circle)
                    .map_err(fail)
            }
            _ => Err(ImportError::NotBounded { instance: id }),
        }
    }

    /// A spline's knots, flat, from the way the file gives them.
    fn flat(
        &self,
        id: Id,
        knots: &Knots,
        degree: usize,
        count: usize,
    ) -> Result<Vec<f64>, ImportError> {
        let n = count;
        let p = degree;
        Ok(match knots {
            Knots::Explicit {
                multiplicities,
                values,
                ..
            } => {
                if multiplicities.len() != values.len() {
                    return Err(ImportError::Malformed {
                        instance: id,
                        attribute: 0,
                        what: "knots and multiplicities of different lengths",
                    });
                }
                values
                    .iter()
                    .zip(multiplicities)
                    .flat_map(|(&k, &m)| core::iter::repeat_n(k, m as usize))
                    .collect()
            }
            Knots::Bezier => (0..n + p + 1)
                .map(|i| if i <= p { 0.0 } else { 1.0 })
                .collect(),
            Knots::Uniform => (0..n + p + 1).map(|i| i as f64 - p as f64).collect(),
            Knots::QuasiUniform => (0..n + p + 1)
                .map(|i| i.saturating_sub(p).min(n - p) as f64)
                .collect(),
        })
    }

    /// Control points with their weights, lengths in meters.
    fn weighted(
        &self,
        points: &[Id],
        weights: Option<&[f64]>,
    ) -> Result<Vec<[f64; 4]>, ImportError> {
        points
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let q = self.point(p)?;
                Ok([q.x, q.y, q.z, weights.map_or(1.0, |w| w[i])])
            })
            .collect()
    }

    fn nurbs_curve(&self, id: Id, b: &BSplineCurve) -> Result<NurbsCurve, ImportError> {
        let p = b.degree as usize;
        let n = b.control_points.len();
        let flat = self.flat(id, &b.knots, p, n)?;
        let pts = self.weighted(&b.control_points, b.weights.as_deref())?;
        let made = Raw {
            degree: p,
            flat,
            points: pts.into_iter().map(|q| vec![q]).collect(),
        }
        .make()
        .map_err(|x| invalid(id, x.0))?;
        let (points, weights) = split(made.points.iter().map(|c| c[0]));
        NurbsCurve::new(made.knots, points, b.weights.is_some().then_some(weights))
            .map_err(|e| invalid(id, e))
    }

    fn nurbs_surface(&self, id: Id, b: &BSplineSurface) -> Result<NurbsSurface, ImportError> {
        let (p, q) = (b.u_degree as usize, b.v_degree as usize);
        let nu = b.control_points.len();
        let nv = b.control_points.first().map_or(0, Vec::len);
        let fu = self.flat(id, &b.u_knots, p, nu)?;
        let fv = self.flat(id, &b.v_knots, q, nv)?;
        let mut rows: Vec<Vec<[f64; 4]>> = Vec::with_capacity(nu);
        for (i, row) in b.control_points.iter().enumerate() {
            let w = b.weights.as_ref().map(|w| &w[i][..]);
            rows.push(self.weighted(row, w)?);
        }
        // Along u, each column (fixed v) a curve; then along v, each row.
        let along_u = Raw {
            degree: p,
            flat: fu,
            points: rows,
        }
        .make_closing()
        .map_err(|x| invalid(id, x.0))?;
        let mu = along_u.points.len();
        let columns: Vec<Vec<[f64; 4]>> = (0..nv)
            .map(|j| (0..mu).map(|i| along_u.points[i][j]).collect())
            .collect();
        let along_v = Raw {
            degree: q,
            flat: fv,
            points: columns,
        }
        .make_closing()
        .map_err(|x| invalid(id, x.0))?;
        let mv = along_v.points.len();
        // Row-major: u index outer.
        let (points, weights) = split(
            (0..mu)
                .flat_map(|i| (0..mv).map(move |j| (i, j)))
                .map(|(i, j)| along_v.points[j][i]),
        );
        NurbsSurface::new(
            [along_u.knots, along_v.knots],
            points,
            b.weights.is_some().then_some(weights),
        )
        .map_err(|e| invalid(id, e))
    }

    /// A surface.
    pub(crate) fn surface(&self, id: Id) -> Result<Surface, ImportError> {
        let fail = |e| invalid(id, e);
        match self.e.surface(id, id)? {
            SSurface::Plane { position } => Ok(Surface::Plane(Plane::new(self.frame(*position)?))),
            SSurface::Cylinder { position, radius } => {
                Cylinder::new(self.frame(*position)?, self.len(*radius))
                    .map(Surface::Cylinder)
                    .map_err(fail)
            }
            SSurface::Cone {
                position,
                radius,
                semi_angle,
            } => {
                let f = self.frame(*position)?;
                let alpha = semi_angle * self.angle;
                let r = self.len(*radius);
                // A cone given at its apex: its frame moved one radius'
                // worth along the axis, where it has a radius ryukei holds.
                let (f, r) = if r > 0.0 {
                    (f, r)
                } else {
                    let h = 1.0;
                    let tan = ryukei::math::tan(alpha);
                    let o = f.origin() + f.z().get() * h;
                    (Frame::from_axes(o, f.z(), f.x()).map_err(fail)?, h * tan)
                };
                Cone::new(f, r, alpha).map(Surface::Cone).map_err(fail)
            }
            SSurface::Sphere { position, radius } => {
                Sphere::new(self.frame(*position)?, self.len(*radius))
                    .map(Surface::Sphere)
                    .map_err(fail)
            }
            SSurface::Torus {
                position,
                major,
                minor,
                select_outer: None,
            } => Torus::new(self.frame(*position)?, self.len(*major), self.len(*minor))
                .map(Surface::Torus)
                .map_err(fail),
            SSurface::BSpline(b) => self
                .nurbs_surface(id, b)
                .map(|n| Surface::Nurbs(Arc::new(n))),
            SSurface::Extrusion { curve, vector } => {
                let c = self.curve(*curve)?;
                let Entity::Vector { orientation, .. } = self.e.get(id, *vector)? else {
                    return Err(crate::schema::wrong(*vector, "a VECTOR"));
                };
                let d = self.unit(*orientation)?;
                Extrusion::from_unit(c, d)
                    .map(|x| Surface::Extrusion(Arc::new(x)))
                    .map_err(fail)
            }
            SSurface::Revolution { curve, axis } => {
                let c = self.curve(*curve)?;
                let (o, z) = self.axis(*axis)?;
                // The frame's x points from the axis to the curve, so the
                // curve lies in its xz half-plane.
                let [t0, t1] = c.domain();
                let mid = if t0.is_finite() && t1.is_finite() {
                    (t0 + t1) / 2.0
                } else {
                    0.0
                };
                // The file's axis kept bit for bit: normalizing it again
                // would move it an ulp each time it is read.
                let d = c.point(mid) - o;
                let x = (d - z.get() * d.dot(z.get()))
                    .normalize()
                    .ok_or_else(|| fail(ryukei::Error::Degenerate))?;
                let frame = Frame::from_axes(o, z, x)
                    .or_else(|_| Frame::new(o, z.get(), x.get()))
                    .map_err(fail)?;
                Revolution::new(frame, c)
                    .map(|r| Surface::Revolution(Arc::new(r)))
                    .map_err(fail)
            }
            SSurface::Offset { basis, distance } => {
                match self.surface(*basis)?.offset(self.len(*distance)) {
                    Some(s) => s.map_err(fail),
                    None => Err(ImportError::NotBounded { instance: id }),
                }
            }
            SSurface::RectangularTrimmed { basis, .. } => self.surface(*basis),
            _ => Err(unsupported(self.e, id)),
        }
    }

    /// A curve in a surface's parameter plane, its axes scaled by `scale`;
    /// `None` where a circle or an ellipse would be scaled unevenly (the
    /// pcurve is then found another way).
    pub(crate) fn curve2(&self, id: Id, scale: [f64; 2]) -> Result<Option<Curve2>, ImportError> {
        let fail = |e| invalid(id, e);
        let even = scale[0] == scale[1];
        let point2 = |p: Id| -> Result<Point2, ImportError> {
            let c = self.e.point(id, p)?.coords();
            if c.len() != 2 {
                return Err(crate::schema::wrong(p, "a point in two dimensions"));
            }
            Ok(Point2::new(c[0] * scale[0], c[1] * scale[1]))
        };
        let frame2 = |pl: Id| -> Result<Frame2, ImportError> {
            let Placement::Axis2d {
                location,
                ref_direction,
            } = *self.e.placement(id, pl)?
            else {
                return Err(crate::schema::wrong(pl, "an AXIS2_PLACEMENT_2D"));
            };
            let x = match ref_direction {
                Some(r) => {
                    let c = self.e.direction(pl, r)?.coords();
                    Vector2::new(c[0], *c.get(1).unwrap_or(&0.0))
                }
                None => Vector2::new(1.0, 0.0),
            };
            Frame2::new(point2(location)?, x, true).map_err(fail)
        };
        Ok(match self.e.curve(id, id)? {
            SCurve::Line { point, vector } => {
                let Entity::Vector { orientation, .. } = self.e.get(id, *vector)? else {
                    return Err(crate::schema::wrong(*vector, "a VECTOR"));
                };
                let c = self.e.direction(*vector, *orientation)?.coords();
                let d = Vector2::new(c[0] * scale[0], *c.get(1).unwrap_or(&0.0) * scale[1]);
                Some(Curve2::Line2(Line2::new(point2(*point)?, d).map_err(fail)?))
            }
            SCurve::Circle { position, radius } if even => Some(Curve2::Circle2(
                Circle2::new(frame2(*position)?, radius * scale[0]).map_err(fail)?,
            )),
            SCurve::Ellipse {
                position,
                semi_axis_1,
                semi_axis_2,
            } if even && semi_axis_1 >= semi_axis_2 => Some(Curve2::Ellipse2(
                Ellipse2::new(
                    frame2(*position)?,
                    semi_axis_1 * scale[0],
                    semi_axis_2 * scale[0],
                )
                .map_err(fail)?,
            )),
            SCurve::BSpline(b) => {
                let p = b.degree as usize;
                let flat = self.flat(id, &b.knots, p, b.control_points.len())?;
                let pts: Vec<[f64; 4]> = b
                    .control_points
                    .iter()
                    .enumerate()
                    .map(|(i, &q)| {
                        let q = point2(q)?;
                        Ok([q.x, q.y, 0.0, b.weights.as_ref().map_or(1.0, |w| w[i])])
                    })
                    .collect::<Result<_, ImportError>>()?;
                let made = Raw {
                    degree: p,
                    flat,
                    points: pts.into_iter().map(|q| vec![q]).collect(),
                }
                .make()
                .map_err(|x| invalid(id, x.0))?;
                let pts: Vec<Point2> = made
                    .points
                    .iter()
                    .map(|c| Point2::new(c[0][0], c[0][1]))
                    .collect();
                let w: Vec<f64> = made.points.iter().map(|c| c[0][3]).collect();
                Some(Curve2::Nurbs2(Arc::new(
                    NurbsCurve2::new(made.knots, pts, b.weights.is_some().then_some(w))
                        .map_err(fail)?,
                )))
            }
            SCurve::Trimmed { basis, .. } => self.curve2(*basis, scale)?,
            _ => None,
        })
    }
}

/// Points with weights as points and weights.
fn split(h: impl Iterator<Item = [f64; 4]>) -> (Vec<Point3>, Vec<f64>) {
    h.map(|q| (Point3::new(q[0], q[1], q[2]), q[3])).unzip()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::entities;

    /// A placement at (1, 2, 3) about z, and its points and directions.
    const PLACE: &str = "#90=CARTESIAN_POINT('',(1.,2.,3.));#91=DIRECTION('',(0.,0.,1.));\
                         #92=DIRECTION('',(1.,0.,0.));#93=AXIS2_PLACEMENT_3D('',#90,#91,#92);";

    /// The geometry of `data` (with `PLACE`) in millimetres.
    fn mm(data: &str) -> Entities {
        entities(&format!("{PLACE}{data}"))
    }

    fn geo(e: &Entities) -> Geo<'_> {
        Geo {
            e,
            length: 1e-3,
            angle: 1.0,
        }
    }

    fn close(a: Point3, b: Point3) -> bool {
        a.distance(b) <= 1e-15
    }

    #[test]
    fn placements_keep_unit_axes_and_scale_lengths() {
        let e = mm("");
        let f = geo(&e).frame(93).unwrap();
        assert_eq!(f.origin(), Point3::new(1e-3, 2e-3, 3e-3));
        assert_eq!((f.z(), f.x()), (UnitVector3::Z, UnitVector3::X));
        // An axis written without unit length is normalized.
        let e = entities(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=DIRECTION('',(0.,0.,2.));\
             #3=DIRECTION('',(1.,1.,0.));#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);",
        );
        let f = geo(&e).frame(4).unwrap();
        assert_eq!(f.z(), UnitVector3::Z);
        assert!(
            (f.x().get() - Vector3::new(1.0, 1.0, 0.0).normalize().unwrap().get()).length() < 1e-15
        );
    }

    #[test]
    fn analytic_curves() {
        let e = mm(
            "#1=CIRCLE('',#93,5.);#2=ELLIPSE('',#93,2.,6.);#3=VECTOR('',#92,7.);\
             #4=LINE('',#90,#3);",
        );
        let g = geo(&e);
        let Curve::Circle(c) = g.curve(1).unwrap() else {
            panic!()
        };
        assert_eq!(c.radius(), 5e-3);
        // An ellipse whose first axis is the shorter turns a quarter.
        let Curve::Ellipse(el) = g.curve(2).unwrap() else {
            panic!()
        };
        assert_eq!((el.major(), el.minor()), (6e-3, 2e-3));
        assert!((el.frame().x().get() - Vector3::new(0.0, 1.0, 0.0)).length() < 1e-15);
        assert!(close(
            g.curve(2).unwrap().point(0.0),
            Point3::new(1e-3, 8e-3, 3e-3)
        ));
        // A line runs on its length, whatever the vector's magnitude.
        let l = g.curve(4).unwrap();
        assert!(close(l.point(1e-3), Point3::new(2e-3, 2e-3, 3e-3)));
    }

    #[test]
    fn splines_polylines_and_wrappers() {
        let e = mm(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=CARTESIAN_POINT('',(3.,4.,0.));\
             #5=CARTESIAN_POINT('',(3.,4.,12.));\
             #3=BEZIER_CURVE('',2,(#1,#2,#5),.UNSPECIFIED.,.F.,.F.);\
             #4=POLYLINE('',(#1,#2,#5));#6=TRIMMED_CURVE('',#4,(#1),(#5),.T.,.CARTESIAN.);\
             #7=OFFSET_CURVE_2D('',#3,1.,.F.);",
        );
        let g = geo(&e);
        let b = g.curve(3).unwrap();
        assert_eq!(b.domain(), [0.0, 1.0]);
        assert!(close(b.point(1.0), Point3::new(3e-3, 4e-3, 12e-3)));
        // A polyline is a degree 1 spline on its length: 5 then 12.
        let p = g.curve(4).unwrap();
        assert!((p.domain()[1] - 17e-3).abs() < 1e-15);
        assert!(close(p.point(5e-3), Point3::new(3e-3, 4e-3, 0.0)));
        // A trimmed curve is its basis.
        assert_eq!(g.curve(6).unwrap(), p);
        assert_eq!(g.curve(7), Err(ImportError::NotBounded { instance: 7 }));
    }

    #[test]
    fn analytic_surfaces_and_their_offsets() {
        let e = mm(
            "#1=CYLINDRICAL_SURFACE('',#93,5.);#2=OFFSET_SURFACE('',#1,2.,.F.);\
             #3=CONICAL_SURFACE('',#93,0.,0.5);#4=SPHERICAL_SURFACE('',#93,4.);\
             #5=TOROIDAL_SURFACE('',#93,10.,3.);#6=PLANE('',#93);\
             #7=RECTANGULAR_TRIMMED_SURFACE('',#4,0.,1.,0.,1.,.T.,.T.);",
        );
        let g = geo(&e);
        let Surface::Cylinder(c) = g.surface(2).unwrap() else {
            panic!()
        };
        assert!((c.radius() - 7e-3).abs() < 1e-18);
        // A cone given at its apex is moved along its axis, apex kept.
        let s = g.surface(3).unwrap();
        let Surface::Cone(k) = &s else { panic!() };
        assert_eq!(k.half_angle(), 0.5);
        assert!(close(
            s.point(0.3, k.apex_v()),
            Point3::new(1e-3, 2e-3, 3e-3)
        ));
        assert!(matches!(g.surface(4).unwrap(), Surface::Sphere(_)));
        assert!(matches!(g.surface(5).unwrap(), Surface::Torus(_)));
        assert!(matches!(g.surface(6).unwrap(), Surface::Plane(_)));
        assert_eq!(g.surface(7).unwrap(), g.surface(4).unwrap());
    }

    #[test]
    fn swept_surfaces() {
        let e = mm("#1=CIRCLE('',#93,5.);#2=VECTOR('',#91,1.);\
             #3=SURFACE_OF_LINEAR_EXTRUSION('',#1,#2);\
             #10=DIRECTION('',(0.,1.,0.));#11=AXIS2_PLACEMENT_3D('',#12,#10,#92);\
             #12=CARTESIAN_POINT('',(20.,2.,3.));#13=CIRCLE('',#11,4.);\
             #14=AXIS1_PLACEMENT('',#90,#91);#15=SURFACE_OF_REVOLUTION('',#13,#14);");
        let g = geo(&e);
        assert!(matches!(g.surface(3).unwrap(), Surface::Extrusion(_)));
        // A circle in the xz half-plane, turned about z: its frame's x
        // points to it.
        let Surface::Revolution(r) = g.surface(15).unwrap() else {
            panic!()
        };
        assert!((r.frame().x().get() - Vector3::new(1.0, 0.0, 0.0)).length() < 1e-12);
    }

    #[test]
    fn pcurves_scale_by_their_surfaces_units() {
        let e = entities(
            "#1=CARTESIAN_POINT('',(1.,2.));#2=DIRECTION('',(0.,1.));#3=VECTOR('',#2,3.);\
             #4=LINE('',#1,#3);#5=AXIS2_PLACEMENT_2D('',#1,#2);#6=CIRCLE('',#5,2.);",
        );
        let g = Geo {
            e: &e,
            length: 1e-3,
            angle: 1.0,
        };
        let Some(Curve2::Line2(l)) = g.curve2(4, [2.0, 1e-3]).unwrap() else {
            panic!()
        };
        assert_eq!(l.origin(), Point2::new(2.0, 2e-3));
        // A circle scaled unevenly has no circle form.
        assert_eq!(g.curve2(6, [2.0, 1e-3]).unwrap(), None);
        assert!(matches!(
            g.curve2(6, [1e-3, 1e-3]).unwrap(),
            Some(Curve2::Circle2(_))
        ));
    }
}
