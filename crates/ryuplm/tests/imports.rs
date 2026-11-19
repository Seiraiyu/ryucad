//! The committed import fixtures (designs 734 and 935): an Altium-shaped
//! and a KiCad-shaped BOM, an indented BOM, an item master workbook with
//! legacy numbers in three schemes and its parent/child structure, a
//! Latin-1 spreadsheet, a PLM's JSON item export, a JSON Lines dump and a
//! nested XML BOM in windows-1252, each read through a mapping written for
//! it. Written by us, nothing from a customer or vendor.
//! `UPDATE_FIXTURES=1` writes them.

use ryuplm::proto::Request;
use ryuplm::testing::{Cell, xlsx};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/imports")
}

const ALTIUM: &str = "\"Comment\",\"Description\",\"Designator\",\"Footprint\",\"LibRef\",\"Quantity\",\"Manufacturer 1\",\"Manufacturer Part Number 1\"\r\n\
\"10k\",\"Resistor, 1%, 0603\",\"R1, R2, R5\",\"R0603\",\"RES\",\"3\",\"Ohmcraft Demo\",\"OD-0603-10K\"\r\n\
\"100n\",\"Capacitor, X7R, 0402\",\"C1, C2, C3, C4\",\"C0402\",\"CAP\",\"4\",\"Farad Demo\",\"FD-0402-104\"\r\n\
\"MCU\",\"Microcontroller, QFN-32\",\"U1\",\"QFN32\",\"MCU\",\"1\",\"Silicon Demo\",\"SD-MCU-32\"\r\n";

const KICAD: &str = "\"Id\";\"Designator\";\"Footprint\";\"Quantity\";\"Designation\";\"Supplier and ref\";\"MPN\"\n\
1;\"R1,R2\";\"Resistor_SMD:R_0603\";2;\"4k7\";;\"OD-0603-4K7\"\n\
2;\"C1\";\"Capacitor_SMD:C_0402\";1;\"1u\";;\"FD-0402-105\"\n\
3;\"J1\";\"Connector:USB_C\";1;\"USB-C\";;\"CN-USBC-16\"\n";

const INDENTED: &str = "Level,Part Number,Description,Qty,Find\n\
0,ASM-1000,Gearbox,,\n\
1,ASM-1100,Input stage,1,10\n\
2,PRT-1110,Input shaft,1,10\n\
2,PRT-1120,Bearing 6202,2,20\n\
1,PRT-1200,Housing,1,20\n\
1,PRT-1120,Bearing 6202,2,30\n";

/// The Latin-1 spreadsheet's text; written as Latin-1 bytes.
const LATIN1: &str = "Teilenummer;Benennung;Größe;Werkstoff\n\
DE-0001;Schraube;M3 × 8;Stahl verzinkt\n\
DE-0002;Unterlegscheibe;Ø 3,2;Edelstahl\n\
DE-0003;Gehäuse;150 × 80;Aluminium eloxiert\n";

fn latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).expect("Latin-1"))
        .collect()
}

/// A PLM-shaped JSON item export: an AML as an array of sources, numbers
/// as written (`0.120`, `1.50E-2`), an escaped `×`, a null description.
const PLM_ITEMS: &str = r#"{
  "export": {"system": "Demo PLM", "generated": "2026-11-18"},
  "items": [
    {"id": "IT-1", "number": "MECH-00101", "name": "Bracket", "description": "Bracket, bent, 2 mm",
     "weight": 0.120, "makeBuy": "make", "sources": []},
    {"id": "IT-2", "number": "100-0101", "name": "Screw", "description": "Screw M3 × 6",
     "weight": 0.0011, "tolerance": 1.50E-2, "makeBuy": "buy",
     "sources": [{"manufacturer": "Fastener Demo", "mpn": "FD-M3-6", "status": "active"},
                 {"manufacturer": "Bolt Demo", "mpn": "BD-36", "status": "nrnd"}]},
    {"id": "IT-3", "number": "AB100101", "name": "Label", "description": null,
     "weight": 0.0005, "makeBuy": "buy", "obsolete": false,
     "sources": [{"manufacturer": "Label Demo", "mpn": "LD-1", "status": "active"}]}
  ]
}
"#;

