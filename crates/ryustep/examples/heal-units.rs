//! Each body of each STEP file given, healed with no budget: its work
//! units and seconds, then the largest units. For calibrating ryucad's
//! `heal.budget` (plan 15.A, Task 29).
//!
//! `cargo run --release -p ryustep --example heal-units -- <file>…`

use ryukei::work::Work;
use ryustep::{ReadOptions, open};
use std::time::Instant;

fn main() {
    let mut most = (0u64, String::new());
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("a file");
        let Ok(opened) = open(&bytes) else {
            eprintln!("{path}: not readable");
            continue;
        };
        for k in 0..opened.held.len() {
            let opts = ReadOptions {
                key: ryukei::name::OpKey(1),
                heal: Default::default(),
            };
            let w = Work::unlimited();
            let t = Instant::now();
            let r = opened.body_with(k, &opts, &w);
            let units = w.spent();
            println!(
                "{path} #{k} {units} units {:.2} s {}",
                t.elapsed().as_secs_f64(),
                if r.result.is_ok() {
                    "healed"
                } else {
                    "refused"
                }
            );
            if r.result.is_ok() && units > most.0 {
                most = (units, format!("{path} #{k}"));
            }
        }
    }
    println!("largest healed: {} units ({})", most.0, most.1);
}
