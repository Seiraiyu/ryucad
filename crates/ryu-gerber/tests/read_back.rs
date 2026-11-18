//! What ryu-gerber writes reads back as what went in (design 732): flashes,
//! regions, arcs, draws, polarity and attributes, by its own test reader;
//! over hand-made layers and seeded symbols at every quarter turn. gerbv
//! (ryueda's tests/gerbv.rs, devcontainer only) is the third-party check.

use ryu_board::geom::{Arc, Dir, Edge, Mdeg, Pen, Place, Point, Region, Stroke};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape, circle, symbol_shape};
use ryu_board::symbol::{Corners, StandardSymbol};
use ryu_board::testing::{Lcg, seeded_symbol};
use ryu_gerber::excellon::{self, Drill, Drills, Hole, SlotForm};
use ryu_gerber::gerber::{self, Item, Layer};
use ryu_gerber::read::{self, Kind};
use ryu_gerber::{Error, Software, same_contour};

fn sw() -> Software {
    Software {
        vendor: "Seiraiyu".into(),
        application: "test".into(),
        version: "1.0.0".into(),
    }
}

/// Whether two pieces draw the same: regions by their contours in cyclic
/// order (either way round), strokes by path and pen.
fn same_piece(a: &Piece, b: &Piece) -> bool {
    match (a, b) {
        (Piece::Region(x), Piece::Region(y)) => {
            x.holes.is_empty()
                && y.holes.is_empty()
                && (same_contour(&x.outer, &y.outer) || same_contour(&x.outer.reversed(), &y.outer))
        }
        (Piece::Stroke(x), Piece::Stroke(y)) => x == y,
        _ => false,
    }
}

/// The pieces a written part becomes: itself, or for a square pen one
/// hull region per segment.
fn expected(part: &Part) -> Vec<Part> {
    match &part.piece {
        Piece::Stroke(Stroke {
            path,
            pen: pen @ Pen::Square(_),
        }) => path
            .iter()
            .map(|e| {
                let Edge::Segment(s) = e else {
                    unreachable!("filtered")
                };
                Part {
                    polarity: part.polarity,
                    piece: Piece::Region(ryu_board::shape::line_outline(s.a, s.b, *pen).unwrap()),
                }
            })
            .collect(),
        _ => vec![part.clone()],
    }
}

/// Whether the writer takes the part (the refusals are tested below).
fn writable(p: &Part) -> bool {
    match &p.piece {
        Piece::Region(r) => r.holes.is_empty(),
        Piece::Stroke(s) => match s.pen {
            Pen::Round(w) => w > 0,
            Pen::Square(w) => w > 0 && s.path.iter().all(|e| matches!(e, Edge::Segment(_))),
        },
        Piece::Swept { .. } => false,
    }
}