/// A JSON Lines dump with CRLF line ends and a blank line.
const DUMP: &str = "{\"sku\": \"DE-0101\", \"title\": \"Mutter M4\", \"qty_on_hand\": 120}\r\n{\"sku\": \"DE-0102\", \"title\": \"Scheibe \u{d8} 4,3\", \"qty_on_hand\": 0}\r\n\r\n{\"sku\": \"DE-0103\", \"title\": \"Geh\u{e4}usedeckel\", \"qty_on_hand\": 7}\r\n";

/// A nested XML BOM in a namespace, written as windows-1252.
const NESTED: &str = r#"<?xml version="1.0" encoding="windows-1252"?>
<bom:Export xmlns:bom="urn:example:demo-bom" generated="2026-11-18">
  <bom:Item number="ASM-2000">
    <bom:Description>Pumpe, komplett</bom:Description>
    <bom:Components>
      <bom:Component number="ASM-2100" qty="1" find="10">
        <bom:Description>Laufrad – Baugruppe</bom:Description>
        <bom:Components>
          <bom:Component number="PRT-2110" qty="1" find="10"><bom:Description>Laufrad</bom:Description></bom:Component>
          <bom:Component number="PRT-2120" qty="6" find="20"><bom:Description>Schraube M4 × 10</bom:Description></bom:Component>
        </bom:Components>
      </bom:Component>
      <bom:Component number="PRT-2200" qty="1" find="20"><bom:Description>Gehäuse</bom:Description></bom:Component>
    </bom:Components>
  </bom:Item>
</bom:Export>
"#;

fn cp1252(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '–' => 0x96,
            c => u8::try_from(u32::from(c)).expect("in windows-1252"),
        })
        .collect()
}

/// The item master: legacy numbers in three schemes (`MECH-00012`,
/// `100-0042`, `AB123456`), and its structure in the same sheet, as an
/// ERP's flat item-and-structure export has it: a `Parent` column naming
/// the parent's item ID, blank on top rows.
fn master() -> Vec<u8> {
    let items: Vec<Vec<Cell>> = vec![
        vec![
            Cell::Str("Item ID"),
            Cell::Str("Part Number"),
            Cell::Str("Description"),
            Cell::Str("Make/Buy"),
            Cell::Str("Weight (kg)"),
            Cell::Str("Parent"),
            Cell::Str("Qty"),
            Cell::Str("Legacy Note"),
        ],
        vec![
            Cell::Num("1004"),
            Cell::Str("MECH-00013"),
            Cell::Str("Kit, bracket"),
            Cell::Str("M"),
            Cell::Num("0.3"),
            Cell::Skip,
            Cell::Skip,
            Cell::Skip,
        ],
        vec![
            Cell::Num("1001"),
            Cell::Str("MECH-00012"),
            Cell::Str("Bracket, 6061"),
            Cell::Str("M"),
            Cell::Num("0.25"),
            Cell::Num("1004"),
            Cell::Num("1"),
            Cell::Str("from the 2009 system"),
        ],
        vec![
            Cell::Num("1002"),
            Cell::Str("100-0042"),
            Cell::Inline("Screw M3 × 8"),
            Cell::Str("B"),
            Cell::Num("0.002"),
            Cell::Num("1004"),
            Cell::Num("4"),
            Cell::Skip,
        ],
        vec![
            Cell::Num("1003"),
            Cell::Str("AB123456"),
            Cell::Str("Label, serial"),
            Cell::Str("B"),
            Cell::Skip,
            Cell::Num("1004"),
            Cell::Num("1"),
            Cell::Str("see drawing"),
        ],
    ];
    let notes: Vec<Vec<Cell>> = vec![vec![Cell::Str(
        "Exported for ryuplm's tests; made-up parts.",
    )]];
    xlsx(&[("Notes", notes), ("Items", items)])
}

