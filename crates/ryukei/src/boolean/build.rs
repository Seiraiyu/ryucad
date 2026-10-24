//! The shell builder: kept pieces into a new body, through Euler
//! operators.
//!
//! The kept pieces are laid out as a [`Layout`] (global edges and
//! vertices by their table indices, faces by piece, geometry shared as the
//! operands share it) and built by [`crate::layout`], which refuses a
//! non-manifold result.
//!
//! Each shell is first built as a solid region of its own. A shell whose
//! outward side is inside it (a point just off one of its faces, along
//! the outward normal, classified against it) is a void: it goes in the
//! region of the smallest outer shell around it, and the body is built
//! again with it made by MVS there.

use super::table::{Boundary, Member, Table};
use super::{FaceRef, Fail};
use crate::geom::{Box3, Curve2, Point3};
use crate::intersect::{Inclusion, face_box, point_in_faces};
use crate::layout::{self, BuildError, Built, Layout, Plan};
use crate::name::{BooleanRole, OpKey, Role};
use crate::split::Piece;
use crate::topo::{Body, FaceId, Sense, SurfaceId};
use std::collections::BTreeMap;

/// A piece kept for the result, run backwards when `reversed`.
#[derive(Clone, Debug)]
pub(crate) struct Kept {
    pub(crate) piece: Piece,
    pub(crate) reversed: bool,
}

impl Kept {
    /// Its loops as the result runs them.
    pub(crate) fn loops(&self) -> Vec<Boundary> {
        if !self.reversed {
            return self.piece.loops.clone();
        }
        self.piece
            .loops
            .iter()
            .map(|l| match l {
                Boundary::Lone(g) => Boundary::Lone(*g),
                Boundary::Arcs(a) => {
                    Boundary::Arcs(a.iter().rev().map(|&(g, s)| (g, s.flipped())).collect())
                }
            })
            .collect()
    }
}

/// The scaffold names' role: every entity is renamed afterwards.
const ROLE: Role = Role::Boolean(BooleanRole::Merged);

/// Builds the kept pieces into a body.
pub(crate) fn build(ops: &[Body], table: &Table, kept: &[Kept], key: OpKey) -> Result<Built, Fail> {
    let layout = lay_out(ops, table, kept);
    let fail = |e: BuildError| match e {
        BuildError::NonManifold { at, faces } => Fail::NonManifold {
            at,
            faces: faces.into_iter().map(|k| kept[k].piece.face).collect(),
        },
        BuildError::Inconsistent => Fail::Inconsistent,
    };
    let plan = Plan::new(&layout).map_err(fail)?;
    let shells = plan.shells();
    let first = plan
        .build(key, ROLE, &shells, &vec![None; shells.len()])
        .map_err(fail)?;
    // Which shells are voids, and the outer shell around each.
    let mut outer_of: Vec<Option<usize>> = vec![None; shells.len()];
    let mut voids = Vec::new();
    // A lone shell bounds the result's only region: never a void.
    if shells.len() == 1 {
        return Ok(first);
    }
    for (s, faces) in shells.iter().enumerate() {
        if is_void(ops, table, kept, &first, faces)? {
            voids.push(s);
        }
    }
    if voids.is_empty() {
        return Ok(first);
    }
    let outers: Vec<usize> = (0..shells.len()).filter(|s| !voids.contains(s)).collect();
    for &s in &voids {
        let p = point_on(ops, table, kept, &first, &shells[s])?;
        let mut best: Option<(f64, usize)> = None;
        for &o in &outers {
            let faces: Vec<FaceId> = shells[o].iter().map(|&k| first.faces[k]).collect();
            match point_in_faces(&first.body, &faces, p) {
                Ok(Inclusion::Inside) => {
                    let b = faces
                        .iter()
                        .map(|&f| face_box(&first.body, f))
                        .reduce(|x, y| x.union(&y))
                        .map_or(0.0, |b| volume(&b));
                    if best.is_none_or(|(v, _)| b < v) {
                        best = Some((b, o));
                    }
                }
                Ok(Inclusion::Outside) => {}
                _ => return Err(uncertain(kept, &shells[s])),
            }
        }
        // A void outside every outer shell can't be in a regularized
        // result.
        outer_of[s] = Some(best.ok_or(Fail::Inconsistent)?.1);
    }
    plan.build(key, ROLE, &shells, &outer_of).map_err(fail)
}

