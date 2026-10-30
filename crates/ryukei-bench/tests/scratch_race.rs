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
    for (w, b) in [("block", &block), ("pin", &pin)] {
        for f in b.faces() {
            let x = b.face(f).unwrap();
            eprintln!("SCRATCH face {w} {:?} {:?} {}", f, x.name, b.surface(x.surface).unwrap().debug_kind());
        }
    }
    let mut places = nurbs_race::places(n);
    if let Ok(k) = std::env::var("PIN") { places = vec![nurbs_race::places(1000)[k.parse::<usize>().unwrap()]]; }
    let tools: Vec<Tool> = places.iter().enumerate().map(|(i, p)| Tool { key: ToolKey(i as u128 + 1), body: &pin, place: *p }).collect();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    eprintln!("SCRATCH-START");
    if let Ok(v) = std::env::var("T3") { unsafe { std::env::set_var("SCRATCH_T3", v) } }
    if let Ok(v) = std::env::var("DBG") { unsafe { std::env::set_var("SCRATCH_DBG", v) } }
    let t = Instant::now();
    let r = pool.install(|| boolean(OpKey(3), BooleanOp::Subtract, &block, &tools));
    eprintln!("{n} pins flush={flush}: {:.2} s, {:?}", t.elapsed().as_secs_f64(), r.as_ref().map(|b| b.0.counts()));
}

#[test]
fn scratch_each() {
    use rayon::prelude::*;
    let flush = std::env::var("FLUSH").is_ok();
    let block = nurbs_race::block();
    let pin = if flush { nurbs_race::flush_pin() } else { nurbs_race::clear_pin() };
    let places = nurbs_race::places(1000);
    let bad: Vec<(usize, String)> = places.par_iter().enumerate().filter_map(|(i, p)| {
        let tools = [Tool { key: ToolKey(1), body: &pin, place: *p }];
        match boolean(OpKey(3), BooleanOp::Subtract, &block, &tools) {
            Ok(_) => None,
            Err(e) => Some((i, format!("{e:?}"))),
        }
    }).collect();
    eprintln!("SCRATCH bad {} {:?}", bad.len(), &bad[..bad.len().min(20)]);
}
