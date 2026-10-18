//! Swept surfaces that keep their generating curve: an extrusion moves a
//! planar curve along a direction.

use super::bound2::SLACK;
use super::{
    Curve, NurbsCurve, Point3, Projection, Reparam, Rigid, SurfaceBound, SurfaceEval, UnitVector3,
    Vector3,
};
use crate::Error;
use crate::math;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};

/// `S(u, v) = C(u) + v d`: a planar curve `C` moved along a unit direction
/// `d` that is not in the curve's plane.
///
/// `u` is the curve's parameter (periodic when the curve is) and `v` the
/// distance along `d`, unbounded like a cylinder's height. The natural
/// normal is along `C'(u) × d`, so a circle extruded along its frame's
/// `z` has a cylinder's outward normal.
///
/// Projection reduces to the curve: the best `v` for each `u` is
/// `(p - C(u)) · d`, so projecting onto the surface is projecting onto the
/// curve as seen along `d`. That view is built once, at construction; it
/// takes no part in equality.
#[derive(Clone, Debug)]
pub struct Extrusion {
    curve: Curve,
    dir: UnitVector3,
    normal: UnitVector3,
    /// `|d · n|`: the sine of the angle between `d` and the curve's plane.
    lean: f64,
    flat: Flat,
}

/// The curve as seen along the direction, for projection.
#[derive(Clone, Debug)]
enum Flat {
    /// The direction is the plane's normal, bit for bit: a point moved
    /// along it into the plane through `origin` projects onto the curve
    /// itself.
    Along { origin: Point3 },
    /// The curve's exact NURBS form with every control point moved along
    /// the direction onto the plane through `origin` perpendicular to it,
    /// and stored relative to `origin`; `reparam` relates its parameter to
    /// the curve's.
    Nurbs {
        origin: Point3,
        curve: NurbsCurve,
        reparam: Reparam,
    },
}

impl PartialEq for Extrusion {
    fn eq(&self, other: &Self) -> bool {
        (&self.curve, self.dir) == (&other.curve, other.dir)
    }
}

/// `v / |v|`, for a vector known not to vanish whatever its units (an
/// area, a cross product of unit vectors); `None` if it does.
fn direction(v: Vector3) -> Option<UnitVector3> {
    let len = v.length();
    (len > 0.0 && len.is_finite()).then(|| {
        let u = v / len;
        // Rounding can leave a few ulps; renormalizing once more is exact
        // enough for `new_unchecked`.
        UnitVector3::new_unchecked(u / u.length())
    })
}

/// The mean of some points.
fn centroid(points: &[Point3]) -> Point3 {
    let sum = points.iter().fold(Vector3::ZERO, |s, &p| s + p.to_vector());
    Point3::ORIGIN + sum / points.len() as f64
}

/// The plane of a NURBS's control points, as its unit normal and the
/// control points' centroid.
///
/// Two candidate normals: Newell's, over the closed control polygon, and
/// the cross product of the point farthest from the centroid with the
/// point farthest from the line through both. Newell's fits best when the
/// polygon encloses area; the second survives polygons whose signed area
/// cancels (an S-shaped curve). The candidate with the smaller misfit wins
/// (Newell's on a tie). Fails with `Degenerate` when every control point
/// is within [`LINEAR_RESOLUTION`] of one line, or some control point is
/// farther than that from the chosen plane.
fn nurbs_plane(n: &NurbsCurve) -> Result<(UnitVector3, Point3), Error> {
    let points: Vec<Point3> = n.control_points().collect();
    let o = centroid(&points);
    let rel: Vec<Vector3> = points.iter().map(|&p| p - o).collect();
    let far = |from: &dyn Fn(Vector3) -> f64| {
        rel.iter()
            .copied()
            .fold((Vector3::ZERO, 0.0), |(best, d), r| {
                let dr = from(r);
                if dr > d { (r, dr) } else { (best, d) }
            })
    };
    let (a, reach) = far(&|r: Vector3| r.length());
    if reach <= LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    let axis = a / reach;
    let (b, off) = far(&|r: Vector3| (r - axis * r.dot(axis)).length());
    if off <= LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    let mut newell = Vector3::ZERO;
    for (i, &p) in rel.iter().enumerate() {
        let q = rel[(i + 1) % rel.len()];
        newell += Vector3::new(
            (p.y - q.y) * (p.z + q.z),
            (p.z - q.z) * (p.x + q.x),
            (p.x - q.x) * (p.y + q.y),
        );
    }
    let misfit = |n: UnitVector3| rel.iter().map(|r| r.dot(n.get()).abs()).fold(0.0, f64::max);
    let best = [direction(newell), direction(a.cross(b))]
        .into_iter()
        .flatten()
        .map(|n| (n, misfit(n)))
        .reduce(|x, y| if y.1 < x.1 { y } else { x })
        .ok_or(Error::Degenerate)?;
    if best.1 > LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    Ok((best.0, o))
}

