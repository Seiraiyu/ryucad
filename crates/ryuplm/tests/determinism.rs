//! What a catalogue answers, bit-exact on every platform.

mod golden;

use golden::Golden;
use ryuplm::load::Catalogue;

#[test]
fn catalogues_answer_the_same_everywhere() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut g = Golden::new(dir.join("tests/golden/ryuplm.txt"));
    let files = ryuplm::testing::catalogue("golden", 300);
    let cat = Catalogue::load(&files).unwrap();
    // The bytes of every file, so the canonical form can't drift.
    for (p, b) in &files {
        g.section("files").str(p).bytes(b);
    }
    let faults = ryuplm::judge::all(&files, &cat, &Default::default());
    g.section("check")
        .str(&serde_json::to_string(&ryuplm::fault::list(&faults)).unwrap());
    let config = ryupdm::config::Config::read_in(&files, "ryuplm").unwrap();
    for s in ["mech", "fast"] {
        g.section("next")
            .str(&ryuplm::numbers::next(&files, &cat, &config, Some(s), None).unwrap());
    }
    for (id, item) in &cat.items {
        g.section("content")
            .str(id.as_str())
            .str(&ryuplm::pdm::content_of(item));
        g.section("textconv")
            .bytes(&ryuplm::textconv::textconv(item.to_text().as_bytes()));
    }
    g.finish();
}

#[test]
fn structure_answers_the_same_everywhere() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut g = Golden::new(dir.join("tests/golden/structure.txt"));
    let (files, mut reps) = ryuplm::testing::structure("golden", 300);
    for (p, b) in &files {
        g.section("files").str(p).bytes(b);
    }
    let cat = Catalogue::load(&files).unwrap();
    let at = ryuplm::reps::At::Working;
    let faults = ryuplm::judge::all(&files, &cat, &Default::default());
    g.section("judge")
        .str(&serde_json::to_string(&ryuplm::fault::list(&faults)).unwrap());
    let joined = ryuplm::bom::joined(&cat, &mut reps, &at);
    g.section("joined")
        .str(&serde_json::to_string(&ryuplm::fault::list(&joined)).unwrap());
    let linked = ryuplm::links::linked(&cat);
    for id in cat.items.keys() {
        let (lines, _) = ryuplm::bom::of(&cat, &linked, id, &mut reps, &at);
        let shown: Vec<serde_json::Value> = lines
            .iter()
            .map(|j| ryuplm::bom::line_json(&cat, j))
            .collect();
        g.section("bom")
            .str(id.as_str())
            .str(&serde_json::to_string(&shown).unwrap());
    }
    // An edited copy: every seventh item's first line doubled, every
    // eleventh's last line removed.
    let mut b = cat.clone();
    for (k, item) in b.items.values_mut().enumerate() {
        if k % 7 == 0
            && let Some(l) = item.bom.first_mut()
        {
            l.quantity = format!("{}", 2 * l.quantity.parse::<u64>().unwrap_or(1));
        }
        if k % 11 == 0 {
            item.bom.pop();
        }
    }
    let d = ryuplm::bom_diff::between(&cat, &at, &b, &at, None, &mut reps);
    g.section("bom_diff")
        .str(&serde_json::to_string(&d).unwrap());
    for id in cat.items.keys().take(40) {
        let t = ryuplm::where_used::target(&cat, id);
        let mut problems = Vec::new();
        let rows = ryuplm::where_used::rows_in(&cat, &t, &mut reps, &at, &mut problems);
        g.section("where_used")
            .str(&serde_json::to_string(&rows).unwrap())
            .str(&serde_json::to_string(&problems).unwrap());
    }
    g.finish();
}

