//! Hidden lines judged against a mesh (design 326, plan 10.C): what a
//! test needs to read a [`Projection`] and to check it independently of
//! the kernel's certificates.

use ryukei::geom::{Curve, Point2, Point3, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::project::{Class, Piece, Placed, Projection, Source, View, Visibility};
use ryukei::topo::Body;

/// A view along `towards` with z up, else y up.
pub fn look(towards: Vector3) -> View {
    let up = if towards.cross(Vector3::new(0.0, 0.0, 1.0)).length() > 0.1 * towards.length() {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    View::new(towards, up).unwrap()
}

/// The placed bodies moved into place, as the view draws them.
pub fn placed_bodies(placed: &[Placed]) -> Vec<Body> {
    placed
        .iter()
        .map(|p| p.body.transformed(&p.place).unwrap())
        .collect()
}

/// The diagonal of the bodies' faces' boxes.
pub fn size(bodies: &[Body]) -> f64 {
    let b = bodies
        .iter()
        .flat_map(|b| b.faces().map(move |f| ryukei::intersect::face_box(b, f)))
        .reduce(|a, b| a.union(&b))
        .expect("faces");
    (b.max() - b.min()).length()
}

/// The 3D curve a piece is a stretch of, in the placed frame.
pub fn curve_of<'a>(p: &'a Projection, bodies: &'a [Body], q: &Piece) -> &'a Curve {
    match q.source {
        Source::Edge { body, edge, .. } => {
            let b = &bodies[body as usize];
            b.curve(b.edge(edge).unwrap().curve).unwrap()
        }
        Source::Silhouette {
            body,
            face,
            ordinal,
            ..
        } => {
            &p.silhouettes[body as usize]
                .branches
                .iter()
                .find(|s| s.face == face && s.ordinal == ordinal)
                .unwrap()
                .curve
        }
    }
}

/// A piece's image sampled at `n + 1` parameters, ends included.
pub fn image_points(q: &Piece, n: usize) -> Vec<Point2> {
    let [t0, t1] = q.image.range;
    (0..=n)
        .map(|k| q.image.point(t0 + (t1 - t0) * k as f64 / n as f64))
        .collect()
}

/// A piece's image length, its image sampled at 257 parameters.
pub fn image_length(q: &Piece) -> f64 {
    image_points(q, 256)
        .windows(2)
        .map(|w| w[0].distance(w[1]))
        .sum()
}

/// Where a class's lengths go in [`Lengths`]: sharp edges and sheets'
/// boundaries (and edges whose class is uncertain), smooth edges, and
/// silhouettes, as OCCT's sharp, `Rg1` and outline compounds.
pub const SHARP: usize = 0;
pub const SMOOTH: usize = 1;
pub const OUTLINE: usize = 2;

pub fn slot(c: Class) -> usize {
    match c {
        Class::Smooth => SMOOTH,
        Class::Silhouette => OUTLINE,
        _ => SHARP,
    }
}

/// Image lengths drawn, by class, each overlap counted once (its
/// nearest piece's).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lengths {
    pub visible: [f64; 3],
    pub hidden: [f64; 3],
    /// Pieces no ray settled.
    pub uncertain: f64,
}

/// Whether no piece piece `i` lies on is nearer (ties to the later
/// index), so overlaps count once.
pub fn nearest(p: &Projection, i: usize) -> bool {
    let mid = |q: &Piece| (q.depth[0] + q.depth[1]) / 2.0;
    let q = &p.pieces[i];
    q.overlaps.iter().all(|&o| {
        let other = &p.pieces[o as usize];
        mid(other) < mid(q) || (mid(other) == mid(q) && o as usize > i)
    })
}

fn add(l: &mut Lengths, q: &Piece, v: Visibility, length: f64) {
    match v {
        Visibility::Visible => l.visible[slot(q.class)] += length,
        Visibility::Hidden(_) => l.hidden[slot(q.class)] += length,
        Visibility::Uncertain => l.uncertain += length,
    }
}

/// What a drawing draws, by class.
pub fn lengths(p: &Projection) -> Lengths {
    let mut l = Lengths::default();
    for (i, q) in p.pieces.iter().enumerate() {
        if nearest(p, i) {
            add(&mut l, q, q.visibility, image_length(q));
        }
    }
    l
}

/// The placed bodies' meshes, for judging visibility by rays.
pub struct Oracle {
    triangles: Vec<[Point3; 3]>,
    /// Each triangle's box in the image plane: `[x0, y0, x1, y1]`.
    boxes: Vec<[f64; 4]>,
    view: View,
    chord: f64,
    size: f64,
}

impl Oracle {
    /// The bodies meshed at `chord`; `None` when a face doesn't mesh.
    pub fn new(bodies: &[Body], view: &View, chord: f64) -> Option<Self> {
        let params = MeshParams {
            max_triangles: 4_000_000,
            ..MeshParams::new(chord, 0.2)
        };
        let mut triangles = Vec::new();
        for b in bodies {
            let m = tessellate(b, &params).ok()?;
            if !m.faults.is_empty() {
                return None;
            }
            triangles.extend(
                m.welded_triangles()
                    .map(|t| t.map(|i| m.positions[i as usize])),
            );
        }
        let boxes = triangles
            .iter()
            .map(|t| {
                let q = t.map(|p| view.image(p));
                [
                    q[0].x.min(q[1].x).min(q[2].x),
                    q[0].y.min(q[1].y).min(q[2].y),
                    q[0].x.max(q[1].x).max(q[2].x),
                    q[0].y.max(q[1].y).max(q[2].y),
                ]
            })
            .collect();
        Some(Self {
            triangles,
            boxes,
            view: *view,
            chord,
            size: size(bodies),
        })
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn chord(&self) -> f64 {
        self.chord
    }

    /// How many faces a ray from `p` towards the viewer crosses, or
    /// `None` where the mesh can't say: a crossing through a mesh edge or
    /// vertex (counted twice or not at all), a crossing a little way off
    /// (the mesh lies a chord off the surface it crosses near `p`), or,
    /// from a silhouette, a grazing crossing within its tangent wedge.
    /// Crossings within four chords of `p` are its own faces.
    pub fn count(&self, p: Point3, silhouette: bool) -> Option<u32> {
        let d = self.view.towards().get();
        let q = self.view.image(p);
        let own = 4.0 * self.chord;
        let wedge = (8.0 * self.size * self.chord).sqrt();
        let mut n = 0;
        for (t, b) in self.triangles.iter().zip(&self.boxes) {
            if q.x < b[0] || q.x > b[2] || q.y < b[1] || q.y > b[3] {
                continue;
            }
            let Some((s, edge, cos)) = cross(p, d, t) else {
                continue;
            };
            if s <= own {
                continue;
            }
            if edge < 1e-9 || s < 50.0 * self.chord || (silhouette && s < wedge && cos < 0.1) {
                return None;
            }
            n += 1;
        }
        Some(n)
    }
}

/// Where the ray `o + s d` crosses the triangle (Möller and Trumbore):
/// `s`, the least barycentric coordinate, and the cosine between the ray
/// and the triangle's normal.
fn cross(o: Point3, d: Vector3, [a, b, c]: &[Point3; 3]) -> Option<(f64, f64, f64)> {
    let (e1, e2) = (*b - *a, *c - *a);
    let normal = e1.cross(e2);
    let area = normal.length();
    if area == 0.0 {
        return None;
    }
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det.abs() <= 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let sv = o - *a;
    let u = sv.dot(h) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = sv.cross(e1);
    let v = d.dot(qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let s = e2.dot(qv) * inv;
    if s <= 0.0 {
        return None;
    }
    Some((s, u.min(v).min(1.0 - u - v), (normal.dot(d) / area).abs()))
}

/// A piece the mesh judges otherwise.
#[derive(Clone, Debug)]
pub struct Disagreement {
    pub piece: usize,
    pub source: Source,
    /// Where along the piece, as a fraction.
    pub at: f64,
    pub kernel: Visibility,
    pub mesh: u32,
}

/// What [`judge`] found.
#[derive(Clone, Debug, Default)]
pub struct Judged {
    /// Samples the mesh settled.
    pub samples: usize,
    /// Samples it couldn't.
    pub unsure: usize,
    /// Pieces the kernel left `Visibility::Uncertain`.
    pub uncertain: usize,
    pub pieces: usize,
    pub wrong: Vec<Disagreement>,
}

/// Every piece the mesh can judge, at a quarter, a half and three
/// quarters along: visible exactly when the mesh counts no crossing.
/// Pieces lying on others, and shorter in the image than eighty chords,
/// aren't judged; a sample within twenty chords of another piece's image
/// is unsure (the mesh's outlines lie a chord off the true ones).
pub fn judge(p: &Projection, bodies: &[Body], oracle: &Oracle) -> Judged {
    let margin = 20.0 * oracle.chord;
    let images: Vec<Vec<Point2>> = p.pieces.iter().map(|q| image_points(q, 32)).collect();
    let mut out = Judged {
        pieces: p.pieces.len(),
        ..Judged::default()
    };
    for (i, q) in p.pieces.iter().enumerate() {
        if q.visibility == Visibility::Uncertain {
            out.uncertain += 1;
            continue;
        }
        if !q.overlaps.is_empty() || image_length(q) < 4.0 * margin {
            continue;
        }
        let c = curve_of(p, bodies, q);
        let [t0, t1] = q.image.range;
        for f in [0.25, 0.5, 0.75] {
            let x = c.point(t0 + (t1 - t0) * f);
            let at = oracle.view.image(x);
            let crowded = p.pieces.iter().enumerate().any(|(j, other)| {
                j != i && other.source != q.source && near(&images[j], at, margin)
            });
            let verdict = if crowded {
                None
            } else {
                oracle.count(x, q.class == Class::Silhouette)
            };
            let Some(n) = verdict else {
                out.unsure += 1;
                continue;
            };
            out.samples += 1;
            if matches!(q.visibility, Visibility::Hidden(_)) != (n > 0) {
                out.wrong.push(Disagreement {
                    piece: i,
                    source: q.source,
                    at: f,
                    kernel: q.visibility,
                    mesh: n,
                });
            }
        }
    }
    out
}

/// Whether `p` lies within `r` of the polyline.
fn near(line: &[Point2], p: Point2, r: f64) -> bool {
    line.windows(2).any(|w| {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let len2 = ab.dot(ab);
        let t = if len2 == 0.0 {
            0.0
        } else {
            ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
        };
        (a + ab * t).distance(p) <= r
    })
}

/// [`lengths`] with each piece the mesh judged taking the mesh's
/// verdict, by the majority of its settled samples; a piece the mesh
/// didn't judge keeps the kernel's.
pub fn mesh_lengths(p: &Projection, bodies: &[Body], oracle: &Oracle) -> Lengths {
    let j = judge(p, bodies, oracle);
    let mut flipped = vec![0usize; p.pieces.len()];
    for w in &j.wrong {
        flipped[w.piece] += 1;
    }
    let mut l = Lengths::default();
    for (i, q) in p.pieces.iter().enumerate() {
        if !nearest(p, i) {
            continue;
        }
        let v = if flipped[i] >= 2 {
            match q.visibility {
                Visibility::Visible => Visibility::Hidden(1),
                _ => Visibility::Visible,
            }
        } else {
            q.visibility
        };
        add(&mut l, q, v, image_length(q));
    }
    l
}

/// The largest amount by which `|n · towards|` passes its bound at nine
/// points of each silhouette, `n` the face's unit normal at the
/// silhouette's pcurve: at most `2 × tolerance × |∇(n · towards)|` (the
/// curve lies within its tolerance of where `n · towards` vanishes),
/// the gradient by central differences. At most 0 when every point
/// holds.
pub fn silhouette_excess(bodies: &[Body], p: &Projection, view: &View) -> f64 {
    let t = view.towards().get();
    let mut worst = f64::NEG_INFINITY;
    for (b, sil) in bodies.iter().zip(&p.silhouettes) {
        for s in &sil.branches {
            let surface = b.surface(b.face(s.face).unwrap().surface).unwrap();
            let f = |u: f64, v: f64| surface.normal(u, v).map(|n| n.get().dot(t));
            let (c2, map) = &s.pcurve;
            for k in 0..9 {
                let x = s.range[0] + (s.range[1] - s.range[0]) * (k as f64 + 0.5) / 9.0;
                let uv = c2.point(map[0] + map[1] * x);
                let h = 1e-7;
                let (Some(f0), Some(fu0), Some(fu1), Some(fv0), Some(fv1)) = (
                    f(uv.x, uv.y),
                    f(uv.x - h, uv.y),
                    f(uv.x + h, uv.y),
                    f(uv.x, uv.y - h),
                    f(uv.x, uv.y + h),
                ) else {
                    continue; // a pole: no normal to judge
                };
                let su = surface
                    .point(uv.x + h, uv.y)
                    .distance(surface.point(uv.x - h, uv.y));
                let sv = surface
                    .point(uv.x, uv.y + h)
                    .distance(surface.point(uv.x, uv.y - h));
                let grad = (fu1 - fu0).abs() / su.max(1e-300) + (fv1 - fv0).abs() / sv.max(1e-300);
                let bound = 2.0 * s.tolerance * grad + 1e-9;
                worst = worst.max(f0.abs() - bound);
            }
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::name::OpKey;
    use ryukei::primitive;
    use ryukei::project::{ViewOptions, view};

    #[test]
    fn a_block_from_a_corner_agrees_with_its_mesh() {
        let (b, _) = primitive::block(OpKey(1), 0.01, 0.02, 0.03, &Rigid::IDENTITY).unwrap();
        let v = look(Vector3::new(1.0, -1.0, 1.0));
        let placed = [Placed {
            body: &b,
            place: Rigid::IDENTITY,
        }];
        let p = view(&placed, &v, &ViewOptions::default()).unwrap();
        let bodies = placed_bodies(&placed);
        let o = Oracle::new(&bodies, &v, size(&bodies) * 1e-3).unwrap();
        let j = judge(&p, &bodies, &o);
        assert!(j.wrong.is_empty(), "{:?}", j.wrong);
        assert!(j.samples >= 12 * 3 / 2, "{j:?}");
        // Nine edges seen, three hidden, each foreshortened by √(2/3).
        let l = lengths(&p);
        let f = (2.0f64 / 3.0).sqrt();
        assert!((l.visible[SHARP] - 3.0 * 0.06 * f).abs() < 1e-12, "{l:?}");
        assert!((l.hidden[SHARP] - 0.06 * f).abs() < 1e-12, "{l:?}");
        assert_eq!(mesh_lengths(&p, &bodies, &o), l);
    }

    #[test]
    fn a_rays_crossings_are_counted_and_its_own_faces_are_not() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let v = look(Vector3::new(0.0, 0.0, 1.0));
        let o = Oracle::new(&[b], &v, 1e-3).unwrap();
        // Below the block: two faces in front.
        assert_eq!(o.count(Point3::new(0.3, 0.4, -1.0), false), Some(2));
        // On its bottom face: one, its top.
        assert_eq!(o.count(Point3::new(0.3, 0.4, 0.0), false), Some(1));
        // On its top: none.
        assert_eq!(o.count(Point3::new(0.3, 0.4, 1.0), false), Some(0));
    }

    #[test]
    fn a_spheres_silhouette_is_square_to_the_view() {
        let (b, _) = primitive::sphere(OpKey(1), 0.02, &Rigid::IDENTITY).unwrap();
        let v = look(Vector3::new(0.3, -0.4, 0.8));
        let placed = [Placed {
            body: &b,
            place: Rigid::IDENTITY,
        }];
        let p = view(&placed, &v, &ViewOptions::default()).unwrap();
        assert!(silhouette_excess(&placed_bodies(&placed), &p, &v) <= 0.0);
    }
}