/// The kept pieces as a layout: global edges and vertices keep their
/// table indices, faces are the pieces in order, and geometry is shared
/// as the operands share it: a surface per operand surface, a curve per
/// carrier, a pcurve per face, carrier and curve.
fn lay_out(ops: &[Body], table: &Table, kept: &[Kept]) -> Layout {
    let mut out = Layout {
        vertices: table
            .vertices
            .iter()
            .map(|v| layout::Vertex {
                point: v.point,
                tolerance: v.tolerance,
            })
            .collect(),
        ..Layout::default()
    };
    let mut curves: BTreeMap<Member, usize> = BTreeMap::new();
    for e in &table.edges {
        let curve = *curves.entry(e.members[0]).or_insert_with(|| {
            out.curves.push(e.curve.clone());
            out.curves.len() - 1
        });
        out.edges.push(layout::Edge {
            start: e.start,
            end: e.end,
            curve,
            interval: e.interval,
            tolerance: e.tolerance,
        });
    }
    let mut surfaces: BTreeMap<(usize, SurfaceId), usize> = BTreeMap::new();
    let mut curves2: BTreeMap<(FaceRef, Member), Vec<(Curve2, usize)>> = BTreeMap::new();
    for k in kept {
        let src = k.piece.face;
        let b = &ops[src.op];
        let s = b.faces[src.face].surface;
        let surface = *surfaces.entry((src.op, s)).or_insert_with(|| {
            out.surfaces.push(b.surfaces[s].clone());
            out.surfaces.len() - 1
        });
        let mut sense = b.faces[src.face].sense;
        if k.reversed {
            sense = sense.flipped();
        }
        let loops = k
            .loops()
            .into_iter()
            .map(|l| match l {
                Boundary::Lone(g) => layout::Boundary::Lone(g),
                Boundary::Arcs(a) => layout::Boundary::Arcs(
                    a.into_iter()
                        .map(|(g, s)| {
                            let (c, map) = &table.edges[g].pcurves[&src];
                            let list = curves2.entry((src, table.edges[g].members[0])).or_default();
                            let id = match list.iter().find(|x| x.0 == *c) {
                                Some(x) => x.1,
                                None => {
                                    out.curves2.push(c.clone());
                                    list.push((c.clone(), out.curves2.len() - 1));
                                    out.curves2.len() - 1
                                }
                            };
                            layout::Use {
                                edge: g,
                                sense: s,
                                pcurve: Some((id, *map)),
                            }
                        })
                        .collect(),
                ),
            })
            .collect();
        out.faces.push(layout::Face {
            surface,
            sense,
            loops,
        });
    }
    out
}

fn volume(b: &Box3) -> f64 {
    let d = b.max() - b.min();
    d.x * d.y * d.z
}

fn uncertain(kept: &[Kept], shell: &[usize]) -> Fail {
    let f = kept[shell[0]].piece.face;
    Fail::Uncertain(f, f)
}

/// Whether a shell's faces point into what it encloses: a point just
/// off one of them, along its outward normal, is inside the shell.
fn is_void(
    ops: &[Body],
    table: &Table,
    kept: &[Kept],
    built: &Built,
    shell: &[usize],
) -> Result<bool, Fail> {
    let faces: Vec<FaceId> = shell.iter().map(|&k| built.faces[k]).collect();
    let size = faces
        .iter()
        .map(|&f| face_box(&built.body, f))
        .reduce(|x, y| x.union(&y))
        .map_or(1.0, |b| (b.max() - b.min()).length());
    for &k in shell {
        for attempt in 0..4 {
            let Some((q, uv)) = super::classify::inner_point(ops, table, &kept[k].piece, attempt)
            else {
                continue;
            };
            let src = kept[k].piece.face;
            let b = &ops[src.op];
            let surface = &b.surfaces[b.faces[src.face].surface];
            let Some(n) = surface.normal(uv.x, uv.y) else {
                continue;
            };
            let mut sign = if b.faces[src.face].sense == Sense::Forward {
                1.0
            } else {
                -1.0
            };
            if kept[k].reversed {
                sign = -sign;
            }
            // Nearest first: a point pushed further off a face may
            // cross into the solid beyond a neighbour meeting it at a
            // shallow angle.
            for eps in [1e-6, 1e-5, 1e-4] {
                let p = q + n.get() * (sign * eps * size);
                match point_in_faces(&built.body, &faces, p) {
                    Ok(Inclusion::Inside) => return Ok(true),
                    Ok(Inclusion::Outside) => return Ok(false),
                    _ => {}
                }
            }
        }
    }
    Err(uncertain(kept, shell))
}

