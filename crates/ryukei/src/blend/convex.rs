//! Which way an edge bends: convex, concave or smooth, certified along
//! its whole length, and which edges meet tangentially at a vertex.
//!
//! An edge's faces have outward normals `n₀` and `n₁`; `n₀`'s fin runs
//! along the edge's direction `d`. The edge is convex where
//! `(n₀ × n₁) · d > 0`: the faces fold away from each other, so a ball
//! rolling inside the body touches both. The sign is certified over
//! pieces of the edge in interval arithmetic, halving where rounding
//! can't tell; an edge whose faces are tangent somewhere is smooth, and
//! one certified to bend both ways changes convexity.

use crate::geom::jet::{JVec, Jet};
use crate::geom::pipe::anchor_normal;
use crate::geom::{Surface, UnitVector3, Vector3};
use crate::interval::Interval;
use crate::tol::ANGULAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FinId, Sense, VertexId};

/// Which way an edge bends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bend {
    Convex,
    Concave,
    /// The faces are tangent somewhere along it.
    Smooth,
}

/// Why an edge's bend could not be given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BendFail {
    /// Certified convex in one place and concave in another.
    Changes,
    /// Rounding left a stretch unsigned where the faces are not tangent.
    Uncertain,
}

/// Pieces the edge is first cut into.
const PIECES: usize = 8;

/// The deepest a piece is halved.
const MAX_DEPTH: u32 = 16;

/// The most pieces one edge may take.
const MAX_PIECES: usize = 1 << 12;

/// Float samples taken first, ends included.
const SAMPLES: usize = 17;

type IVec3 = [Interval; 3];

/// How `body`'s edge bends: smooth if its faces' normals are within
/// [`ANGULAR_RESOLUTION`] at one of 17 samples or in the middle of a
/// stretch rounding can't sign, otherwise convex or concave by a
/// certified sign along the whole edge. Its two fins must be on faces on
/// analytic surfaces or pipes, each with a pcurve.
pub(crate) fn bend(body: &Body, edge: EdgeId) -> Result<Bend, BendFail> {
    let e = &body.edges[edge];
    let fins: Vec<FinId> = body.edge_fins(edge).collect();
    let fins = if body.fins[fins[0]].sense == Sense::Forward {
        [fins[0], fins[1]]
    } else {
        [fins[1], fins[0]]
    };
    let curve = &body.curves[e.curve];
    let [t0, t1] = e.interval;
    let sides = fins.map(|f| Side::of(body, f));
    for i in 0..SAMPLES {
        let t = t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64;
        if tangent_normals(sides[0].normal(t), sides[1].normal(t)) {
            return Ok(Bend::Smooth);
        }
    }
    let mut stack: Vec<([f64; 2], u32)> = (0..PIECES)
        .rev()
        .map(|i| {
            let at = |k: usize| t0 + (t1 - t0) * k as f64 / PIECES as f64;
            ([at(i), if i + 1 == PIECES { t1 } else { at(i + 1) }], 0)
        })
        .collect();
    let (mut convex, mut concave) = (false, false);
    let mut budget = MAX_PIECES;
    while let Some((r, depth)) = stack.pop() {
        let jets = curve.jet_box(r);
        let n = [
            sides[0].normal_box(r, jets[0]),
            sides[1].normal_box(r, jets[0]),
        ];
        let f = match n {
            [Some(a), Some(b)] => dot(cross(a, b), jets[1]),
            _ => Interval::new(f64::NEG_INFINITY, f64::INFINITY).expect("ordered"),
        };
        if f.lo() > 0.0 {
            convex = true;
        } else if f.hi() < 0.0 {
            concave = true;
        } else if depth < MAX_DEPTH && budget > 1 {
            budget -= 1;
            let m = (r[0] + r[1]) / 2.0;
            stack.push(([m, r[1]], depth + 1));
            stack.push(([r[0], m], depth + 1));
            continue;
        } else {
            let m = (r[0] + r[1]) / 2.0;
            return if tangent_normals(sides[0].normal(m), sides[1].normal(m)) {
                Ok(Bend::Smooth)
            } else {
                Err(BendFail::Uncertain)
            };
        }
        if convex && concave {
            return Err(BendFail::Changes);
        }
    }
    Ok(if convex { Bend::Convex } else { Bend::Concave })
}

