use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::name::OpKey;
use ryukei_testkit::nurbs_race;
use std::time::Instant;

#[test]
fn scratch() {
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let flush = std::env::var("FLUSH").is_ok();
    let t = Instant::now();
    let block = nurbs_race::block();
    let pin = if flush { nurbs_race::flush_pin() } else { nurbs_race::clear_pin() };
    eprintln!("bodies {:.2} s; block {:?} pin {:?}", t.elapsed().as_secs_f64(), block.counts(), pin.counts());
    let places = nurbs_race::places(n);
    let tools: Vec<Tool> = places.iter().enumerate().map(|(i, p)| Tool { key: ToolKey(i as u128 + 1), body: &pin, place: *p }).collect();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    eprintln!("SCRATCH-START");
    if let Ok(v) = std::env::var("T3") { unsafe { std::env::set_var("SCRATCH_T3", v) } }
    if let Ok(v) = std::env::var("DBG") { unsafe { std::env::set_var("SCRATCH_DBG", v) } }
    let t = Instant::now();
    let r = pool.install(|| boolean(OpKey(3), BooleanOp::Subtract, &block, &tools));
    eprintln!("{n} pins flush={flush}: {:.2} s, {:?}", t.elapsed().as_secs_f64(), r.as_ref().map(|b| b.0.counts()));
}
