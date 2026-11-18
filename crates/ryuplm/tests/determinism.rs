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
