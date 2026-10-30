//! Times each body of a STEP file as ryustep reads it, for finding the
//! bodies that make an import slow or fail:
//!
//! ```text
//! cargo run --release -p ryustep --example step-probe -- <file.step> [--slow SECONDS]
//! ```
//!
//! Prints the open (parse, decode, product tree), then a line per body as
//! it finishes (its representation, product, seconds, faces or error),
//! and at the end every body slower than `--slow` (10 s), slowest first.
//! Bodies are read in parallel on rayon's pool, as `ryustep::read` reads
//! them; a body still running is one with no line yet.

use rayon::prelude::*;
use ryukei::name::OpKey;
use ryustep::ReadOptions;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: step-probe <file.step> [--slow SECONDS]");
        std::process::exit(2);
    };
    let slow: f64 = match args.iter().position(|a| a == "--slow") {
        Some(i) => args[i + 1].parse().expect("--slow takes seconds"),
        None => 10.0,
    };
    let bytes = std::fs::read(path).expect("the file reads");
    let t = Instant::now();
    let opened = ryustep::open(&bytes).expect("a STEP file");
    println!(
        "opened in {:.1} s: {} products, {} bodies",
        t.elapsed().as_secs_f64(),
        opened.products.definitions.len(),
        opened.held.len()
    );
    let opts = ReadOptions {
        key: OpKey::from_bytes(b"step-probe"),
        heal: Default::default(),
    };
    let start = Instant::now();
    let mut times: Vec<(f64, String)> = (0..opened.held.len())
        .into_par_iter()
        .map(|k| {
            let t = Instant::now();
            let b = opened.body(k, &opts);
            let s = t.elapsed().as_secs_f64();
            let product = b
                .definition
                .map(|d| opened.products.definitions[d].product_id.clone())
                .unwrap_or_default();
            let what = match &b.result {
                Ok((body, _, _)) => format!("{} faces", body.faces().count()),
                Err(e) => format!("{e:?}"),
            };
            let line = format!("#{} {product}: {s:.2} s, {what}", b.representation);
            println!("[{:7.1} s] {line}", start.elapsed().as_secs_f64());
            (s, line)
        })
        .collect();
    times.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total: f64 = times.iter().map(|t| t.0).sum();
    println!(
        "{} bodies in {:.1} s ({total:.1} s of work); slower than {slow} s:",
        times.len(),
        start.elapsed().as_secs_f64()
    );
    for (s, line) in times.iter().take_while(|t| t.0 > slow) {
        println!("  {s:8.1} s  {line}");
    }
}