/// A point of a shell: one of its vertices, or a point of its first
/// face.
fn point_on(
    ops: &[Body],
    table: &Table,
    kept: &[Kept],
    built: &Built,
    shell: &[usize],
) -> Result<Point3, Fail> {
    let f = built.faces[shell[0]];
    let b = &built.body;
    for l in b.face_loops(f) {
        if let Some(v) = b.loop_(l).and_then(|l| l.vertex) {
            return Ok(b.vertex(v).expect("live").point);
        }
        if let Some(fin) = b.loop_fins(l).next() {
            let v = b.fin_start(fin).expect("live");
            return Ok(b.vertex(v).expect("live").point);
        }
    }
    let (q, _) = super::classify::inner_point(ops, table, &kept[shell[0]].piece, 0)
        .ok_or_else(|| uncertain(kept, shell))?;
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean::table;
    use crate::geom::{Rigid, Vector3};
    use crate::mesh::{MeshParams, check_mesh, tessellate};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::split::split_all;

    /// The volume and area of a body's mesh at a chord of 2e-3.
    fn volume(b: &Body) -> (f64, f64) {
        let m = tessellate(b, &MeshParams::new(2e-3, 0.3)).unwrap();
        assert!(m.faults.is_empty(), "{:?}", m.faults);
        check_mesh(b, &m).unwrap();
        m.welded_triangles()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
                (a.dot(b.cross(c)) / 6.0, (b - a).cross(c - a).length() / 2.0)
            })
            .fold((0.0, 0.0), |(v, s), (dv, ds)| (v + dv, s + ds))
    }

    /// A body built again from its own faces.
    fn rebuilt(b: &Body) -> Body {
        let ops = [b.clone()];
        let t = table::build(&ops).unwrap();
        let pieces = split_all(&ops, &t).unwrap();
        let kept: Vec<Kept> = pieces
            .into_iter()
            .map(|piece| Kept {
                piece,
                reversed: false,
            })
            .collect();
        let out = build(&ops, &t, &kept, OpKey(77)).unwrap().body;
        assert_eq!(crate::check::check(&out), [], "{}", out.dump());
        out
    }

    fn same(a: &Body) {
        let b = rebuilt(a);
        let (ca, cb) = (a.counts(), b.counts());
        assert_eq!(ca, cb);
        // Both meshes lie within the chord of the same surfaces, though
        // faces whose loops come in another order mesh differently.
        let ((va, area), (vb, _)) = (volume(a), volume(&b));
        assert!((va - vb).abs() <= 2.0 * 2e-3 * area, "{va} {vb}");
    }

    #[test]
    fn primitives_build_again() {
        let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        let k = OpKey(1);
        same(&primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0);
        same(&primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0);
        same(&primitive::cone(k, 1.0, 2.0, &t).unwrap().0);
        same(&primitive::frustum(k, 1.0, 0.4, 2.0, &t).unwrap().0);
        same(&primitive::sphere(k, 1.0, &t).unwrap().0);
        same(&primitive::torus(k, 2.0, 0.5, &t).unwrap().0);
    }

    #[test]
    fn handles_and_cavities_build_again() {
        use crate::geom::{Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
        use crate::sweep::{
            Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude, revolve,
        };
        let square = |x: f64, y: f64, s: f64, key: u128| -> Vec<Segment> {
            let c = [
                Point2::new(x, y),
                Point2::new(x + s, y),
                Point2::new(x + s, y + s),
                Point2::new(x, y + s),
            ];
            (0..4)
                .map(|i| Segment {
                    key: SegmentKey(key + i as u128),
                    curve: Curve2::Line2(Line2::through(c[i], c[(i + 1) % 4]).unwrap()),
                    t0: 0.0,
                    t1: s,
                })
                .collect()
        };
        let circle = |x: f64, y: f64, r: f64, key: u128| Segment {
            key: SegmentKey(key),
            curve: Curve2::Circle2(
                Circle2::new(
                    Frame2::new(Point2::new(x, y), Vector2::new(1.0, 0.0), true).unwrap(),
                    r,
                )
                .unwrap(),
            ),
            t0: 0.0,
            t1: core::f64::consts::TAU,
        };
        // A plate with a square hole and a round one: genus two.
        let plate = ValidProfile::new(Profile {
            frame: Frame::WORLD,
            loops: vec![
                Loop {
                    key: LoopKey(1),
                    segments: square(0.0, 0.0, 4.0, 1),
                },
                Loop {
                    key: LoopKey(2),
                    segments: square(0.5, 0.5, 1.0, 10),
                },
                Loop {
                    key: LoopKey(3),
                    segments: vec![circle(3.0, 3.0, 0.5, 20)],
                },
            ],
        })
        .unwrap();
        same(
            &extrude(OpKey(2), &plate, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
                .unwrap()
                .0,
        );
        // A ring with a round hollow inside it: a cavity.
        let frame = Frame::new(
            crate::geom::Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let ring = ValidProfile::new(Profile {
            frame,
            loops: vec![
                Loop {
                    key: LoopKey(1),
                    segments: square(1.0, -0.5, 1.0, 1),
                },
                Loop {
                    key: LoopKey(2),
                    segments: vec![circle(1.5, 0.0, 0.25, 9)],
                },
            ],
        })
        .unwrap();
        let axis = Axis::new(crate::geom::Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let hollow = revolve(OpKey(3), &ring, axis, 0.0, core::f64::consts::TAU)
            .unwrap()
            .0;
        assert_eq!(hollow.counts().shells, 2);
        same(&hollow);
    }
}