/// Whether two unit normals are within [`ANGULAR_RESOLUTION`] of each
/// other.
fn tangent_normals(a: Vector3, b: Vector3) -> bool {
    a.dot(b) > 0.0 && a.cross(b).length() <= ANGULAR_RESOLUTION
}

/// Whether two edges leaving a vertex in directions `a` and `b` meet
/// tangentially: one runs on where the other stops, within
/// [`ANGULAR_RESOLUTION`] plus `slack`, how far their directions there
/// may be from the true ones (see [`leaving`]).
pub(crate) fn tangent_directions(a: UnitVector3, b: UnitVector3, slack: f64) -> bool {
    let (a, b) = (a.get(), b.get());
    a.dot(b) < 0.0 && a.cross(b).length() <= ANGULAR_RESOLUTION + slack
}

/// The direction an edge leaves `v` in, from its curve's tangent at the
/// end at `v` (its start when it is closed), and how far that may be
/// from the true direction: nothing for a line, circle or ellipse, and
/// for a fitted NURBS four times its tolerance over the length of its
/// piece at that end (a fit within `δ` of the curve turns up to about
/// `δ / h` from it over a piece `h` long). `None` where the tangent
/// vanishes.
pub(crate) fn leaving(body: &Body, edge: EdgeId, v: VertexId) -> Option<(UnitVector3, f64)> {
    let e = &body.edges[edge];
    let curve = &body.curves[e.curve];
    let [t0, t1] = e.interval;
    let from_start = e.start == v;
    let dir = if from_start {
        curve.tangent(t0)
    } else {
        curve.tangent(t1).map(|t| -t)
    }?;
    let slack = match curve {
        crate::geom::Curve::Nurbs(n) => {
            let knots = n.knots().knots();
            let (a, b) = if from_start {
                let next = knots.iter().copied().find(|&k| k > t0).unwrap_or(t1);
                (t0, next.min(t1))
            } else {
                let prev = knots.iter().rev().copied().find(|&k| k < t1).unwrap_or(t0);
                (prev.max(t0), t1)
            };
            let h = curve.point(a).distance(curve.point(b));
            if h > 0.0 {
                4.0 * e.tolerance.get() / h
            } else {
                f64::INFINITY
            }
        }
        _ => 0.0,
    };
    Some((dir, slack))
}

/// One face of an edge, read through its fin.
struct Side<'a> {
    body: &'a Body,
    fin: FinId,
    surface: &'a Surface,
    sign: f64,
}

impl<'a> Side<'a> {
    fn of(body: &'a Body, fin: FinId) -> Self {
        let face = &body.faces[body.loops[body.fins[fin].loop_].face];
        Self {
            body,
            fin,
            surface: &body.surfaces[face.surface],
            sign: if face.sense == Sense::Forward {
                1.0
            } else {
                -1.0
            },
        }
    }

    /// The face's outward normal at the edge's parameter `t`, through its
    /// pcurve; zero where the surface has none.
    fn normal(&self, t: f64) -> Vector3 {
        let uv = self.body.fin_uv(self.fin, t).expect("a fin with a pcurve");
        self.surface
            .normal(uv.x, uv.y)
            .map_or(Vector3::ZERO, |n| n.get() * self.sign)
    }

