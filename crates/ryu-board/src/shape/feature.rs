//! A feature's shape: its symbol placed, the outline its pen sweeps, its
//! surface, or its text.

use super::stroke::{arc_outline, line_outline};
use super::{Part, Piece, Shape, ShapeError, symbol_shape, text_shape};
use crate::geom::{Edge, Pen, Stroke};
use crate::model::{FeatureKind, Features, Library, Polarity, Symbol};
use crate::symbol::StandardSymbol;

/// The shape of symbol `index` of a table, in its own coordinates, built
/// once and kept.
pub(crate) fn table_symbol<'a>(
    features: &'a Features,
    index: u32,
    lib: &Library,
) -> &'a Result<Shape, ShapeError> {
    static MISSING: std::sync::OnceLock<Result<Shape, ShapeError>> = std::sync::OnceLock::new();
    let Some(slot) = features.symbol_shapes.get(index as usize) else {
        return MISSING.get_or_init(|| {
            Err(ShapeError::UnknownSymbol(
                "a symbol number past the table".into(),
            ))
        });
    };
    slot.get_or_init(|| match &features.symbols()[index as usize] {
        Symbol::Standard(s) => symbol_shape(s),
        Symbol::Unknown(name) => Err(ShapeError::UnknownSymbol(name.clone())),
        Symbol::User(name) => {
            if let Some(cycle) = lib.cycle(name) {
                return Err(ShapeError::SymbolCycle(cycle.to_vec()));
            }
            let user = lib
                .symbol(name)
                .ok_or_else(|| ShapeError::UnknownSymbol(name.clone()))?;
            let mut whole = Shape::default();
            for i in 0..user.features.features().len() {
                let s = feature_shape(&user.features, i, lib)
                    .as_ref()
                    .map_err(Clone::clone)?;
                whole.parts.extend(s.parts.iter().cloned());
                whole.approximate = whole.approximate.max(s.approximate);
                whole.missing.extend(s.missing.iter().copied());
            }
            whole.missing.sort_unstable();
            whole.missing.dedup();
            Ok(whole)
        }
    })
}

/// What a line or arc drawn with symbol `index` is drawn with: a round or
/// square pen, or the symbol's own regions.
enum Drawn {
    Pen(Pen),
    Shape(Vec<crate::geom::Region>),
}

fn drawn(features: &Features, index: u32, lib: &Library) -> Result<Drawn, ShapeError> {
    match features.symbols().get(index as usize) {
        Some(Symbol::Standard(StandardSymbol::Round { d })) => Ok(Drawn::Pen(Pen::Round(*d))),
        Some(Symbol::Standard(StandardSymbol::Square { s })) => Ok(Drawn::Pen(Pen::Square(*s))),
        _ => {
            let shape = table_symbol(features, index, lib)
                .as_ref()
                .map_err(Clone::clone)?;
            Ok(Drawn::Shape(
                shape
                    .parts
                    .iter()
                    .filter_map(|p| match &p.piece {
                        Piece::Region(r) => Some(r.clone()),
                        _ => None,
                    })
                    .collect(),
            ))
        }
    }
}

fn swept(
    path: Edge,
    drawn: Drawn,
    outline: Option<crate::geom::Region>,
    polarity: Polarity,
) -> Shape {
    let piece = match drawn {
        Drawn::Pen(pen) => match outline {
            Some(r) => Piece::Region(r),
            None => Piece::Stroke(Stroke {
                path: vec![path],
                pen,
            }),
        },
        Drawn::Shape(pen) if pen.is_empty() => return Shape::default(),
        Drawn::Shape(pen) => Piece::Swept { path, pen },
    };
    Shape {
        parts: vec![Part { polarity, piece }],
        ..Shape::default()
    }
}

