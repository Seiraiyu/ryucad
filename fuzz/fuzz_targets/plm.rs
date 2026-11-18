//! ryuplm from bytes: the first byte picks what the rest is read as. An
//! item, manifest or settings file that reads must write canonically and
//! read back the same; a merge of three readable items must give a file
//! that reads (or text with markers); textconv never panics; a catalogue
//! of what reads is judged without panicking; ryueda's bom answer read
//! and a joined BOM judged.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else { return };
    let id = ryuplm::id::Id::parse("k3v9q2m8x4c1p", "item").unwrap();
    match mode % 6 {
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
        _ => {
            // A small catalogue: the seeded one, with this item added.
            let mut f: BTreeMap<String, Vec<u8>> = ryuplm::testing::catalogue("fuzz", 12);
            f.insert(ryuplm::ids::item_path(&id), rest.to_vec());
            if let Ok(c) = ryuplm::load::Catalogue::load(&f) {
                let _ = ryuplm::judge::all(&f, &c, &Default::default());
            }
        }
    }
});
