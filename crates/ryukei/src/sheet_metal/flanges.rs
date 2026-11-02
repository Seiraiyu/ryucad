//! Flanges: the rigid pieces between bends.
//!
//! Faces that are neither a bend's cylinders nor its walls (the other
//! faces meeting the cylinders) join across their shared edges, and the
//! planes across each bend line at one end of a bend (one on each side of
//! the sheet) join each other, so a flange whose sides meet only bends
//! and end faces (a channel's web) is still one piece. Each piece's sides
//! are the two planes its bends' lines give it, `t` apart and facing
//! apart; with no bends at all, the body is one flat piece whose sides are
//! the pair of opposite planes reaching furthest. Every other face of a
//! piece must lie in the slab between its sides ([`super::slab`]).

use super::bends::{Found, Piece};
use super::cells::Verdict;
use super::shape::{self, Flat, across, edges_of, flat};
use super::slab::{self, Slab};
use crate::SheetMetalError;
use crate::name::Name;
use crate::tol::ANGULAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use std::collections::{BTreeMap, BTreeSet};

/// A flange piece before it is named.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Part {
    /// Sorted.
    pub(super) faces: Vec<FaceId>,
    /// Each side's plane and faces (sorted), the side holding the
    /// least-named face first.
    pub(super) sides: [(Flat, Vec<FaceId>); 2],
}

impl Part {
    pub(super) fn slab(&self, thickness: f64) -> Slab {
        Slab::behind(&self.sides[0].0, thickness)
    }
}

/// What a body's faces are.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layout {
    pub(super) parts: Vec<Part>,
    /// The part each face of one belongs to.
    pub(super) of_face: BTreeMap<FaceId, usize>,
    /// Each piece's walls, sorted.
    pub(super) walls: Vec<Vec<FaceId>>,
    pub(super) thickness: f64,
    /// The first bend piece a side face meets other than across a bend
    /// line, and that face. When set, the layout is only far enough along
    /// to name the bend: `parts` is empty and `of_face` gives each face
    /// the root of its group, for [`super::recognize`] to group the pieces
    /// by and refuse it as `BendBlocked`.
    pub(super) blocked: Option<(usize, FaceId)>,
}

/// A union-find over face IDs.
struct Sets {
    up: BTreeMap<FaceId, FaceId>,
}

impl Sets {
    fn root(&self, mut f: FaceId) -> FaceId {
        while let Some(&g) = self.up.get(&f) {
            if g == f {
                break;
            }
            f = g;
        }
        f
    }

    /// Joins two sets, the lower ID the root, so roots don't depend on
    /// the order of joins.
    fn join(&mut self, a: FaceId, b: FaceId) {
        let (ra, rb) = (self.root(a), self.root(b));
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.up.insert(hi, lo);
    }
}

/// Whether two planes are one within `eps`.
fn same(a: &Flat, c: &Flat, eps: f64, scale: f64) -> bool {
    a.normal.get().dot(c.normal.get()) > 0.0
        && shape::parallel(a.normal, c.normal, ANGULAR_RESOLUTION + eps / scale)
        && a.height(c.origin).abs() <= eps
}

/// The two planes of a piece's sides from its bends' lines, each once.
fn planes_of(
    b: &Body,
    pieces: &[(usize, usize, &Piece)],
    eps: f64,
    scale: f64,
) -> Vec<(Flat, FaceId)> {
    let mut out: Vec<(Flat, FaceId)> = Vec::new();
    for &(_, end, p) in pieces {
        for outer in [false, true] {
            for f in p.sides(end, outer) {
                let Some(fl) = flat(b, f) else { continue };
                if !out.iter().any(|(x, _)| same(x, &fl, eps, scale)) {
                    out.push((fl, f));
                }
            }
        }
    }
    out
}

