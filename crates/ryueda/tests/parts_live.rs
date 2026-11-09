//! The jlc catalogue's real site, synced into a temporary cache: for a
//! person to run before a release (`RYUEDA_PARTS_LIVE=1`), never in CI.
//! Nothing it downloads is kept (the owner's condition on plan 387's
//! question 248): it prints the download's size and time, and checks the
//! attribute mapping still types most of what it should.

#[test]
#[ignore = "downloads the real catalogue: RYUEDA_PARTS_LIVE=1, --ignored"]
fn the_live_jlc_catalogue_syncs_and_reads() {
    if std::env::var("RYUEDA_PARTS_LIVE").as_deref() != Ok("1") {
        eprintln!("set RYUEDA_PARTS_LIVE=1 to sync the real catalogue");
        return;
    }
    use ryueda_parts::catalogue::Catalogue;
    use ryueda_parts::source::Source;
    let dir = tempfile::tempdir().unwrap();
    let t = std::time::Instant::now();
    let v = ryueda::parts::sync_with(
        &ryueda::net::Https::new(),
        ryueda_parts::jlc::BASE,
        dir.path(),
        "jlc",
        ryueda::parts::now(),
        &Default::default(),
    )
    .unwrap();
    println!(
        "synced {} parts, {} files, {} bytes, {} rows left out, catalogue of {}, in {:?}",
        v["parts"],
        v["files"],
        v["bytes"],
        v["skipped_rows"],
        v["created"],
        t.elapsed()
    );
    let on_disk: u64 = walk(&dir.path().join("jlc/current"));
    println!("index on disk: {on_disk} bytes");
    let jlc = ryueda_parts::jlc::Jlc::open(&dir.path().join("jlc"))
        .unwrap()
        .unwrap();
    println!("index parts: {}", jlc.parts());
    let t = std::time::Instant::now();
    let caps = jlc
        .search(
            &ryueda_parts::query::Query {
                category: Some(ryueda_parts::category::Category::Capacitor),
                conditions: vec![],
                stock: None,
                basic: false,
                limit: usize::MAX,
            },
            &Default::default(),
        )
        .unwrap();
    println!(
        "capacitors: {} (loaded and judged in {:?})",
        caps.len(),
        t.elapsed()
    );
    let typed = caps
        .iter()
        .filter(|h| h.record.parameters.contains_key("capacitance"))
        .count();
    println!("capacitors with a capacitance: {typed} of {}", caps.len());
    for name in [
        "voltage_rating",
        "package",
        "tolerance",
        "dielectric",
        "operating_temperature",
    ] {
        let n = caps
            .iter()
            .filter(|h| h.record.parameters.contains_key(name))
            .count();
        println!("capacitors with {name}: {n}");
    }
    let res = jlc
        .search(
            &ryueda_parts::query::Query {
                category: Some(ryueda_parts::category::Category::Resistor),
                conditions: vec![],
                stock: None,
                basic: false,
                limit: usize::MAX,
            },
            &Default::default(),
        )
        .unwrap();
    for name in ["resistance", "tolerance", "power_rating", "package"] {
        let n = res
            .iter()
            .filter(|h| h.record.parameters.contains_key(name))
            .count();
        println!("resistors with {name}: {n} of {}", res.len());
    }
    assert!(
        typed * 10 >= caps.len() * 9,
        "the attribute mapping lost capacitance"
    );
}

fn walk(p: &std::path::Path) -> u64 {
    std::fs::read_dir(p)
        .map(|d| {
            d.flatten()
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum()
        })
        .unwrap_or(0)
}

/// DigiKey's real API with a person's own keys (from the environment:
/// `RYUEDA_DIGIKEY_CLIENT_ID`, `RYUEDA_DIGIKEY_CLIENT_SECRET`; and
/// `RYUEDA_DIGIKEY_URL=https://sandbox-api.digikey.com` for its sandbox):
/// for a person to run before a release (`RYUEDA_PARTS_LIVE=1`), never in
/// CI. Nothing it fetches is kept: its cache is a temporary folder. It
/// checks the v4 answers still read the way our fixtures say, and prints
/// what it found.
#[test]
#[ignore = "calls DigiKey with your keys: RYUEDA_PARTS_LIVE=1, --ignored"]
fn the_live_digikey_api_answers_as_our_fixtures_say() {
    if std::env::var("RYUEDA_PARTS_LIVE").as_deref() != Ok("1") {
        eprintln!("set RYUEDA_PARTS_LIVE=1 and DigiKey keys to call the real API");
        return;
    }
    let dk = ryueda_parts::distributor::distributor("digikey").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let live = ryueda::distributors::Distributors {
        config: None,
        cache: Some(dir.path().to_path_buf()),
        ..ryueda::distributors::Distributors::here()
    };
    let src = live
        .open(dk)
        .unwrap()
        .expect("set RYUEDA_DIGIKEY_CLIENT_ID and RYUEDA_DIGIKEY_CLIENT_SECRET");
    let t = std::time::Instant::now();
    let q = ryueda_parts::query::Query {
        category: Some(ryueda_parts::category::Category::Capacitor),
        conditions: vec![
            ryueda_parts::query::Condition::parse("capacitance=100nF").unwrap(),
            ryueda_parts::query::Condition::parse("package=0402").unwrap(),
        ],
        stock: None,
        basic: false,
        limit: 50,
    };
    let hits = src.search(&q, &Default::default()).unwrap();
    println!("search: {} hits in {:?}", hits.len(), t.elapsed());
    assert!(!hits.is_empty(), "a 100 nF 0402 capacitor at DigiKey");
    let typed = hits
        .iter()
        .filter(|h| h.record.parameters.contains_key("capacitance"))
        .count();
    println!("with a capacitance: {typed} of {}", hits.len());
    assert!(typed * 2 >= hits.len(), "most hits type their capacitance");
    let key = hits[0].record.key.clone();
    let r = src
        .get(&key, &Default::default())
        .unwrap()
        .expect("its details");
    println!(
        "{key}: {} {} lifecycle {:?} stock {:?}",
        r.manufacturer, r.mpn, r.lifecycle, r.stock
    );
    assert!(r.lifecycle.is_some(), "DigiKey gives a status we read");
    assert!(!r.price_breaks.is_empty());
}
