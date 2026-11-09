//! The NURBS race (design 94, #254) without OCCT: one cut of `n` pins,
//! clear or flush, on one rayon thread, timed. For profiling and for
//! the plan 15.E's measurements; the bar itself is `tests/nurbs.rs`'s.
//!
//! `cargo run --release -p ryukei-bench --example race-probe -- clear 1000`

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::name::OpKey;
use ryukei_testkit::nurbs_race;
use std::time::Instant;

fn machine() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.into())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let kind = args.get(1).map_or("clear", String::as_str);
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    let block = nurbs_race::block();
    let pin = match kind {
        "flush" => nurbs_race::flush_pin(),
        "clear" => nurbs_race::clear_pin(),
        other => panic!("clear or flush, not {other}"),
    };
    let places = nurbs_race::places(n);
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &pin,
            place: *p,
        })
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let t = Instant::now();
    let (cut, _) = pool
        .install(|| boolean(OpKey(3), BooleanOp::Subtract, &block, &tools))
        .expect("the race cuts");
    println!(
        "{kind} {n}: {:.2} s, {} faces, on {}, one thread",
        t.elapsed().as_secs_f64(),
        cut.counts().faces,
        machine()
    );
}
