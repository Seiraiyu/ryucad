//! Bodies and measures for testing meshes.

use ryukei::Error;
use ryukei::euler;
use ryukei::geom::{KnotVector, NurbsSurface, Point3, Rigid, Surface, Vector3};
use ryukei::mesh::BodyMesh;
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::{Body, Sense};
use std::sync::Arc;

/// A block `[0, dx] × [0, dy] × [0, dz]` whose top face is a bicubic
/// NURBS patch: flat along its four edges, raised by `bump` over its
/// middle (by the four inner control points), then moved by `placement`.
pub fn bump_block(
    key: OpKey,
    [dx, dy, dz]: [f64; 3],
    bump: f64,
    placement: &Rigid,
) -> Result<Body, Error> {
    let (mut b, _) = primitive::block(key, dx, dy, dz, placement)?;
    let inverse = placement.inverse();
    let top = b
        .faces()
        .find(|&f| {
            b.face_loops(f).all(|l| {
                b.loop_fins(l).all(|fin| {
                    let v = b.fin_start(fin).expect("live fin");
                    let p = inverse.apply_point(b.vertex(v).expect("live vertex").point);
                    (p.z - dz).abs() <= 1e-9 * dz.max(1.0)
                })
            })
        })
        .expect("a top face");
    let knots = || KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]);
    let mut points = Vec::with_capacity(16);
    for i in 0..4 {
        for j in 0..4 {
            let inner = (1..3).contains(&i) && (1..3).contains(&j);
            let z = dz + if inner { bump } else { 0.0 };
            let local = Point3::new(dx * f64::from(i) / 3.0, dy * f64::from(j) / 3.0, z);
            points.push(placement.apply_point(local));
        }
    }
    let s = NurbsSurface::new([knots()?, knots()?], points, None)?;
    let id = b.add_surface(Surface::Nurbs(Arc::new(s)));
    euler::set_surface(&mut b, top, id, Sense::Forward)?;
    ryukei::pcurve::fill(&mut b)?;
    Ok(b)
}

/// The volume a closed mesh encloses and its area.
pub fn volume_and_area(m: &BodyMesh) -> (f64, f64) {
    let (mut vol, mut area) = (0.0, 0.0);
    for [a, b, c] in m.welded_triangles() {
        let [a, b, c] = [a, b, c].map(|i| m.positions[i as usize].to_vector());
        vol += a.dot(b.cross(c)) / 6.0;
        area += (b - a).cross(c - a).length() / 2.0;
    }
    (vol, area)
}

/// The distance from `p` to the triangle `[a, b, c]`.
pub fn point_triangle(p: Point3, [a, b, c]: [Point3; 3]) -> f64 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let n = ab.cross(ac);
    let inside = |e: Vector3, q: Vector3| e.cross(q).dot(n) >= 0.0;
    let (bp, cp) = (p - b, p - c);
    if inside(ab, ap) && inside(c - b, bp) && inside(a - c, cp) && n.length() > 0.0 {
        return ap.dot(n).abs() / n.length();
    }
    let seg = |a: Point3, b: Point3| {
        let d = b - a;
        let t = ((p - a).dot(d) / d.length_squared()).clamp(0.0, 1.0);
        (a + d * t).distance(p)
    };
    seg(a, b).min(seg(b, c)).min(seg(c, a))
}

