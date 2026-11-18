//! Seeded catalogues for tests, the golden and the fuzz target.

use crate::catalogue::{Class, MANIFEST, Manifest};
use crate::id::ProjectId;
use crate::item::Item;
use crate::units::Unit;
use serde_json::json;
use std::collections::BTreeMap;

/// A catalogue of `n` items from `seed`: three classes, numbers in two
/// schemes and a legacy form (some duplicated, some unschemed), AMLs,
/// declared properties, own lines naming other items.
pub fn catalogue(seed: &str, n: usize) -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    let mut m = Manifest::new(ProjectId::from_bytes([7; 16]), Unit::Mm);
    m.classes.insert(
        "fastener".into(),
        Class {
            properties: Some(vec!["thread".into()]),
        },
    );
    m.classes
        .insert("machined".into(), Class { properties: None });
    m.classes.insert("label".into(), Class::default());
    f.insert(MANIFEST.to_owned(), m.to_text().into_bytes());
    let config = json!({"format": 5, "kind": "ryupdm.config",
        "part_number": {"schemes": [
            {"name": "mech", "class": "machined", "pattern": "MECH-\\d{5}", "next": "MECH-{seq:5}"},
            {"name": "fast", "class": "fastener", "pattern": "F\\d{4}", "next": "F{seq:4}"},
            {"name": "legacy", "pattern": "\\d{3}-\\d{4}"}]},
        "properties": {"thread": {"type": "string"}, "finish": {"type": "string"}}});
    f.insert(
        "ryupdm.json".to_owned(),
        crate::canonical::to_string(&config).into_bytes(),
    );
    let mut ids = Vec::new();
    for k in 0..n {
        let id = crate::ids::random_from(Some(seed), |s| {
            ids.iter().any(|i: &crate::id::Id| i.as_str() == s)
        })
        .unwrap();
        ids.push(id.clone());
        let class = ["fastener", "machined", "label"][k % 3];
        let number = match k % 7 {
            0 => format!("MECH-{:05}", k),
            1 => format!("F{:04}", k),
            2 => format!("{:03}-{:04}", k % 1000, k),
            3 => format!("Legacy {k}/b"),
            // Thirteen numbers shared out: duplicates.
            _ => format!("{:03}-{:04}", k % 13, k % 13),
        };
        let mut item = Item::new(&number);
        item.class = Some(class.into());
        item.name = Some(format!("Item {k}"));
        if class == "fastener" {
            item.properties
                .insert("thread".into(), json!(format!("M{}", 2 + k % 4)));
        }
        if k % 5 == 0 {
            item.aml.push(json!({"manufacturer": format!("Maker {}", k % 4), "mpn": format!("P{k}"), "status": "active"}));
        }
        f.insert(crate::ids::item_path(&id), item.to_text().into_bytes());
    }
    f
}

