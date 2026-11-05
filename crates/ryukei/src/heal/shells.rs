//! Shells: which way each faces, and which are cavities, from geometry.
//!
//! Once its faces agree with each other, a shell can face only two ways.
//! The body is built with every shell a solid of its own, and each
//! shell's volume decides: positive when its faces point out of what it
//! encloses. A shell of planar faces bounded by straight edges is summed
//! exactly over its faces' fans ([`crate::predicates::volume_sign`]);
//! any other is meshed coarsely (a chord of a hundredth of the body's
//! size), whose volume lies within `4 × chord × area` of the shell's,
//! the area that of its curved faces (planar faces mesh in their planes):
//! a sign further than that from zero is certain. The mesh is as coarse
//! in angle as the mesher allows (a right angle): the bound needs only
//! the chord. Else the mesh is made finer, to an eighth of its chord or
//! to what the volume it measured needs if that is finer (but no finer
//! than a sixty-fourth: a coarse mesh can measure almost nothing for a
//! body that encloses plenty), up to four times and down to ten
//! resolutions;
//! then, or past a budget of triangles, it is [`HealError::Uncertain`].
//!
//! Which shells are cavities follows from which encloses which. Every
//! vertex of one shell (a point of its mesh if it has none: a sphere or
//! torus with no loops) is classified against each other shell whose box
//! holds it, by the parity of a ray's crossings with that shell's mesh,
//! decided by exact orientation tests; a vertex within the mesh's chord
//! (plus the shell's tolerances) of it decides nothing. A shell whose
//! vertices fall both inside and outside another crosses it:
//! [`HealError::ShellsCross`]. A shell inside an even number of others
//! (none, two, ...) is the outer shell of a solid and must enclose
//! positive volume; one inside an odd number is a cavity of the
//! innermost shell enclosing it and must face into the cavity. What the
//! file said about voids gives way to this. Shells that face the wrong
//! way are turned over, face by face.

use super::build::SolidFaces;
use crate::geom::{Box3, Point3, Surface, Vector3};
use crate::mesh::{BodyMesh, MeshParams, tessellate_in};
use crate::name::Name;
use crate::predicates::{Sign, orient3d, volume_sign};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, ShellId};
use crate::work::Meter;
use crate::{Error, HealError};
use std::collections::BTreeMap;

/// What the shells' geometry says: which of the shape's faces to turn
/// over, and the solids (an outer shell and its cavities, each the
/// shape's faces, sorted).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Judged {
    pub(super) turn: Vec<usize>,
    pub(super) solids: Vec<SolidFaces>,
}

impl Judged {
    /// Whether the body as built, every shell a solid of its own, stands.
    pub(super) fn stands(&self) -> bool {
        self.turn.is_empty() && self.solids.iter().all(|s| s.1.is_empty())
    }
}

/// One shell of the built body.
struct Shell {
    faces: Vec<FaceId>,
    /// Its least face name, to report it by.
    name: Name,
    bounds: Box3,
    /// The most any of its entities' tolerances is.
    tolerance: f64,
    positive: bool,
}

/// Meshes made before a sign is uncertain.
const ROUNDS: usize = 5;

/// The most triangles a mesh for a sign may have.
const BUDGET: u64 = 4_000_000;

/// Ray directions for classifying a point against a mesh: none along an
/// axis or a diagonal, where models line up.
const DIRECTIONS: [[f64; 3]; 4] = [
    [0.8432, 0.3721, 0.3879],
    [-0.2914, 0.9137, 0.2833],
    [0.1871, -0.4187, 0.8886],
    [-0.7313, -0.5917, 0.3393],
];

