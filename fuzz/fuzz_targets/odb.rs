//! ODB++ from bytes: the first byte picks what the rest is read as (a
//! features file, `eda/data`, a matrix, a components file, a netlist, a
//! tools file, a step header, a font, attribute declarations and a list,
//! a stackup, `misc/info`, or an archive of a whole job, which is read
//! and every layer's shapes built). Every reader gives a value or a typed
//! fault, never a panic, and a features file that reads writes and reads
//! back to itself.

#![no_main]

use std::collections::BTreeSet;

use libfuzzer_sys::fuzz_target;
use ryu_board::model::{FeatureKind, Side, Symbol};
use ryu_board::symbol::StandardSymbol;
use ryudfm::odb::attrs::{Decls, attrlist};
use ryudfm::odb::features::{Context, features};
use ryudfm::odb::text::decode;
use ryudfm::odb::units::Units;
use ryudfm::odb::{Job, Limits, ReadOptions, components, eda, font, info, matrix, netlist, read, stackup, step, tools};
use ryudfm::testing::write_features;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let text = decode(rest);
    let decls = Decls::default();
    let user = BTreeSet::from(["user_pad".to_string()]);
    let units = if mode & 0x80 == 0 { Units::Inch } else { Units::Mm };
    match mode & 0x7f {
        0 => {
            let cx = Context { path: "f", units, decls: &decls, user: &user };
            let Ok(p) = features(&text, &cx) else { return };
            // Trapezoids can't be named; text widths that aren't hundredths
            // of 12 mils can't be written back.
            let writable = p.symbols.iter().all(|s| !matches!(s, Symbol::Standard(StandardSymbol::Trapezoid { .. })))
                && p.features.iter().all(|f| match &f.kind {
                    FeatureKind::Text(t) => t.line_width % 3_048 == 0 && !t.string.contains('\'') && !t.font.contains(' '),
                    FeatureKind::Barcode(b) => !b.string.contains('\'') && !b.kind.contains(' '),
                    _ => true,
                });
            let f = p.into_features();
            if writable {
                let again = write_features(&f);
                let back = features(&again, &cx).expect("written features read").into_features();
                assert_eq!(write_features(&back), again);
            }
        }
        1 => {
            let _ = eda::eda(&text, "e", units, &decls);
        }
        2 => {
            if let Ok(m) = matrix::matrix(&text) {
                let _ = m.layers();
            }
        }
        3 => {
            let _ = components::components(&text, "c", Side::Top, units, &decls);
        }
        4 => {
            let _ = netlist::netlist(&text, units);
        }
        5 => {
            let _ = tools::tools(&text, units);
        }
        6 => {
            let _ = step::header(&text, units);
        }
        7 => {
            let _ = font::font("f", &text);
        }
        8 => {
            let mut d = Decls::default();
            let _ = d.read(&text);
            let _ = attrlist(&text, &d);
        }
        9 => {
            let _ = stackup::from_xml(&text);
        }
        10 => {
            let _ = info::info(&text);
        }
        _ => {
            let limits = Limits { entries: 10_000, bytes: 64 << 20, file_bytes: 16 << 20 };
            let Ok(job) = Job::from_archive("fuzz", rest, limits) else { return };
            let Ok(b) = read(&job, &ReadOptions::default()) else { return };
            let all: Vec<usize> = (0..b.layers.len()).collect();
            for s in 0..b.steps.len() {
                b.build_shapes(s, &all);
            }
        }
    }
});
