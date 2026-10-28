//! The bodies of design 55's large-assembly race: the five primitives,
//! then for each seed a sweep, a boolean and a blend made from its
//! bytes, those that build, in seed order. `ryukei-bench`'s STEP race
//! and ryucad's import bar (design 120) place them as parts.

use crate::Lcg;
use crate::blend_bytes::blend_case;
use crate::boolean_bytes::boolean_case;
use crate::sweep_bytes::sweep;
use rayon::prelude::*;
use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;

/// The primitives and every body seeds `0..seeds` make, empty ones
/// dropped. Seeds are worked in parallel; the order never depends on it.
pub fn bodies(seeds: u64) -> Vec<Body> {
    let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
    let k = OpKey(1);
    let mut out = vec![
        primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
        primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
        primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
        primitive::sphere(k, 1.0, &t).unwrap().0,
        primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
    ];
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    let made: Vec<Vec<Body>> = (0..seeds)
        .into_par_iter()
        .map(|seed| {
            let mut here = Vec::new();
            if let Some((b, _)) = sweep(&bytes(seed)) {
                here.push(b);
            }
            if let Some(case) = boolean_case(&bytes(seed))
                && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
            {
                here.push(b);
            }
            if let Some(case) = blend_case(&bytes(seed))
                && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
            {
                here.push(b);
            }
            here
        })
        .collect();
    out.extend(made.into_iter().flatten());
    out.retain(|b| b.faces().next().is_some());
    out
}

/// `n` race bodies a STEP file can describe (a torus face whose loop
/// spirals can't be written: design 55's 7.B limit), from as many seeds
/// as `n` bodies.
pub fn writable(n: usize) -> Vec<Body> {
    let mut out = bodies(n as u64);
    out.retain(|b| ryukei::heal::seamed(b, 1e-7).is_ok());
    out.truncate(n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bodies_are_the_same_on_any_number_of_threads() {
        let dumps = |threads| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let made = pool.install(|| bodies(3));
            made.iter().map(Body::dump).collect::<Vec<_>>()
        };
        let one = dumps(1);
        assert!(one.len() > 5, "the primitives and what seeds make");
        assert_eq!(one, dumps(4));
    }
}