/// A catalogue with structure from `seed`: [`catalogue`]'s items, with
/// own lines from item k to the next `k % 3` items (never back, so no
/// loop); every tenth item linked to ryucad assembly `a<k>`, whose lines
/// place screw `screw<k>` (no item links it: `bom_unlinked`) and
/// assembly `a<k+10>` (item k+10's), every thirtieth also naming item
/// k+10 by an own line (`bom_line_twice`); every fifteenth linked to
/// ryueda design `d<k>`, whose one line places component `c<k>`, which
/// item k+5 links. The representations' lines are held in memory.
pub fn structure(seed: &str, n: usize) -> (BTreeMap<String, Vec<u8>>, crate::reps::Fixed) {
    use crate::id::Id;
    use crate::item::{ItemRef, Line, Link};
    use crate::reps::RepLine;
    let mut f = catalogue(seed, n);
    let cat = crate::load::Catalogue::load(&f).expect("the seeded catalogue reads");
    let ids: Vec<Id> = cat.items.keys().cloned().collect();
    let mut items: Vec<Item> = ids.iter().map(|id| cat.items[id].clone()).collect();
    let (cad, eda) = (
        ProjectId::from_bytes([3; 16]),
        ProjectId::from_bytes([4; 16]),
    );
    let part = |s: String| Id::parse(&s, "part").expect("a part ID");
    let mut reps = crate::reps::Fixed::default();
    let mut made = 0u64;
    for k in 0..ids.len() {
        let mut add: Vec<(usize, String)> = (1..=(k % 3))
            .filter(|d| k + d < ids.len())
            .map(|d| (k + d, format!("{}", 1 + k % 4)))
            .collect();
        if k % 30 == 0 && k + 10 < ids.len() {
            add.push((k + 10, "2".to_owned()));
        }
        for (child, quantity) in add {
            made += 1;
            let find = format!("{}", 10 * (items[k].bom.len() + 1));
            items[k].bom.push(Line {
                line: Id::parse(&crate::ids::text(made), "line").expect("a line ID"),
                item: ItemRef {
                    catalogue: None,
                    item: ids[child].clone(),
                },
                quantity,
                unit: None,
                find: Some(find),
                refdes: None,
                notes: None,
            });
        }
        if k % 10 == 0 {
            items[k].links.push(Link {
                product: "ryucad".into(),
                project: cad.clone(),
                part: part(format!("a{k}")),
            });
            let mut lines = vec![RepLine {
                key: format!("screw{k}"),
                project: Some(cad.clone()),
                part: part(format!("screw{k}")),
                quantity: 4,
                find: Some("1".into()),
                refdes: None,
            }];
            if k + 10 < ids.len() {
                lines.push(RepLine {
                    key: format!("a{}", k + 10),
                    project: Some(cad.clone()),
                    part: part(format!("a{}", k + 10)),
                    quantity: 2,
                    find: Some("2".into()),
                    refdes: None,
                });
            }
            reps.0
                .insert(("ryucad".into(), cad.clone(), part(format!("a{k}"))), lines);
        }
        if k % 15 == 0 {
            items[k].links.push(Link {
                product: "ryueda".into(),
                project: eda.clone(),
                part: part(format!("d{k}")),
            });
            reps.0.insert(
                ("ryueda".into(), eda.clone(), part(format!("d{k}"))),
                vec![RepLine {
                    key: format!("self/c{k}\u{0}10k\u{0}R0603"),
                    project: Some(eda.clone()),
                    part: part(format!("c{k}")),
                    quantity: 3,
                    find: None,
                    refdes: Some("R1,R2,R3".into()),
                }],
            );
            if k + 5 < ids.len() {
                items[k + 5].links.push(Link {
                    product: "ryueda".into(),
                    project: eda.clone(),
                    part: part(format!("c{k}")),
                });
                reps.0.insert(
                    ("ryueda".into(), eda.clone(), part(format!("c{k}"))),
                    Vec::new(),
                );
            }
        }
    }
    for (id, item) in ids.iter().zip(&items) {
        f.insert(crate::ids::item_path(id), item.to_text().into_bytes());
    }
    (f, reps)
}