/// Judges the shells of `body`, built from the shape with every shell a
/// solid of its own; `index` gives each face name's place in the shape.
pub(super) fn judge(
    body: &Body,
    index: &BTreeMap<Name, usize>,
    m: &Meter,
) -> Result<Judged, Error> {
    let ids: Vec<ShellId> = body
        .regions()
        .skip(1)
        .flat_map(|r| body.region_shells(r).collect::<Vec<_>>())
        .collect();
    let faces_of = |sh: ShellId| -> Vec<FaceId> { body.shell_faces(sh).collect() };
    let mut size = 0.0f64;
    let mut all: Option<Box3> = None;
    for f in body.faces() {
        let b = crate::intersect::coarse_box(body, f);
        all = Some(all.map_or(b, |a| a.union(&b)));
    }
    if let Some(b) = all {
        size = (b.max() - b.min()).length();
    }
    let polygonal = |faces: &[FaceId]| faces.iter().all(|&f| plane_fan(body, f).is_some());
    let mut chord = (size * 1e-2).max(10.0 * LINEAR_RESOLUTION);
    let mut mesh: Option<BodyMesh> = None;
    let mut shells: Vec<Shell> = Vec::new();
    for &sh in &ids {
        let faces = faces_of(sh);
        let name = faces
            .iter()
            .map(|&f| body.faces[f].name)
            .min()
            .expect("a shell has faces");
        let bounds = faces
            .iter()
            .map(|&f| crate::intersect::coarse_box(body, f))
            .reduce(|a, b| a.union(&b))
            .expect("faces");
        let tolerance = shell_tolerance(body, &faces);
        let positive = if polygonal(&faces) {
            let triangles: Vec<[Point3; 3]> = faces
                .iter()
                .flat_map(|&f| plane_fan(body, f).expect("polygonal"))
                .collect();
            match volume_sign(&triangles) {
                Sign::Positive => true,
                Sign::Negative => false,
                Sign::Zero => return Err(HealError::Uncertain { at: name }.into()),
            }
        } else {
            let mut sign = None;
            for _ in 0..ROUNDS {
                let made = match &mesh {
                    Some(made) => made,
                    None => {
                        let params = MeshParams {
                            max_triangles: BUDGET,
                            ..MeshParams::new(chord, core::f64::consts::FRAC_PI_2)
                        };
                        mesh = Some(
                            tessellate_in(body, &params, m)
                                .map_err(|_| HealError::Uncertain { at: name })?,
                        );
                        mesh.as_ref().expect("made")
                    }
                };
                let finer = match mesh_sign(body, made, &faces, chord, name)? {
                    Ok(s) => {
                        sign = Some(s);
                        break;
                    }
                    Err(needed) => (chord / 8.0).min(needed).max(chord / 64.0),
                };
                if finer < 10.0 * LINEAR_RESOLUTION {
                    break;
                }
                chord = finer;
                mesh = None;
            }
            sign.ok_or(HealError::Uncertain { at: name })?
        };
        shells.push(Shell {
            faces,
            name,
            bounds,
            tolerance,
            positive,
        });
    }
    // Which encloses which: only with more than one shell.
    let n = shells.len();
    let mut inside: Vec<Vec<bool>> = vec![vec![false; n]; n];
    if n > 1 {
        let made = match mesh {
            Some(made) => made,
            None => tessellate_in(
                body,
                &MeshParams::new(chord, core::f64::consts::FRAC_PI_2),
                m,
            )
            .map_err(|_| HealError::Uncertain { at: shells[0].name })?,
        };
        let triangles: Vec<Vec<[Point3; 3]>> = shells
            .iter()
            .map(|s| shell_triangles(body, &made, &s.faces).map(|(t, _)| t))
            .collect::<Result<_, _>>()
            .map_err(|at| HealError::Uncertain { at })?;
        for i in 0..n {
            let mut points = shell_points(body, &shells[i].faces);
            if points.is_empty() {
                // A sphere or torus with no loops: a point of its mesh.
                points.extend(triangles[i].first().map(|t| t[0]));
            }
            for j in 0..n {
                if i == j || !shells[j].bounds.intersects(&shells[i].bounds) {
                    continue;
                }
                let band = chord + shells[j].tolerance + LINEAR_RESOLUTION;
                let mut seen: Option<bool> = None;
                for &p in &points {
                    // Each triangle a ray is tested against.
                    if m.charge(triangles[j].len() as u64).is_err() {
                        return Err(Error::Cancelled);
                    }
                    let Some(x) = classify(&triangles[j], &shells[j].bounds, p, band) else {
                        continue;
                    };
                    match seen {
                        Some(y) if y != x => {
                            let (a, b) = sorted(shells[i].name, shells[j].name);
                            return Err(HealError::ShellsCross { a, b }.into());
                        }
                        _ => seen = Some(x),
                    }
                }
                match seen {
                    Some(x) => inside[i][j] = x,
                    None => {
                        return Err(HealError::Uncertain { at: shells[i].name }.into());
                    }
                }
            }
        }
        for i in 0..n {
            for j in 0..i {
                if inside[i][j] && inside[j][i] {
                    let (a, b) = sorted(shells[i].name, shells[j].name);
                    return Err(HealError::ShellsCross { a, b }.into());
                }
            }
        }
    }
    let depth: Vec<usize> = (0..n)
        .map(|i| (0..n).filter(|&j| inside[i][j]).count())
        .collect();
    let place = |f: FaceId| index[&body.faces[f].name];
    let faces_in = |s: &Shell| {
        let mut x: Vec<usize> = s.faces.iter().map(|&f| place(f)).collect();
        x.sort_unstable();
        x
    };
    let mut turn: Vec<usize> = Vec::new();
    for (i, s) in shells.iter().enumerate() {
        let want = depth[i].is_multiple_of(2);
        if s.positive != want {
            turn.extend(faces_in(s));
        }
    }
    turn.sort_unstable();
    let mut solids: BTreeMap<usize, SolidFaces> = BTreeMap::new();
    for i in (0..n).filter(|&i| depth[i].is_multiple_of(2)) {
        solids.insert(i, (faces_in(&shells[i]), Vec::new()));
    }
    for i in (0..n).filter(|&i| depth[i] % 2 == 1) {
        // The innermost shell holding it: the one at the greatest depth.
        let host = (0..n)
            .filter(|&j| inside[i][j])
            .max_by_key(|&j| depth[j])
            .expect("a cavity is inside a shell");
        let entry = solids.get_mut(&host).expect("its host is a solid");
        entry.1.push(faces_in(&shells[i]));
    }
    let mut solids: Vec<SolidFaces> = solids.into_values().collect();
    for s in &mut solids {
        s.1.sort();
    }
    solids.sort();
    Ok(Judged { turn, solids })
}

