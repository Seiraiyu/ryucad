//! KiCad import of the files in `tests/data/kicad`, which we wrote in
//! KiCad 9's formats (no KiCad library file is committed): what each
//! becomes, that importing again changes nothing, that a dry run writes
//! nothing, and that a changed file is written again in place.

use ryueda::error::Code;
use ryueda::footprint::{Drill, Footprint, Model, PadKind, PadShape, Shape};
use ryueda::id::Id;
use ryueda::kicad::import::{MODELS_LINE, Options, import};
use ryueda::libcomp::LibComp;
use ryueda::session::Session;
use ryueda::symbol::{PinType, Side, Symbol};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/kicad")
}

/// Everything committed: both symbol files, every footprint folder, with
/// the folders a symbol's footprints and the models are found in.
fn everything() -> Options {
    let d = data();
    Options {
        paths: vec![
            d.join("symbols/Ryu_Test.kicad_sym"),
            d.join("footprints/Ryu_Test.pretty"),
        ],
        models: vec![d.join("3dmodels")],
        ..Options::default()
    }
}

fn project() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    ryueda::server::init(dir.path(), None, false).unwrap();
    let s = Session::open(dir.path()).unwrap();
    (dir, s)
}

fn read<T: serde::de::DeserializeOwned>(root: &Path, rel: &str) -> T {
    let v: Value = serde_json::from_slice(&std::fs::read(root.join(rel)).unwrap()).unwrap();
    let mut o = v.as_object().unwrap().clone();
    o.remove("kind");
    o.remove("format");
    serde_json::from_value(Value::Object(o)).unwrap()
}

