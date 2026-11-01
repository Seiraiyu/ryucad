//! Every long entry point under a work budget and a cancel (plan 15.A):
//! it charges, the same units on one thread and eight, a budget of what it
//! spends is enough, one unit less is `BudgetExceeded` (judged alike on
//! every thread count), and a cancel set before it starts is `Cancelled`.

use ryukei::Error;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::work::Work;
use std::sync::atomic::AtomicBool;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

/// What `call` spends, after checking the rules above.
fn bounded<T>(name: &str, call: impl Fn(&Work) -> Result<T, Error> + Sync) -> u64
where
    T: Send + std::fmt::Debug,
{
    let free = Work::unlimited();
    if let Err(e) = call(&free) {
        panic!("{name}: {e:?}");
    }
    let spent = free.spent();
    assert!(spent > 0, "{name} charged nothing");
    let mut over = Vec::new();
    for threads in [1, 8] {
        let pool = pool(threads);
        let w = Work::unlimited();
        pool.install(|| call(&w)).unwrap();
        assert_eq!(w.spent(), spent, "{name} on {threads} threads");
        let w = Work::unlimited().with_budget(spent);
        assert!(
            pool.install(|| call(&w)).is_ok(),
            "{name}: a budget of what it spends"
        );
        let w = Work::unlimited().with_budget(spent - 1);
        match pool.install(|| call(&w)) {
            Err(Error::BudgetExceeded { spent: s, budget }) => {
                assert_eq!(budget, spent - 1);
                assert!(s > budget);
                over.push(s);
            }
            other => panic!("{name} on {threads} threads, one unit short: {other:?}"),
        }
    }
    assert_eq!(
        over[0], over[1],
        "{name}: where it stops depends on threads"
    );
    let flag = AtomicBool::new(true);
    let w = Work::unlimited().with_cancel(&flag);
    assert!(
        matches!(call(&w), Err(Error::Cancelled)),
        "{name}: cancelled"
    );
    spent
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// A 10 mm cube with a 3 mm rod through it: a boolean of a few traces.
fn drilled() -> Body {
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let (rod, _) = primitive::cylinder(OpKey(2), 0.0015, 0.02, &at(0.005, 0.005, -0.005)).unwrap();
    let tool = Tool {
        key: ToolKey(1),
        body: &rod,
        place: Rigid::IDENTITY,
    };
    boolean(OpKey(3), BooleanOp::Subtract, &cube, &[tool])
        .unwrap()
        .0
}

#[test]
fn the_check_is_bounded() {
    let body = drilled();
    let spent = bounded("check_intersections", |w| {
        ryukei::check::check_intersections_with(&body, w)
    });
    let w = Work::unlimited();
    assert_eq!(
        ryukei::check::check_intersections_with(&body, &w).unwrap(),
        ryukei::check::check_intersections(&body)
    );
    eprintln!("check_intersections: {spent} units");
}

#[test]
fn a_boolean_is_bounded() {
    let (cube, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
    let (rod, _) = primitive::cylinder(OpKey(2), 0.0015, 0.02, &at(0.005, 0.005, -0.005)).unwrap();
    let (ball, _) = primitive::sphere(OpKey(4), 0.003, &at(0.01, 0.01, 0.01)).unwrap();
    let tools = [
        Tool {
            key: ToolKey(1),
            body: &rod,
            place: Rigid::IDENTITY,
        },
        Tool {
            key: ToolKey(2),
            body: &ball,
            place: Rigid::IDENTITY,
        },
    ];
    let plain = boolean(OpKey(3), BooleanOp::Subtract, &cube, &tools)
        .unwrap()
        .0;
    let spent = bounded("boolean", |w| {
        ryukei::boolean::boolean_with(OpKey(3), BooleanOp::Subtract, &cube, &tools, w)
    });
    let w = Work::unlimited();
    let (with, _) =
        ryukei::boolean::boolean_with(OpKey(3), BooleanOp::Subtract, &cube, &tools, &w).unwrap();
    assert_eq!(with.dump(), plain.dump());
    eprintln!("boolean: {spent} units");
}
