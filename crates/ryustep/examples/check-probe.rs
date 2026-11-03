//! Scratch (planning 15.C): heal every body of a STEP file and time its
//! self-intersection check. Run with RYUKEI_CHECK_PROBE=1 for per-pair lines.
use ryukei::name::OpKey;
use ryustep::ReadOptions;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let only: Option<u64> = args.get(2).map(|a| a.parse().unwrap());
    let opened = ryustep::open(&bytes).unwrap();
    let opts = ReadOptions { key: OpKey(7), heal: Default::default() };
    let mut seen = Vec::new();
    for k in 0..opened.held.len() {
        let t = Instant::now();
        let b = opened.body(k, &opts);
        if only.is_some_and(|r| r != b.representation) { continue; }
        let Ok((body, _, _)) = b.result else { println!("#{} refused", b.representation); continue };
        let d = body.dump_unnamed();
        if seen.contains(&d) { continue; }
        seen.push(d);
        let heal = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let w = ryukei::work::Work::unlimited();
        let faults = ryukei::check::check_intersections_with(&body, &w).unwrap();
        println!("#{} faces {} heal {:.2} s check {:.2} s units {} faults {:?}",
            b.representation, body.faces().count(), heal, t.elapsed().as_secs_f64(), w.spent(), faults.len());
    }
}
