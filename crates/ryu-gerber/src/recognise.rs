//! Flashes recognised exactly: a region that is, edge for edge in cyclic
//! order, the circle, rectangle or obround standard aperture on its own
//! box is flashed; anything else stays a region. Recognising from the
//! geometry (rather than from what made it) keeps the writer
//! product-neutral and can never flash a shape it doesn't draw.

use ryu_board::geom::{Contour, Edge, Nm, Place, Point, Region, ceil_sqrt};
use ryu_board::shape::{Piece, symbol_shape};
use ryu_board::symbol::{Corners, StandardSymbol};

/// A standard aperture: Gerber's `C`, `R` and `O`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Aperture {
    Circle(Nm),
    Rect(Nm, Nm),
    Obround(Nm, Nm),
}

/// A standard aperture flashed at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flash {
    pub at: Point,
    pub aperture: Aperture,
}

impl Aperture {
    /// The aperture flashed at `at`, as `ryu-board` draws the standard
    /// symbol; `None` for sizes no symbol fits.
    pub fn region(self, at: Point) -> Option<Region> {
        let sym = match self {
            Aperture::Circle(d) => StandardSymbol::Round { d },
            Aperture::Rect(w, h) => StandardSymbol::Rect {
                w,
                h,
                corners: Corners::SHARP,
            },
            Aperture::Obround(w, h) => StandardSymbol::Oval { w, h },
        };
        let s = symbol_shape(&sym).ok()?;
        match s.parts.as_slice() {
            [p] => match &p.piece {
                Piece::Region(r) => Some(Place::at(at).region(r)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// `c`'s edges from the one that starts at its least point.
fn from_least(c: &Contour) -> Vec<Edge> {
    let e = c.edges();
    let first = (0..e.len()).min_by_key(|&i| e[i].start()).unwrap_or(0);
    e[first..].iter().chain(&e[..first]).copied().collect()
}

/// Whether two contours have the same edges in the same cyclic order.
/// A full circle is one closed curve whichever point it starts from, so
/// two are the same when their centres, radii and turns are.
pub fn same_contour(a: &Contour, b: &Contour) -> bool {
    if let ([Edge::Arc(x)], [Edge::Arc(y)]) = (a.edges(), b.edges())
        && x.is_full()
        && y.is_full()
    {
        return x.centre == y.centre && x.radius2() == y.radius2() && x.dir == y.dir;
    }
    from_least(a) == from_least(b)
}

/// The flash that draws exactly `r`, if one does.
pub fn flash_of(r: &Region) -> Option<Flash> {
    if !r.holes.is_empty() {
        return None;
    }
    // A circle: one full arc of integer radius, from any start, either
    // way round.
    if let [Edge::Arc(a)] = r.outer.edges()
        && a.is_full()
    {
        let r2 = a.radius2();
        let rad = ceil_sqrt(r2);
        if rad * rad != r2 || rad <= 0 {
            return None;
        }
        return Some(Flash {
            at: a.centre,
            aperture: Aperture::Circle((2 * rad) as Nm),
        });
    }
    let b = r.bounds();
    let (w, h) = (b.max.x - b.min.x, b.max.y - b.min.y);
    if w <= 0 || h <= 0 || (b.min.x + b.max.x) % 2 != 0 || (b.min.y + b.max.y) % 2 != 0 {
        return None;
    }
    let at = Point::new((b.min.x + b.max.x) / 2, (b.min.y + b.max.y) / 2);
    for aperture in [Aperture::Rect(w, h), Aperture::Obround(w, h)] {
        if let Some(c) = aperture.region(at)
            && (same_contour(&c.outer, &r.outer) || same_contour(&c.outer.reversed(), &r.outer))
        {
            return Some(Flash { at, aperture });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::Mdeg;
    use ryu_board::model::Polarity;
    use ryu_board::shape::Shape;
    use ryu_board::symbol::pad as pads;

    fn placed(sym: StandardSymbol, at: Point, quarter_turns: i64) -> Region {
        let s: Shape = symbol_shape(&sym).unwrap().placed(
            &Place {
                at,
                rot: Mdeg::new(90_000 * quarter_turns),
                mirror: false,
                scale: None,
            },
            Polarity::Positive,
        );
        match &s.parts[0].piece {
            Piece::Region(r) => r.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn standard_pads_are_flashed_at_their_centres_whichever_way_turned() {
        let at = Point::new(5_000_000, -2_000_000);
        let rect = StandardSymbol::Rect {
            w: 1_000_000,
            h: 500_000,
            corners: Corners::SHARP,
        };
        for q in 0..4 {
            let f = flash_of(&placed(rect, at, q)).unwrap();
            assert_eq!(f.at, at);
            let want = if q % 2 == 0 {
                Aperture::Rect(1_000_000, 500_000)
            } else {
                Aperture::Rect(500_000, 1_000_000)
            };
            assert_eq!(f.aperture, want, "{q} quarter turns");
        }
        let oval = StandardSymbol::Oval {
            w: 1_200_000,
            h: 600_000,
        };
        assert_eq!(
            flash_of(&placed(oval, at, 1)).unwrap().aperture,
            Aperture::Obround(600_000, 1_200_000)
        );
        assert_eq!(
            flash_of(&placed(
                StandardSymbol::Oval {
                    w: 600_000,
                    h: 1_200_000
                },
                at,
                0
            ))
            .unwrap()
            .aperture,
            Aperture::Obround(600_000, 1_200_000)
        );
        let round = StandardSymbol::Round { d: 800_000 };
        for q in 0..4 {
            let f = flash_of(&placed(round, at, q)).unwrap();
            assert_eq!((f.at, f.aperture), (at, Aperture::Circle(800_000)));
        }
        // The same contour run the other way is the same shape.
        let r = placed(rect, at, 0);
        let back = Region::solid(r.outer.reversed());
        assert_eq!(
            flash_of(&back).unwrap().aperture,
            Aperture::Rect(1_000_000, 500_000)
        );
    }

    #[test]
    fn anything_else_stays_a_region() {
        let at = Point::new(0, 0);
        // Rounded corners, a trapezoid, an odd size (no integer centre).
        assert!(flash_of(&placed(pads::roundrect(1_000_000, 600_000, 250_000), at, 0)).is_none());
        assert!(
            flash_of(&placed(
                pads::trapezoid(1_000_000, 600_000, 100_000, 0),
                at,
                0
            ))
            .is_none()
        );
        assert!(
            flash_of(&placed(
                StandardSymbol::Rect {
                    w: 1_000_001,
                    h: 500_000,
                    corners: Corners::SHARP
                },
                at,
                0
            ))
            .is_none()
        );
        // A rectangle turned by other than a quarter isn't axis-aligned.
        let s = symbol_shape(&StandardSymbol::Rect {
            w: 1_000_000,
            h: 500_000,
            corners: Corners::SHARP,
        })
        .unwrap()
        .placed(
            &Place {
                at,
                rot: Mdeg::new(30_000),
                mirror: false,
                scale: None,
            },
            Polarity::Positive,
        );
        let Piece::Region(r) = &s.parts[0].piece else {
            panic!()
        };
        assert!(flash_of(r).is_none());
    }

    #[test]
    fn apertures_draw_as_the_standard_symbols() {
        let at = Point::new(3, 4);
        for a in [
            Aperture::Circle(10),
            Aperture::Rect(10, 4),
            Aperture::Obround(4, 10),
        ] {
            let r = a.region(at).unwrap();
            assert_eq!(flash_of(&r).unwrap(), Flash { at, aperture: a });
        }
    }
}