fn sorted(a: Name, b: Name) -> (Name, Name) {
    if a <= b { (a, b) } else { (b, a) }
}

/// The most any tolerance on the shell's faces' boundaries is, at least
/// resolution.
fn shell_tolerance(b: &Body, faces: &[FaceId]) -> f64 {
    let mut t = LINEAR_RESOLUTION;
    for &f in faces {
        for l in b.face_loops(f) {
            if let Some(v) = b.loops[l].vertex {
                t = t.max(b.vertices[v].tolerance.get());
            }
            for fin in b.loop_fins(l) {
                let e = &b.edges[b.fins[fin].edge];
                t = t
                    .max(e.tolerance.get())
                    .max(b.vertices[e.start].tolerance.get())
                    .max(b.vertices[e.end].tolerance.get());
            }
        }
    }
    t
}

/// A planar face bounded only by straight edges as fan triangles, each
/// loop fanned from its first corner.
fn plane_fan(b: &Body, f: FaceId) -> Option<Vec<[Point3; 3]>> {
    if !matches!(b.surfaces[b.faces[f].surface], Surface::Plane(_)) {
        return None;
    }
    let mut out = Vec::new();
    for l in b.face_loops(f) {
        if b.loops[l].vertex.is_some() {
            return None;
        }
        let mut corners = Vec::new();
        for fin in b.loop_fins(l) {
            let e = &b.edges[b.fins[fin].edge];
            if !matches!(b.curves[e.curve], crate::geom::Curve::Line(_)) {
                return None;
            }
            corners.push(b.vertices[b.start_of(fin)].point);
        }
        for i in 1..corners.len().saturating_sub(1) {
            out.push([corners[0], corners[i], corners[i + 1]]);
        }
    }
    Some(out)
}

