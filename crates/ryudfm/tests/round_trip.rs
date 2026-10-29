//! Seeded features written as ODB++ by the test writer read back to the
//! same model, bit for bit.

use std::collections::BTreeSet;

use ryu_board::geom::{Mdeg, Place, Point};
use ryu_board::model::{
    AttrValue, Barcode, Feature, FeatureKind, Features, Polarity, Symbol, Text,
};
use ryu_board::symbol::StandardSymbol;
use ryu_board::testing::{Lcg, seeded_features};
use ryudfm::odb::attrs::Decls;
use ryudfm::odb::features::{Context, features};
use ryudfm::odb::units::Units;
use ryudfm::testing::{canonical, symbol_name, write_features};

/// Seeded features with what the generator leaves out: attributes, IDs,
/// D-codes, text, barcodes and a user symbol; no trapezoids, which ODB++
/// can't name.
fn seeded(seed: u64) -> Features {
    let mut g = Lcg::new(seed);
    let f = seeded_features(&mut g, 40, 400, 200_000_000);
    let mut symbols: Vec<Symbol> = f
        .symbols()
        .iter()
        .map(|s| match s {
            Symbol::Standard(StandardSymbol::Trapezoid { .. }) => {
                Symbol::Standard(StandardSymbol::Round { d: 1_000 })
            }
            Symbol::Standard(s) => Symbol::Standard(canonical(s)),
            s => s.clone(),
        })
        .collect();
    symbols.push(Symbol::User("my_pad".into()));
    let mut out: Vec<Feature> = f.features().to_vec();
    for (i, f) in out.iter_mut().enumerate() {
        match g.range(0, 5) {
            0 => {
                f.attributes.insert(".smd".into(), AttrValue::Flag);
            }
            1 => {
                f.attributes.insert(
                    ".geometry".into(),
                    AttrValue::Text(format!("geo {}", g.range(0, 3))),
                );
            }
            2 => {
                f.attributes
                    .insert(".pad_usage".into(), AttrValue::Text("via".into()));
                f.attributes.insert(
                    ".padstack_id".into(),
                    AttrValue::Number(format!("{}", g.range(0, 99))),
                );
            }
            _ => {}
        }
        if g.coin() {
            f.id = Some(i as u64 * 7 + 1);
        }
        if g.range(0, 3) == 0 {
            f.dcode = Some(g.range(10, 99) as i32);
        }
    }
    let place = |g: &mut Lcg| Place {
        at: Point::new(
            g.range(-1_000_000, 1_000_000),
            g.range(-1_000_000, 1_000_000),
        ),
        rot: Mdeg::new(g.range(0, 359_999)),
        mirror: g.coin(),
        scale: None,
    };
    for k in 0..20 {
        out.push(Feature::new(
            FeatureKind::Text(Text {
                font: "standard".into(),
                place: place(&mut g),
                width: g.range(100_000, 2_000_000),
                height: g.range(100_000, 2_000_000),
                line_width: 3_048 * g.range(1, 300),
                string: format!("R{k} 10k"),
            }),
            Polarity::Positive,
        ));
    }
    out.push(Feature::new(
        FeatureKind::Barcode(Barcode {
            place: place(&mut g),
            kind: "UPC39".into(),
            string: "12345".into(),
        }),
        Polarity::Positive,
    ));
    let user = symbols.len() as u32 - 1;
    out.push(Feature::new(
        FeatureKind::Pad {
            symbol: user,
            place: place(&mut g),
        },
        Polarity::Negative,
    ));
    Features::new(symbols, out)
}

fn read_back(text: &str) -> Features {
    let decls = Decls::default();
    let user = BTreeSet::from(["my_pad".to_string()]);
    let cx = Context {
        path: "features",
        units: Units::Inch,
        decls: &decls,
        user: &user,
    };
    let p = features(text, &cx).unwrap();
    assert_eq!(p.nums.rounded(), 0, "every number exact");
    assert!(p.unknown.is_empty());
    p.into_features()
}

#[test]
fn seeded_features_go_round() {
    for seed in 1..=25 {
        let f = seeded(seed);
        let text = write_features(&f);
        let back = read_back(&text);
        assert_eq!(back.symbols(), f.symbols(), "seed {seed}");
        for (i, (a, b)) in f.features().iter().zip(back.features()).enumerate() {
            assert_eq!(a, b, "seed {seed}, feature {i}");
        }
        assert_eq!(back, f, "seed {seed}");
        // And the text it writes again is the same text.
        assert_eq!(write_features(&back), text);
    }
}

#[test]
fn every_standard_symbol_but_the_trapezoid_has_a_name() {
    let mut g = Lcg::new(9);
    for _ in 0..2_000 {
        let s = ryu_board::testing::seeded_symbol(&mut g);
        match symbol_name(&s) {
            Some(n) => assert_eq!(
                StandardSymbol::parse(&n, Units::Mm.names()),
                Some(canonical(&s)),
                "{n}"
            ),
            None => assert!(matches!(s, StandardSymbol::Trapezoid { .. })),
        }
    }
}