fn mappings() -> Vec<(&'static str, Value)> {
    vec![
        (
            "altium",
            json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv", "file": "altium*.csv", "delimiter": ","},
            "key": "Manufacturer Part Number 1",
            "fields": {"name": {"from": "Comment", "trim": true}, "description": {"from": "Description", "trim": true},
                "aml.0.manufacturer": {"from": "Manufacturer 1"}, "aml.0.mpn": {"from": "Manufacturer Part Number 1"}},
            "class": "electronic", "numbers": "assign", "scheme": "elec",
            "bom": {"root": "PCBA-100", "quantity": "Quantity", "refdes": "Designator"}}),
        ),
        (
            "kicad",
            json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv", "file": "kicad*.csv", "delimiter": ";"},
            "key": "MPN",
            "fields": {"name": {"from": "Designation"}, "description": {"from": "Footprint"}},
            "class": "electronic", "numbers": "assign", "scheme": "elec",
            "bom": {"root": "PCBA-200", "quantity": "Quantity", "refdes": "Designator"}}),
        ),
        (
            "indented",
            json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv", "file": "indented*.csv"},
            "key": "Part Number",
            "fields": {"number": {"from": "Part Number"}, "description": {"from": "Description"}},
            "class": "machined",
            "bom": {"level": "Level", "quantity": "Qty", "find": "Find"}}),
        ),
        (
            "master",
            json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "xlsx", "file": "item-master*.xlsx", "sheet": "Items"},
            "key": "Item ID",
            "fields": {"number": {"from": "Part Number", "trim": true}, "description": {"from": "Description", "trim": true},
                "make_buy": {"from": "Make/Buy", "values": {"M": "make", "B": "buy"}},
                "properties.weight": {"from": "Weight (kg)"}},
            "class": "machined", "numbers": "keep",
            "bom": {"parent": "Parent", "quantity": "Qty"}}),
        ),
        (
            "latin1",
            json!({"format": 1, "kind": "ryuplm.import",
            "reader": {"type": "csv", "file": "latin1*.csv", "encoding": "latin1", "delimiter": ";"},
            "key": "Teilenummer",
            "fields": {"number": {"from": "Teilenummer"}, "name": {"from": "Benennung"}, "properties.size": {"from": "Größe"}},
            "class": "machined", "numbers": "keep"}),
        ),
        (
            "plm-json",
            json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "json", "file": "plm-items*.json", "records": "/items"},
            "key": "/id",
            "fields": {"number": {"from": "/number"}, "name": {"from": "/name"}, "description": {"from": "/description"},
                "properties.weight": {"from": "/weight"}, "make_buy": {"from": "/makeBuy"},
                "aml.0.manufacturer": {"from": "/sources/0/manufacturer"}, "aml.0.mpn": {"from": "/sources/0/mpn"},
                "aml.0.status": {"from": "/sources/0/status"},
                "aml.1.manufacturer": {"from": "/sources/1/manufacturer"}, "aml.1.mpn": {"from": "/sources/1/mpn"},
                "aml.1.status": {"from": "/sources/1/status"}},
            "class": "machined", "numbers": "keep"}),
        ),
        (
            "jsonl",
            json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "json", "file": "dump*.jsonl", "lines": true},
            "key": "/sku",
            "fields": {"number": {"from": "/sku"}, "name": {"from": "/title"}},
            "class": "machined", "numbers": "keep"}),
        ),
        (
            "nested-xml",
            json!({"format": 2, "kind": "ryuplm.import",
            "reader": {"type": "xml", "file": "nested-bom*.xml", "records": "/Export/Item", "children": "Components/Component"},
            "key": "@number",
            "fields": {"number": {"from": "@number"}, "description": {"from": "Description"}},
            "class": "machined", "numbers": "keep",
            "bom": {"level": "#level", "quantity": "@qty", "find": "@find"}}),
        ),
    ]
}

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut f = vec![
        ("altium-bom.csv".to_owned(), ALTIUM.as_bytes().to_vec()),
        ("kicad-bom.csv".to_owned(), KICAD.as_bytes().to_vec()),
        ("indented-bom.csv".to_owned(), INDENTED.as_bytes().to_vec()),
        ("item-master.xlsx".to_owned(), master()),
        ("latin1-items.csv".to_owned(), latin1(LATIN1)),
        ("plm-items.json".to_owned(), PLM_ITEMS.as_bytes().to_vec()),
        ("dump.jsonl".to_owned(), DUMP.as_bytes().to_vec()),
        ("nested-bom.xml".to_owned(), cp1252(NESTED)),
    ];
    for (n, m) in mappings() {
        f.push((
            format!("{n}.json"),
            ryuplm::canonical::to_string(&m).into_bytes(),
        ));
    }
    f
}