/// The triangles of a shell's faces in the mesh, or the least face name
/// of a face the mesh left out; with them, the length of the polylines
/// round planar faces closed by [`polyline_fan`].
fn shell_triangles(
    b: &Body,
    m: &BodyMesh,
    faces: &[FaceId],
) -> Result<(Vec<[Point3; 3]>, f64), Name> {
    let by_face: BTreeMap<FaceId, &crate::mesh::FaceMesh> =
        m.faces.iter().map(|fm| (fm.face, fm)).collect();
    let mut out = Vec::new();
    let mut fanned = 0.0;
    let mut missing: Option<Name> = None;
    for &f in faces {
        let Some(fm) = by_face.get(&f) else {
            if let Some((fan, length)) = polyline_fan(b, m, f) {
                out.extend(fan);
                fanned += length;
                continue;
            }
            let n = b.faces[f].name;
            missing = Some(missing.map_or(n, |x| x.min(n)));
            continue;
        };
        for t in &m.triangles[fm.triangles.start as usize..fm.triangles.end as usize] {
            out.push(t.map(|c| m.positions[m.corners[c as usize].position as usize]));
        }
    }
    match missing {
        Some(n) => Err(n),
        None => Ok((out, fanned)),
    }
}

/// A planar face the mesher left out (`BadDomain` on a lens of two
/// curves that come within a chord of each other: the Switchwire's Revo
/// toolhead, #240), closed by a fan over each loop's edge polylines, which
/// the mesh shares with the neighbouring faces, so it stays closed by
/// index; and the polylines' length. Shell facing needs only signs from
/// the mesh, never a triangulation of the face:
///
/// - the fan's signed coverage of its plane is the polygon's (1 inside,
///   0 outside, however the polygon folds), so it and any triangulation
///   of the polygon differ by a closed cycle in the plane, which adds
///   nothing to a volume, to a winding number about a point off the
///   plane, or to the parity of a ray's crossings;
/// - the polygon lies within the chord and the edges' tolerances of the
///   face (the polylines are the edges' samples), as a mesh of the face
///   would, and every point of it is covered by the fan, so a point
///   further than a band from the fan is as far from the polygon.
///
/// `None` for a face not on a plane, or whose edges have no polylines.
fn polyline_fan(b: &Body, m: &BodyMesh, f: FaceId) -> Option<(Vec<[Point3; 3]>, f64)> {
    if !matches!(b.surfaces[b.faces[f].surface], Surface::Plane(_)) {
        return None;
    }
    let lines: BTreeMap<crate::topo::EdgeId, &crate::mesh::EdgePolyline> =
        m.edges.iter().map(|e| (e.edge, e)).collect();
    let mut out = Vec::new();
    let mut length = 0.0;
    for l in b.face_loops(f) {
        if b.loops[l].vertex.is_some() {
            continue;
        }
        let mut ring: Vec<u32> = Vec::new();
        for fin in b.loop_fins(l) {
            let fi = &b.fins[fin];
            let line = lines.get(&fi.edge)?;
            let mut pts = line.positions.clone();
            if fi.sense == crate::topo::Sense::Reversed {
                pts.reverse();
            }
            // Each polyline starts where the one before ended.
            let skip = usize::from(ring.last() == pts.first());
            ring.extend(pts.into_iter().skip(skip));
        }
        if ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        let at = |i: u32| m.positions[i as usize];
        for w in ring.windows(2) {
            length += at(w[0]).distance(at(w[1]));
        }
        if let (Some(&a), Some(&z)) = (ring.first(), ring.last()) {
            length += at(z).distance(at(a));
        }
        for i in 1..ring.len().saturating_sub(1) {
            out.push([at(ring[0]), at(ring[i]), at(ring[i + 1])]);
        }
    }
    Some((out, length))
}

