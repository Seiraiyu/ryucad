//! Bit-exact determinism of the solver. Every CI platform and CPU level
//! must reproduce `tests/golden/solve.txt`: solved positions, and every
//! part of the report. After a deliberate change, regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p ryusketch --test determinism` and review
//! the diff like any other change.

mod common;

use common::{jostle, polygon, slot};
use ryukei_testkit::{Golden, Lcg, Section};
use ryusketch::{Entity, Report, Sketch, solve};

fn record(s: &mut Section, sketch: &Sketch, r: &Report) {
    for e in sketch.entities.values() {
        match *e {
            Entity::Point { at, .. } => {
                s.f64(at[0]);
                s.f64(at[1]);
            }
            Entity::Circle { r, .. } => s.f64(r),
            _ => {}
        }
    }
    s.str(&format!(
        "{} {} {:?} {} {:?} {:?} {:?} {:?} {:?}",
        r.converged,
        r.iterations,
        r.state,
        r.dof,
        r.free,
        r.redundant,
        r.conflicting,
        r.loops,
        r.open
    ));
}

fn polygons(golden: &mut Golden) {
    let mut s = golden.section("polygons");
    let mut g = Lcg::new(0x706f_6c79);
    for i in 0..256 {
        let size = g.next_f64(0.001, 50.0);
        let mut sk = polygon(&mut g, 3 + i % 7, size);
        jostle(&mut sk, &mut g, 0.01);
        // Some under-defined, some over.
        match i % 5 {
            0 => {
                sk.constraints.remove(&2);
            }
            1 => {
                sk.constraints
                    .insert(500, ryusketch::Constraint::Length(common::SIDES + 1, 1.0));
            }
            _ => {}
        }
        let r = solve(&sk).unwrap();
        record(&mut s, &r.sketch, &r.report);
    }
}

fn slots(golden: &mut Golden) {
    let mut s = golden.section("slots");
    let mut g = Lcg::new(0x736c_6f74);
    for _ in 0..256 {
        let (l, w) = (g.next_f64(0.001, 100.0), g.next_f64(0.001, 10.0));
        let mut sk = slot(l, w);
        jostle(&mut sk, &mut g, 0.0005);
        let r = solve(&sk).unwrap();
        record(&mut s, &r.sketch, &r.report);
    }
}

#[test]
fn determinism() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/solve.txt"
    ));
    polygons(&mut golden);
    slots(&mut golden);
    golden.finish();
}
