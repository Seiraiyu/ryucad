//! Shapes whose faces cross or overlap (design 299's self-union), for
//! its tests and the `self_union` fuzz target: bodies written as files
//! describe them ([`ryukei::heal::seamed`]) and joined into one solid of
//! several shells, shells turned inside out, and polyhedra of triangles
//! whose faces may cross within one shell.
//!
//! [`crossing_case`] makes one from bytes: two or three primitives placed
//! on a coarse grid, each a shell of one solid, some turned inside out.
//! [`Crossing::winding`] is the reference the tests hold a repair to: the
//! number of parts facing out that hold a point, less those turned in.

use crate::euler_script::Bytes;
use ryukei::geom::{Curve, Frame, Line, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::heal::{Edge, Face, Loop, Shape, Shell, Solid, Use, Vertex, seamed};
use ryukei::intersect::{Inclusion, point_in_body};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::{Body, Sense};
use std::collections::BTreeMap;

/// Two shapes as one, each keeping its own shells and solids.
pub fn joined(a: &Shape, b: &Shape) -> Shape {
    let mut s = a.clone();
    let (ns, nc, nc2) = (s.surfaces.len(), s.curves.len(), s.curves2.len());
    let (nv, ne, nf, nsh) = (
        s.vertices.len(),
        s.edges.len(),
        s.faces.len(),
        s.shells.len(),
    );
    s.surfaces.extend(b.surfaces.iter().cloned());
    s.curves.extend(b.curves.iter().cloned());
    s.curves2.extend(b.curves2.iter().cloned());
    s.vertices.extend(b.vertices.iter().copied());
    s.edges.extend(b.edges.iter().map(|e| Edge {
        curve: e.curve + nc,
        start: e.start + nv,
        end: e.end + nv,
        ..*e
    }));
    s.faces.extend(b.faces.iter().map(|f| {
        Face {
            surface: f.surface + ns,
            loops: f
                .loops
                .iter()
                .map(|l| match l {
                    Loop::Edges(list) => Loop::Edges(
                        list.iter()
                            .map(|u| Use {
                                edge: u.edge + ne,
                                pcurve: u.pcurve.map(|c| c + nc2),
                                ..*u
                            })
                            .collect(),
                    ),
                    Loop::Vertex(v) => Loop::Vertex(v + nv),
                })
                .collect(),
            ..f.clone()
        }
    }));
    s.shells.extend(b.shells.iter().map(|sh| Shell {
        faces: sh.faces.iter().map(|f| f + nf).collect(),
        closed: sh.closed,
    }));
    s.solids.extend(b.solids.iter().map(|so| Solid {
        outer: so.outer + nsh,
        voids: so.voids.iter().map(|v| v + nsh).collect(),
    }));
    s
}

/// `s` with every shell in one solid: the first its outer shell, the
/// rest its cavities, whichever way they face.
pub fn one_solid(mut s: Shape) -> Shape {
    s.solids = vec![Solid {
        outer: 0,
        voids: (1..s.shells.len()).collect(),
    }];
    s
}

/// Turns every face of shell `k` over: each face's sense and loops
/// reversed, so the shell faces the other way.
pub fn turn_shell(s: &mut Shape, k: usize) {
    for &f in &s.shells[k].faces.clone() {
        let face = &mut s.faces[f];
        face.sense = face.sense.flipped();
        for l in &mut face.loops {
            if let Loop::Edges(list) = l {
                list.reverse();
                for u in list.iter_mut() {
                    u.sense = u.sense.flipped();
                }
            }
        }
    }
}

/// One closed shell of planar triangles, as a file of one solid gives
/// it: each triangle's corners counterclockwise seen from the side its
/// face faces, every edge shared by exactly two triangles running it
/// opposite ways. Faces may cross each other; healing doesn't look.
pub fn polyhedron(points: &[Point3], triangles: &[[usize; 3]]) -> Shape {
    let mut s = Shape::default();
    for &p in points {
        s.vertices.push(Vertex {
            point: p,
            tolerance: 0.0,
            seam: false,
        });
    }
    let mut edges: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for t in triangles {
        let [a, b, c] = t.map(|i| points[i]);
        let normal = (b - a).cross(c - a);
        let frame = Frame::new(a, normal, b - a).expect("a triangle with area");
        s.surfaces.push(Surface::Plane(Plane::new(frame)));
        let mut uses = Vec::new();
        for k in 0..3 {
            let (i, j) = (t[k], t[(k + 1) % 3]);
            let key = (i.min(j), i.max(j));
            let e = *edges.entry(key).or_insert_with(|| {
                let (p, q) = (points[key.0], points[key.1]);
                s.curves
                    .push(Curve::Line(Line::through(p, q).expect("an edge")));
                s.edges.push(Edge {
                    curve: s.curves.len() - 1,
                    interval: [0.0, p.distance(q)],
                    start: key.0,
                    end: key.1,
                    tolerance: 0.0,
                });
                s.edges.len() - 1
            });
            uses.push(Use {
                edge: e,
                sense: if i < j {
                    Sense::Forward
                } else {
                    Sense::Reversed
                },
                pcurve: None,
            });
        }
        s.faces.push(Face {
            surface: s.surfaces.len() - 1,
            sense: Sense::Forward,
            loops: vec![Loop::Edges(uses)],
            tolerance: 0.0,
        });
    }
    s.shells.push(Shell {
        faces: (0..s.faces.len()).collect(),
        closed: true,
    });
    s.solids.push(Solid {
        outer: 0,
        voids: Vec::new(),
    });
    s
}

/// Shapes whose parts cross, with the parts they were made from.
pub struct Crossing {
    /// Every part a shell of one solid.
    pub shape: Shape,
    /// Each part where it is placed, and whether it is turned inside out.
    pub parts: Vec<(Body, bool)>,
}

impl Crossing {
    /// The number of parts facing out that hold `p`, less those turned
    /// in: the winding number the shape's faces give `p`. `None` where `p`
    /// is on a part's boundary or a part can't say.
    pub fn winding(&self, p: Point3) -> Option<i64> {
        let mut w = 0;
        for (b, turned) in &self.parts {
            match point_in_body(b, p).ok()? {
                Inclusion::Inside => w += if *turned { -1 } else { 1 },
                Inclusion::Outside => {}
                Inclusion::Boundary => return None,
            }
        }
        Some(w)
    }
}

/// Two or three primitives from bytes (blocks, cylinders, spheres and
/// cones, sizes from a coarse list) placed on a grid of quarters so they
/// overlap, each a shell of one solid, each turned inside out by a bit
/// (the first never, so something faces out). `None` when the bytes run
/// out or a part can't be made. A first byte whose top bit is set adds
/// the flat-span twin's prism (#585), whose spline has a flat end span,
/// placed on the grid as the others are, so its span may lie on a block's
/// face; the bit is masked off for the rest, so inputs whose first byte is
/// below `0x80` read as before.
pub fn crossing_case(data: &[u8]) -> Option<Crossing> {
    let flat = data.first().is_some_and(|x| x & 0x80 != 0);
    let masked: Vec<u8> = data
        .iter()
        .enumerate()
        .map(|(i, &x)| if i == 0 { x & 0x7f } else { x })
        .collect();
    let mut b = Bytes::new(&masked);
    if b.is_empty() {
        return None;
    }
    let count = 2 + b.pick(2);
    let mut parts: Vec<(Body, bool)> = Vec::new();
    let mut shape: Option<Shape> = None;
    for k in 0..count {
        let size = |b: &mut Bytes| 1.0 + 0.25 * b.pick(8) as f64;
        let at = |b: &mut Bytes| -1.0 + 0.25 * b.pick(9) as f64;
        let place = Rigid::translation(Vector3::new(at(&mut b), at(&mut b), at(&mut b)));
        let key = OpKey(k as u128 + 1);
        let body = match b.pick(4) {
            0 => primitive::block(key, size(&mut b), size(&mut b), size(&mut b), &place),
            1 => primitive::cylinder(key, 0.5 * size(&mut b), size(&mut b), &place),
            2 => primitive::sphere(key, 0.5 * size(&mut b), &place),
            _ => primitive::cone(key, 0.5 * size(&mut b), size(&mut b), &place),
        }
        .ok()?
        .0;
        let turned = k > 0 && b.pick(2) == 1;
        let mut s = seamed(&body, 1e-7).ok()?;
        if turned {
            turn_shell(&mut s, 0);
        }
        shape = Some(match shape {
            None => s,
            Some(t) => joined(&t, &s),
        });
        parts.push((body, turned));
    }
    if flat {
        let at = |b: &mut Bytes| -1.0 + 0.25 * b.pick(9) as f64;
        let place = Rigid::translation(Vector3::new(at(&mut b), at(&mut b), at(&mut b)));
        let prism = crate::nurbs::flat_span_twin().0.transformed(&place).ok()?;
        let s = seamed(&prism, 1e-7).ok()?;
        shape = Some(match shape {
            None => s,
            Some(t) => joined(&t, &s),
        });
        parts.push((prism, false));
    }
    Some(Crossing {
        shape: one_solid(shape?),
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tetrahedron_heals() {
        let p = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ];
        let s = polyhedron(&p, &[[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]]);
        let (b, _, _) = ryukei::heal::heal(OpKey(1), &s, &Default::default()).unwrap();
        assert!(ryukei::check::check(&b).is_empty());
        assert_eq!(b.faces().count(), 4);
    }

    #[test]
    fn cases_from_bytes_cross() {
        let data: Vec<u8> = (0u8..64).map(|k| k.wrapping_mul(37)).collect();
        let made = (0..16).filter_map(|k| crossing_case(&data[k..])).count();
        assert!(made > 8, "{made}");
    }

    #[test]
    fn a_crossing_case_can_hold_the_flat_span_twin() {
        // A first byte with its top bit set adds the twin's prism, placed
        // on the grid like the other parts.
        let case = crossing_case(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).expect("a case");
        assert!(
            case.shape
                .surfaces
                .iter()
                .any(|s| matches!(s, ryukei::geom::Surface::Nurbs(_))),
            "the twin's spline is in the shape"
        );
        // Without the bit, the same bytes read as they did.
        let plain = crossing_case(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).expect("a case");
        assert_eq!(plain.parts.len() + 1, case.parts.len());
    }
}