/// The sign of a shell's volume from the mesh, if it is further from zero
/// than the mesh can be wrong by, else the chord that would make the
/// volume measured a sure sign (half what its bound allows), or an
/// eighth of this one where a face of the shell has no mesh.
fn mesh_sign(
    b: &Body,
    m: &BodyMesh,
    faces: &[FaceId],
    chord: f64,
    name: Name,
) -> Result<Result<bool, f64>, Error> {
    // A face the mesher couldn't mesh at this chord (a thin band can stall
    // when coarse) asks for a finer one, as an unsure volume does.
    let Ok((triangles, fanned)) = shell_triangles(b, m, faces) else {
        return Ok(Err(chord / 8.0));
    };
    let by_face: BTreeMap<FaceId, &crate::mesh::FaceMesh> =
        m.faces.iter().map(|fm| (fm.face, fm)).collect();
    let mut curved = 0.0;
    for &f in faces {
        if matches!(b.surfaces[b.faces[f].surface], Surface::Plane(_)) {
            continue;
        }
        let fm = by_face[&f];
        for t in &m.triangles[fm.triangles.start as usize..fm.triangles.end as usize] {
            let [p, q, r] = t.map(|c| m.positions[m.corners[c as usize].position as usize]);
            curved += (q - p).cross(r - p).length() / 2.0;
        }
    }
    if triangles.is_empty() {
        return Err(HealError::Uncertain { at: name }.into());
    }
    let o = triangles[0][0];
    let volume: f64 = triangles
        .iter()
        .map(|[p, q, r]| {
            let (p, q, r) = (*p - o, *q - o, *r - o);
            p.dot(q.cross(r)) / 6.0
        })
        .sum();
    // A fanned planar face's polygon strays from the face by at most the
    // band along its polylines: a strip of at most `band × length`, whose
    // part of the volume (a third of its distance from `o` along the
    // normal times its area) is within a third of the mesh's size times
    // that (#549).
    let band = chord + shell_tolerance(b, faces) + LINEAR_RESOLUTION;
    let size = Box3::from_points(triangles.iter().flatten().copied())
        .map_or(0.0, |x| (x.max() - x.min()).length());
    let bound = 4.0 * chord * curved * 1.01 + band * fanned * size / 3.0 * 1.01;
    if volume.abs() > bound && volume.abs() > 0.0 {
        Ok(Ok(volume > 0.0))
    } else {
        Ok(Err(volume.abs() / (8.0 * curved * 1.01)))
    }
}

/// Every vertex point of a shell's faces.
fn shell_points(b: &Body, faces: &[FaceId]) -> Vec<Point3> {
    let mut out: BTreeMap<crate::topo::VertexId, Point3> = BTreeMap::new();
    for &f in faces {
        for l in b.face_loops(f) {
            if let Some(v) = b.loops[l].vertex {
                out.insert(v, b.vertices[v].point);
            }
            for fin in b.loop_fins(l) {
                let v = b.start_of(fin);
                out.insert(v, b.vertices[v].point);
            }
        }
    }
    out.into_values().collect()
}

/// Whether `p` lies inside the closed mesh `triangles`, unless it lies
/// within `band` of it or no ray settles it.
fn classify(triangles: &[[Point3; 3]], bounds: &Box3, p: Point3, band: f64) -> Option<bool> {
    let grown = bounds.grow(band);
    if !grown.intersects(&Box3::from_point(p)) {
        return Some(false);
    }
    if triangles.iter().any(|&t| near(p, t) <= band) {
        return None;
    }
    let reach = 2.0 * (grown.max() - grown.min()).length() + 1.0;
    'rays: for d in DIRECTIONS {
        let q = p + Vector3::new(d[0], d[1], d[2]) * reach;
        let mut crossings = 0usize;
        for &[a, b, c] in triangles {
            let (sp, sq) = (orient3d(a, b, c, p), orient3d(a, b, c, q));
            if sq == Sign::Zero {
                continue 'rays;
            }
            if sp == Sign::Zero {
                // `p` lies in the triangle's plane, further than `band`
                // from it: the ray meets that plane only at `p`, outside
                // the triangle (a vertex in a planar face's plane).
                continue;
            }
            if sp == sq {
                continue;
            }
            let s = [
                orient3d(p, q, a, b),
                orient3d(p, q, b, c),
                orient3d(p, q, c, a),
            ];
            if s.contains(&Sign::Zero) {
                // Unless the ray misses it plainly.
                let pos = s.contains(&Sign::Positive);
                let neg = s.contains(&Sign::Negative);
                if pos && neg {
                    continue;
                }
                continue 'rays;
            }
            if s[0] == s[1] && s[1] == s[2] {
                crossings += 1;
            }
        }
        return Some(crossings % 2 == 1);
    }
    None
}

