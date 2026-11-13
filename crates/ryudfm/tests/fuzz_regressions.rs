//! Inputs the `odb` fuzz target found, replayed (ryucad plan 30.A): a
//! features file that reads must write, read and write again to the same
//! text, as `fuzz/fuzz_targets/odb.rs`'s mode 0 asserts.

use std::collections::BTreeSet;

use ryu_board::model::{FeatureKind, Symbol};
use ryu_board::symbol::StandardSymbol;
use ryudfm::odb::attrs::Decls;
use ryudfm::odb::features::{Context, features};
use ryudfm::odb::text::decode;
use ryudfm::odb::units::Units;
use ryudfm::testing::write_features;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// The `odb` target's mode 0 on `hex`: the input must read and be
/// writable, and what it writes must go round.
fn goes_round(hex: &str) {
    let data = bytes(hex);
    let (&mode, rest) = data.split_first().unwrap();
    assert_eq!(mode & 0x7f, 0, "a features file");
    let text = decode(rest);
    let decls = Decls::default();
    let user = BTreeSet::from(["user_pad".to_string()]);
    let units = if mode & 0x80 == 0 {
        Units::Inch
    } else {
        Units::Mm
    };
    let cx = Context {
        path: "f",
        units,
        decls: &decls,
        user: &user,
    };
    let p = features(&text, &cx).expect("the input reads");
    let writable = p
        .symbols
        .iter()
        .all(|s| !matches!(s, Symbol::Standard(StandardSymbol::Trapezoid { .. })))
        && p.features.iter().all(|f| match &f.kind {
            FeatureKind::Text(t) => {
                t.line_width % 3_048 == 0 && !t.string.contains('\'') && !t.font.contains(' ')
            }
            FeatureKind::Barcode(b) => !b.string.contains('\'') && !b.kind.contains(' '),
            _ => true,
        });
    assert!(writable, "the target writes it");
    let again = write_features(&p.into_features());
    let back = features(&again, &cx)
        .expect("written features read")
        .into_features();
    assert_eq!(write_features(&back), again);
}

/// Nightly 2026-11-05 (crash-bc9db8fc…, inch): an island with no area,
/// `0 0` → `0 0` → `49.9872 0` → `0 0`, came back turned the other way.
#[test]
fn an_island_with_no_area_goes_round() {
    goes_round(
        "00554c4954533d494e43480d0a230d0a234c6179657220ffffffffffffff0f0d0a230d0a53205020300d0a4f422030203020490d0a4f5320312e3936382030203020312e5d3831313032340d0a4f53203020300d4f5320312e3936ffffffffffff16aa67666666666666063020312e5d3831313032340d0a4f53203020300d0a4f450d0a53450d0a",
    );
}

/// Nightly 2026-11-12 (crash-067bdf1f…, mm): another island with no
/// area, out and back along one segment.
#[test]
fn an_island_out_and_back_goes_round() {
    goes_round(
        "80554e4954533d4d4d0a230a234e756d2046656174757265730a230a24312072656374313435302e3078343139312e300a24322072323330302e300a24332072313030302e300a24342072656374313030302e3078343139310a24332072313030302e300a24342072656374313030302e3078343139312e300a0a230a234665617475726520617474726962757465206e616d65730a230a403020525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252525252522e7061645f75736167650a4031202e67656f6d657472790a4032202e736d640a4033202e746561725f64726f700a0a230a234665617475726520617474726962757465207465787420737472696e67730a230a2630205649415f524f554e44443630303030300a230a234c617933342e313532343038202d38332e3130383039390a4f53203133342e333133343833202d38322e3831353831310a4f53203133342e33313731383d010000322e3830393830332e39343138343853203133372e363634333232202d38312e3031393736312d38352e353839333430203134352e3239383730202d38362e3635302033205020302059200a5320502030203b330a4f42203133342e303438323732202d38362e31353020490a4f53203133342e303438323732202d38362e3135300a4f53203133352e363430313632202d38352e3630313632204f53203133332e34393930202d38352e3135300a4f53203133342e303438323732202d38362e3135300a4f450a53450a50203134392e3830202d38362e3536373020342050203020382039302e30203b303d302c320a50203133332e3530202d38322e383030203220502030203820302e30203b303d300a",
    );
}

/// Nightly 2026-11-09 (crash-0fe79372…, inch): `rect106.2992x459267717`
/// is out of range in mils, so the reader keeps it as unknown; written
/// into a millimetre file it read as a rectangle in microns, rounded.
#[test]
fn a_name_unknown_in_mils_goes_round() {
    goes_round(
        "00554e4954533d494e43480d0a234c617965725f506879736963616c5f4f726465723d300d0a234c617965725f436f6c6f723d3235350d0a230d0a23466561747572652073796d627839312e353531320d0a243220726563743130322e333632327834372e323434310d0a243320726563743130362e32393932783435393236373731370d0a726563743132392e",
    );
}
