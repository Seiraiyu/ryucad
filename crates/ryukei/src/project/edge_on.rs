//! Traced faces seen edge-on (design 326): their image is their boundary.

use crate::geom::sheet::Sheet;
use crate::geom::{Surface, Vector3};
use crate::intersect::Failure;
use crate::interval::Interval;
use crate::tol::ANGULAR_RESOLUTION;
use crate::work::Meter;

/// Cells per direction of the first grid.
const GRID: usize = 8;

/// How often a cell is halved before it counts as not edge-on.
const DEPTH: u32 = 4;

/// Whether `surface` is certified edge-on over `rect` seen along `t`.
pub(super) fn edge_on(
    surface: &Surface,
    t: Vector3,
    rect: [[f64; 2]; 2],
    m: &Meter,
) -> Result<bool, Failure> {
    if let Surface::Extrusion(e) = surface
        && e.dir().get().cross(t).length() <= ANGULAR_RESOLUTION
    {
        return Ok(true);
    }
    if !rect.iter().flatten().all(|x| x.is_finite()) {
        return Ok(false);
    }
    let sheet = Sheet::of(surface);
    let cut = |r: [f64; 2], i: usize| r[0] + (r[1] - r[0]) * i as f64 / GRID as f64;
    for i in 0..GRID {
        for j in 0..GRID {
            let cell = [
                [cut(rect[0], i), cut(rect[0], i + 1)],
                [cut(rect[1], j), cut(rect[1], j + 1)],
            ];
            if !cell_edge_on(&sheet, t, cell, DEPTH, m)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn cell_edge_on(
    sheet: &Sheet,
    t: Vector3,
    cell: [[f64; 2]; 2],
    depth: u32,
    m: &Meter,
) -> Result<bool, Failure> {
    // A cell of a face judged edge-on.
    m.charge(1)?;
    let Some(n) = sheet.normal_jet_box(cell) else {
        return Ok(false);
    };
    let i = Interval::point;
    let f = n[0].v * i(t.x) + n[1].v * i(t.y) + n[2].v * i(t.z);
    if f.lo() > 0.0 || f.hi() < 0.0 {
        return Ok(false);
    }
    let low = n
        .iter()
        .map(|c| {
            let v = c.v;
            if v.lo() > 0.0 || v.hi() < 0.0 {
                v.lo().abs().min(v.hi().abs())
            } else {
                0.0
            }
        })
        .fold(0.0, f64::max);
    if low > 0.0 && f.mag() <= ANGULAR_RESOLUTION * low {
        return Ok(true);
    }
    if depth == 0 {
        return Ok(false);
    }
    let [[u0, u1], [v0, v1]] = cell;
    let (um, vm) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
    for u in [[u0, um], [um, u1]] {
        for v in [[v0, vm], [vm, v1]] {
            if !cell_edge_on(sheet, t, [u, v], depth - 1, m)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Curve, Extrusion, Frame, Surface};

    fn tube() -> Surface {
        let c = Curve::Circle(Circle::new(Frame::WORLD, 0.3).unwrap());
        Surface::Extrusion(std::sync::Arc::new(
            Extrusion::new(c, Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        ))
    }

    #[test]
    fn an_extrusion_along_the_view_is_edge_on_and_across_it_is_not() {
        let r = [[0.0, core::f64::consts::TAU], [0.0, 1.0]];
        // unmetered: a unit test
        let m = Meter::free();
        assert!(edge_on(&tube(), Vector3::new(0.0, 0.0, 1.0), r, &m).unwrap());
        assert!(!edge_on(&tube(), Vector3::new(0.0, 1.0, 0.0), r, &m).unwrap());
    }

    #[test]
    fn a_spline_cylinder_along_the_view_is_edge_on() {
        let s =
            crate::geom::Surface::Cylinder(crate::geom::Cylinder::new(Frame::WORLD, 0.3).unwrap());
        let (n, _) = s
            .to_nurbs([[0.0, core::f64::consts::TAU], [0.0, 1.0]])
            .unwrap();
        let spline = Surface::Nurbs(std::sync::Arc::new(n));
        // unmetered: a unit test
        let m = Meter::free();
        let r = spline.domain();
        assert!(edge_on(&spline, Vector3::new(0.0, 0.0, 1.0), r, &m).unwrap());
    }
}