fn codes(r: &Value) -> Vec<String> {
    r["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["code"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn kicads_files_import_exactly() {
    let (dir, mut s) = project();
    let root = dir.path();
    let r = import(&mut s, &everything(), None).unwrap();
    assert_eq!(r["refused"], json!([]), "{r}");
    assert_eq!(
        r["symbols"]["added"],
        json!([
            "lib/self/symbol/ryu_test_nand2",
            "lib/self/symbol/ryu_test_r",
            "lib/self/symbol/ryu_test_sensor"
        ])
    );
    assert_eq!(
        r["components"]["added"],
        json!([
            "lib/self/libcomp/ryu_test_nand2",
            "lib/self/libcomp/ryu_test_r",
            "lib/self/libcomp/ryu_test_r_10k",
            "lib/self/libcomp/ryu_test_sensor"
        ])
    );
    assert_eq!(
        r["footprints"]["added"],
        json!([
            "lib/self/footprint/ryu_test_every_pad",
            "lib/self/footprint/ryu_test_r_0402"
        ])
    );
    assert_eq!(
        r["models"],
        json!({"added": ["every_pad.step", "r_0402.step"], "reused": 0})
    );
    let mut notices = codes(&r);
    notices.sort();
    assert_eq!(
        notices,
        [
            "import_demorgan_dropped",
            "import_ignored",
            "import_layer_unmapped",
            "import_power_skipped",
            "import_rounded",
            // The footprint the gates name, and a model the footprint does.
            "import_unresolved",
            "import_unresolved"
        ]
    );
    // The sensor's footprint has pads its symbol doesn't map: warnings.
    let faults = r["faults"].as_array().unwrap();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0]["reference"], "lib/self/libcomp/ryu_test_sensor");

    // Two gates with a supply unit; their De Morgan style dropped.
    let nand: Symbol = read(root, "library/symbols/ryu_test_nand2.json");
    assert_eq!(
        (nand.units, nand.pins.len(), nand.graphics.len()),
        (3, 8, 3)
    );
    let vdd = &nand.pins["8"];
    assert_eq!(
        (vdd.name.as_str(), vdd.ty, vdd.unit, vdd.side, vdd.at),
        ("VDD", PinType::PowerIn, Some(3), Side::Up, [0, 12_700_000])
    );
    assert_eq!(nand.pins["1"].shape, ryueda::symbol::PinShape::Line);
    let sensor: Symbol = read(root, "library/symbols/ryu_test_sensor.json");
    let alts: Vec<&str> = sensor.pins["1"]
        .alternates
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(alts, ["GPIO", "TX"]);
    assert_eq!(sensor.pins["4"].ty, PinType::NoConnect);
    assert!(sensor.graphics.values().any(|g| matches!(
        g,
        ryueda::symbol::Graphic::Text {
            rotation_mdeg: 90_000,
            ..
        }
    )));

    // A derived symbol: a component drawn by its parent, with the
    // parent's properties under its own.
    let r10k: LibComp = read(root, "library/components/ryu_test_r_10k.json");
    assert_eq!(r10k.symbol.id.as_str(), "ryu_test_r");
    assert_eq!((r10k.value.as_str(), r10k.prefix.as_str()), ("10k", "R"));
    assert_eq!(
        (r10k.manufacturer.as_str(), r10k.mpn.as_str()),
        ("Yageo", "RC0402FR-0710KL")
    );
    assert_eq!(r10k.offers[0].sku, "C25744");
    assert_eq!(r10k.footprints[0].footprint.id.as_str(), "ryu_test_r_0402");
    let r_comp: LibComp = read(root, "library/components/ryu_test_r.json");
    assert_eq!(
        (r_comp.datasheet.as_str(), r_comp.description.as_str()),
        ("", "Resistor")
    );
    assert_eq!(r_comp.parameters["ki_keywords"], "R res resistor");
    assert!(!root.join("library/components/ryu_test_gnd.json").exists());
    let nand_comp: LibComp = read(root, "library/components/ryu_test_nand2.json");
    assert!(nand_comp.footprints.is_empty());

    // Footprints, y up: every pad form, a keepout, models by their bytes.
    let fp = |id: &str| -> Footprint { read(root, &format!("library/footprints/{id}.json")) };
    let pad = |f: &Footprint, id: &str| f.pads[&Id::parse(id, "pad").unwrap()].clone();
    let r0402 = fp("ryu_test_r_0402");
    assert_eq!(pad(&r0402, "p1").at, [-510_000, 0]);
    assert_eq!(
        pad(&r0402, "p1").shape,
        PadShape::Roundrect { ratio_ppm: 250_000 }
    );
    assert_eq!(r0402.fields.reference.as_ref().unwrap().at, [0, 1_170_000]);
    let every = fp("ryu_test_every_pad");
    assert_eq!(pad(&every, "p1").at, [-1_500_000, -500_000]);
    assert_eq!(pad(&every, "p2").kind, PadKind::Through);
    assert_eq!(
        pad(&every, "p2").drill,
        Some(Drill::Slot {
            size: [600_000, 1_200_000],
            offset: [0, -100_000]
        })
    );
    assert_eq!(pad(&every, "p3").number, None);
    assert_eq!(
        pad(&every, "p4").shape,
        PadShape::Trapezoid {
            delta: [200_000, -100_000]
        }
    );
    assert!(matches!(
        pad(&every, "p5").shape,
        PadShape::Chamfered {
            ratio_ppm: 200_000,
            ..
        }
    ));
    assert!(
        matches!(&pad(&every, "p6").shape, PadShape::Custom { primitives, .. } if primitives.len() == 1)
    );
    assert_eq!(
        every.keepouts.values().next().unwrap().polygon[0],
        [-1_000_000, 1_000_000]
    );
    assert!(every.graphics.values().any(|g| g.layer == "edge"));
    assert!(
        every
            .graphics
            .values()
            .all(|g| !matches!(g.shape, Shape::Line { .. }))
    );
    // The .wrl it names is found as the STEP file beside it.
    assert_eq!(every.models.len(), 1);
    assert_eq!(every.models[0].rotation_mdeg, [0, 0, -90_000]);
    for (f, step) in [(&r0402, "R_0402.step"), (&every, "Every_Pad.step")] {
        let Model::Step(m) = &f.models[0].model;
        let bytes = std::fs::read(root.join(&m.path)).unwrap();
        assert_eq!(
            bytes,
            std::fs::read(data().join("3dmodels/Ryu_Test.3dshapes").join(step)).unwrap()
        );
        assert_eq!(m.blake3, blake3::hash(&bytes).to_hex().to_string());
    }
    let attrs = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
    assert_eq!(attrs, format!("{MODELS_LINE}\n"));

    // The library holds no error: nothing an import writes is one.
    let p = s.project();
    assert!(p.faults().iter().all(|f| !f.is_error()), "{:?}", p.faults());

    // Again: nothing to write.
    let again = import(&mut s, &everything(), None).unwrap();
    assert_eq!(again["files"], json!([]));
    assert_eq!(again["symbols"]["unchanged"], 3);
    assert_eq!(again["models"], json!({"added": [], "reused": 2}));
    assert_eq!(again["revision"], r["revision"]);
}

