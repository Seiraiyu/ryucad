//! Times how soon a body's heal stops once cancelled (#485):
//!
//! ```text
//! cargo run --release -p ryustep --example cancel-probe -- <file.step> [STEPS]
//! ```
//!
//! Heals each body once to time it, then again cancelled at `STEPS`
//! (default 8) evenly spaced moments of that time, printing for each how
//! long the heal ran on after its cancel and what it answered.

use ryukei::name::OpKey;
use ryukei::work::Work;
use ryustep::ReadOptions;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: cancel-probe <file.step> [STEPS]");
        std::process::exit(2);
    };
    let steps: u32 = args.get(2).map_or(8, |s| s.parse().expect("a count"));
    let bytes = std::fs::read(path).expect("the file reads");
    let opened = ryustep::open(&bytes).expect("a STEP file");
    let opts = ReadOptions {
        key: OpKey::from_bytes(b"cancel-probe"),
        heal: Default::default(),
    };
    for k in 0..opened.held.len() {
        let t = Instant::now();
        let b = opened.body(k, &opts);
        let whole = t.elapsed();
        let ok = b.result.is_ok();
        println!(
            "body {k} (#{}): {:.2} s, ok {ok}",
            b.representation,
            whole.as_secs_f64()
        );
        let mut worst = Duration::ZERO;
        for i in 1..steps {
            let at = whole.mul_f64(f64::from(i) / f64::from(steps));
            let flag = AtomicBool::new(false);
            let (late, what) = std::thread::scope(|s| {
                let h = s.spawn(|| {
                    let b = opened.body_with(k, &opts, &Work::unlimited().with_cancel(&flag));
                    (Instant::now(), b.result.err().map(|e| format!("{e:?}")))
                });
                std::thread::sleep(at);
                let sent = Instant::now();
                flag.store(true, Ordering::SeqCst);
                let (done, what) = h.join().unwrap();
                (done.saturating_duration_since(sent), what)
            });
            worst = worst.max(late);
            println!(
                "  cancelled at {:.2} s: stopped {:.3} s later, {}",
                at.as_secs_f64(),
                late.as_secs_f64(),
                what.unwrap_or_else(|| "ok".into())
            );
        }
        println!("  worst {:.3} s", worst.as_secs_f64());
    }
}
