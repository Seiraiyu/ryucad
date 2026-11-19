//! ryuplm from bytes: the first byte picks what the rest is read as. An
//! item, manifest or settings file that reads must write canonically and
//! read back the same; a merge of three readable items must give a file
//! that reads (or text with markers); textconv never panics; a catalogue
//! of what reads is judged without panicking; ryueda's bom answer read
//! and a joined BOM judged; an import mapping that reads writes
//! canonically and reads back; the CSV and XLSX readers on any bytes never
//! panic, nor does a small source planned through the seeded import, nor
//! the JSON and XML readers and their surveys, nor a seeded history
//! planned from any bytes, nor a date read by any pattern; a reference
//! that parses prints back the same and stands or doesn't without
//! panicking.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else { return };
    let id = ryuplm::id::Id::parse("k3v9q2m8x4c1p", "item").unwrap();
    match mode % 14 {
        0 => {
            if let Ok(item) = ryuplm::item::Item::read(&id, rest) {
                let text = item.to_text();
                assert!(ryuplm::canonical::is_canonical(&text));
                assert_eq!(ryuplm::item::Item::read(&id, text.as_bytes()).unwrap(), item);
                let _ = ryuplm::aml::judge(&id, &item.aml);
                let _ = ryuplm::pdm::content_of(&item);
            }
        }
        1 => {
            if let Ok(m) = ryuplm::catalogue::Manifest::read(rest) {
                assert_eq!(ryuplm::catalogue::Manifest::read(m.to_text().as_bytes()).unwrap(), m);
            }
        }
        2 => {
            // Three items split by NUL bytes: base, ours, theirs.
            let parts: Vec<&[u8]> = rest.splitn(3, |b| *b == 0).collect();
            if let [b, o, t] = parts.as_slice() {
                let (out, outcome) = ryuplm::merge::bytes(b, o, t);
                if outcome == ryuplm::merge::Outcome::Clean
                    && ryuplm::item::Item::read(&id, o).is_ok()
                    && ryuplm::item::Item::read(&id, t).is_ok()
                    && (b.is_empty() || ryuplm::item::Item::read(&id, b).is_ok())
                {
                    ryuplm::item::Item::read(&id, &out).expect("a clean merge of items is an item");
                }
            }
        }
        3 => {
            let _ = ryuplm::textconv::textconv(rest);
        }
        4 => {
            // ryueda's bom answer, and the structured catalogue with this
            // item added: judged and joined without panicking.
            let _ = ryuplm::reps::eda_answer(rest, &ryuplm::id::ProjectId::from_bytes([4; 16]));
            let (mut f, mut reps) = ryuplm::testing::structure("fuzz", 31);
            f.insert(ryuplm::ids::item_path(&id), rest.to_vec());
            if let Ok(c) = ryuplm::load::Catalogue::load(&f) {
                let _ = ryuplm::judge::all(&f, &c, &Default::default());
                let _ = ryuplm::bom::joined(&c, &mut reps, &ryuplm::reps::At::Working);
            }
        }
        5 => {
            // A small catalogue: the seeded one, with this item added.
            let mut f: BTreeMap<String, Vec<u8>> = ryuplm::testing::catalogue("fuzz", 12);
            f.insert(ryuplm::ids::item_path(&id), rest.to_vec());
            if let Ok(c) = ryuplm::load::Catalogue::load(&f) {
                let _ = ryuplm::judge::all(&f, &c, &Default::default());
            }
        }
        6 => {
            // A mapping file: if it reads, it writes canonically and reads back.
            if let Ok(m) = ryuplm::import::mapping::Mapping::read("fuzz", rest) {
                let text = m.to_text();
                assert!(ryuplm::canonical::is_canonical(&text));
                assert_eq!(ryuplm::import::mapping::Mapping::read("fuzz", text.as_bytes()).unwrap(), m);
            }
        }
        7 => {
            // The CSV reader on any bytes, in a sniffed encoding and delimiter,
            // and its table: never a panic.
            let enc = ryuplm::import::text::sniff(rest);
            if let Ok(s) = ryuplm::import::text::decode(rest, enc) {
                let d = ryuplm::import::csv::sniff(&s);
                if let Ok(raw) = ryuplm::import::csv::parse(&s, d) {
                    let _ = ryuplm::import::rows::Table::from_raw(raw, 1);
                }
            }
        }
        8 => {
            // The XLSX reader on any bytes.
            let _ = ryuplm::import::xlsx::read(rest, None);
            let _ = ryuplm::import::xlsx::sheets(rest);
        }
        9 => {
            // A small source through the golden's mapping, planned against
            // its catalogue: never a panic.
            let (files, m, _, _) = ryuplm::testing::import_source("fuzz", 12);
            if let Ok(s) = std::str::from_utf8(rest)
                && let Ok(raw) = ryuplm::import::csv::parse(s, ',')
                && let Ok(t) = ryuplm::import::rows::Table::from_raw(raw, 1)
                && let Ok(cat) = ryuplm::load::Catalogue::load(&files)
                && let Ok(config) = ryupdm::config::Config::read_in(&files, "ryuplm")
                && let Ok((rows, _)) = ryuplm::import::records::map(&t, &m, &config, cat.manifest.default_unit)
            {
                let _ = ryuplm::import::run::plan(&files, &cat, &config, &m, &rows, None, 10);
            }
        }
        10 => {
            // The JSON reader on any bytes, as records, with children and as
            // JSON Lines, and its survey: never a panic.
            use ryuplm::import::json::{self, Shape};
            for shape in [
                Shape { records: "/items", children: Some("/c"), lines: false },
                Shape { records: "", children: None, lines: false },
                Shape { records: "", children: Some("/c"), lines: true },
            ] {
                if let Ok(r) = json::read(rest, shape) {
                    let _ = ryuplm::import::rows::Table::from_records(r);
                }
            }
            let _ = json::survey(rest, false, None);
            let _ = json::survey(rest, true, None);
        }
        11 => {
            // The XML reader on any bytes, with children, and its survey.
            if let Ok(r) = ryuplm::import::xml::read(rest, "/a/b", Some("c/d")) {
                let _ = ryuplm::import::rows::Table::from_records(r);
            }
            let _ = ryuplm::import::xml::survey(rest, None);
        }
        12 => {
            // A reference: what parses prints back as it was read, and is
            // judged against the seeded structure without panicking.
            if let Ok(s) = std::str::from_utf8(rest)
                && let Some(r) = ryuplm::refs::Ref::parse(s)
            {
                assert_eq!(r.text(), s);
                assert_eq!(ryuplm::refs::Ref::parse(&r.text()), Some(r.clone()));
                let (f, _) = ryuplm::testing::structure("fuzz", 31);
                if let Ok(c) = ryuplm::load::Catalogue::load(&f) {
                    let _ = ryuplm::refs::standing(&c, &r);
                }
            }
        }
        _ => {
            // A seeded history planned from any bytes, and any first line
            // read as a date pattern over the rest: never a panic.
            use ryuplm::import::transform::{DatePattern, Dates, read_date};
            let Ok(s) = std::str::from_utf8(rest) else { return };
            let (files, m, _) = ryuplm::testing::history_source("fuzz", 8);
            if let Ok(raw) = ryuplm::import::csv::parse(s, ',')
                && let Ok(t) = ryuplm::import::rows::Table::from_raw(raw, 1)
                && let Ok(cat) = ryuplm::load::Catalogue::load(&files)
                && let Ok(config) = ryupdm::config::Config::read_in(&files, "ryuplm")
                && let Ok((rows, _)) =
                    ryuplm::import::records::map(&t, &m, &config, cat.manifest.default_unit)
            {
                let _ = ryuplm::import::run::plan(&files, &cat, &config, &m, &rows, None, 10);
            }
            let (pattern, value) = s.split_once('\n').unwrap_or((s, ""));
            if let Ok(p) = DatePattern::parse(pattern) {
                for d in [Dates::S1900, Dates::S1904] {
                    let _ = read_date(value, std::slice::from_ref(&p), d);
                }
            }
        }
    }
});