/// The extent of a planar face: the area of the box of its vertices in
/// its plane, for choosing a flat body's sides.
fn extent(b: &Body, f: FaceId, plane: &Flat) -> f64 {
    let n = plane.normal.get();
    let helper = if n.x.abs() < 0.9 {
        crate::geom::Vector3::new(1.0, 0.0, 0.0)
    } else {
        crate::geom::Vector3::new(0.0, 1.0, 0.0)
    };
    let x = n.cross(helper);
    let y = n.cross(x);
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for e in edges_of(b, f) {
        for p in shape::ends(b, e) {
            let d = p - plane.origin;
            for (k, a) in [d.dot(x), d.dot(y)].into_iter().enumerate() {
                lo[k] = lo[k].min(a);
                hi[k] = hi[k].max(a);
            }
        }
    }
    (hi[0] - lo[0]).max(0.0) * (hi[1] - lo[1]).max(0.0)
}

/// Two planar faces facing apart with material between them: the depth
/// between their planes, if they are.
pub(super) fn depth(a: &Flat, c: &Flat, eps: f64, scale: f64) -> Option<f64> {
    let opposite = a.normal.get().dot(c.normal.get()) < 0.0
        && shape::parallel(a.normal, c.normal, ANGULAR_RESOLUTION + eps / scale);
    let d = -a.height(c.origin);
    (opposite && d > eps).then_some(d)
}

/// The sides of a body with no bends: the pair of opposite planar faces
/// with the greatest extent between them, ties to the least name.
pub(super) fn flat_sides(b: &Body, tol: f64, scale: f64) -> Option<(Flat, Flat, f64)> {
    let planar: Vec<(FaceId, Flat, f64)> = b
        .faces()
        .filter_map(|f| flat(b, f).map(|p| (f, p, extent(b, f, &p))))
        .collect();
    let mut best: Option<(f64, Name, Flat, Flat, f64)> = None;
    for (i, (f, p, ef)) in planar.iter().enumerate() {
        for (g, q, eg) in &planar[i + 1..] {
            let eps = tol + shape::face_tolerance(b, *f).max(shape::face_tolerance(b, *g));
            let Some(d) = depth(p, q, eps, scale) else {
                continue;
            };
            let score = ef + eg;
            let name = b.faces[*f].name.min(b.faces[*g].name);
            let better = match &best {
                None => true,
                Some((s, n, ..)) => score > *s || (score == *s && name < *n),
            };
            if better {
                best = Some((score, name, *p, *q, d));
            }
        }
    }
    best.map(|(_, _, p, q, d)| (p, q, d))
}