#[test]
fn imports_answer_the_same_everywhere() {
    use ryuplm::import::{mapping::Mapping, records, rows::Table, run};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut g = Golden::new(dir.join("tests/golden/imports.txt"));
    // SAFETY: the golden reads one seed.
    unsafe { std::env::set_var(ryuplm::ids::SEED_VAR, "golden") };
    let (mut files, m, csv1, csv2) = ryuplm::testing::import_source("golden", 400);
    let config = ryupdm::config::Config::read_in(&files, "ryuplm").unwrap();
    for (k, csv) in [csv1, csv2].iter().enumerate() {
        let table = Table::from_raw(ryuplm::import::csv::parse(csv, ',').unwrap(), 1).unwrap();
        let cat = Catalogue::load(&files).unwrap();
        let (rows, refusals) =
            records::map(&table, &m, &config, cat.manifest.default_unit).unwrap();
        g.section("refusals")
            .str(&serde_json::to_string(&refusals).unwrap());
        let out = run::plan(&files, &cat, &config, &m, &rows, None, 50).unwrap();
        g.section(&format!("report{k}"))
            .str(&serde_json::to_string(&out.report).unwrap());
        for (id, it) in &out.items {
            let text = it.to_text();
            g.section(&format!("items{k}")).str(id.as_str()).str(&text);
            files.insert(ryuplm::ids::item_path(id), text.into_bytes());
        }
    }
    let m2: Mapping = m.clone();
    g.section("mapping").str(&m2.to_text());
    g.finish();
}

#[test]
fn tree_imports_answer_the_same_everywhere() {
    use ryuplm::import::{mapping::ReaderKind, records, run};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut g = Golden::new(dir.join("tests/golden/trees.txt"));
    // SAFETY: the goldens read one seed.
    unsafe { std::env::set_var(ryuplm::ids::SEED_VAR, "golden") };
    let (files, m, csv, _) = ryuplm::testing::import_source("golden", 400);
    let config = ryupdm::config::Config::read_in(&files, "ryuplm").unwrap();
    let cat = Catalogue::load(&files).unwrap();
    let (json_text, xml_text) = ryuplm::testing::csv_as_trees(&csv);
    for (kind, name, text) in [
        (ReaderKind::Json, "json", json_text),
        (ReaderKind::Xml, "xml", xml_text),
    ] {
        let tm = ryuplm::testing::tree_mapping(&m, kind);
        let table = ryuplm::import::read_table(text.as_bytes(), &tm.reader).unwrap();
        g.section(&format!("{name}_columns"))
            .str(&table.columns.join("\n"));
        let (rows, refusals) =
            records::map(&table, &tm, &config, cat.manifest.default_unit).unwrap();
        g.section(&format!("{name}_refusals"))
            .str(&serde_json::to_string(&refusals).unwrap());
        let out = run::plan(&files, &cat, &config, &tm, &rows, None, 50).unwrap();
        g.section(&format!("{name}_report"))
            .str(&serde_json::to_string(&out.report).unwrap());
        for (id, it) in &out.items {
            g.section(&format!("{name}_items"))
                .str(id.as_str())
                .str(&it.to_text());
        }
        g.section(&format!("{name}_mapping")).str(&tm.to_text());
    }
    g.finish();
}

#[test]
fn history_imports_answer_the_same_everywhere() {
    use ryuplm::import::{history, records, rows::Table, run};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut g = Golden::new(dir.join("tests/golden/history.txt"));
    // SAFETY: the goldens read one seed.
    unsafe { std::env::set_var(ryuplm::ids::SEED_VAR, "golden") };
    let (mut files, m, csv) = ryuplm::testing::history_source("golden", 400);
    let config = ryupdm::config::Config::read_in(&files, "ryuplm").unwrap();
    let table = Table::from_raw(ryuplm::import::csv::parse(&csv, ',').unwrap(), 1).unwrap();
    for round in 0..2 {
        let cat = Catalogue::load(&files).unwrap();
        let (rows, refusals) =
            records::map(&table, &m, &config, cat.manifest.default_unit).unwrap();
        g.section("refusals")
            .str(&serde_json::to_string(&refusals).unwrap());
        let out = run::plan(&files, &cat, &config, &m, &rows, None, 50).unwrap();
        g.section(&format!("report{round}"))
            .str(&serde_json::to_string(&out.report).unwrap());
        for (id, it) in &out.items {
            let text = it.to_text();
            g.section(&format!("items{round}"))
                .str(id.as_str())
                .str(&text);
            files.insert(ryuplm::ids::item_path(id), text.into_bytes());
        }
    }
    let cat = Catalogue::load(&files).unwrap();
    let foreign: Vec<(String, String)> = history::foreign(&files, &cat, &config)
        .into_iter()
        .map(|e| (e.message, e.details["label"].to_string()))
        .collect();
    g.section("foreign")
        .str(&serde_json::to_string(&foreign).unwrap());
    g.section("mapping").str(&m.to_text());
    g.finish();
}
