//! Bit-exact determinism of the solver. Every CI platform and CPU level
//! must reproduce `tests/golden/mate.txt`: solved poses, and every part
//! of the report. After a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryumate --test determinism` and review
//! the diff like any other change.

mod common;

use common::{direction, fixed, free, jostle, mechanism, on, turned};
use ryukei_testkit::{Golden, Lcg, Section};
use ryumate::{Kind, Mate, Pose, Solved, solve};

fn record(s: &mut Section, r: &Solved) {
    for p in r.poses.values() {
        for x in p.rotation.iter().flatten().chain(&p.origin) {
            s.f64(*x);
        }
    }
    let rep = &r.report;
    s.str(&format!(
        "{} {} {} {:?} {} {:?} {:?} {:?}",
        rep.converged,
        rep.iterations,
        rep.changed,
        rep.state,
        rep.dof,
        rep.mates,
        rep.redundant,
        rep.conflicting
    ));
    for b in rep.bodies.values() {
        s.str(&format!("{:?}", b.state));
        let f = &b.freedom;
        s.u64(f.dof as u64);
        for t in &f.translations {
            t.iter().for_each(|x| s.f64(*x));
        }
        for a in &f.rotations {
            a.direction.iter().chain(&a.through).for_each(|x| s.f64(*x));
        }
    }
}

/// Mechanisms jostled off where they hold, some given a mate twice.
fn mechanisms(golden: &mut Golden) {
    let mut s = golden.section("mechanisms");
    let mut g = Lcg::new(0x6d61_7465);
    for i in 0..128 {
        let size = g.next_f64(0.001, 50.0);
        let (mut asm, _) = mechanism(&mut g, 2 + i % 9, size);
        if i % 4 == 1 {
            let m: Mate = asm.mates[&1];
            asm.mates.insert(100, m);
        }
        jostle(&mut asm, &mut g, 0.05, size * 0.01);
        record(&mut s, &solve(&asm).unwrap());
    }
}

/// Chains of 19 links on driven revolutes, each started up to a radian
/// and 3 mm off.
fn chains(golden: &mut Golden) {
    let mut s = golden.section("chains");
    let mut g = Lcg::new(0x6368_6169);
    for _ in 0..32 {
        let mut asm = ryumate::Assembly::default();
        asm.bodies.insert(0, fixed(Pose::IDENTITY));
        for k in 1..20u32 {
            let off = Pose {
                origin: [0.01, 0.0, 0.0],
                ..Pose::IDENTITY
            };
            let n = direction(&mut g);
            let start = turned(
                n,
                g.next_f64(-1.0, 1.0),
                [
                    0.01 * f64::from(k) + g.next_f64(-0.003, 0.003),
                    g.next_f64(-0.003, 0.003),
                    0.0,
                ],
            );
            asm.bodies.insert(k, free(start));
            asm.mates.insert(
                k,
                Mate {
                    kind: Kind::Revolute {
                        angle: Some(g.next_f64(-1.0, 1.0)),
                    },
                    a: on(k - 1, off),
                    b: on(k, Pose::IDENTITY),
                },
            );
        }
        let r = solve(&asm).unwrap();
        assert!(r.report.converged);
        record(&mut s, &r);
    }
}

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/mate.txt"
    ));
    mechanisms(&mut golden);
    chains(&mut golden);
    golden.finish();
}
