//! Hidden-line views timed on one rayon thread (design 484, plan 17.C):
//! the OCCT race's drilled plate and the race bodies, each timed three
//! ways: its silhouettes alone, its view without silhouettes, and its
//! whole view. For profiling; the bars are `tests/projection.rs`'s and
//! `ryukei/tests/hidden_lines.rs`'s.
//!
//! `cargo run --release -p ryukei-bench --example view-probe -- plate 1000`
//! `cargo run --release -p ryukei-bench --example view-probe -- race 0 200`

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::project::{Placed, View, ViewOptions, Visibility, silhouettes, view};
use ryukei::topo::Body;
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

/// The OCCT race's plate: `cols × rows` bores of 0.3 at a pitch of 1 in
/// a block 0.5 thick (as `benches/projection.rs` builds it).
fn plate(cols: usize, rows: usize) -> Body {
    let block = primitive::block(OpKey(1), cols as f64, rows as f64, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..rows)
        .flat_map(|j| {
            (0..cols).map(move |i| {
                Rigid::translation(Vector3::new(i as f64 + 0.5, j as f64 + 0.5, -0.25))
            })
        })
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
    boolean(OpKey(3), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0
}

/// Times `body` three ways and prints one line; returns the whole view's
/// seconds.
fn probe(name: &str, body: &Body, v: &View) -> f64 {
    let placed = [Placed {
        body,
        place: Rigid::IDENTITY,
    }];
    let t = Instant::now();
    let sil = silhouettes(body, v);
    let s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let edges = view(&placed, v, &ViewOptions { silhouettes: false });
    let e = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let whole = view(&placed, v, &ViewOptions::default());
    let w = t.elapsed().as_secs_f64();
    let what = match &whole {
        Ok(p) => format!(
            "{} pieces, {} uncertain",
            p.pieces.len(),
            p.pieces
                .iter()
                .filter(|q| q.visibility == Visibility::Uncertain)
                .count()
        ),
        Err(err) => format!("refused {err:?}"),
    };
    let ok = |r: bool| if r { "ok" } else { "refused" };
    println!(
        "{name}: silhouettes {s:.3} s ({}), edges only {e:.3} s ({}), whole {w:.3} s: {what}",
        ok(sil.is_ok()),
        ok(edges.is_ok())
    );
    let slots: Vec<String> = ryukei::project::PROFILE
        .iter()
        .map(|a| {
            format!(
                "{:.3}",
                a.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 * 1e-9
            )
        })
        .collect();
    println!("  slots: {}", slots.join(" "));
    w
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: usize, d: usize| args.get(k).and_then(|s| s.parse().ok()).unwrap_or(d);
    let v = View::new(Vector3::new(1.0, -1.0, 1.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    println!("{}, one thread", machine());
    pool.install(|| match args.get(1).map_or("plate", String::as_str) {
        "plate" => {
            let n = arg(2, 1000);
            let (c, r) = match n {
                1 => (1, 1),
                100 => (10, 10),
                _ => (40, 25),
            };
            probe(&format!("plate {n}"), &plate(c, r), &v);
        }
        "race" => {
            let bodies = ryukei_testkit::race::bodies(200);
            let (from, to) = (arg(2, 0), arg(3, bodies.len()).min(bodies.len()));
            let mut total = 0.0;
            for (i, b) in bodies.iter().enumerate().take(to).skip(from) {
                total += probe(&format!("race {i}"), b, &v);
            }
            println!("race {from}..{to}: whole views {total:.1} s");
        }
        other => panic!("plate or race, not {other}"),
    });
}