fn build(features: &Features, index: usize, lib: &Library) -> Result<Shape, ShapeError> {
    let f = &features.features()[index];
    let shape = match &f.kind {
        FeatureKind::Line { a, b, symbol } => {
            let d = drawn(features, *symbol, lib)?;
            let outline = match d {
                Drawn::Pen(pen) => line_outline(*a, *b, pen),
                Drawn::Shape(_) => None,
            };
            swept(Edge::line(*a, *b), d, outline, f.polarity)
        }
        FeatureKind::Arc { arc, symbol } => {
            let d = drawn(features, *symbol, lib)?;
            let outline = match d {
                Drawn::Pen(pen) => arc_outline(arc, pen)?,
                Drawn::Shape(_) => None,
            };
            swept(Edge::Arc(*arc), d, outline, f.polarity)
        }
        FeatureKind::Pad { symbol, place } => table_symbol(features, *symbol, lib)
            .as_ref()
            .map_err(Clone::clone)?
            .placed(place, f.polarity),
        FeatureKind::Surface { regions } => Shape::regions(f.polarity, regions.iter().cloned()),
        FeatureKind::Text(t) => {
            let font = lib
                .font(&t.font)
                .ok_or_else(|| ShapeError::UnknownFont(t.font.clone()))?;
            let mut s = text_shape(t, font)?;
            for p in &mut s.parts {
                p.polarity = p.polarity.times(f.polarity);
            }
            s
        }
        FeatureKind::Barcode(_) => Shape {
            skipped: true,
            ..Shape::default()
        },
    };
    if let Some(b) = shape.bounds()
        && !(b.min.in_range() && b.max.in_range())
    {
        return Err(ShapeError::OutOfRange);
    }
    Ok(shape)
}

