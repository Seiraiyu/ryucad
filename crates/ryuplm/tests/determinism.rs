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
