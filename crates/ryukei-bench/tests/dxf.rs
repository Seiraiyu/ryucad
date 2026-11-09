//! DXF against its reference (design 346, plan 16.B): every flat pattern
//! ryuformats writes, of seeded sheet metal parts unfolded and of a ryucad
//! project's export, read by ezdxf: R2000, audited with no error and
//! nothing fixed, our five layers, each outline's and hole's area the
//! pattern's, every bend line where the pattern puts it, every note.
//! Devcontainer only:
//! `cargo test -p ryukei-bench --features dxf --release --test dxf -- --nocapture`.
//! The Python is `RYUCAD_DXF_PYTHON`, else the devcontainer's
//! `/opt/usd/bin/python`.

use ryuformats::dxf::{DxfOptions, DxfUnit, note, write};
use ryukei::name::OpKey;
use ryukei::sheet_metal::{Allowance, FlatPattern, SheetOptions, UnfoldSpec, recognize, unfold};
use ryukei_testkit::sheet_metal::{Leg, strip};
use serde_json::{Value, json};
use std::f64::consts::FRAC_PI_2;
use std::path::{Path, PathBuf};

fn reference(dir: &Path, files: &[PathBuf]) -> Vec<Value> {
    let python =
        std::env::var("RYUCAD_DXF_PYTHON").unwrap_or_else(|_| "/opt/usd/bin/python".into());
    let manifest = dir.join("manifest.json");
    std::fs::write(&manifest, json!({ "files": files }).to_string()).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dxf/reference.py");
    let out = std::process::Command::new(&python)
        .arg(&script)
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| panic!("{python}: {e}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Strips of one to four legs at quarter, eighth and half turns, both
/// ways, unfolded at a K-factor of 0.44 or a developed length.
fn patterns() -> Vec<(String, FlatPattern)> {
    let turns = [
        FRAC_PI_2,
        -FRAC_PI_2,
        FRAC_PI_2 / 2.0,
        3.0 * FRAC_PI_2 / 2.0,
    ];
    let mut out = Vec::new();
    for n in 1..=3usize {
        for (i, a) in turns.iter().enumerate() {
            let mut legs = vec![Leg::Run(0.02)];
            for j in 0..n {
                let angle = if j % 2 == 0 { *a } else { -*a };
                legs.push(Leg::Turn {
                    angle,
                    radius: 0.001 + 0.0005 * j as f64,
                });
                legs.push(Leg::Run(0.015 + 0.005 * j as f64));
            }
            let b = strip(OpKey(n as u128 * 10 + i as u128), &legs, 0.0012, 0.03);
            let m = recognize(&b, &SheetOptions::default()).unwrap();
            let allowance = if i % 2 == 0 {
                Allowance::KFactor(0.44)
            } else {
                Allowance::Length(0.002)
            };
            let u = unfold(OpKey(1), &b, &m, &UnfoldSpec::all(allowance)).unwrap();
            out.push((format!("strip-{n}-{i}"), u.pattern));
        }
    }
    out
}

#[test]
fn ezdxf_reads_every_flat_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    let mut expect = Vec::new();
    for (name, p) in patterns() {
        for unit in [DxfUnit::Mm, DxfUnit::In] {
            let f = dir.path().join(format!("{name}-{}.dxf", unit.as_str()));
            std::fs::write(&f, write(&p, &DxfOptions { unit, notes: true }).unwrap()).unwrap();
            files.push(f);
            expect.push((p.clone(), unit));
        }
    }
    let got = reference(dir.path(), &files);
    assert_eq!(got.len(), files.len());
    for (r, (p, unit)) in got.iter().zip(&expect) {
        let f = r["file"].as_str().unwrap();
        assert_eq!(r["version"], "AC1015", "{f}");
        assert_eq!(r["errors"], json!([]), "{f}");
        assert_eq!(r["fixes"], 0, "{f}: ezdxf fixed something");
        for l in ["OUTLINE", "INNER", "BEND_UP", "BEND_DOWN", "BEND_NOTES"] {
            assert!(
                r["layers"].as_array().unwrap().iter().any(|x| x == l),
                "{f}: {l}"
            );
        }
        let k = unit.per_meter();
        let mut want: Vec<f64> = p
            .loops
            .iter()
            .filter(|l| l.outer)
            .map(|l| l.area().abs() * k * k)
            .collect();
        want.sort_by(f64::total_cmp);
        let outline: Vec<f64> = serde_json::from_value(r["outline"].clone()).unwrap();
        assert_eq!(outline.len(), want.len(), "{f}");
        for (a, b) in outline.iter().zip(&want) {
            assert!((a - b).abs() <= 1e-6 * b, "{f}: outline {a} against {b}");
        }
        assert_eq!(
            r["bend_lines"].as_array().unwrap().len(),
            p.bend_lines.len(),
            "{f}"
        );
        // The reference lists BEND_UP's lines before BEND_DOWN's: each of
        // the pattern's is found on its direction's layer, where it puts it.
        for l in &p.bend_lines {
            let layer = match l.direction {
                ryukei::sheet_metal::BendDirection::Up => "BEND_UP",
                ryukei::sheet_metal::BendDirection::Down => "BEND_DOWN",
            };
            let at = |v: &Value, i: usize| v.as_array().unwrap()[i].as_f64().unwrap();
            let found = r["bend_lines"].as_array().unwrap().iter().any(|b| {
                b["layer"] == layer
                    && (at(&b["from"], 0) - l.start.x * k).abs() < 1e-9
                    && (at(&b["from"], 1) - l.start.y * k).abs() < 1e-9
                    && (at(&b["to"], 0) - l.end.x * k).abs() < 1e-9
                    && (at(&b["to"], 1) - l.end.y * k).abs() < 1e-9
            });
            assert!(found, "{f}: bend line {l:?}");
        }
        let notes: Vec<String> = p.bend_lines.iter().map(|l| note(l, *unit, "%%d")).collect();
        // ezdxf reads %%d back as written.
        assert_eq!(r["notes"], json!(notes), "{f}");
    }
    eprintln!("ezdxf read {} DXF files clean", files.len());
}

#[test]
fn ezdxf_reads_ryucads_export() {
    use ryucad::proto::Request;
    use ryucad::session::Session;
    let dir = tempfile::tempdir().unwrap();
    ryucad::server::init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    let must = |s: &mut Session, m: &str, v: Value| {
        ryucad::server::handle(s, Request::from_rpc(m, Some(v)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    must(&mut s, "part.create", json!({"part": "p"}));
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "u", "plane": "xy"}),
    );
    let pts = [[0.0, 30.0], [0.0, 0.0], [40.0, 0.0], [40.0, 30.0]];
    let mut ops = Vec::new();
    for (i, at) in pts.iter().enumerate() {
        ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
        ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
    }
    for i in 1..=3 {
        ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line", "start": format!("p{i}"), "end": format!("p{}", i + 1)}));
    }
    must(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "u", "ops": ops}),
    );
    must(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "base_flange", "id": "b", "sketch": "u",
        "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": "0.44"}, "depth": "25 mm"}),
    );
    let out = dir.path().join("u.dxf");
    must(
        &mut s,
        "export",
        json!({"part": "p", "format": "dxf", "out": out}),
    );
    let r = &reference(dir.path(), &[out])[0];
    assert_eq!(r["errors"], json!([]));
    assert_eq!(r["fixes"], 0);
    assert_eq!(r["bend_lines"].as_array().unwrap().len(), 2);
    let flat = must(&mut s, "flat.get", json!({"part": "p"}));
    let want = flat["loops"][0]["area_mm2"].as_f64().unwrap();
    let got = r["outline"][0].as_f64().unwrap();
    assert!((got - want).abs() <= 1e-6 * want, "{got} against {want}");
}
