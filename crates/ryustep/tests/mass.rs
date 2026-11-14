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

/// KiCad 9.0.9's `Symbol.3dshapes/Smolhaj_Scale_0.1.step`, body 5 (116
/// faces, 100 of them splines): the slowest mass #308 names, healed under
/// the key ryucad's import gives part `body` (step-hunt's), weighed on one
/// thread to the same volume and bound, bit for bit. The file is the
/// pinned KiCad corpus's (`sh scripts/fetch-kicad.sh`, into `target/kicad`
/// or `$RYUEDA_KICAD_CORPUS`), never committed, so this runs by name:
///
/// ```text
/// RYUKEI_BARS=1 cargo test -p ryustep --release --test mass -- --ignored smolhaj --nocapture
/// ```
///
/// Measured 37.0 to 37.9 s on cox-worker-2 (AMD Ryzen 7 9700X, one thread,
/// load 1.1 to 2.5, 2026-11-10) with the bound mesh at 0.05 rad. Since
/// v0.9.1 the bound mesh is at 0.15 rad with each triangle's certified
/// turn in the area bound (#308, the owner's decision): the same values,
/// bounds a little wider, under step-hunt's 30 s mark with
/// `RYUKEI_BARS=1`, else ten times looser.
#[test]
#[ignore = "needs the pinned KiCad corpus: sh scripts/fetch-kicad.sh"]
fn smolhaj_body_5_weighs_in_bit_for_bit() {
    let bar = if std::env::var("RYUKEI_BARS").as_deref() == Ok("1") {
        30.0
    } else {
        300.0
    };
    let corpus = std::env::var_os("RYUEDA_KICAD_CORPUS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/kicad")
        });
    let path = corpus.join("kicad-packages3D-9.0.9/Symbol.3dshapes/Smolhaj_Scale_0.1.step");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let opened = ryustep::open(&bytes).unwrap();
    let opts = ReadOptions {
        key: OpKey::from_bytes(b"ryucad.feature\0body\0body"),
        heal: Default::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let read = pool.install(|| opened.body(5, &opts));
    let body = &read.result.as_ref().unwrap().0;
    let t = Instant::now();
    let m = pool
        .install(|| mass(body, 1.0, &MassOptions::default()))
        .unwrap();
    let s = t.elapsed().as_secs_f64();
    println!(
        "Smolhaj body 5: mass {s:.2} s, {:?}, area {:?}",
        m.volume, m.area
    );
    let bits = |v: Value| match v {
        Value::Bounded { value, bound } => (value.to_bits(), bound.to_bits()),
        Value::Exact(x) => panic!("an exact value {x}"),
    };
    assert_eq!(
        bits(m.volume),
        (
            2.1423404921823934e-6f64.to_bits(),
            3.834075368398429e-8f64.to_bits()
        )
    );
    assert_eq!(
        bits(m.area),
        (
            0.0021604480982536293f64.to_bits(),
            5.226259438657934e-5f64.to_bits()
        )
    );
    assert!(s <= bar, "{s:.1} s, past {bar} s");
}
