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

/// The entities `ryu_draw::dxf` writes for `drawing.get`'s items, by DXF
/// type: a segment each (a whole arc a `CIRCLE`), a closing line where a
/// closed path doesn't end at its start, a `SOLID` for a filled path of
/// two or three lines (an arrowhead), a `TEXT` for each text.
fn drawn_counts(items: &[Value]) -> std::collections::BTreeMap<String, u64> {
    let mut out = std::collections::BTreeMap::new();
    let mut add = |k: &str| *out.entry(k.to_owned()).or_insert(0) += 1;
    for it in items {
        let Some(p) = it.get("path") else {
            add("TEXT");
            continue;
        };
        if it.get("stroke").is_none() && it.get("fill").is_none() {
            continue;
        }
        let segs = p["segments"].as_array().unwrap();
        let lines = segs.iter().all(|s| s.get("line").is_some());
        if it.get("fill").is_some() && lines && (2..=3).contains(&segs.len()) {
            add("SOLID");
            continue;
        }
        for sg in segs {
            if sg.get("line").is_some() {
                add("LINE");
            } else if let Some(a) = sg.get("arc") {
                let whole = a["sweep"].as_f64().unwrap().abs() >= std::f64::consts::TAU - 1e-12;
                add(if whole { "CIRCLE" } else { "ARC" });
            } else if sg.get("ellipse").is_some() {
                add("ELLIPSE");
            } else {
                add("SPLINE");
            }
        }
        // The writer closes a closed path whose last segment doesn't end
        // at its start, by the same arithmetic.
        let pt = |v: &Value| [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()];
        let end = segs.last().map_or(pt(&p["start"]), |sg| {
            if let Some(q) = sg.get("line") {
                pt(q)
            } else if let Some(a) = sg.get("arc") {
                let (c, r) = (pt(&a["centre"]), a["radius"].as_f64().unwrap());
                let t = a["from"].as_f64().unwrap() + a["sweep"].as_f64().unwrap();
                let (sn, cs) = ryukei::math::sin_cos(t);
                [c[0] + r * cs, c[1] + r * sn]
            } else if let Some(e) = sg.get("ellipse") {
                let (c, ea, eb) = (pt(&e["centre"]), pt(&e["a"]), pt(&e["b"]));
                let t = e["from"].as_f64().unwrap() + e["sweep"].as_f64().unwrap();
                let (sn, cs) = ryukei::math::sin_cos(t);
                [
                    c[0] + (ea[0] * cs + eb[0] * sn),
                    c[1] + (ea[1] * cs + eb[1] * sn),
                ]
            } else {
                pt(sg["bezier"]["points"].as_array().unwrap().last().unwrap())
            }
        });
        if p["closed"] == true && end != pt(&p["start"]) {
            add("LINE");
        }
    }
    out
}