/// Lays a body's faces out into flange pieces and bend walls. `scale` is
/// the body's size, for angles from lengths.
pub(super) fn layout(
    b: &Body,
    found: &Found,
    thickness: f64,
    tol: f64,
    scale: f64,
) -> Result<Layout, SheetMetalError> {
    let name = |f: FaceId| b.faces[f].name;
    let mut cylinders = BTreeSet::new();
    let mut sides = BTreeSet::new();
    for p in &found.pieces {
        cylinders.extend(p.faces());
        sides.extend(p.lines.iter().map(|l| l.side));
    }
    // Walls: what meets a piece's cylinders other than across its lines.
    let mut walls: Vec<Vec<FaceId>> = Vec::with_capacity(found.pieces.len());
    let mut blocked = None;
    for (i, p) in found.pieces.iter().enumerate() {
        let mut w = BTreeSet::new();
        for c in p.faces() {
            for e in edges_of(b, c) {
                let Some(g) = across(b, e, c) else { continue };
                if cylinders.contains(&g) || p.lines.iter().any(|l| l.edge == e) {
                    continue;
                }
                if sides.contains(&g) {
                    blocked = blocked.or(Some((i, g)));
                    continue;
                }
                w.insert(g);
            }
        }
        walls.push(w.into_iter().collect());
    }
    let all_walls: BTreeSet<FaceId> = walls.iter().flatten().copied().collect();
    let free = |f: FaceId| !cylinders.contains(&f) && !all_walls.contains(&f);
    let mut sets = Sets {
        up: b.faces().filter(|&f| free(f)).map(|f| (f, f)).collect(),
    };
    for e in b.edges() {
        if let [f, g] = shape::faces_of(b, e).as_slice()
            && free(*f)
            && free(*g)
        {
            sets.join(*f, *g);
        }
    }
    for p in &found.pieces {
        for end in [0, 1] {
            let at: Vec<FaceId> = [false, true]
                .into_iter()
                .flat_map(|o| p.sides(end, o))
                .collect();
            for w in at.windows(2) {
                sets.join(w[0], w[1]);
            }
        }
    }
    let mut groups: BTreeMap<FaceId, Vec<FaceId>> = BTreeMap::new();
    for f in b.faces().filter(|&f| free(f)) {
        groups.entry(sets.root(f)).or_default().push(f);
    }
    if blocked.is_some() {
        let of_face = groups
            .values()
            .enumerate()
            .flat_map(|(k, fs)| fs.iter().map(move |&f| (f, k)))
            .collect();
        return Ok(Layout {
            parts: Vec::new(),
            of_face,
            walls,
            thickness,
            blocked,
        });
    }
    if found.pieces.is_empty() && groups.len() > 1 {
        return Err(SheetMetalError::Disconnected);
    }
    // Which piece end each group's faces touch.
    let mut ends: BTreeMap<FaceId, Vec<(usize, usize, &Piece)>> = BTreeMap::new();
    for (i, p) in found.pieces.iter().enumerate() {
        for end in [0, 1] {
            if let Some(&f) = p.sides(end, false).first() {
                ends.entry(sets.root(f)).or_default().push((i, end, p));
            }
        }
    }
    let mut parts = Vec::new();
    let mut of_face = BTreeMap::new();
    for (root, faces) in &groups {
        let least = faces.iter().map(|&f| name(f)).min().expect("a face");
        let eps = tol
            + faces
                .iter()
                .map(|&f| shape::face_tolerance(b, f))
                .fold(0.0, f64::max);
        let planes: Vec<(Flat, FaceId)> = match ends.get(root) {
            Some(list) => planes_of(b, list, eps, scale),
            None if found.pieces.is_empty() => match flat_sides(b, tol, scale) {
                Some((p, q, _)) => vec![(p, faces[0]), (q, faces[0])],
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let [(p, pf), (q, qf)] = planes.as_slice() else {
            return Err(SheetMetalError::NotSheet {
                face: planes.get(2).map_or(least, |(_, f)| name(*f)),
            });
        };
        // Both planes come from bend lines (each piece's are its own
        // thickness apart, which the pieces agree on) or are the flat body's
        // pair, so this guards tolerances, not a body built to fail it.
        match depth(p, q, eps, scale) {
            Some(d) if (d - thickness).abs() <= eps => {}
            _ => {
                let mut pair = [name(*pf), name(*qf)];
                pair.sort_unstable();
                return Err(SheetMetalError::MixedThickness { faces: pair });
            }
        }
        let on = |plane: &Flat| -> Vec<FaceId> {
            faces
                .iter()
                .copied()
                .filter(|&f| flat(b, f).is_some_and(|x| same(plane, &x, eps, scale)))
                .collect()
        };
        let (mut a, mut c) = ((*p, on(p)), (*q, on(q)));
        let least_of = |fs: &[FaceId]| fs.iter().map(|&f| name(f)).min();
        if least_of(&c.1) < least_of(&a.1) || a.1.is_empty() {
            core::mem::swap(&mut a, &mut c);
        }
        let part = Part {
            faces: faces.clone(),
            sides: [a, c],
        };
        let slab = part.slab(thickness);
        for &f in faces {
            if part.sides.iter().any(|(_, s)| s.contains(&f)) {
                continue;
            }
            match slab::face_within(b, f, &slab, tol) {
                Verdict::Yes => {}
                Verdict::No => return Err(SheetMetalError::OutOfSlab { face: name(f) }),
                Verdict::Unsure => return Err(SheetMetalError::Uncertain { at: name(f) }),
            }
        }
        for &f in faces {
            of_face.insert(f, parts.len());
        }
        parts.push(part);
    }
    Ok(Layout {
        parts,
        of_face,
        walls,
        thickness,
        blocked: None,
    })
}
