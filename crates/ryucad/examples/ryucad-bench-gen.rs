//! Writes design 192's benchmark project (`ryucad::bench::scale`):
//!
//! ```text
//! cargo run --release -p ryucad --features bench --example ryucad-bench-gen -- \
//!     target/scale/p [--parts 10000]
//! ```
//!
//! The folder must be empty or missing. The race bodies' STEP file is
//! made once and kept beside it (`race-<n>.step`): making the bodies
//! takes minutes, the rest seconds.

use ryucad::bench::scale::{Spec, race_file, write};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut spec = Spec::full();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--parts" => {
                let n = args.next().and_then(|n| n.parse().ok()).expect("--parts N");
                spec = Spec::parts(n);
            }
            _ => root = Some(PathBuf::from(a)),
        }
    }
    let root = root.expect("usage: ryucad-bench-gen <folder> [--parts N]");
    if std::fs::read_dir(&root).is_ok_and(|mut d| d.next().is_some()) {
        panic!("{} isn't empty", root.display());
    }
    let n = spec.imports();
    let kept = root
        .parent()
        .unwrap_or(&root)
        .join(format!("race-{n}.step"));
    let t = Instant::now();
    let race = match std::fs::read(&kept) {
        Ok(b) => b,
        Err(_) => {
            let b = race_file(n);
            std::fs::create_dir_all(kept.parent().expect("a folder")).expect("make it");
            std::fs::write(&kept, &b).expect("keep the race file");
            b
        }
    };
    let race_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let s = write(&root, &spec, &race);
    println!(
        "{}: {} parts, {} assemblies, {} occurrences, {} features, {} files in {:.1} s \
         (race file {:.1} s); digest {}",
        root.display(),
        s.parts,
        s.assemblies,
        s.occurrences,
        s.features,
        s.files,
        t.elapsed().as_secs_f64(),
        race_s,
        s.digest
    );
}
