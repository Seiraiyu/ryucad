//! Mass properties of the committed corpus (ryukei 15.B; issues #307 and
//! #308, step-hunt's `volume` stage): every body the manifest lists as
//! healed with a volume, its mass properties on one thread under 30 s
//! (step-hunt's `--slow` mark), the volume within its bound and within
//! two thousandths of OCCT's. Release only. `RYUKEI_BARS=1` holds the
//! 30 s as it is, else ten times looser, as ryucad's bars are.

use ryukei::measure::{MassOptions, Value, mass};
use ryukei::name::OpKey;
use ryustep::{ReadOptions, read};
use std::time::Instant;

/// step-hunt's slow mark, in seconds.
const BAR: f64 = 30.0;

fn dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: run it in release"
)]
fn the_corpus_weighs_in_under_the_mark() {
    let bar = if std::env::var("RYUKEI_BARS").as_deref() == Ok("1") {
        BAR
    } else {
        10.0 * BAR
    };
    let text = std::fs::read_to_string(dir().join("manifest.txt")).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let opts = ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    };
    let mut slow = Vec::new();
    let mut files: Vec<&str> = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f[2] == "ok" && !files.contains(&f[0]) {
            files.push(f[0]);
        }
    }
    for name in files {
        let model = read(&std::fs::read(dir().join(name)).unwrap(), &opts).unwrap();
        for b in &model.bodies {
            let Some(occt) = text.lines().find_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                (f.len() == 4 && f[0] == name && f[1] == b.representation.to_string())
                    .then(|| f[3].parse::<f64>().unwrap() * 1e-9)
            }) else {
                continue;
            };
            let body = &b.result.as_ref().unwrap().0;
            let t = Instant::now();
            let m = pool.install(|| mass(body, 1.0, &MassOptions::default()));
            let s = t.elapsed().as_secs_f64();
            let m = m.unwrap_or_else(|e| panic!("{name} #{}: {e:?}", b.representation));
            let Value::Bounded { value, bound } = m.volume else {
                panic!("{name}: an exact volume");
            };
            println!(
                "{name} #{}: {s:.2} s, {value:e} m³ ± {bound:e}, OCCT {occt:e}",
                b.representation
            );
            assert!(bound.is_finite() && bound < 1e-2 * value, "{name}: {bound}");
            assert!(
                (value - occt).abs() <= 2e-3 * occt,
                "{name}: {value} against {occt}"
            );
            if s > bar {
                slow.push(format!("{name} #{}: {s:.1} s", b.representation));
            }
        }
    }
    assert!(slow.is_empty(), "past {bar} s: {slow:#?}");
}
