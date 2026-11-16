//! Bit-exact boards on every platform and CPU level:
//! `tests/golden/ryudfm.txt` holds the hashes of each committed board's
//! model, of every layer's shapes, of its default renders (PNG from
//! above and below, and SVG), and of its copper (islands, findings, gaps
//! under 0.2 mm, merged layers and a merged render), for every board
//! `manifest.txt` declares but the pinned ones. After a deliberate change, or a board added,
//! regenerate with `UPDATE_GOLDEN=1 cargo test -p ryudfm --test
//! determinism` and review the diff.

use std::path::Path;

use ryu_board::testing::Golden;
use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::render::{Args, Format, Side, render};
use ryudfm::testing::manifest;

#[test]
fn boards_are_bit_exact() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = dir.join("tests/corpus");
    let mut golden = Golden::new(dir.join("tests/golden/ryudfm.txt"));
    let declared = manifest(&std::fs::read_to_string(corpus.join("manifest.txt")).unwrap());
    for d in declared.iter().filter(|d| !d.pinned) {
        let job = d
            .archive
            .as_ref()
            .map(|(a, _)| a)
            .or(d.folder.as_ref())
            .unwrap();
        let options = ReadOptions {
            step: d.step.clone(),
            ..ReadOptions::default()
        };
        let b = read_path(&corpus.join(job), &options).unwrap();
        {
            // The model as read, before any shape is built (each feature's
            // shape slot is part of its debug text).
            let mut s = golden.section(&format!("{}.model", d.name));
            s.str(&format!("{b:?}"));
        }
        {
            let mut s = golden.section(&format!("{}.shapes", d.name));
            for step in 0..b.steps.len() {
                for layer in 0..b.layers.len() {
                    for shape in b.layer_shapes(step, layer) {
                        s.shape(shape);
                    }
                }
            }
        }
        for (name, side, format) in [
            ("png", Side::Top, Format::Png),
            ("png.bottom", Side::Bottom, Format::Png),
            ("svg", Side::Top, Format::Svg),
        ] {
            let args = Args {
                side: Some(side),
                format: Some(format),
                ..Args::default()
            };
            let a = render(&b, None, &args, ryudfm::units::Unit::Mm).unwrap();
            golden
                .section(&format!("{}.render.{name}", d.name))
                .bytes(&a.binary.unwrap());
        }
        // The copper, from the board read again (renders above built its
        // shapes; the copper is built from nothing).
        let cb = ryudfm::copper::CopperBoard::new(std::sync::Arc::new(
            read_path(&corpus.join(job), &options).unwrap(),
        ));
        {
            let mut s = golden.section(&format!("{}.copper.islands", d.name));
            for i in cb.copper.islands() {
                for p in &i.pieces {
                    s.str(&cb.island_name(None, p));
                }
                s.str("|");
                for h in &i.holes {
                    s.str(&cb.reference(None, *h));
                }
                s.str(";");
            }
        }
        golden
            .section(&format!("{}.copper.findings", d.name))
            .str(&serde_json::to_string(&ryudfm::copper::findings(&cb, None)).unwrap());
        {
            let mut s = golden.section(&format!("{}.copper.clearance", d.name));
            for l in 0..cb.built.copper.len() {
                for g in cb.gaps(l, 200_000).iter() {
                    s.str(&cb.island_name(None, &g.a));
                    s.str(&cb.island_name(None, &g.b));
                    s.i64(g.distance.nm);
                    s.i64(g.distance.bound);
                }
            }
        }
        {
            let mut s = golden.section(&format!("{}.copper.merged", d.name));
            for l in 0..cb.built.copper.len() {
                let m = cb.merged(l);
                for p in &m.polygons {
                    for r in p.rings() {
                        for q in r {
                            s.i64(q.x);
                            s.i64(q.y);
                        }
                        s.str("|");
                    }
                }
                s.i64(m.bound);
            }
        }
        let a = ryudfm::render::render_copper(
            &cb,
            None,
            &Args {
                merged: true,
                format: Some(Format::Png),
                ..Args::default()
            },
            ryudfm::units::Unit::Mm,
        )
        .unwrap();
        golden
            .section(&format!("{}.copper.render", d.name))
            .bytes(&a.binary.unwrap());
    }
    golden.finish();
}
