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
//! the chord. Else the shell's winding number decides, about a point
//! further from its mesh than the chord and its tolerances (stepped off
//! a triangle along its normal): it is the mesh's, by exact orientation
//! tests, +1 where the faces point out. Else the mesh is made finer, to
//! an eighth of its chord, up to four times and down to ten resolutions;
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

/// The most points of its mesh a shell with no vertices is judged by in
/// [`enclosing`].
const MESH_POINTS: usize = 512;

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
                if let Some(s) = mesh_sign(body, made, &faces, chord, name, m)? {
                    sign = Some(s);
                    break;
                }
                let finer = chord / 8.0;
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
    // Which encloses which: only with more than one shell. A pair no
    // vertex decides (all within the chord of the other shell: a lens a
    // few hundredths of a millimetre from the can round it, issue #317)
    // is judged again on a mesh an eighth as coarse, as the facing is.
    let n = shells.len();
    let mut inside: Vec<Vec<bool>> = vec![vec![false; n]; n];
    if n > 1 {
        let mut made = match mesh {
            Some(made) => made,
            None => tessellate_in(
                body,
                &MeshParams::new(chord, core::f64::consts::FRAC_PI_2),
                m,
            )
            .map_err(|_| HealError::Uncertain { at: shells[0].name })?,
        };
        let mut round = 1;
        loop {
            match enclosing(body, &made, &shells, chord, m)? {
                Ok(x) => {
                    inside = x;
                    break;
                }
                Err(at) => {
                    let finer = chord / 8.0;
                    if round >= ROUNDS || finer < 10.0 * LINEAR_RESOLUTION {
                        return Err(HealError::Uncertain { at }.into());
                    }
                    round += 1;
                    chord = finer;
                    let params = MeshParams {
                        max_triangles: BUDGET,
                        ..MeshParams::new(chord, core::f64::consts::FRAC_PI_2)
                    };
                    made =
                        tessellate_in(body, &params, m).map_err(|_| HealError::Uncertain { at })?;
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

/// Which of `shells` each lies inside, by its vertices against the
/// others' triangles in `m` within `chord`: `Err` naming a shell whose
/// vertices decide nothing against some other shell its box meets, and
/// [`HealError::ShellsCross`] for one whose vertices fall both sides.
fn enclosing(
    body: &Body,
    made: &BodyMesh,
    shells: &[Shell],
    chord: f64,
    m: &Meter,
) -> Result<Result<Vec<Vec<bool>>, Name>, Error> {
    let n = shells.len();
    let mut inside: Vec<Vec<bool>> = vec![vec![false; n]; n];
    let triangles: Vec<Tris> = shells
        .iter()
        .map(|s| shell_triangles(body, made, &s.faces).map(|(t, _)| Tris::new(t)))
        .collect::<Result<_, _>>()
        .map_err(|at| HealError::Uncertain { at })?;
    for i in 0..n {
        let mut points = shell_points(body, &shells[i].faces);
        if points.is_empty() {
            // A sphere or torus with no loops: points spread over its
            // mesh. One alone decided a shell crossing another by which
            // side it fell on (a ball through a block's top was cut out
            // as a cavity, v0.9.0-pre.1's gate); several fall both sides,
            // so such a pair is `ShellsCross`, as shells with vertices are.
            let tris = &triangles[i].tris;
            let stride = tris.len().div_ceil(MESH_POINTS).max(1);
            points.extend(tris.iter().step_by(stride).map(|t| t[0]));
        }
        for j in 0..n {
            if i == j || !shells[j].bounds.intersects(&shells[i].bounds) {
                continue;
            }
            let band = chord + shells[j].tolerance + LINEAR_RESOLUTION;
            let mut seen: Option<bool> = None;
            for &p in &points {
                let (x, tested) = classify(&triangles[j], &shells[j].bounds, p, band);
                // The point, and each triangle it is tested against (#618:
                // every vertex against every triangle of the other shell
                // was 77 million units for a board and its parts).
                if m.charge(1 + tested).is_err() {
                    return Err(Error::Cancelled);
                }
                let Some(x) = x else {
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
                None => return Ok(Err(shells[i].name)),
            }
        }
    }
    Ok(Ok(inside))
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

/// Whether the faces of one closed shell of `body` (a solid's, or a
/// closed sheet's) enclose positive volume, their normals pointing out:
/// summed exactly over planar faces' fans, else from meshes made finer
/// until the measured volume's sign is sure, as healing judges shells.
/// `Uncertain` naming the faces' least name where it stays unsure.
pub(crate) fn facing(body: &Body, faces: &[FaceId], m: &Meter) -> Result<bool, Error> {
    let name = faces
        .iter()
        .map(|&f| body.faces[f].name)
        .min()
        .ok_or(Error::Degenerate)?;
    if faces.iter().all(|&f| plane_fan(body, f).is_some()) {
        let triangles: Vec<[Point3; 3]> = faces
            .iter()
            .flat_map(|&f| plane_fan(body, f).expect("polygonal"))
            .collect();
        return match volume_sign(&triangles) {
            Sign::Positive => Ok(true),
            Sign::Negative => Ok(false),
            Sign::Zero => Err(HealError::Uncertain { at: name }.into()),
        };
    }
    let size = faces
        .iter()
        .map(|&f| crate::intersect::coarse_box(body, f))
        .reduce(|a, b| a.union(&b))
        .map_or(0.0, |b| (b.max() - b.min()).length());
    let mut chord = (size * 1e-2).max(10.0 * LINEAR_RESOLUTION);
    for _ in 0..ROUNDS {
        let params = MeshParams {
            max_triangles: BUDGET,
            ..MeshParams::new(chord, core::f64::consts::FRAC_PI_2)
        };
        let made =
            tessellate_in(body, &params, m).map_err(|_| HealError::Uncertain { at: name })?;
        if let Some(s) = mesh_sign(body, &made, faces, chord, name, m)? {
            return Ok(s);
        }
        let finer = chord / 8.0;
        if finer < 10.0 * LINEAR_RESOLUTION {
            break;
        }
        chord = finer;
    }
    Err(HealError::Uncertain { at: name }.into())
}

/// The sign of a shell's volume from the mesh, if it is further from zero
/// than the mesh can be wrong by, or else if the shell winds round a point
/// further from the mesh than the mesh can be wrong by ([`wound`]); `None`
/// where neither is sure, or a face of the shell has no mesh (a thin band
/// can stall when coarse), for a mesh an eighth as coarse to judge.
///
/// Until v0.8.2 an unsure volume asked for the chord that would make it
/// sure, down to a sixty-fourth: a thin tube coarser than its radius
/// measures next to nothing, so the Voron 0.2's filament (0.9 mm across,
/// 0.3 m long) was meshed at once at 48 µm, 248,000 triangles of spline,
/// where an eighth (380 µm) lets its winding number decide.
fn mesh_sign(
    b: &Body,
    m: &BodyMesh,
    faces: &[FaceId],
    chord: f64,
    name: Name,
    meter: &Meter,
) -> Result<Option<bool>, Error> {
    // A face the mesher couldn't mesh at this chord (a thin band can stall
    // when coarse) asks for a finer one, as an unsure volume does.
    let Ok((triangles, fanned)) = shell_triangles(b, m, faces) else {
        return Ok(None);
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
    // that.
    let band = chord + shell_tolerance(b, faces) + LINEAR_RESOLUTION;
    let size = Box3::from_points(triangles.iter().flatten().copied())
        .map_or(0.0, |x| (x.max() - x.min()).length());
    let bound = 4.0 * chord * curved * 1.01 + band * fanned * size / 3.0 * 1.01;
    if volume.abs() > bound && volume.abs() > 0.0 {
        return Ok(Some(volume > 0.0));
    }
    wound(&triangles, band, meter)
}

/// Triangles of the mesh tried as places to step off from, in [`wound`].
const STEPS: usize = 64;

/// Whether the shell's faces point out of what they enclose, from its
/// winding number about one point further than `band` from its mesh:
/// `Some` where such a point is found that the mesh winds round, `None`
/// where none of those tried is.
///
/// The mesh of a closed shell is closed by index, and each of its points
/// lies within `band` (the chord and the shell's tolerances) of the face
/// it meshes, moving there along the face as the mesher certifies it; so
/// the straight homotopy from the mesh to the shell keeps clear of a
/// point further than `band` from the mesh, and their winding numbers
/// about it agree. A shell bounds a solid, so its winding number is 0
/// outside and the same one of ±1 everywhere inside: +1 where its faces
/// point out. The point is found by stepping off triangles along their
/// normals, both ways, two, four and eight bands; the winding number is
/// a ray's signed crossings, decided by exact orientation tests.
///
/// A thin tube (a filament, a slotted part) encloses too little volume
/// for the measured volume to be sure of its sign until the chord is a
/// small part of its radius, each finer mesh of the whole body costing
/// more; this needs the chord only under about half of it (#239).
fn wound(triangles: &[[Point3; 3]], band: f64, m: &Meter) -> Result<Option<bool>, Error> {
    let n = triangles.len();
    let Some(bounds) = Box3::from_points(triangles.iter().flatten().copied()) else {
        return Ok(None);
    };
    let stride = n.div_ceil(STEPS).max(1);
    for &[a, b, c] in triangles.iter().step_by(stride) {
        let Some(normal) = (b - a).cross(c - a).normalize() else {
            continue;
        };
        let centre = Point3::new(
            (a.x + b.x + c.x) / 3.0,
            (a.y + b.y + c.y) / 3.0,
            (a.z + b.z + c.z) / 3.0,
        );
        for k in [2.0, 4.0, 8.0] {
            for side in [-1.0, 1.0] {
                let p = centre + normal.get() * (side * k * band);
                // Each triangle measured against and each a ray is tested
                // against.
                if m.charge(2 * n as u64).is_err() {
                    return Err(Error::Cancelled);
                }
                if triangles.iter().any(|&t| near(p, t) <= band) {
                    continue;
                }
                match winding(triangles, &bounds, p) {
                    Some(0) | None => {}
                    Some(1) => return Ok(Some(true)),
                    Some(-1) => return Ok(Some(false)),
                    // A mesh winding twice round a point: faces crossing.
                    Some(_) => return Ok(None),
                }
            }
        }
    }
    Ok(None)
}

/// The signed number of times the triangles wind round `p`, which lies
/// off every one of them: a ray's crossings, +1 where it leaves through a
/// triangle's front (the side its corners wind counterclockwise about),
/// decided by exact orientation tests; `None` where every ray tried
/// passes through an edge or a corner.
fn winding(triangles: &[[Point3; 3]], bounds: &Box3, p: Point3) -> Option<i64> {
    let reach = 2.0 * (bounds.max() - bounds.min()).length() + 2.0 * bounds.min().distance(p) + 1.0;
    'rays: for d in DIRECTIONS {
        let q = p + Vector3::new(d[0], d[1], d[2]) * reach;
        let mut w = 0i64;
        for &[a, b, c] in triangles {
            let (sp, sq) = (orient3d(a, b, c, p), orient3d(a, b, c, q));
            if sq == Sign::Zero {
                continue 'rays;
            }
            // `p` in the triangle's plane but off the triangle: the ray
            // meets that plane only at `p`, outside it.
            if sp == Sign::Zero || sp == sq {
                continue;
            }
            let s = [
                orient3d(p, q, a, b),
                orient3d(p, q, b, c),
                orient3d(p, q, c, a),
            ];
            if s.contains(&Sign::Zero) {
                let pos = s.contains(&Sign::Positive);
                let neg = s.contains(&Sign::Negative);
                if pos && neg {
                    continue;
                }
                continue 'rays;
            }
            if s[0] == s[1] && s[1] == s[2] {
                // `p` below the triangle (behind its front) and the ray
                // leaving through it: out of what the triangles enclose.
                w += if sp == Sign::Positive { 1 } else { -1 };
            }
        }
        return Some(w);
    }
    None
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

/// Whether `p` lies inside the closed mesh `tris`, unless it lies within
/// `band` of it or no ray settles it, and how many triangles it was
/// tested against: only those whose boxes come within `band` of it or
/// meet a ray (#618), so the verdict is the brute force's but where the
/// brute force gave up a ray for a triangle the ray never reaches.
fn classify(tris: &Tris, bounds: &Box3, p: Point3, band: f64) -> (Option<bool>, u64) {
    let grown = bounds.grow(band);
    if !grown.intersects(&Box3::from_point(p)) {
        return (Some(false), 0);
    }
    let mut tested = 0u64;
    // Only triangles whose boxes come within `band` of `p` can be.
    let reach_near = band * (1.0 + 1e-9);
    let mut close = false;
    tris.visit(
        |b| b.near(p, reach_near),
        |t| {
            tested += 1;
            close = near(p, *t) <= band;
            !close
        },
    );
    if close {
        return (None, tested);
    }
    let reach = 2.0 * (grown.max() - grown.min()).length() + 1.0;
    for d in DIRECTIONS {
        let q = p + Vector3::new(d[0], d[1], d[2]) * reach;
        let mut crossings = 0usize;
        // A triangle whose box the segment misses is neither crossed nor
        // touched by it, so it can't change the count or make the ray
        // unclean: testing it (the brute force) only gave the ray up when
        // `q` happened to lie in its plane.
        let clean = tris.visit(
            |b| b.meets(p, q),
            |&[a, b, c]| {
                tested += 1;
                let (sp, sq) = (orient3d(a, b, c, p), orient3d(a, b, c, q));
                if sq == Sign::Zero {
                    return false;
                }
                if sp == Sign::Zero || sp == sq {
                    // `p` in the triangle's plane, further than `band`
                    // from it: the ray meets that plane only at `p`,
                    // outside the triangle (a vertex in a planar face's
                    // plane); or both ends on one side.
                    return true;
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
                    return pos && neg;
                }
                if s[0] == s[1] && s[1] == s[2] {
                    crossings += 1;
                }
                true
            },
        );
        if clean {
            return (Some(crossings % 2 == 1), tested);
        }
    }
    (None, tested)
}

/// A box of a node of [`Tris`]'s tree, grown a little past rounding.
#[derive(Clone, Copy, Debug)]
struct Aabb {
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Aabb {
    /// Whether `p` lies within `d` of the box (by each axis: a box
    /// within `d` of a point holds every point that is).
    fn near(&self, p: Point3, d: f64) -> bool {
        let p = [p.x, p.y, p.z];
        (0..3).all(|k| p[k] >= self.lo[k] - d && p[k] <= self.hi[k] + d)
    }

    /// Whether the segment from `p` to `q` meets the box.
    fn meets(&self, p: Point3, q: Point3) -> bool {
        let (p, q) = ([p.x, p.y, p.z], [q.x, q.y, q.z]);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for k in 0..3 {
            let d = q[k] - p[k];
            if d == 0.0 {
                if p[k] < self.lo[k] || p[k] > self.hi[k] {
                    return false;
                }
                continue;
            }
            let (a, b) = ((self.lo[k] - p[k]) / d, (self.hi[k] - p[k]) / d);
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            t0 = t0.max(a);
            t1 = t1.min(b);
            if t0 > t1 {
                return false;
            }
        }
        true
    }
}

/// A node: its box, its triangles' range in `order`, and its children.
struct Node {
    bounds: Aabb,
    start: usize,
    end: usize,
    kids: Option<(usize, usize)>,
}

/// A shell's triangles with a box tree over them, so a point is tested
/// only against the triangles near it or along its rays (#618).
struct Tris {
    tris: Vec<[Point3; 3]>,
    order: Vec<usize>,
    nodes: Vec<Node>,
    /// How far boxes are grown past their triangles: a part in 10⁹ of the
    /// coordinates' size, far past what rounding the tests can do.
    pad: f64,
}

/// Triangles in a leaf.
const LEAF: usize = 8;

impl Tris {
    fn new(tris: Vec<[Point3; 3]>) -> Self {
        let scale = tris
            .iter()
            .flat_map(|t| t.iter())
            .map(|p| p.x.abs().max(p.y.abs()).max(p.z.abs()))
            .fold(0.0f64, f64::max);
        let mut t = Tris {
            order: (0..tris.len()).collect(),
            tris,
            nodes: Vec::new(),
            pad: scale * 1e-9 + 1e-300,
        };
        if !t.tris.is_empty() {
            t.build(0, t.tris.len());
        }
        t
    }

    fn build(&mut self, start: usize, end: usize) -> usize {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for &i in &self.order[start..end] {
            for p in &self.tris[i] {
                for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(c);
                    hi[k] = hi[k].max(c);
                }
            }
        }
        for k in 0..3 {
            lo[k] -= self.pad;
            hi[k] += self.pad;
        }
        let at = self.nodes.len();
        self.nodes.push(Node {
            bounds: Aabb { lo, hi },
            start,
            end,
            kids: None,
        });
        if end - start > LEAF {
            let axis = (0..3)
                .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
                .expect("three axes");
            let tris = &self.tris;
            let mid = start + (end - start) / 2;
            let key = |i: usize| {
                let t = &tris[i];
                [t[0], t[1], t[2]]
                    .iter()
                    .map(|p| [p.x, p.y, p.z][axis])
                    .sum::<f64>()
            };
            self.order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
                key(a).total_cmp(&key(b)).then(a.cmp(&b))
            });
            let left = self.build(start, mid);
            let right = self.build(mid, end);
            self.nodes[at].kids = Some((left, right));
        }
        at
    }

    /// Calls `each` on every triangle in a node `wanted` takes, every
    /// ancestor taken too, until it answers false; whether it never did.
    fn visit(
        &self,
        wanted: impl Fn(&Aabb) -> bool,
        mut each: impl FnMut(&[Point3; 3]) -> bool,
    ) -> bool {
        let mut stack = Vec::new();
        if !self.nodes.is_empty() {
            stack.push(0);
        }
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !wanted(&node.bounds) {
                continue;
            }
            match node.kids {
                Some((l, r)) => {
                    stack.push(r);
                    stack.push(l);
                }
                None => {
                    for &i in &self.order[node.start..node.end] {
                        if !each(&self.tris[i]) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
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

    /// A box's twelve triangles, counterclockwise seen from outside:
    /// `w × d × h` at the origin.
    fn slab(w: f64, d: f64, h: f64) -> Vec<[Point3; 3]> {
        let p = |x: f64, y: f64, z: f64| Point3::new(x * w, y * d, z * h);
        let quads = [
            [p(0., 0., 0.), p(0., 1., 0.), p(1., 1., 0.), p(1., 0., 0.)],
            [p(0., 0., 1.), p(1., 0., 1.), p(1., 1., 1.), p(0., 1., 1.)],
            [p(0., 0., 0.), p(1., 0., 0.), p(1., 0., 1.), p(0., 0., 1.)],
            [p(0., 1., 0.), p(0., 1., 1.), p(1., 1., 1.), p(1., 1., 0.)],
            [p(0., 0., 0.), p(0., 0., 1.), p(0., 1., 1.), p(0., 1., 0.)],
            [p(1., 0., 0.), p(1., 1., 0.), p(1., 1., 1.), p(1., 0., 1.)],
        ];
        quads
            .iter()
            .flat_map(|[a, b, c, d]| [[*a, *b, *c], [*a, *c, *d]])
            .collect()
    }

    /// A mesh missing its planar faces (as when the mesher refuses them)
    /// is closed again by fans over their edge polylines: the volume is
    /// the full mesh's up to rounding (the cylinder's caps' fans and its
    /// mesher's triangles cover the same polygons), and shell facing gives
    /// the same sign. #240's Revo toolhead.
    #[test]
    fn planar_faces_left_out_are_closed_by_fans() {
        use crate::geom::Rigid;
        let (body, _) =
            crate::primitive::cylinder(crate::name::OpKey(3), 0.01, 0.02, &Rigid::IDENTITY)
                .unwrap();
        let faces: Vec<FaceId> = body.faces().collect();
        let chord = 1e-4;
        let mut m = crate::mesh::tessellate(&body, &MeshParams::new(chord, 0.5)).unwrap();
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
        // unmetered: a test's one judgement.
        let meter = Meter::free();
        assert_eq!(
            mesh_sign(&body, &m, &faces, chord, name, &meter).unwrap(),
            Some(true)
        );
        // A face that isn't planar still asks for a finer mesh.
        let side = faces.iter().copied().find(|f| !planes.contains(f)).unwrap();
        m.faces.retain(|fm| fm.face != side);
        assert!(shell_triangles(&body, &m, &faces).is_err());
        assert_eq!(
            mesh_sign(&body, &m, &faces, chord, name, &meter).unwrap(),
            None
        );
    }

    #[test]
    fn a_slab_winds_once_round_its_inside_and_not_its_outside() {
        let t = slab(1.0, 0.5, 0.25);
        let b = Box3::from_points(t.iter().flatten().copied()).unwrap();
        assert_eq!(winding(&t, &b, Point3::new(0.5, 0.25, 0.1)), Some(1));
        assert_eq!(winding(&t, &b, Point3::new(1.5, 0.25, 0.1)), Some(0));
        let turned: Vec<_> = t.iter().map(|&[a, b, c]| [a, c, b]).collect();
        assert_eq!(winding(&turned, &b, Point3::new(0.5, 0.25, 0.1)), Some(-1));
    }

    /// A slab a hundredth as thick as it is wide, at a band a fifth of its
    /// thickness: its volume can't sign it against `4 × band × area`, but
    /// a point two bands inside does.
    #[test]
    fn a_thin_slab_faces_out_or_in_by_its_winding_number() {
        let t = slab(1.0, 1.0, 0.01);
        let band = 2e-3;
        let volume: f64 = t
            .iter()
            .map(|[p, q, r]| p.to_vector().dot(q.to_vector().cross(r.to_vector())) / 6.0)
            .sum();
        let area = 2.0 * (1.0 + 4.0 * 0.01);
        assert!(volume < 4.0 * band * area, "the volume can't decide");
        // unmetered: a test of the judgement alone
        let m = Meter::free();
        assert_eq!(wound(&t, band, &m).unwrap(), Some(true));
        let turned: Vec<_> = t.iter().map(|&[a, b, c]| [a, c, b]).collect();
        assert_eq!(wound(&turned, band, &m).unwrap(), Some(false));
        // Thinner than two bands, no point inside is clear of it.
        assert_eq!(wound(&slab(1.0, 1.0, 3e-3), band, &m).unwrap(), None);
    }

    /// The brute force `classify` replaced (#618): every triangle for the
    /// band, every triangle for each ray.
    fn every_triangle(t: &[[Point3; 3]], bounds: &Box3, p: Point3, band: f64) -> Option<bool> {
        let grown = bounds.grow(band);
        if !grown.intersects(&Box3::from_point(p)) {
            return Some(false);
        }
        if t.iter().any(|&t| near(p, t) <= band) {
            return None;
        }
        let reach = 2.0 * (grown.max() - grown.min()).length() + 1.0;
        'rays: for d in DIRECTIONS {
            let q = p + Vector3::new(d[0], d[1], d[2]) * reach;
            let mut crossings = 0usize;
            for &[a, b, c] in t {
                let (sp, sq) = (orient3d(a, b, c, p), orient3d(a, b, c, q));
                if sq == Sign::Zero {
                    continue 'rays;
                }
                if sp == Sign::Zero || sp == sq {
                    continue;
                }
                let s = [
                    orient3d(p, q, a, b),
                    orient3d(p, q, b, c),
                    orient3d(p, q, c, a),
                ];
                if s.contains(&Sign::Zero) {
                    if s.contains(&Sign::Positive) && s.contains(&Sign::Negative) {
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

    /// Points round a torus's mesh, its vertices among them, are judged
    /// by the box tree as by testing every triangle, at a small part of
    /// the tests (#618: a board's vertices against its parts' meshes).
    #[test]
    fn the_box_tree_classifies_as_every_triangle_does() {
        use crate::geom::Rigid;
        let (body, _) =
            crate::primitive::torus(crate::name::OpKey(5), 0.02, 0.006, &Rigid::IDENTITY).unwrap();
        let faces: Vec<FaceId> = body.faces().collect();
        let m = crate::mesh::tessellate(&body, &MeshParams::new(1e-4, 0.3)).unwrap();
        let (t, _) = shell_triangles(&body, &m, &faces).unwrap();
        assert!(t.len() > 2000, "{}", t.len());
        let bounds = Box3::from_points(t.iter().flatten().copied()).unwrap();
        let tree = Tris::new(t.clone());
        let mut points: Vec<Point3> = t.iter().step_by(37).map(|x| x[1]).collect();
        let n = 9;
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    let f = |i: usize, s: f64| (i as f64 / (n - 1) as f64 - 0.5) * s;
                    points.push(Point3::new(f(i, 0.06), f(j, 0.06), f(k, 0.016)));
                }
            }
        }
        let (mut tested, mut inside, mut unsure) = (0u64, 0, 0);
        for &p in &points {
            for band in [1e-4, 1e-6] {
                let (got, n) = classify(&tree, &bounds, p, band);
                assert_eq!(
                    got,
                    every_triangle(&t, &bounds, p, band),
                    "at {p:?}, band {band}"
                );
                tested += n;
                inside += usize::from(got == Some(true));
                unsure += usize::from(got.is_none());
            }
        }
        assert!(
            inside > 50 && unsure > 50,
            "{inside} inside, {unsure} unsure"
        );
        let brute = (2 * points.len() * t.len()) as u64;
        assert!(tested * 10 < brute, "{tested} tests against {brute}");
    }
}