/// The distance from `p` to the triangle `[a, b, c]`.
fn near(p: Point3, [a, b, c]: [Point3; 3]) -> f64 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let n = ab.cross(ac);
    let inside = |e: Vector3, q: Vector3| e.cross(q).dot(n) >= 0.0;
    if n.length() > 0.0 && inside(ab, ap) && inside(c - b, p - b) && inside(a - c, p - c) {
        return ap.dot(n).abs() / n.length();
    }
    let seg = |a: Point3, b: Point3| {
        let d = b - a;
        let l = d.dot(d);
        let t = if l > 0.0 {
            ((p - a).dot(d) / l).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (a + d * t).distance(p)
    };
    seg(a, b).min(seg(b, c)).min(seg(c, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mesh missing its planar faces (as when the mesher refuses them)
    /// is closed again by fans over their edge polylines: the volume is
    /// the full mesh's up to rounding (the cylinder's caps' fans and its
    /// mesher's triangles cover the same polygons), and shell facing gives
    /// the same sign. #240's Revo toolhead (#549).
    #[test]
    fn planar_faces_left_out_are_closed_by_fans() {
        use crate::geom::Rigid;
        let (body, _) =
            crate::primitive::cylinder(crate::name::OpKey(3), 0.01, 0.02, &Rigid::IDENTITY)
                .unwrap();
        let faces: Vec<FaceId> = body.faces().collect();
        let chord = 1e-4;
        let mut m =
            crate::mesh::tessellate(&body, &crate::mesh::MeshParams::new(chord, 0.5)).unwrap();
        let volume = |t: &[[Point3; 3]]| -> f64 {
            t.iter()
                .map(|[p, q, r]| p.to_vector().dot(q.to_vector().cross(r.to_vector())) / 6.0)
                .sum()
        };
        let (full, fanned) = shell_triangles(&body, &m, &faces).unwrap();
        assert_eq!(fanned, 0.0);
        let planes: Vec<FaceId> = faces
            .iter()
            .copied()
            .filter(|&f| matches!(body.surfaces[body.faces[f].surface], Surface::Plane(_)))
            .collect();
        assert_eq!(planes.len(), 2);
        m.faces.retain(|fm| !planes.contains(&fm.face));
        let (fans, length) = shell_triangles(&body, &m, &faces).unwrap();
        // Two rims of 2π r, as polylines a little shorter.
        let rim = 2.0 * core::f64::consts::PI * 0.01;
        assert!((length - 2.0 * rim).abs() < 0.01 * rim, "{length}");
        let (a, b) = (volume(&full), volume(&fans));
        assert!((a - b).abs() <= 1e-12 * a.abs(), "{a} against {b}");
        let name = body.faces[faces[0]].name;
        assert_eq!(mesh_sign(&body, &m, &faces, chord, name).unwrap(), Ok(true));
        // A face that isn't planar still asks for a finer mesh.
        let side = faces.iter().copied().find(|f| !planes.contains(f)).unwrap();
        m.faces.retain(|fm| fm.face != side);
        assert!(shell_triangles(&body, &m, &faces).is_err());
        assert_eq!(
            mesh_sign(&body, &m, &faces, chord, name).unwrap(),
            Err(chord / 8.0)
        );
    }
}
