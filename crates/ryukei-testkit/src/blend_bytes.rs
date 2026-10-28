//! Blends from bytes, for the `blend` fuzz target and for replaying what
//! it has found: a body (a primitive, a sweep or a boolean's result, as
//! [`crate::boolean_bytes`] and [`crate::sweep_bytes`] make them), one to
//! three of its edges (mostly meeting at a vertex, so that corners come
//! up), and a fillet or a chamfer in one of its three forms, sized on a
//! coarse grid of fractions of the body's size.

use crate::boolean_bytes::{boolean_case, solid};
use crate::euler_script::Bytes;
use crate::sweep_bytes::sweep;
use ryukei::blend::{BlendKind, Chamfer, EdgeBlend};
use ryukei::boolean::boolean;
use ryukei::name::OpKey;
use ryukei::topo::{Body, EdgeId};

/// A blend to try: the body and its edges' blends.
pub struct BlendCase {
    pub body: Body,
    pub blends: Vec<EdgeBlend>,
}

/// The body from the first byte's choice and as many bytes as the second
/// gives, or `None` when they give none.
pub fn body(data: &[u8]) -> Option<(Body, &[u8])> {
    let (&kind, rest) = data.split_first()?;
    let (&len, rest) = rest.split_first()?;
    let n = usize::from(len).min(rest.len());
    let (bytes, rest) = rest.split_at(n);
    let body = match kind % 3 {
        0 => solid(&mut Bytes::new(bytes))?,
        1 => sweep(bytes)?.0,
        _ => {
            let case = boolean_case(bytes)?;
            boolean(OpKey(9), case.op, &case.target, &case.tools())
                .ok()?
                .0
        }
    };
    Some((body, rest))
}

/// A blend from bytes, or `None` when they give no body or no edge.
pub fn blend_case(data: &[u8]) -> Option<BlendCase> {
    let (body, rest) = body(data)?;
    let mut b = Bytes::new(rest);
    let all: Vec<EdgeId> = body.edges().collect();
    let first = *all.get(b.pick(all.len().max(1)))?;
    let ends = |e: EdgeId| {
        let x = body.edge(e).expect("a live edge");
        [x.start, x.end]
    };
    // More edges: most often at one of the first's vertices.
    let mut picked = vec![first];
    for _ in 0..b.pick(3) {
        let at = ends(first)[b.pick(2)];
        let near: Vec<EdgeId> = all
            .iter()
            .copied()
            .filter(|&e| !picked.contains(&e) && (b.pick(4) == 0 || ends(e).contains(&at)))
            .collect();
        if let Some(&e) = near.get(b.pick(near.len().max(1))) {
            picked.push(e);
        }
    }
    let size = {
        let bx = body
            .faces()
            .map(|f| ryukei::intersect::face_box(&body, f))
            .reduce(|a, c| a.union(&c))?;
        let d = bx.max() - bx.min();
        d.x.min(d.y).min(d.z).max(0.1) * [0.02, 0.05, 0.1, 0.2, 0.3][b.pick(5)]
    };
    let face_of = |e: EdgeId, k: usize| {
        let fin = body.edge_fins(e).nth(k).expect("two fins");
        let face = body
            .loop_(body.fin(fin).expect("a fin").loop_)
            .expect("a loop")
            .face;
        body.face(face).expect("a face").name
    };
    let kind = |b: &mut Bytes, e: EdgeId| match b.pick(4) {
        0 => BlendKind::Chamfer(Chamfer::Equal { distance: size }),
        1 => BlendKind::Chamfer(Chamfer::Distances {
            first: size,
            second: size * [0.5, 0.75][b.pick(2)],
            first_face: face_of(e, b.pick(2)),
        }),
        2 => BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance: size,
            angle: [0.3, 0.6, 0.9][b.pick(3)],
            face: face_of(e, b.pick(2)),
        }),
        _ => BlendKind::Fillet { radius: size },
    };
    // One kind for all, most often (a chamfer naming a face of the first
    // edge being equal on the others); each its own otherwise.
    let shared = kind(&mut b, first);
    let others = match shared {
        BlendKind::Chamfer(Chamfer::Distances { .. } | Chamfer::DistanceAngle { .. }) => {
            BlendKind::Chamfer(Chamfer::Equal { distance: size })
        }
        k => k,
    };
    let each = b.pick(4) == 0;
    let blends = picked
        .iter()
        .enumerate()
        .map(|(i, &e)| EdgeBlend {
            edge: body.edge(e).expect("a live edge").name,
            kind: match (each, i) {
                (true, _) => kind(&mut b, e),
                (false, 0) => shared,
                (false, _) => others,
            },
        })
        .collect();
    Some(BlendCase { body, blends })
}

/// [`blend_case`] on the body with every curve and surface its exact NURBS
/// form ([`crate::nurbs::convert_body`]): the same blends on spline faces,
/// which their analytic forms' results describe. `None` likewise, or
/// where the body can't be converted.
pub fn spline_case(data: &[u8]) -> Option<BlendCase> {
    let mut case = blend_case(data)?;
    crate::nurbs::convert_body(&mut case.body).ok()?;
    Some(case)
}