/// Feature `index`'s shape, built on first use and kept by its table.
pub fn feature_shape<'a>(
    features: &'a Features,
    index: usize,
    lib: &Library,
) -> &'a Result<Shape, ShapeError> {
    features.shapes[index].get_or_init(|| build(features, index, lib))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Arc, Dir, Mdeg, Place, Point};
    use crate::model::{Barcode, Feature, Text, UserSymbol};

    fn table(symbols: Vec<Symbol>, kinds: Vec<(FeatureKind, Polarity)>) -> Features {
        Features::new(
            symbols,
            kinds.into_iter().map(|(k, p)| Feature::new(k, p)).collect(),
        )
    }

    fn round(d: i64) -> Symbol {
        Symbol::Standard(StandardSymbol::Round { d })
    }

    #[test]
    fn pads_place_their_symbol() {
        let f = table(
            vec![Symbol::Standard(StandardSymbol::Rect {
                w: 2000,
                h: 1000,
                corners: crate::symbol::Corners::SHARP,
            })],
            vec![(
                FeatureKind::Pad {
                    symbol: 0,
                    place: Place {
                        at: Point::new(5000, 0),
                        rot: Mdeg::new(90_000),
                        mirror: false,
                        scale: None,
                    },
                },
                Polarity::Negative,
            )],
        );
        let s = feature_shape(&f, 0, &Library::default()).as_ref().unwrap();
        assert_eq!(s.parts[0].polarity, Polarity::Negative);
        let b = s.bounds().unwrap();
        assert_eq!(
            (b.min, b.max),
            (Point::new(4500, -1000), Point::new(5500, 1000))
        );
    }

    #[test]
    fn user_symbols_nest_and_multiply_polarity() {
        let inner = UserSymbol {
            name: "hole".into(),
            features: table(
                vec![round(100)],
                vec![(
                    FeatureKind::Pad {
                        symbol: 0,
                        place: Place::IDENTITY,
                    },
                    Polarity::Negative,
                )],
            ),
        };
        let lib = Library::new(vec![inner], vec![]);
        let f = table(
            vec![Symbol::User("hole".into())],
            vec![(
                FeatureKind::Pad {
                    symbol: 0,
                    place: Place::at(Point::new(10, 10)),
                },
                Polarity::Negative,
            )],
        );
        let s = feature_shape(&f, 0, &lib).as_ref().unwrap();
        assert_eq!(s.parts.len(), 1);
        assert_eq!(s.parts[0].polarity, Polarity::Positive);
    }

    #[test]
    fn cycles_and_unknown_names_have_no_shape() {
        let a = UserSymbol {
            name: "a".into(),
            features: table(
                vec![Symbol::User("a".into())],
                vec![(
                    FeatureKind::Pad {
                        symbol: 0,
                        place: Place::IDENTITY,
                    },
                    Polarity::Positive,
                )],
            ),
        };
        let lib = Library::new(vec![a], vec![]);
        let f = table(
            vec![
                Symbol::User("a".into()),
                Symbol::User("b".into()),
                Symbol::Unknown("zz9".into()),
            ],
            (0..4)
                .map(|i| {
                    (
                        FeatureKind::Pad {
                            symbol: i,
                            place: Place::IDENTITY,
                        },
                        Polarity::Positive,
                    )
                })
                .collect(),
        );
        assert_eq!(
            feature_shape(&f, 0, &lib),
            &Err(ShapeError::SymbolCycle(vec!["a".into()]))
        );
        assert_eq!(
            feature_shape(&f, 1, &lib),
            &Err(ShapeError::UnknownSymbol("b".into()))
        );
        assert_eq!(
            feature_shape(&f, 2, &lib),
            &Err(ShapeError::UnknownSymbol("zz9".into()))
        );
        assert!(matches!(
            feature_shape(&f, 3, &lib),
            Err(ShapeError::UnknownSymbol(_))
        ));
    }

    #[test]
    fn lines_and_arcs_by_their_pens() {
        let rect = Symbol::Standard(StandardSymbol::Rect {
            w: 200,
            h: 100,
            corners: crate::symbol::Corners::SHARP,
        });
        let arc = Arc {
            start: Point::new(1000, 0),
            end: Point::new(0, 1000),
            centre: Point::ORIGIN,
            dir: Dir::Ccw,
        };
        let f = table(
            vec![
                round(200),
                rect,
                round(5000),
                Symbol::Standard(StandardSymbol::Square { s: 200 }),
            ],
            vec![
                (
                    FeatureKind::Line {
                        a: Point::new(0, 0),
                        b: Point::new(1000, 0),
                        symbol: 0,
                    },
                    Polarity::Positive,
                ),
                (
                    FeatureKind::Line {
                        a: Point::new(0, 0),
                        b: Point::new(1000, 0),
                        symbol: 1,
                    },
                    Polarity::Positive,
                ),
                (FeatureKind::Arc { arc, symbol: 2 }, Polarity::Positive),
                (FeatureKind::Arc { arc, symbol: 3 }, Polarity::Positive),
            ],
        );
        let lib = Library::default();
        let piece = |i| {
            feature_shape(&f, i, &lib).as_ref().unwrap().parts[0]
                .piece
                .clone()
        };
        assert!(matches!(piece(0), Piece::Region(_)));
        assert!(matches!(piece(1), Piece::Swept { .. }));
        assert!(matches!(piece(2), Piece::Stroke(_)));
        assert!(matches!(piece(3), Piece::Stroke(_)));
    }

    #[test]
    fn text_barcodes_and_range() {
        let text = Text {
            font: "standard".into(),
            place: Place::IDENTITY,
            width: 1000,
            height: 1000,
            line_width: 100,
            string: "A".into(),
        };
        let far = crate::geom::COORD_LIMIT;
        let f = table(
            vec![round(10_000)],
            vec![
                (FeatureKind::Text(text), Polarity::Positive),
                (
                    FeatureKind::Barcode(Barcode {
                        place: Place::IDENTITY,
                        kind: "UPC39".into(),
                        string: "1".into(),
                    }),
                    Polarity::Positive,
                ),
                (
                    FeatureKind::Pad {
                        symbol: 0,
                        place: Place::at(Point::new(far, 0)),
                    },
                    Polarity::Positive,
                ),
            ],
        );
        let lib = Library::default();
        assert_eq!(
            feature_shape(&f, 0, &lib),
            &Err(ShapeError::UnknownFont("standard".into()))
        );
        assert!(feature_shape(&f, 1, &lib).as_ref().unwrap().skipped);
        assert_eq!(feature_shape(&f, 2, &lib), &Err(ShapeError::OutOfRange));
    }
}