/// A seeded import (the golden's and the fuzz target's): a catalogue's
/// files (two classes, a `mech` scheme, `thread` and `length` declared,
/// and a third of `n` items made by hand with numbers the source also has,
/// so a first import adopts them, some with other descriptions so it
/// reports conflicts), the mapping (numbers kept, class rules, a regex, a
/// value table, a unit, an AML group, a parent column), a first CSV of `n`
/// rows (every sixth past the tenth a line of an earlier row), and a second
/// in which every fifth row's description changes, every seventh line's
/// quantity changes, every eleventh row that is no parent is gone, and
/// twenty new rows appear.
pub fn import_source(
    seed: &str,
    n: usize,
) -> (
    BTreeMap<String, Vec<u8>>,
    crate::import::mapping::Mapping,
    String,
    String,
) {
    let mut f = BTreeMap::new();
    let mut m = Manifest::new(ProjectId::from_bytes([9; 16]), Unit::Mm);
    for c in ["fastener", "machined"] {
        m.classes.insert(c.into(), Class { properties: None });
    }
    f.insert(MANIFEST.to_owned(), m.to_text().into_bytes());
    let config = json!({"format": 5, "kind": "ryupdm.config",
        "part_number": {"schemes": [
            {"name": "mech", "pattern": "MECH-\\d{5}", "next": "MECH-{seq:5}"},
            {"name": "plain", "pattern": "P-\\d{5}"}]},
        "properties": {"thread": {"type": "string"}, "length": {"type": "quantity", "dim": "length"}}});
    f.insert(
        "ryupdm.json".to_owned(),
        crate::canonical::to_string(&config).into_bytes(),
    );
    let mut ids = std::collections::BTreeSet::new();
    for k in (0..n).filter(|k| k % 3 == 1) {
        let id = crate::ids::random_from(Some(seed), |s| ids.contains(s)).unwrap();
        ids.insert(id.as_str().to_owned());
        let mut item = Item::new(&format!("P-{k:05}"));
        item.description = Some(if k % 2 == 0 {
            format!("Hand {k}")
        } else {
            desc(k)
        });
        f.insert(crate::ids::item_path(&id), item.to_text().into_bytes());
    }
    let mapping = crate::import::mapping::Mapping::from_json(
        "seeded",
        &json!({"format": 1, "kind": "ryuplm.import", "reader": {"type": "csv"}, "key": "ID",
            "fields": {
                "number": {"from": "PN", "trim": true},
                "description": {"from": "Desc", "trim": true},
                "properties.thread": {"from": "Desc", "regex": "\\b(M\\d+)\\b"},
                "properties.length": {"from": "Len", "unit": "mm"},
                "make_buy": {"from": "Src", "values": {"M": "make", "P": "buy"}},
                "aml.0.manufacturer": {"from": "Mfr"},
                "aml.0.mpn": {"from": "MPN"}},
            "class": [{"when": {"Kind": "S"}, "class": "fastener"}, {"class": "machined"}],
            "bom": {"parent": "Parent", "quantity": "Qty"}}),
    )
    .expect("the seeded mapping reads");
    fn desc(k: usize) -> String {
        if k % 3 == 0 {
            format!("Screw M{} x {}", 3 + k % 5, 4 + k % 9)
        } else {
            format!("Part {k}")
        }
    }
    let row = |k: usize, second: bool| -> String {
        let mut d = desc(k);
        if second && k % 5 == 0 {
            d += " rev B";
        }
        let parent = (k > 10 && k % 6 == 0).then(|| k / 10 + 1);
        let mut qty = 1 + k % 3;
        if second && k % 7 == 0 {
            qty += 1;
        }
        let (mfr, mpn) = if k % 4 == 1 {
            (format!("Maker {}", k % 6), format!("MP-{k}"))
        } else {
            (String::new(), String::new())
        };
        format!(
            "{},P-{k:05},{d},{},{},{mfr},{mpn},{},{},{},n{}\n",
            k + 1,
            if k % 2 == 0 { "P" } else { "M" },
            if k % 4 == 0 {
                String::new()
            } else {
                (k % 50).to_string()
            },
            if k % 3 == 0 { "S" } else { "" },
            parent.map(|p| p.to_string()).unwrap_or_default(),
            if parent.is_some() {
                qty.to_string()
            } else {
                String::new()
            },
            k % 7,
        )
    };
    let header = "ID,PN,Desc,Src,Len,Mfr,MPN,Kind,Parent,Qty,Note\n";
    let mut first = header.to_owned();
    let mut second = header.to_owned();
    for k in 0..n {
        first += &row(k, false);
        // Rows up to n / 10 are parents: they stay.
        if !(k % 11 == 0 && k > n / 10 + 1) {
            second += &row(k, true);
        }
    }
    for k in n..n + 20 {
        second += &row(k, true);
    }
    (f, mapping, first, second)
}

