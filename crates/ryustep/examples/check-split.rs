//! Splits what `ryucad check` spends on each body of STEP files: its heal,
//! its self-intersection check on one thread, and its mesh at ryucad's
//! normal display quality (a chord of 5e-4 of its box's diagonal, 15°).
//! Bodies are healed under the key ryucad's import of part `body` gives
//! its feature `body`, as step-hunt's cases are, so a case's check here
//! is the one its `check` stage ran.
//!
//! ```text
//! cargo run --release -p ryustep --example check-split -- <file.step>…
//! ```

use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryustep::ReadOptions;
use std::time::Instant;

fn main() {
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("a pool");
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("the file reads");
        let Ok(opened) = ryustep::open(&bytes) else {
            println!("{path}: not readable");
            continue;
        };
        let opts = ReadOptions {
            // `CHECK_SPLIT_KEY=7` heals as ryustep's tests do (`OpKey(7)`).
            key: match std::env::var("CHECK_SPLIT_KEY") {
                Ok(k) => OpKey(k.parse().expect("a key")),
                Err(_) => OpKey::from_bytes(b"ryucad.feature\0body\0body"),
            },
            heal: Default::default(),
        };
        for k in 0..opened.held.len() {
            let t = Instant::now();
            let read = one.install(|| opened.body(k, &opts));
            let heal = t.elapsed().as_secs_f64();
            let Ok((body, _, _)) = &read.result else {
                println!("{path} #{k}: heal {heal:.1} s, refused");
                continue;
            };
            let t = Instant::now();
            let faults = one.install(|| ryukei::check::check_intersections(body));
            let check = t.elapsed().as_secs_f64();
            // ryucad's diagonal for an import: its faces' boxes.
            let diagonal = body
                .faces()
                .map(|f| ryukei::intersect::face_box(body, f))
                .reduce(|a, b| a.union(&b))
                .map_or(1.0, |b| {
                    let d = b.max() - b.min();
                    (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
                });
            let params = MeshParams::new((diagonal * 5e-4).max(1e-6), std::f64::consts::PI / 12.0);
            let t = Instant::now();
            let mesh = one.install(|| tessellate(body, &params));
            let mesh_s = t.elapsed().as_secs_f64();
            println!(
                "{path} #{k}: {} faces, heal {heal:.1} s, check {check:.1} s ({} faults), mesh {mesh_s:.1} s ({})",
                body.counts().faces,
                faults.len(),
                match &mesh {
                    Ok(m) => format!("{} faults", m.faults.len()),
                    Err(e) => format!("{e}"),
                }
            );
        }
    }
}