#[test]
fn the_fixtures_are_as_this_file_writes_them() {
    let update = std::env::var("UPDATE_FIXTURES").is_ok_and(|v| v == "1");
    for (name, bytes) in fixtures() {
        let at = dir().join(&name);
        if update {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&at, &bytes).unwrap();
        } else {
            assert_eq!(
                std::fs::read(&at).unwrap_or_default(),
                bytes,
                "{name}: run with UPDATE_FIXTURES=1"
            );
        }
    }
}

/// A catalogue with the classes, properties and schemes the mappings
/// name, the two PCBAs the flat BOMs belong to, and the mappings.
fn catalogue() -> (tempfile::TempDir, PathBuf) {
    // SAFETY: every test here sets the same seed.
    unsafe { std::env::set_var("RYUPLM_ID_SEED", "fixtures") };
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    ryuplm::server::init(&root, None).unwrap();
    let ask = |r: Request| {
        ryuplm::server::handle_safely(&mut None, None, None, &root, r).map(|a| a.result)
    };
    let edit = |c: Value| {
        ask(Request::SettingsEdit {
            change: serde_json::from_value(c).unwrap(),
            expected_revision: None,
        })
        .unwrap()
    };
    for c in ["electronic", "machined"] {
        edit(json!({"class": c}));
    }
    edit(json!({"scheme": "elec", "pattern": "E-\\d{5}", "next_template": "E-{seq:5}"}));
    edit(json!({"scheme": "mech", "pattern": "MECH-\\d{5}"}));
    edit(json!({"scheme": "legacy", "pattern": "\\d{3}-\\d{4}"}));
    edit(json!({"scheme": "label", "pattern": "[A-Z]{2}\\d{6}"}));
    edit(json!({"scheme": "pcba", "pattern": "PCBA-\\d{3}"}));
    edit(json!({"scheme": "assembly", "pattern": "(ASM|PRT)-\\d{4}"}));
    edit(json!({"scheme": "de", "pattern": "DE-\\d{4}"}));
    edit(json!({"property": "weight", "declaration": {"type": "number"}}));
    edit(json!({"property": "size", "declaration": {"type": "string"}}));
    for n in ["PCBA-100", "PCBA-200"] {
        ask(Request::ItemCreate {
            fields: Default::default(),
            number: Some(n.into()),
            next: None,
            expected_revision: None,
        })
        .unwrap();
    }
    std::fs::create_dir_all(root.join("imports")).unwrap();
    for (n, m) in mappings() {
        std::fs::write(
            root.join(format!("imports/{n}.json")),
            ryuplm::canonical::to_string(&m),
        )
        .unwrap();
    }
    (d, root)
}

fn ask(root: &Path, r: Request) -> ryuplm::error::Result<Value> {
    ryuplm::server::handle_safely(&mut None, None, None, root, r).map(|a| a.result)
}

fn run(root: &Path, name: &str, file: &str) -> Value {
    let file = dir().join(file).display().to_string();
    ask(
        root,
        Request::ImportCheck {
            name: name.into(),
            file: file.clone(),
            limit: None,
        },
    )
    .unwrap();
    let first = ask(
        root,
        Request::ImportRun {
            name: name.into(),
            file: file.clone(),
            dry_run: false,
            prefer: None,
            limit: None,
            expected_revision: None,
        },
    )
    .unwrap();
    let again = ask(
        root,
        Request::ImportRun {
            name: name.into(),
            file,
            dry_run: false,
            prefer: None,
            limit: None,
            expected_revision: None,
        },
    )
    .unwrap();
    assert_eq!(
        again["written"], 0,
        "{name}: an unchanged re-import writes nothing: {again}"
    );
    assert_eq!(again["conflicts"], json!([]));
    first
}

fn bom(root: &Path, item: &str) -> Value {
    ask(
        root,
        Request::BomGet {
            item: item.into(),
            rev: None,
            depth: None,
            within: vec![],
        },
    )
    .unwrap()
}