/// Every drawing a ryucad project makes, each sheet's DXF in millimetres
/// and inches, read by ezdxf (plan 9.D): R2000, audited with no error and
/// nothing fixed, a layer for each line kind and text, and as many
/// entities of each type as the sheet's items make.
#[test]
fn ezdxf_reads_every_drawing_dxf() {
    use ryucad::proto::Request;
    use ryucad::server::handle;
    use ryucad::session::Session;
    let call = |s: &mut Session, m: &str, p: Value| {
        handle(s, Request::from_rpc(m, Some(p)).unwrap()).unwrap_or_else(|e| panic!("{m}: {e}"))
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    ryucad::server::init(&root, None).unwrap();
    let mut s = Session::open(&root).unwrap();
    call(&mut s, "part.create", json!({"part": "p"}));
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 60, "y": 40, "z": 12}}),
    );
    // Its four upright edges rounded, so the top view draws arcs, and a
    // ring joined on its top, whose silhouette the views trace as
    // splines (#655): every segment kind ryu_draw::dxf writes.
    let uprights: Vec<String> = call(&mut s, "inspect", json!({"part": "p"}))["bodies"][0]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["curve"]["type"] == "line"
                && e["curve"]["dir"][2]
                    .as_f64()
                    .is_some_and(|z| z.abs() == 1.0)
        })
        .map(|e| format!("p/b/{}", e["name"].as_str().unwrap()))
        .collect();
    assert_eq!(uprights.len(), 4, "{uprights:?}");
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "fillet", "id": "r", "radius": 4, "edges": uprights}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "cylinder", "id": "h", "operation": "cut", "targets": ["b"],
               "params": {"radius": 6, "height": 12}, "placement": {"origin": [30, 20, 0]}}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "torus", "id": "t", "operation": "join", "targets": ["b"],
               "params": {"major_radius": 5, "minor_radius": 2}, "placement": {"origin": [12, 12, 12]}}),
    );
    // The standard views, the bore's diameter with its fit, a full
    // section through the hole and a detail of it: every item kind,
    // hatching and dimension lines.
    call(
        &mut s,
        "drawing.create",
        json!({"part": "p", "drawing": "d", "name": "Plate", "size": "ansi_c", "views": "standard"}),
    );
    let body = call(&mut s, "inspect", json!({"part": "p"}))["bodies"][0].clone();
    let wall = body["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["surface"]["type"] == "cylinder")
        .map(|f| format!("b/{}", f["name"].as_str().unwrap()))
        .unwrap();
    let ops = json!([{"op": "add", "kind": "annotation",
        "value": {"type": "diameter", "view": "v2", "of": wall, "tolerance": {"fit": "H7"}}}]);
    call(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": ops}),
    );
    call(
        &mut s,
        "drawing.edit",
        json!({"part": "p", "drawing": "d", "ops": [
            {"op": "add", "kind": "view", "value": {"type": "section", "sheet": "s1", "from": "v2",
             "label": "A", "cut": {"origin": ["0 mm", "20 mm", "0 mm"], "normal": [0, 1, 0]}}},
            {"op": "add", "kind": "view", "value": {"type": "detail", "sheet": "s1", "from": "v2",
             "label": "B", "centre": ["30 mm", "20 mm", "12 mm"], "radius": "10 mm"}}]}),
    );
    call(
        &mut s,
        "drawing.layout",
        json!({"part": "p", "drawing": "d"}),
    );
    let got = call(
        &mut s,
        "drawing.get",
        json!({"part": "p", "drawing": "d", "items": true}),
    );
    let items = got["sheets"][0]["items"].as_array().unwrap().clone();
    let want = drawn_counts(&items);
    let out = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    for unit in ["mm", "in"] {
        let f = out.path().join(format!("d-{unit}.dxf"));
        call(
            &mut s,
            "drawing.export",
            json!({"part": "p", "drawing": "d", "format": "dxf", "unit": unit, "out": f.to_str().unwrap()}),
        );
        files.push(f);
    }
    let got = reference(out.path(), &files);
    for r in &got {
        let f = r["file"].as_str().unwrap();
        assert_eq!(r["version"], "AC1015", "{f}");
        assert_eq!(r["errors"], json!([]), "{f}");
        assert_eq!(r["fixes"], 0, "{f}: ezdxf fixed something");
        for l in [
            "VISIBLE",
            "THIN",
            "BORDER",
            "HATCH",
            "CUTTING",
            "DIMENSION",
            "TEXT",
        ] {
            assert!(
                r["layers"].as_array().unwrap().iter().any(|x| x == l),
                "{f}: {l}"
            );
        }
        let counts: std::collections::BTreeMap<String, u64> =
            serde_json::from_value(r["counts"].clone()).unwrap();
        assert_eq!(counts, want, "{f}");
        for kind in [
            "LINE", "ARC", "CIRCLE", "ELLIPSE", "SPLINE", "SOLID", "TEXT",
        ] {
            assert!(
                counts.get(kind).is_some_and(|n| *n > 0),
                "{f}: no {kind}: {counts:?}"
            );
        }
    }
    eprintln!(
        "ezdxf read {} drawing DXF files clean: {want:?}",
        files.len()
    );
}