    /// An enclosure of the outward normal over the edge's parameters `r`,
    /// whose points `p` enclose: an analytic surface's from the points, a
    /// pipe's from its pcurve's parameters. `None` where it can't be
    /// certified.
    fn normal_box(&self, r: [f64; 2], p: IVec3) -> Option<IVec3> {
        let n = match self.surface {
            Surface::Pipe(pipe) => {
                let pc = self.body.fins[self.fin].pcurve?;
                let (a, b) = (pc.param(r[0]), pc.param(r[1]));
                let bx = self.body.curves2[pc.curve].bounds([a.min(b), a.max(b)]);
                let (lo, hi) = (bx.min(), bx.max());
                let [_, n] = pipe.jet_box([[lo.x, hi.x], [lo.y, hi.y]])?;
                n.map(|j| j.v)
            }
            s => {
                let c: JVec<Interval> = p.map(Jet::constant);
                anchor_normal(s, c)?.map(|j| j.v)
            }
        };
        Some(n.map(|x| x * Interval::point(self.sign)))
    }
}

fn cross(a: IVec3, b: IVec3) -> IVec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: IVec3, b: IVec3) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::tests::{block, edge_of};
    use crate::geom::{Rigid, Vector3};
    use crate::name::{FaceRole, OpKey};
    use crate::topo::EntityId;

    fn edge(b: &Body, n: crate::name::Name) -> EdgeId {
        match b.by_name(n) {
            Some(EntityId::Edge(e)) => e,
            _ => panic!("no edge"),
        }
    }

    #[test]
    fn a_boxs_edges_are_convex() {
        let b = block();
        for (x, y) in [
            (FaceRole::PlusX, FaceRole::PlusZ),
            (FaceRole::MinusX, FaceRole::MinusY),
            (FaceRole::PlusY, FaceRole::MinusZ),
        ] {
            assert_eq!(bend(&b, edge(&b, edge_of(&b, x, y))), Ok(Bend::Convex));
        }
    }

    #[test]
    fn rims_are_convex_and_holes_inside_out_are_concave() {
        let (cyl, _) = crate::primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        for e in cyl.edges() {
            assert_eq!(bend(&cyl, e), Ok(Bend::Convex));
        }
        // A boss on a plate: the edge where the boss meets the plate is
        // concave.
        let plate = crate::primitive::block(OpKey(2), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let place = Rigid::translation(Vector3::new(2.0, 2.0, 0.5));
        let tool = crate::boolean::Tool {
            key: crate::boolean::ToolKey(1),
            body: &cyl,
            place,
        };
        let (u, _) =
            crate::boolean::boolean(OpKey(3), crate::boolean::BooleanOp::Unite, &plate, &[tool])
                .unwrap();
        let mut seen = [0; 3];
        for e in u.edges() {
            match bend(&u, e) {
                Ok(Bend::Convex) => seen[0] += 1,
                Ok(Bend::Concave) => seen[1] += 1,
                other => panic!("{other:?}"),
            }
        }
        // 12 box edges and the boss's top rim, and the boss's foot.
        assert_eq!(seen, [13, 1, 0]);
    }

    #[test]
    fn a_slots_laterals_are_smooth() {
        let b = crate::blend::tests::slot();
        let smooth = b
            .edges()
            .filter(|&e| bend(&b, e) == Ok(Bend::Smooth))
            .count();
        assert_eq!(smooth, 4);
    }

    #[test]
    fn tangency_at_a_vertex() {
        let b = crate::blend::tests::slot();
        // At each end of a straight side of the cap, the cap's line and
        // arc meet tangentially; the lateral edge leaves at a right angle.
        let mut tangent = 0;
        for v in b.vertices() {
            let edges = b.vertex_edges(v);
            for (i, &x) in edges.iter().enumerate() {
                for &y in &edges[i + 1..] {
                    let (a, c) = (leaving(&b, x, v).unwrap(), leaving(&b, y, v).unwrap());
                    if tangent_directions(a.0, c.0, a.1 + c.1) {
                        tangent += 1;
                    }
                }
            }
        }
        assert_eq!(tangent, 8);
    }
}
