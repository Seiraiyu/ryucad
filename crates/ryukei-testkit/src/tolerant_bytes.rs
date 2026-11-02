//! Tolerant bodies from exact ones (design 299), for the tolerant
//! boolean, blend and offset tests and the `tolerant` fuzz target: a body
//! written as a file describes it ([`ryukei::heal::seamed`]), every edge's
//! curve moved off its faces and every vertex off its edges, then healed
//! with tightening off, so every gap stays a tolerance. Surfaces aren't
//! moved, so the tolerant body bounds the same solid as the exact one,
//! within its tolerances.

use crate::boolean_bytes::{Case, boolean_case};
use crate::euler_script::Bytes;
use ryukei::Error;
use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{HealOptions, heal, seamed};
use ryukei::name::OpKey;
use ryukei::topo::Body;

/// How far the cases move edges: from about the resolution to a third of
/// healing's cap.
pub const MOVES: [f64; 4] = [1e-7, 1e-6, 1e-5, 3e-5];

/// A direction from bytes, never zero.
fn direction(b: &mut Bytes) -> Vector3 {
    let mut c = || f64::from(b.pick(5) as u8) - 2.0;
    let v = Vector3::new(c(), c(), c());
    if v.length() == 0.0 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        v * (1.0 / v.length())
    }
}

/// `body` with edge `e`'s curve moved by `by` along `edge(e)` and vertex
/// `v` by `by / 2` along `vertex(v)` (unit directions), healed under `key`
/// with tightening off. Fails as writing or healing fails.
pub fn tolerant_with(
    key: OpKey,
    body: &Body,
    by: f64,
    mut edge: impl FnMut(usize) -> Vector3,
    mut vertex: impl FnMut(usize) -> Vector3,
) -> Result<Body, Error> {
    let mut s = seamed(body, 1e-7)?;
    for e in 0..s.edges.len() {
        let moved = s.curves[s.edges[e].curve].transformed(&Rigid::translation(edge(e) * by));
        s.curves.push(moved);
        s.edges[e].curve = s.curves.len() - 1;
    }
    for v in 0..s.vertices.len() {
        s.vertices[v].point += vertex(v) * (by / 2.0);
    }
    let options = HealOptions {
        tighten: false,
        ..HealOptions::default()
    };
    heal(key, &s, &options).map(|(h, _, _)| h)
}

/// [`tolerant_with`], each direction from `b`.
pub fn tolerant(key: OpKey, body: &Body, by: f64, b: &mut Bytes) -> Result<Body, Error> {
    let mut dirs: Vec<Vector3> = Vec::new();
    let s = seamed(body, 1e-7)?;
    for _ in 0..s.edges.len() + s.vertices.len() {
        dirs.push(direction(b));
    }
    let edges = s.edges.len();
    tolerant_with(key, body, by, |e| dirs[e], |v| dirs[edges + v])
}

/// A boolean from bytes ([`boolean_case`]) and its tolerant twin: the
/// target and every tool made tolerant by `by`, one of [`MOVES`].
pub struct TolerantCase {
    pub exact: Case,
    pub tolerant: Case,
    pub by: f64,
}

/// A tolerant case from bytes: the first byte picks the move, the second
/// how many of the rest make the exact case, and the remainder the
/// directions. `None` when the bytes give no case or healing refuses a
/// twin.
pub fn tolerant_case(data: &[u8]) -> Option<TolerantCase> {
    let (&pick, rest) = data.split_first()?;
    let by = MOVES[usize::from(pick) % MOVES.len()];
    let (&len, rest) = rest.split_first()?;
    let n = usize::from(len).min(rest.len());
    let (bytes, rest) = rest.split_at(n);
    let exact = boolean_case(bytes)?;
    let mut b = Bytes::new(rest);
    let target = tolerant(OpKey(0x746f_6c00), &exact.target, by, &mut b).ok()?;
    let tools = exact
        .tools
        .iter()
        .enumerate()
        .map(|(i, t)| tolerant(OpKey(0x746f_6c01 + i as u128), t, by, &mut b).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(TolerantCase {
        tolerant: Case {
            op: exact.op,
            target,
            tools,
        },
        exact,
        by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::primitive;

    #[test]
    fn a_tolerant_twin_has_the_exact_bodys_topology_and_its_tolerances() {
        let (bx, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
        let mut b = Bytes::new(&[3, 1, 4, 1, 5, 9, 2, 6]);
        let t = tolerant(OpKey(9), &bx, 1e-5, &mut b).unwrap();
        assert_eq!(t.counts(), bx.counts());
        assert_eq!(ryukei::check::check(&t), []);
        let worst = t
            .edges()
            .map(|e| t.edge(e).unwrap().tolerance.get())
            .fold(0.0, f64::max);
        assert!(
            (1e-5..ryukei::tol::TOLERANCE_CAP).contains(&worst),
            "{worst}"
        );
    }

    #[test]
    fn a_case_from_bytes_twins_the_target_and_every_tool() {
        let data: Vec<u8> = (0u8..96).map(|k| k.wrapping_mul(37)).collect();
        let case = (0..32)
            .find_map(|k| tolerant_case(&data[k..]))
            .expect("some offset gives a case");
        assert_eq!(case.exact.tools.len(), case.tolerant.tools.len());
        assert!(MOVES.contains(&case.by));
        assert_eq!(case.exact.target.counts(), case.tolerant.target.counts());
    }
}
