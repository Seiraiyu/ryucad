use rayon::prelude::*;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{HealOptions, heal, seamed};
use ryukei::name::OpKey;
use ryukei::primitive;

#[test]
fn scratch() {
    let block = primitive::block(OpKey(1), 4.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.2, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let n: usize = std::env::var("HOLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let places: Vec<Rigid> = (0..n)
        .map(|i| Rigid::translation(Vector3::new(0.5 + i as f64, 0.5, -0.5)))
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: *p,
        })
        .collect();
    let (b, _) = boolean(OpKey(3), BooleanOp::Subtract, &block, &tools).unwrap();
    let shape = seamed(&b, 1e-7).unwrap();
    let bad: Vec<u128> = (0..4000u128)
        .into_par_iter()
        .filter(|&k| heal(OpKey(k), &shape, &HealOptions::default()).is_err())
        .collect();
    eprintln!("SCRATCH bad {} {:?}", bad.len(), &bad[..bad.len().min(10)]);
}

#[test]
fn scratch_one() {
    let block = primitive::block(OpKey(1), 4.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.2, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..3)
        .map(|i| Rigid::translation(Vector3::new(0.5 + i as f64, 0.5, -0.5)))
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: *p,
        })
        .collect();
    let (b, _) = boolean(OpKey(3), BooleanOp::Subtract, &block, &tools).unwrap();
    let shape = seamed(&b, 1e-7).unwrap();
    let k: u128 = std::env::var("KEY").unwrap().parse().unwrap();
    eprintln!(
        "SCRATCH one {:?}",
        heal(OpKey(k), &shape, &HealOptions::default()).map(|x| x.0.counts())
    );
}