#[test]
fn a_dry_run_writes_nothing_and_a_change_is_written_in_place() {
    let (dir, mut s) = project();
    let root = dir.path();
    let before = s.project().library("self").unwrap().revision.clone();
    let dry = import(
        &mut s,
        &Options {
            dry_run: true,
            ..everything()
        },
        None,
    )
    .unwrap();
    assert_eq!(dry["revision"], json!(before));
    assert!(!dry["files"].as_array().unwrap().is_empty());
    assert!(!root.join("library").exists());
    assert!(!root.join(".gitattributes").exists());

    // A guarded import refuses a stale revision.
    let e = import(&mut s, &everything(), Some("stale")).unwrap_err();
    assert_eq!(e.code, Code::Conflict);
    import(&mut s, &everything(), Some(&before)).unwrap();

    // The library's resistor changes its description in KiCad: imported
    // again, its component is written again, and nothing else.
    let tmp = tempfile::tempdir().unwrap();
    let edited = tmp.path().join("Ryu_Test.kicad_sym");
    let text = std::fs::read_to_string(data().join("symbols/Ryu_Test.kicad_sym")).unwrap();
    std::fs::write(&edited, text.replacen("\"Resistor\"", "\"A resistor\"", 1)).unwrap();
    let r = import(
        &mut s,
        &Options {
            paths: vec![edited],
            ..Options::default()
        },
        None,
    )
    .unwrap();
    // The derived 10k takes its description from the resistor, so it
    // changes too.
    assert_eq!(
        r["components"]["updated"],
        json!([
            "lib/self/libcomp/ryu_test_r",
            "lib/self/libcomp/ryu_test_r_10k"
        ])
    );
    assert_eq!(
        r["files"],
        json!([
            "library/components/ryu_test_r.json",
            "library/components/ryu_test_r_10k.json"
        ])
    );
    let c: LibComp = read(root, "library/components/ryu_test_r.json");
    assert_eq!(c.description, "A resistor");
}

#[test]
fn what_isnt_a_kicad_library_is_refused_before_anything_is_written() {
    let (dir, mut s) = project();
    let tmp = tempfile::tempdir().unwrap();
    let legacy = tmp.path().join("old.lib");
    std::fs::write(&legacy, "EESchema-LIBRARY Version 2.4\n").unwrap();
    let broken = tmp.path().join("Broken.kicad_sym");
    std::fs::write(
        &broken,
        "(kicad_symbol_lib (version 20241209)\n  (symbol \"X\"",
    )
    .unwrap();
    let other = tmp.path().join("notes.txt");
    std::fs::write(&other, "hi").unwrap();
    for (path, code) in [
        (legacy, Code::KicadLegacyUnsupported),
        (broken.clone(), Code::KicadUnreadable),
        (other, Code::InvalidParam),
        (tmp.path().join("missing.kicad_sym"), Code::NotFound),
    ] {
        let mut opts = everything();
        opts.paths.push(path.clone());
        let e = import(&mut s, &opts, None).unwrap_err();
        assert_eq!(e.code, code, "{}: {e}", path.display());
    }
    let e = import(
        &mut s,
        &Options {
            paths: vec![broken],
            ..Options::default()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(
        (e.details["line"].as_u64(), e.details["column"].as_u64()),
        (Some(2), Some(3))
    );
    assert!(!dir.path().join("library").exists());
}