/// The unit normal of a curve's plane and a point of that plane. Fails
/// with `Degenerate` for a line, or a NURBS that is straight or not
/// planar (see [`nurbs_plane`]).
fn curve_plane(curve: &Curve) -> Result<(UnitVector3, Point3), Error> {
    match curve {
        Curve::Line(_) => Err(Error::Degenerate),
        Curve::Circle(c) => Ok((c.frame().z(), c.frame().origin())),
        Curve::Ellipse(e) => Ok((e.frame().z(), e.frame().origin())),
        Curve::Nurbs(n) => nurbs_plane(n),
    }
}

/// `r` with its component along `d` removed.
fn across(r: Vector3, d: Vector3) -> Vector3 {
    r - d * r.dot(d)
}

fn same_bits(a: UnitVector3, b: UnitVector3) -> bool {
    let (a, b) = (a.get(), b.get());
    [(a.x, b.x), (a.y, b.y), (a.z, b.z)]
        .iter()
        .all(|(x, y)| x.to_bits() == y.to_bits())
}

/// The rotation of a rigid transform alone. Composing with the reverse
/// translation is exact: the identity matrix multiplies exactly, and
/// `t + -t` is zero.
fn rotation_of(r: &Rigid) -> Rigid {
    Rigid::translation(-r.translation_part()).then_after(r)
}

impl Extrusion {
    /// The curve moved along `dir`, which is normalized.
    ///
    /// Fails with `NonFinite` for a NaN or infinite direction, or
    /// `Degenerate`: for a zero direction; for a line (use a plane); for a
    /// curve that is not planar (a NURBS whose control points are within
    /// [`LINEAR_RESOLUTION`] of one line, or not all within it of one
    /// plane); and for a direction within [`ANGULAR_RESOLUTION`] of the
    /// curve's plane (`|d · n| <= sin(ANGULAR_RESOLUTION)`). Fails with
    /// `OutsideSizeBox` in the rare case that the curve seen along `d`,
    /// centred on its control points' centroid, leaves the size box.
    pub fn new(curve: Curve, dir: Vector3) -> Result<Self, Error> {
        Vector3::try_new(dir.x, dir.y, dir.z)?;
        let d = dir.normalize().ok_or(Error::Degenerate)?;
        let (normal, origin) = curve_plane(&curve)?;
        let lean = d.dot(normal).abs();
        if lean <= math::sin(ANGULAR_RESOLUTION) {
            return Err(Error::Degenerate);
        }
        let flat = if same_bits(d, normal) || same_bits(d, -normal) {
            Flat::Along { origin }
        } else {
            let (n, reparam) = curve.to_nurbs(curve.domain())?;
            let points: Vec<Point3> = n.control_points().collect();
            let origin = centroid(&points);
            let flat = points
                .iter()
                .map(|&q| Point3::ORIGIN + across(q - origin, d.get()))
                .collect();
            Flat::Nurbs {
                origin,
                curve: NurbsCurve::new(n.knots().clone(), flat, Some(n.weights().to_vec()))?,
                reparam,
            }
        };
        Ok(Self {
            curve,
            dir: d,
            normal,
            lean,
            flat,
        })
    }

    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    pub fn dir(&self) -> UnitVector3 {
        self.dir
    }