/// The largest deviation found between each face and its triangles, by
/// sampling every `every`-th triangle at `n + 1` points per side both
/// ways: points of the flat triangle against the surface, and points of
/// the surface at the same parameters against the flat triangle.
pub fn deviation(b: &Body, m: &BodyMesh, n: u32, every: usize) -> f64 {
    let mut worst: f64 = 0.0;
    for fm in &m.faces {
        let s = b
            .surface(b.face(fm.face).expect("live face").surface)
            .expect("live surface");
        for t in fm.triangles.clone().step_by(every) {
            let c = m.triangles[t as usize].map(|i| m.corners[i as usize]);
            let xyz = c.map(|k| m.positions[k.position as usize]);
            for i in 0..=n {
                for j in 0..=n - i {
                    let (wa, wb) = (f64::from(i) / f64::from(n), f64::from(j) / f64::from(n));
                    let wc = 1.0 - wa - wb;
                    let flat = Point3::ORIGIN
                        + xyz[0].to_vector() * wa
                        + xyz[1].to_vector() * wb
                        + xyz[2].to_vector() * wc;
                    worst = worst.max(s.distance(flat));
                    let u = c[0].uv.x * wa + c[1].uv.x * wb + c[2].uv.x * wc;
                    let v = c[0].uv.y * wa + c[1].uv.y * wb + c[2].uv.y * wc;
                    worst = worst.max(point_triangle(s.point(u, v), xyz));
                }
            }
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::check::check;

    #[test]
    fn a_bump_block_is_valid() {
        let at = Rigid::translation(Vector3::new(1.0, 2.0, 3.0));
        let b = bump_block(OpKey(1), [1.0, 2.0, 0.5], 0.3, &at).unwrap();
        assert_eq!(check(&b), []);
        let nurbs = b
            .faces()
            .filter(|&f| {
                matches!(
                    b.surface(b.face(f).unwrap().surface),
                    Some(Surface::Nurbs(_))
                )
            })
            .count();
        assert_eq!(nurbs, 1);
    }

    #[test]
    fn distances_to_a_triangle() {
        let t = [
            Point3::ORIGIN,
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ];
        assert_eq!(point_triangle(Point3::new(0.2, 0.2, 3.0), t), 3.0);
        assert_eq!(point_triangle(Point3::new(-1.0, 0.0, 0.0), t), 1.0);
        assert!((point_triangle(Point3::new(1.0, 1.0, 0.0), t) - 0.5f64.sqrt()).abs() < 1e-15);
    }
}

/// The twin of a Coilcraft SQ coil's wire end (#598): one plane face
/// whose loop runs down a half circle of radius `r` about the origin in
/// z = 0, from (0, r) through (r, 0) to (0, -r), and back up an ellipse
/// through the same two points, its major axis `a` along the diagonal
/// y = x. The ellipse lies outside the circle above the x-axis and
/// inside below it, crossing it at (r, 0): a figure-eight sliver at most
/// about `a - r` wide, whose sides cross, as the coils' do.
pub fn crossing_sliver(r: f64, a: f64) -> ryukei::heal::Shape {
    use ryukei::geom::{Circle, Curve, Ellipse, Frame, Plane};
    use ryukei::heal::{Edge, Face, Loop, Shape, Shell, Use, Vertex};
    let z = Vector3::new(0.0, 0.0, 1.0);
    let o = Point3::new(0.0, 0.0, 0.0);
    let circle = Frame::new(o, z, Vector3::new(1.0, 0.0, 0.0)).expect("a frame");
    let diagonal = Frame::new(o, z, Vector3::new(1.0, 1.0, 0.0)).expect("a frame");
    // Through (0, ±r): r² / 2a² + r² / 2b² = 1.
    let b = r * a / ryukei::math::sqrt(2.0 * a * a - r * r);
    // (0, r) lies at 45° in the diagonal frame: (a cos s, b sin s) with
    // tan s = a / b, and (0, -r) half a turn on.
    let s0 = ryukei::math::atan2(a, b);
    let vertex = |y: f64| Vertex {
        point: Point3::new(0.0, y, 0.0),
        tolerance: 0.0,
        seam: false,
    };
    let half = core::f64::consts::FRAC_PI_2;
    Shape {
        surfaces: vec![Surface::Plane(Plane::new(circle))],
        curves: vec![
            Curve::Circle(Circle::new(circle, r).expect("a circle")),
            Curve::Ellipse(Ellipse::new(diagonal, a, b).expect("an ellipse")),
        ],
        curves2: Vec::new(),
        vertices: vec![vertex(r), vertex(-r)],
        edges: vec![
            Edge {
                curve: 0,
                interval: [-half, half],
                start: 1,
                end: 0,
                tolerance: 0.0,
            },
            Edge {
                curve: 1,
                interval: [s0 - core::f64::consts::PI, s0],
                start: 1,
                end: 0,
                tolerance: 0.0,
            },
        ],
        faces: vec![Face {
            surface: 0,
            sense: Sense::Forward,
            loops: vec![Loop::Edges(vec![
                Use {
                    edge: 0,
                    sense: Sense::Reversed,
                    pcurve: None,
                },
                Use {
                    edge: 1,
                    sense: Sense::Forward,
                    pcurve: None,
                },
            ])],
            tolerance: 0.0,
        }],
        shells: vec![Shell {
            faces: vec![0],
            closed: false,
        }],
        solids: Vec::new(),
        sheets: vec![0],
    }
}
