//! Bit-exact output on every platform and CPU level (design 732):
//! `tests/golden/ryu-gerber.txt` holds hashes of a seeded layer of every
//! standard symbol and of seeded drills in both slot forms. After a
//! deliberate change: `UPDATE_GOLDEN=1 cargo test -p ryu-gerber --test
//! determinism`, and review the diff.

use ryu_board::geom::{Mdeg, Pen, Place, Point};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape, symbol_shape};
use ryu_board::testing::{Golden, Lcg, seeded_symbol};
use ryu_gerber::Software;
use ryu_gerber::excellon::{self, Drill, Drills, Hole, SlotForm};
use ryu_gerber::gerber::{self, Item, Layer};

fn sw() -> Software {
    Software {
        vendor: "Seiraiyu".into(),
        application: "golden".into(),
        version: "1.0.0".into(),
    }
}

fn writable(p: &Part) -> bool {
    match &p.piece {
        Piece::Region(r) => r.holes.is_empty(),
        Piece::Stroke(s) => match s.pen {
            Pen::Round(w) => w > 0,
            Pen::Square(w) => {
                w > 0
                    && s.path
                        .iter()
                        .all(|e| matches!(e, ryu_board::geom::Edge::Segment(_)))
            }
        },
        Piece::Swept { .. } => false,
    }
}

fn seeded_layer(seed: u64, n: i64) -> Layer {
    let mut g = Lcg::new(seed);
    let mut items = Vec::new();
    for i in 0..n {
        let Ok(s) = symbol_shape(&seeded_symbol(&mut g)) else {
            continue;
        };
        let place = Place {
            at: Point::new(
                g.range(-50_000_000, 50_000_000),
                g.range(-50_000_000, 50_000_000),
            ),
            rot: Mdeg::new(90_000 * (i % 4)),
            mirror: false,
            scale: None,
        };
        let mut shape = s.placed(&place, Polarity::Positive);
        shape.parts.retain(writable);
        if shape.parts.is_empty() {
            continue;
        }
        items.push(Item {
            shape: Shape {
                parts: shape.parts,
                ..Shape::default()
            },
            function: g.coin().then(|| "SMDPad,CuDef".to_owned()),
            net: Some(format!("n{}", i % 7)),
            pad: Some((format!("U{i}"), "1".to_owned())),
            component: Some(format!("U{i}")),
        });
    }
    Layer {
        function: "Copper,L1,Top".into(),
        negative: false,
        items,
    }
}

fn seeded_drills(seed: u64, n: usize) -> Drills {
    let mut g = Lcg::new(seed);
    let holes = (0..n)
        .map(|i| {
            let at = Point::new(
                g.range(-40_000_000, 40_000_000),
                g.range(-40_000_000, 40_000_000),
            );
            let d = g.range(150, 3_000) * 1_000;
            Hole {
                drill: if i % 4 == 0 {
                    Drill::Slot {
                        a: at,
                        b: at.offset(g.range(1, 2_000) * 1_000, 0),
                        d,
                    }
                } else {
                    Drill::Round { at, d }
                },
                function: Some("Plated,PTH,ComponentDrill".into()),
            }
        })
        .collect();
    Drills {
        function: "Plated,1,2,PTH".into(),
        holes,
    }
}

#[test]
fn gerber_and_excellon_are_bit_exact() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/ryu-gerber.txt"
    ));
    let layer = seeded_layer(0x6e7b_0001, 300);
    assert!(layer.items.len() > 150, "{}", layer.items.len());
    golden
        .section("gerber.seeded")
        .bytes(gerber::write(&layer, &sw()).unwrap().as_bytes());
    let d = seeded_drills(0x6e7b_0002, 120);
    for (name, form) in [
        ("excellon.g85", SlotForm::G85),
        ("excellon.routed", SlotForm::Routed),
    ] {
        golden
            .section(name)
            .bytes(excellon::write(&d, &sw(), form).unwrap().as_bytes());
    }
    golden.finish();
}