fn back(layer: &Layer) -> read::Image {
    let text = gerber::write(layer, &sw()).unwrap();
    read::gerber(&text).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

#[test]
fn a_hand_made_layer_comes_back() {
    let rect = symbol_shape(&StandardSymbol::Rect {
        w: 1_000_000,
        h: 500_000,
        corners: Corners::SHARP,
    })
    .unwrap()
    .placed(
        &Place {
            at: Point::new(5_000_000, 2_000_000),
            rot: Mdeg::new(90_000),
            mirror: false,
            scale: None,
        },
        Polarity::Positive,
    );
    let rounded = symbol_shape(&ryu_board::symbol::pad::roundrect(
        1_200_000, 600_000, 250_000,
    ))
    .unwrap()
    .placed(
        &Place::at(Point::new(-3_000_000, 1_000_000)),
        Polarity::Positive,
    );
    let track = Shape {
        parts: vec![Part {
            polarity: Polarity::Positive,
            piece: Piece::Stroke(Stroke {
                path: vec![
                    Edge::line(Point::new(0, 0), Point::new(1_000_000, 0)),
                    Edge::Arc(Arc {
                        start: Point::new(1_000_000, 0),
                        end: Point::new(2_000_000, 1_000_000),
                        centre: Point::new(1_000_000, 1_000_000),
                        dir: Dir::Ccw,
                    }),
                ],
                pen: Pen::Round(250_000),
            }),
        }],
        ..Shape::default()
    };
    let clear = Shape::regions(
        Polarity::Negative,
        [Region::solid(
            circle(Point::new(7_000_000, 7_000_000), 150_000).unwrap(),
        )],
    );
    let items = vec![
        Item {
            shape: rect.clone(),
            function: Some("SMDPad,CuDef".into()),
            net: Some("rail:gnd".into()),
            pad: Some(("R1".into(), "1".into())),
            component: Some("R1".into()),
        },
        Item {
            shape: rounded.clone(),
            function: Some("SMDPad,CuDef".into()),
            ..Item::default()
        },
        Item {
            shape: track.clone(),
            function: Some("Conductor".into()),
            net: Some("s1/n,2".into()),
            ..Item::default()
        },
        Item {
            shape: clear.clone(),
            ..Item::default()
        },
    ];
    let layer = Layer {
        function: "Copper,L1,Top".into(),
        negative: false,
        items,
    };
    let img = back(&layer);
    assert_eq!(img.attrs[".FileFunction"], "Copper,L1,Top");
    assert_eq!(img.attrs[".FilePolarity"], "Positive");
    assert_eq!(img.attrs[".GenerationSoftware"], "Seiraiyu,test,1.0.0");
    assert_eq!(img.attrs[".SameCoordinates"], "Original");
    assert_eq!(img.objects.len(), 4, "{img:#?}");
    let o = &img.objects;
    assert_eq!(
        o[0].kind,
        Kind::Flash {
            at: Point::new(5_000_000, 2_000_000),
            aperture: ryu_gerber::Aperture::Rect(500_000, 1_000_000)
        }
    );
    assert_eq!(o[0].attrs[".P"], "R1,1");
    assert_eq!(o[0].attrs[".N"], "rail:gnd");
    assert_eq!(o[0].attrs[".C"], "R1");
    assert_eq!(o[0].function.as_deref(), Some("SMDPad,CuDef"));
    assert!(matches!(o[1].kind, Kind::Region(_)));
    assert_eq!(o[1].function.as_deref(), Some("SMDPad,CuDef"));
    assert!(o[1].attrs.is_empty());
    // The comma in the net is escaped on the way out.
    assert_eq!(o[2].attrs[".N"], "s1/n\\u002C2");
    assert_eq!(o[2].function.as_deref(), Some("Conductor"));
    assert!(!o[3].dark);
    for (shape, obj) in [&rect, &rounded, &track, &clear].into_iter().zip(o) {
        assert!(
            same_piece(&shape.parts[0].piece, &obj.shape().parts[0].piece),
            "{obj:?}"
        );
        assert_eq!(shape.parts[0].polarity, obj.shape().parts[0].polarity);
    }
}

#[test]
fn seeded_symbols_at_every_quarter_turn_come_back() {
    let mut g = Lcg::new(0x5c_9e7b);
    let mut checked = 0;
    for i in 0..400 {
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
        let shape = s.placed(&place, Polarity::Positive);
        let parts: Vec<Part> = shape
            .parts
            .iter()
            .filter(|p| writable(p))
            .cloned()
            .collect();
        if parts.is_empty() {
            continue;
        }
        let want: Vec<Part> = parts.iter().flat_map(expected).collect();
        let layer = Layer {
            function: "Legend,Top".into(),
            negative: false,
            items: vec![Item {
                shape: Shape {
                    parts,
                    ..Shape::default()
                },
                ..Item::default()
            }],
        };
        let img = back(&layer);
        assert_eq!(img.objects.len(), want.len(), "seed step {i}: {img:#?}");
        for (w, o) in want.iter().zip(&img.objects) {
            let got = o.shape();
            assert!(
                same_piece(&w.piece, &got.parts[0].piece),
                "seed step {i}: {w:?} came back {o:?}"
            );
            assert_eq!(w.polarity, got.parts[0].polarity, "seed step {i}");
        }
        checked += 1;
    }
    assert!(checked > 200, "only {checked} symbols written");
}

#[test]
fn what_gerber_cant_hold_is_refused() {
    let ring = Shape::regions(
        Polarity::Positive,
        [Region::ring(
            circle(Point::new(0, 0), 1_000).unwrap(),
            circle(Point::new(0, 0), 500).unwrap(),
        )],
    );
    let layer = |shape| Layer {
        function: "Legend,Top".into(),
        negative: false,
        items: vec![Item {
            shape,
            ..Item::default()
        }],
    };
    assert_eq!(
        gerber::write(&layer(ring), &sw()).unwrap_err(),
        Error::Holes
    );
    let swept = Shape {
        parts: vec![Part {
            polarity: Polarity::Positive,
            piece: Piece::Swept {
                path: Edge::line(Point::new(0, 0), Point::new(10, 0)),
                pen: vec![Region::solid(circle(Point::new(0, 0), 5).unwrap())],
            },
        }],
        ..Shape::default()
    };
    assert!(matches!(
        gerber::write(&layer(swept), &sw()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn drills_come_back_in_both_slot_forms() {
    let mut g = Lcg::new(7);
    let mut holes = Vec::new();
    for i in 0..60 {
        let p = |g: &mut Lcg| {
            Point::new(
                g.range(-40_000_000, 40_000_000),
                g.range(-40_000_000, 40_000_000),
            )
        };
        let d = g.range(100, 3_000) * 1_000;
        let drill = if i % 3 == 0 {
            let a = p(&mut g);
            Drill::Slot {
                a,
                b: a.offset(g.range(1, 3_000) * 1_000, g.range(-1_000, 1_000) * 1_000),
                d,
            }
        } else {
            Drill::Round { at: p(&mut g), d }
        };
        let function = Some(
            if i % 2 == 0 {
                "Plated,PTH,ViaDrill"
            } else {
                "Plated,PTH,ComponentDrill"
            }
            .to_owned(),
        );
        holes.push(Hole { drill, function });
    }
    let d = Drills {
        function: "Plated,1,2,PTH".into(),
        holes: holes.clone(),
    };
    for form in [SlotForm::G85, SlotForm::Routed] {
        let text = excellon::write(&d, &sw(), form).unwrap();
        let img = read::excellon(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(img.attrs[".FileFunction"], "Plated,1,2,PTH");
        // Tools rise in diameter.
        assert!(
            img.tools
                .windows(2)
                .all(|w| (w[0].1, &w[0].2) <= (w[1].1, &w[1].2)),
            "{:?}",
            img.tools
        );
        let mut got: Vec<(Drill, Option<String>)> = img.holes.clone();
        let mut want: Vec<(Drill, Option<String>)> = holes
            .iter()
            .map(|h| (h.drill, h.function.clone()))
            .collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "{form:?}");
    }
}
