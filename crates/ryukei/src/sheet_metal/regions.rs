//! Where each face, edge and vertex of a flat lies, for refolding: in a
//! strip it folds, or in a group of faces that moves rigidly, and the
//! motion of each group and the folded axis of each strip.
//!
//! A strip is the box `0 <= x <= ba`, `y` in its span, `0 <= z <= t` of
//! its frame, grown by the tolerance. A face is in a strip when every
//! point sampled along its edges is in the box and one lies strictly
//! between its bend lines. A plane square to every strip it crosses (a
//! bracket's end face) may run across: refolding takes it to one plane.
//! Any other face that crosses a bend line is `NotRadial`: refolding cut
//! every face it can at the bend lines first ([`super::imprint`]). The
//! faces in no strip join into groups across the edges they share, and
//! the strips join the groups: each group beside a strip's near bend line
//! moves as that strip's near flange does, and the group beside its far
//! line as its far flange.

use super::shape;
use super::{FlatMap, Strip};
use crate::SheetMetalError;
use crate::geom::{Frame, Point3, Rigid, Surface, Vector3};
use crate::name::Name;
use crate::topo::{Body, EdgeId, FaceId, VertexId};
use std::collections::BTreeMap;

/// Where an entity of a flat lies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Region {
    /// In strip `k`, which folds.
    Strip(usize),
    /// A plane square to the strips it crosses: a face only.
    Across,
    /// In group `g`, which moves rigidly.
    Group(usize),
}

/// A flat's regions and how each moves.
#[derive(Clone, Debug)]
pub(super) struct Regions {
    pub(super) faces: BTreeMap<FaceId, Region>,
    pub(super) edges: BTreeMap<EdgeId, Region>,
    pub(super) vertices: BTreeMap<VertexId, Region>,
    /// Each group's motion from the flat to where it is folded.
    pub(super) motions: Vec<Rigid>,
    /// Each strip's axis where it is folded, for the strips that fold.
    pub(super) axes: Vec<Option<Frame>>,
}

/// A point in a strip's coordinates.
fn local(s: &Strip, p: Point3) -> Vector3 {
    s.frame.to_local(p)
}

/// Whether a point in a strip's coordinates lies in its box, grown by
/// `eps`.
pub(super) fn inside(s: &Strip, t: f64, l: Vector3, eps: f64) -> bool {
    -eps <= l.x
        && l.x <= s.length + eps
        && s.span[0] - eps <= l.y
        && l.y <= s.span[1] + eps
        && -eps <= l.z
        && l.z <= t + eps
}

/// Whether a point in the box lies strictly between its bend lines.
fn between(s: &Strip, t: f64, l: Vector3, eps: f64) -> bool {
    inside(s, t, l, eps) && eps < l.x && l.x < s.length - eps
}

/// Points along an edge: its ends and three between.
pub(super) fn samples(b: &Body, e: EdgeId) -> Vec<Point3> {
    let edge = &b.edges[e];
    let [t0, t1] = edge.interval;
    let c = &b.curves[edge.curve];
    let mut out: Vec<Point3> = shape::ends(b, e).to_vec();
    out.extend([0.25, 0.5, 0.75].map(|f| c.point(t0 + f * (t1 - t0))));
    out
}

/// Whether a plane face is square to a strip's `y`, the axis it folds
/// about.
fn square_to(b: &Body, f: FaceId, s: &Strip) -> bool {
    let Surface::Plane(p) = &b.surfaces[b.faces[f].surface] else {
        return false;
    };
    let n = p.frame().z().get();
    (n.dot(s.frame.y().get()).abs() - 1.0).abs() <= 1e-12
}

/// A strip's flange's motion, by name.
fn motion(map: &FlatMap, name: Name, at: Name) -> Result<Rigid, SheetMetalError> {
    map.flanges
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, m)| *m)
        .ok_or(SheetMetalError::Uncertain { at })
}

