use ryukei::boolean::BooleanOp;
use ryukei::geom::{Rigid, Vector3};
use ryukei_bench::occt;
use ryukei_testkit::nurbs_race;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::time::Instant;

fn to_occt(name: &str, b: ryukei::topo::Body) -> occt::Solid {
    let bytes = write(&WriteModel { products: vec![Product { id: name.into(), name: name.into(), bodies: vec![b], children: vec![], ..Default::default() }] },
        &WriteOptions { unit: LengthUnit::Millimeter, time_stamp: "2026-10-30T00:00:00".into(), ..WriteOptions::default() }).unwrap();
    let path = std::env::temp_dir().join(format!("ryukei-8f-{name}.stp"));
    std::fs::write(&path, bytes).unwrap();
    occt::Solid::read_step(&path, false).unwrap()
}

#[test]
fn scratch() {
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let flush = std::env::var("FLUSH").is_ok();
    let block = to_occt("block", nurbs_race::block());
    let pin = to_occt("pin", if flush { nurbs_race::flush_pin() } else { nurbs_race::clear_pin() });
    eprintln!("block faces {} vol {} pin faces {} vol {}", block.faces(), block.volume(), pin.faces(), pin.volume());
    let pins: Vec<occt::Solid> = nurbs_race::places(n).iter().map(|p| {
        let t = p.translation_part() * 1000.0;
        pin.placed(&Rigid::translation(Vector3::new(t.x, t.y, t.z)))
    }).collect();
    let t = Instant::now();
    let r = block.boolean(BooleanOp::Subtract, &pins, false);
    eprintln!("OCCT {n} pins flush={flush}: {:.2} s, {:?}", t.elapsed().as_secs_f64(), r.map(|s| (s.faces(), s.volume())));
}