    /// The unit normal of the curve's plane (for a circle or an ellipse,
    /// its frame's `z`).
    pub fn normal(&self) -> UnitVector3 {
        self.normal
    }

    pub(super) fn point(&self, u: f64, v: f64) -> Point3 {
        self.curve.point(u) + self.dir.get() * v
    }

    pub(super) fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let c = self.curve.eval(u);
        let d = self.dir.get();
        SurfaceEval {
            point: c.point + d * v,
            du: c.d1,
            dv: d,
            duu: c.d2,
            duv: Vector3::ZERO,
            dvv: Vector3::ZERO,
        }
    }

    /// The direction of `T × d`, with `T` the curve's unit tangent; `None`
    /// where that vanishes. `|T × d| >= |d · n| > 0` for a tangent in the
    /// plane, so it never vanishes for a nonzero tangent.
    pub(super) fn surface_normal(&self, u: f64) -> Option<UnitVector3> {
        direction(self.curve.tangent(u)?.get().cross(self.dir.get()))
    }

    pub(super) fn domain(&self) -> [[f64; 2]; 2] {
        [self.curve.domain(), [f64::NEG_INFINITY, f64::INFINITY]]
    }

    /// The nearest point: the curve seen along `d` projected on (globally,
    /// as [`Curve::project`] or [`NurbsCurve::project`] does), then the
    /// best `v` for that `u`. A periodic `u` is in `[start, start +
    /// period)`.
    pub(super) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        let d = self.dir.get();
        let u = match &self.flat {
            Flat::Along { origin } => self.curve.project(p - d * (p - *origin).dot(d)).param,
            Flat::Nurbs {
                origin,
                curve,
                reparam,
            } => {
                let s = curve.project(Point3::ORIGIN + across(p - *origin, d)).param;
                let u = reparam.from_nurbs(s);
                match self.curve.period() {
                    Some(period) if u >= self.curve.domain()[0] + period => u - period,
                    _ => u,
                }
            }
        };
        let v = (p - self.curve.point(u)).dot(d);
        let point = self.point(u, v);
        Projection {
            param: (u, v),
            point,
            distance: point.distance(p),
        }
    }

    /// `Suu = C''` and the other second derivatives vanish. The unit
    /// normal `N = T × d / |T × d|` turns at most `|T'| / |T × d|`, and
    /// `|T × d| >= |d · n|`, so it turns along `u` at most the curve's
    /// turning rate over `|d · n|`.
    pub(super) fn derivative_bound(&self, [ru, _]: [[f64; 2]; 2]) -> SurfaceBound {
        let c = self.curve.derivative_bound(ru);
        SurfaceBound {
            duu: c.d2,
            duv: 0.0,
            dvv: 0.0,
            turn_u: c.turn / self.lean * SLACK,
            turn_v: 0.0,
        }
    }

    /// The extrusion moved by a rigid transform; parameters are unchanged.
    /// The view along the direction is turned with it rather than built
    /// again, so this never fails.
    pub(super) fn transformed(&self, r: &Rigid) -> Self {
        let flat = match &self.flat {
            Flat::Along { origin } => Flat::Along {
                origin: r.apply_point(*origin),
            },
            Flat::Nurbs {
                origin,
                curve,
                reparam,
            } => Flat::Nurbs {
                origin: r.apply_point(*origin),
                curve: curve.transformed(&rotation_of(r)),
                reparam: *reparam,
            },
        };
        let (dir, normal) = (r.apply_unit(self.dir), r.apply_unit(self.normal));
        Self {
            curve: self.curve.transformed(r),
            dir,
            normal,
            lean: dir.dot(normal).abs(),
            flat,
        }
    }
}

#[cfg(test)]
mod tests;
