//! Seeded crossing shells (design 299, plan 14.C): two or three placed
//! primitives written as one solid's shells, some turned inside out
//! (`ryukei_testkit::crossing`), healed with repair, and given to
//! `self_union` where healing left them crossing (no vertex of either
//! inside the other). Every case is a typed refusal or a body that checks
//! clean, meshes, and holds a point of a grid exactly where the parts'
//! winding number is one or more.
//! Release only: about a minute in release, far longer in a debug build.

use ryukei::Error;
use ryukei::check::{check, check_intersections};
use ryukei::geom::Point3;
use ryukei::heal::{HealOptions, heal};
use ryukei::intersect::{Inclusion, point_in_body};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::repair::self_union;
use ryukei_testkit::Lcg;
use ryukei_testkit::crossing::crossing_case;

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes in a debug build")]
fn seeded_crossings_heal_to_where_their_winding_is_one_or_more() {
    let mut rng = Lcg::new(0x14c);
    let (mut compared, mut refused, mut asked) = (0, 0, 0);
    for seed in 0..48 {
        let data: Vec<u8> = (0..40).map(|_| rng.next_u64() as u8).collect();
        let Some(case) = crossing_case(&data) else {
            continue;
        };
        let healed = match heal(OpKey(7), &case.shape, &HealOptions::default()) {
            Ok((b, _, _)) => b,
            Err(Error::InvalidHeal(_)) => {
                refused += 1;
                continue;
            }
            Err(e) => panic!("seed {seed}: {e:?}"),
        };
        // Shells crossing with no vertex of either inside the other heal
        // as they are (healing runs no full check): a caller's self-union
        // repairs them.
        let body = if check_intersections(&healed).is_empty() {
            healed
        } else {
            asked += 1;
            match self_union(OpKey(9), &healed) {
                Ok((b, _)) => b,
                Err(Error::InvalidRepair(_)) => {
                    refused += 1;
                    continue;
                }
                Err(e) => panic!("seed {seed}: {e:?}"),
            }
        };
        assert_eq!(check(&body), [], "seed {seed}");
        assert_eq!(check_intersections(&body), [], "seed {seed}");
        let m = tessellate(&body, &MeshParams::new(1e-3, 0.5)).unwrap();
        assert_eq!(m.faults, [], "seed {seed}");
        let mut tried = 0;
        for i in 0..8 {
            for j in 0..8 {
                for k in 0..8 {
                    let at = |n: i32| -1.1 + 0.5 * f64::from(n) + 0.0123;
                    let p = Point3::new(at(i), at(j), at(k));
                    let Some(w) = case.winding(p) else { continue };
                    let Ok(inc) = point_in_body(&body, p) else {
                        continue;
                    };
                    if inc == Inclusion::Boundary {
                        continue;
                    }
                    tried += 1;
                    assert_eq!(
                        inc == Inclusion::Inside,
                        w >= 1,
                        "seed {seed} at {p:?}: winding {w}"
                    );
                }
            }
        }
        assert!(tried > 100, "seed {seed}: {tried}");
        compared += 1;
    }
    println!("{compared} compared ({asked} repaired on asking), {refused} refused");
    assert!(compared >= 24, "{compared} compared, {refused} refused");
}