/// Classifies a flat for refolding the strips `folds` marks. `eps` is
/// the tolerance boxes grow by.
pub(super) fn classify(
    b: &Body,
    map: &FlatMap,
    folds: &[bool],
    eps: f64,
) -> Result<Regions, SheetMetalError> {
    let t = map.thickness;
    let strips: Vec<(usize, &Strip)> = map
        .strips
        .iter()
        .enumerate()
        .filter(|(k, _)| folds[*k])
        .collect();
    let edge_points: BTreeMap<EdgeId, Vec<Point3>> =
        b.edges().map(|e| (e, samples(b, e))).collect();
    let mut faces: BTreeMap<FaceId, Region> = BTreeMap::new();
    let mut candidates: Vec<FaceId> = Vec::new();
    for f in b.faces() {
        let pts: Vec<Point3> = shape::edges_of(b, f)
            .iter()
            .flat_map(|e| edge_points[e].iter().copied())
            .collect();
        let mut region = None;
        let mut crossed = Vec::new();
        for &(k, s) in &strips {
            let ls: Vec<Vector3> = pts.iter().map(|&p| local(s, p)).collect();
            let all = ls.iter().all(|&l| inside(s, t, l, eps));
            let some = ls.iter().any(|&l| between(s, t, l, eps));
            if all && some {
                region = Some(Region::Strip(k));
                break;
            }
            if some {
                crossed.push(s);
            }
        }
        match region {
            Some(r) => {
                faces.insert(f, r);
            }
            None if crossed.is_empty() => candidates.push(f),
            None if crossed.iter().all(|s| square_to(b, f, s)) => {
                faces.insert(f, Region::Across);
            }
            None => {
                return Err(SheetMetalError::NotRadial {
                    face: b.faces[f].name,
                });
            }
        }
    }
    // Groups: faces in no strip joined across their edges.
    let index: BTreeMap<FaceId, usize> = candidates
        .iter()
        .enumerate()
        .map(|(i, &f)| (f, i))
        .collect();
    let mut parent: Vec<usize> = (0..candidates.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for (i, &f) in candidates.iter().enumerate() {
        for g in shape::neighbours(b, f) {
            if let Some(&j) = index.get(&g) {
                let (a, c) = (find(&mut parent, i), find(&mut parent, j));
                if a != c {
                    parent[a.max(c)] = a.min(c);
                }
            }
        }
    }
    // The faces beside each folded strip's near bend line and its far
    // one: the faces beside one line are one flange's, joined though no
    // face of it joins them (a flange between two bends has only its two
    // sides, which the end faces running across join to everything).
    let mut sides: Vec<[Vec<usize>; 2]> = vec![[Vec::new(), Vec::new()]; map.strips.len()];
    for &(k, s) in &strips {
        for (&f, r) in &faces {
            if *r != Region::Strip(k) {
                continue;
            }
            for e in shape::edges_of(b, f) {
                let Some(g) = shape::across(b, e, f) else {
                    continue;
                };
                let Some(&i) = index.get(&g) else {
                    continue;
                };
                let x = local(s, shape::middle(b, e)).x;
                if x.abs() <= eps {
                    sides[k][0].push(i);
                } else if (x - s.length).abs() <= eps {
                    sides[k][1].push(i);
                } else {
                    return Err(SheetMetalError::BendBlocked {
                        bend: s.bend,
                        face: b.faces[g].name,
                    });
                }
            }
        }
        for side in &sides[k] {
            for w in side.windows(2) {
                let (a, c) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                if a != c {
                    parent[a.max(c)] = a.min(c);
                }
            }
        }
    }
    let mut group_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, &f) in candidates.iter().enumerate() {
        let root = find(&mut parent, i);
        let n = group_of.len();
        let g = *group_of.entry(root).or_insert(n);
        faces.insert(f, Region::Group(g));
    }
    let groups = group_of.len();
    let mut near: Vec<Option<usize>> = vec![None; map.strips.len()];
    let mut far: Vec<Option<usize>> = vec![None; map.strips.len()];
    for &(k, s) in &strips {
        let group = |i: usize| match faces[&candidates[i]] {
            Region::Group(g) => g,
            _ => unreachable!("a candidate is in a group"),
        };
        let (Some(&a), Some(&c)) = (sides[k][0].first(), sides[k][1].first()) else {
            return Err(SheetMetalError::Uncertain { at: s.bend });
        };
        let (a, c) = (group(a), group(c));
        if a == c {
            return Err(SheetMetalError::BendBlocked {
                bend: s.bend,
                face: b.faces[candidates[sides[k][1][0]]].name,
            });
        }
        near[k] = Some(a);
        far[k] = Some(c);
    }
    // Every group is reached across one strip's far line at most.
    let mut reached_by: Vec<Option<usize>> = vec![None; groups];
    for &(k, s) in &strips {
        let g = far[k].expect("found above");
        if reached_by[g].replace(k).is_some() {
            return Err(SheetMetalError::Uncertain { at: s.bend });
        }
    }
    // Motions, from the groups no strip reaches outwards. Folding every
    // strip, a group moves back exactly by the inverse of its flange's
    // motion into the flat; folding some, by the product of the folds
    // between it and the fixed flange, each in the flat
    // (`M_near M_far⁻¹`).
    let every = folds.iter().all(|&f| f);
    let mut motions: Vec<Option<Rigid>> = reached_by
        .iter()
        .map(|r| r.is_none().then_some(Rigid::IDENTITY))
        .collect();
    let mut axes: Vec<Option<Frame>> = vec![None; map.strips.len()];
    let mut left: Vec<(usize, &Strip)> = strips.clone();
    while !left.is_empty() {
        let before = left.len();
        let mut rest = Vec::new();
        for (k, s) in left {
            let (gn, gf) = (near[k].expect("found"), far[k].expect("found"));
            let Some(pn) = motions[gn] else {
                rest.push((k, s));
                continue;
            };
            let mn = motion(map, s.near, s.bend)?;
            let mf = motion(map, s.far, s.bend)?;
            if every {
                motions[gf] = Some(mf.inverse());
                axes[k] = Some(s.axis);
            } else {
                let back = pn.then_after(&mn);
                motions[gf] = Some(back.then_after(&mf.inverse()));
                axes[k] = Some(s.axis.transformed(&back));
            }
        }
        if rest.len() == before {
            return Err(SheetMetalError::Uncertain { at: rest[0].1.bend });
        }
        left = rest;
    }
    let motions: Vec<Rigid> = motions
        .into_iter()
        .map(|m| m.expect("every group reached"))
        .collect();
    // Edges and vertices: in a strip's box, else with a face's group.
    let in_strip = |pts: &[Point3]| {
        strips.iter().find_map(|&(k, s)| {
            pts.iter()
                .all(|&p| inside(s, t, local(s, p), eps))
                .then_some(k)
        })
    };
    let mut edges = BTreeMap::new();
    for e in b.edges() {
        let region = match in_strip(&edge_points[&e]) {
            Some(k) => Region::Strip(k),
            None => shape::faces_of(b, e)
                .iter()
                .find_map(|f| match faces[f] {
                    Region::Group(g) => Some(Region::Group(g)),
                    _ => None,
                })
                .ok_or(SheetMetalError::Uncertain {
                    at: b.edges[e].name,
                })?,
        };
        edges.insert(e, region);
    }
    let mut vertices = BTreeMap::new();
    for v in b.vertices() {
        let p = b.vertices[v].point;
        let region = match in_strip(&[p]) {
            Some(k) => Region::Strip(k),
            None => b
                .vertex_edges(v)
                .iter()
                .find_map(|e| match edges[e] {
                    Region::Group(g) => Some(Region::Group(g)),
                    _ => None,
                })
                .ok_or(SheetMetalError::Uncertain {
                    at: b.vertices[v].name,
                })?,
        };
        vertices.insert(v, region);
    }
    Ok(Regions {
        faces,
        edges,
        vertices,
        motions,
        axes,
    })
}