/// A cell for [`xlsx`]: a shared string, an inline string, a number as
/// written, a boolean, or no cell at all (the next one keeps its column).
#[derive(Clone, Copy, Debug)]
pub enum Cell<'a> {
    Str(&'a str),
    Inline(&'a str),
    Num(&'a str),
    Bool(bool),
    Skip,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn column(mut c: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).expect("ASCII")
}

/// An XLSX workbook of `sheets`, as a spreadsheet program writes one:
/// shared strings with one rich-text run in every other, `r` references,
/// an empty row left out. Written by us, for tests and the fixtures.
pub fn xlsx(sheets: &[(&str, Vec<Vec<Cell>>)]) -> Vec<u8> {
    const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let mut shared: Vec<String> = Vec::new();
    let mut parts: Vec<(String, String)> = Vec::new();
    for (k, (_, rows)) in sheets.iter().enumerate() {
        let mut x = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"{NS}\"><sheetData>"
        );
        for (i, row) in rows.iter().enumerate() {
            if row.is_empty() {
                continue;
            }
            x += &format!("<row r=\"{}\">", i + 1);
            for (j, c) in row.iter().enumerate() {
                let r = format!("{}{}", column(j), i + 1);
                match c {
                    Cell::Skip => {}
                    Cell::Str(s) => {
                        let at = shared.iter().position(|x| x == s).unwrap_or_else(|| {
                            shared.push((*s).to_owned());
                            shared.len() - 1
                        });
                        x += &format!("<c r=\"{r}\" t=\"s\"><v>{at}</v></c>");
                    }
                    Cell::Inline(s) => {
                        x += &format!(
                            "<c r=\"{r}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                            escape(s)
                        )
                    }
                    Cell::Num(n) => x += &format!("<c r=\"{r}\"><v>{n}</v></c>"),
                    Cell::Bool(b) => {
                        x += &format!("<c r=\"{r}\" t=\"b\"><v>{}</v></c>", u8::from(*b))
                    }
                }
            }
            x += "</row>";
        }
        x += "</sheetData></worksheet>";
        parts.push((format!("xl/worksheets/sheet{}.xml", k + 1), x));
    }
    let mut sst = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<sst xmlns=\"{NS}\" count=\"{0}\" uniqueCount=\"{0}\">",
        shared.len()
    );
    for (i, s) in shared.iter().enumerate() {
        sst += &if i % 2 == 1 {
            format!(
                "<si><r><t xml:space=\"preserve\">{}</t></r></si>",
                escape(s)
            )
        } else {
            format!("<si><t>{}</t></si>", escape(s))
        };
    }
    sst += "</sst>";
    let mut wb = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"{NS}\" xmlns:r=\"{R}\"><sheets>"
    );
    let mut rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for (k, (name, _)) in sheets.iter().enumerate() {
        wb += &format!(
            "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
            escape(name),
            k + 1,
            k + 1
        );
        rels += &format!(
            "<Relationship Id=\"rId{}\" Type=\"{R}/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
            k + 1,
            k + 1
        );
    }
    wb += "</sheets></workbook>";
    rels += &format!(
        "<Relationship Id=\"rId{}\" Type=\"{R}/sharedStrings\" Target=\"sharedStrings.xml\"/></Relationships>",
        sheets.len() + 1
    );
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/></Types>".to_owned();
    let root_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{R}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"
    );
    let mut all: Vec<(String, String)> = vec![
        ("[Content_Types].xml".into(), types),
        ("_rels/.rels".into(), root_rels),
        ("xl/workbook.xml".into(), wb),
        ("xl/_rels/workbook.xml.rels".into(), rels),
        ("xl/sharedStrings.xml".into(), sst),
    ];
    all.extend(parts);
    let entries: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_bytes()))
        .collect();
    ryupdm::package::zip::write(&entries).expect("a small workbook fits a zip")
}