#[test]
fn each_fixture_imports_through_its_mapping_and_again_writes_nothing() {
    let (_d, root) = catalogue();
    let a = run(&root, "altium", "altium-bom.csv");
    assert_eq!(
        (
            a["created"]["count"].clone(),
            a["lines"]["added"]["count"].clone()
        ),
        (json!(3), json!(3))
    );
    let b = bom(&root, "PCBA-100");
    assert_eq!(b["lines"].as_array().unwrap().len(), 3, "{b}");
    let k = run(&root, "kicad", "kicad-bom.csv");
    assert_eq!(k["created"]["count"], 3);
    let i = run(&root, "indented", "indented-bom.csv");
    assert_eq!(i["created"]["count"], 5, "PRT-1120 twice is one item");
    assert_eq!(i["lines"]["added"]["count"], 5);
    let m = run(&root, "master", "item-master.xlsx");
    assert_eq!(
        (
            m["created"]["count"].clone(),
            m["lines"]["added"]["count"].clone()
        ),
        (json!(4), json!(3))
    );
    let kit = bom(&root, "MECH-00013");
    assert_eq!(kit["lines"].as_array().unwrap().len(), 3, "{kit}");
    let l = run(&root, "latin1", "latin1-items.csv");
    assert_eq!(l["created"]["count"], 3);
    let shown = ask(
        &root,
        Request::ItemShow {
            item: "DE-0001".into(),
        },
    )
    .unwrap();
    assert_eq!(
        shown["item"]["properties"]["size"], "M3 × 8",
        "Latin-1 read as Latin-1"
    );
    let w = ask(
        &root,
        Request::ItemShow {
            item: "MECH-00012".into(),
        },
    )
    .unwrap();
    // ryupdm has no mass dimension: weight is a number, in kg as its column says.
    assert_eq!(w["item"]["properties"]["weight"], json!(0.25));
    assert_eq!(
        w["item"]["extra"]["master"],
        json!({"Legacy Note": "from the 2009 system"})
    );
    // Every legacy scheme is declared: check is clean of unschemed numbers.
    let c = ask(&root, Request::Check { within: vec![] }).unwrap();
    let codes: Vec<&str> = c["faults"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(!codes.contains(&"part_number_unschemed"), "{c}");
}

#[test]
fn each_tree_fixture_imports_through_its_mapping_and_again_writes_nothing() {
    let (_d, root) = catalogue();
    let show =
        |item: &str| ask(&root, Request::ItemShow { item: item.into() }).unwrap()["item"].clone();
    let p = run(&root, "plm-json", "plm-items.json");
    assert_eq!(p["created"]["count"], 3, "{p}");
    let screw = show("100-0101");
    assert_eq!(screw["description"], "Screw M3 × 6");
    assert_eq!(screw["aml"].as_array().unwrap().len(), 2, "{screw}");
    assert_eq!(screw["aml"][1]["mpn"], "BD-36");
    assert_eq!(
        screw["extra"]["plm-json"],
        json!({"/tolerance": "1.50E-2"}),
        "a number's digits as written"
    );
    assert_eq!(show("MECH-00101")["properties"]["weight"], json!(0.12));
    assert_eq!(
        show("AB100101")["extra"]["plm-json"],
        json!({"/obsolete": "false"})
    );
    let j = run(&root, "jsonl", "dump.jsonl");
    assert_eq!(j["created"]["count"], 3, "{j}");
    assert_eq!(show("DE-0103")["name"], "Gehäusedeckel");
    let n = run(&root, "nested-xml", "nested-bom.xml");
    assert_eq!(
        (
            n["created"]["count"].clone(),
            n["lines"]["added"]["count"].clone()
        ),
        (json!(5), json!(4)),
        "{n}"
    );
    assert_eq!(bom(&root, "ASM-2000")["lines"].as_array().unwrap().len(), 2);
    assert_eq!(bom(&root, "ASM-2100")["lines"].as_array().unwrap().len(), 2);
    assert_eq!(
        show("ASM-2100")["description"],
        "Laufrad – Baugruppe",
        "windows-1252"
    );
    assert_eq!(show("PRT-2120")["description"], "Schraube M4 × 10");
}
