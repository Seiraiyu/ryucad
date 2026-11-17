//! Gerber X2 layers (Ucamco's specification, revision 2023.08):
//! millimetres at `%FSLAX46Y46*%`, so a nanometre is one unit and every
//! coordinate is written exactly; file attributes first; apertures
//! numbered from D10 in first use, each with its `.AperFunction`;
//! standard apertures flashed where a region is exactly one; other
//! regions with true arcs (`G75`); round-pen strokes drawn; each item's
//! object attributes set before it and deleted after. No dates: the same
//! layer gives the same bytes.

use crate::fmt::{coord, field, mm6};
use crate::recognise::{Aperture, flash_of};
use crate::{Error, Software};
use ryu_board::geom::{Contour, Dir, Edge, Nm, Pen, Point, Region, Stroke};
use ryu_board::model::Polarity;
use ryu_board::shape::{Piece, Shape, line_outline};
use std::fmt::Write as _;

/// One layer to write.
#[derive(Clone, Debug, Default)]
pub struct Layer {
    /// `.FileFunction`'s value as X2 writes it: `Copper,L1,Top`,
    /// `Soldermask,Bot`, `Profile,NP`.
    pub function: String,
    /// `.FilePolarity`: `Negative` for a mask, whose image is its
    /// openings.
    pub negative: bool,
    pub items: Vec<Item>,
}

/// One object of a layer and what X2 says of it.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub shape: Shape,
    /// `.AperFunction`: `SMDPad,CuDef`, `ComponentPad`, `ViaPad`,
    /// `Conductor`, `Profile`.
    pub function: Option<String>,
    /// `.N`: its net.
    pub net: Option<String>,
    /// `.P`: its component's reference and its pad's number.
    pub pad: Option<(String, String)>,
    /// `.C`: its component's reference.
    pub component: Option<String>,
}

/// What a piece is written as.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Obj {
    Flash(Point, Aperture),
    Draw(Vec<Edge>, Nm),
    Region(Contour),
}

impl Obj {
    fn template(&self) -> Option<Aperture> {
        match self {
            Obj::Flash(_, a) => Some(*a),
            Obj::Draw(_, w) => Some(Aperture::Circle(*w)),
            Obj::Region(_) => None,
        }
    }
}

/// An item's pieces as objects, each dark (`true`) or clear.
fn objects(item: &Item) -> Result<Vec<(bool, Obj)>, Error> {
    let mut out = Vec::new();
    for part in &item.shape.parts {
        let dark = part.polarity == Polarity::Positive;
        match &part.piece {
            Piece::Region(r) => {
                if !r.holes.is_empty() {
                    return Err(Error::Holes);
                }
                out.push((
                    dark,
                    match flash_of(r) {
                        Some(f) => Obj::Flash(f.at, f.aperture),
                        None => Obj::Region(r.outer.clone()),
                    },
                ));
            }
            Piece::Stroke(Stroke {
                path,
                pen: Pen::Round(w),
            }) => {
                if *w <= 0 {
                    return Err(Error::Unsupported("a pen of no width"));
                }
                out.push((dark, Obj::Draw(path.clone(), *w)));
            }
            Piece::Stroke(Stroke {
                path,
                pen: pen @ Pen::Square(_),
            }) => {
                for e in path {
                    let Edge::Segment(s) = e else {
                        return Err(Error::Unsupported("a square pen along an arc"));
                    };
                    let r: Region = line_outline(s.a, s.b, *pen)
                        .ok_or(Error::Unsupported("a pen of no width"))?;
                    out.push((
                        dark,
                        match flash_of(&r) {
                            Some(f) => Obj::Flash(f.at, f.aperture),
                            None => Obj::Region(r.outer),
                        },
                    ));
                }
            }
            Piece::Swept { .. } => return Err(Error::Unsupported("a swept pen")),
        }
    }
    Ok(out)
}

fn aperture_text(a: Aperture) -> String {
    match a {
        Aperture::Circle(d) => format!("C,{}", mm6(d)),
        Aperture::Rect(w, h) => format!("R,{}X{}", mm6(w), mm6(h)),
        Aperture::Obround(w, h) => format!("O,{}X{}", mm6(w), mm6(h)),
    }
}

/// The object attributes an item sets, in a fixed order.
fn object_attributes(i: &Item) -> Vec<String> {
    let mut out = Vec::new();
    if let Some((c, p)) = &i.pad {
        out.push(format!("%TO.P,{},{}*%", field(c), field(p)));
    }
    if let Some(n) = &i.net {
        out.push(format!("%TO.N,{}*%", field(n)));
    }
    if let Some(c) = &i.component {
        out.push(format!("%TO.C,{}*%", field(c)));
    }
    out
}

/// What the writer has set so far: the aperture, the interpolation mode
/// and the polarity.
struct State {
    aperture: Option<usize>,
    mode: Option<u8>,
    dark: bool,
}

impl State {
    fn select(&mut self, w: &mut String, d: usize) {
        if self.aperture != Some(d) {
            writeln!(w, "D{d}*").unwrap();
            self.aperture = Some(d);
        }
    }

    fn mode(&mut self, w: &mut String, m: u8) {
        if self.mode != Some(m) {
            writeln!(w, "G0{m}*").unwrap();
            self.mode = Some(m);
        }
    }

    /// A path from a `D02` at its start; another `D02` where an edge
    /// doesn't start where the last ended. In a region (`region`) that
    /// would start a second contour, so it is refused there.
    fn path(&mut self, w: &mut String, edges: &[Edge], region: bool) -> Result<(), Error> {
        let mut at: Option<Point> = None;
        for e in edges {
            let s = e.start();
            if at != Some(s) {
                if region && at.is_some() {
                    return Err(Error::Unsupported("a full circle within a contour"));
                }
                writeln!(w, "X{}Y{}D02*", coord(s.x, s)?, coord(s.y, s)?).unwrap();
            }
            match e {
                Edge::Segment(g) => {
                    self.mode(w, 1);
                    writeln!(w, "X{}Y{}D01*", coord(g.b.x, g.b)?, coord(g.b.y, g.b)?).unwrap();
                    at = Some(g.b);
                }
                Edge::Arc(a) => {
                    self.mode(w, if a.dir == Dir::Cw { 2 } else { 3 });
                    // A full circle ends where it starts (ryu-board lets
                    // its end lie anywhere on the start's ray).
                    let end = if a.is_full() { a.start } else { a.end };
                    let (i, j) = (a.centre.x - a.start.x, a.centre.y - a.start.y);
                    writeln!(
                        w,
                        "X{}Y{}I{}J{}D01*",
                        coord(end.x, end)?,
                        coord(end.y, end)?,
                        coord(i, a.centre)?,
                        coord(j, a.centre)?
                    )
                    .unwrap();
                    at = Some(end);
                }
            }
        }
        Ok(())
    }
}

/// The layer as a Gerber X2 file.
pub fn write(layer: &Layer, sw: &Software) -> Result<String, Error> {
    let items: Vec<(&Item, Vec<(bool, Obj)>)> = layer
        .items
        .iter()
        .map(|i| Ok((i, objects(i)?)))
        .collect::<Result<_, Error>>()?;
    // Apertures by template and function, D10 up in first use.
    let mut apertures: Vec<(Aperture, Option<&str>)> = Vec::new();
    for (item, objs) in &items {
        for (_, o) in objs {
            if let Some(t) = o.template() {
                let key = (t, item.function.as_deref());
                if !apertures.contains(&key) {
                    apertures.push(key);
                }
            }
        }
    }
    let number = |t: Aperture, f: Option<&str>| -> usize {
        10 + apertures
            .iter()
            .position(|k| *k == (t, f))
            .expect("every template was collected")
    };
    let mut out = String::new();
    let w = &mut out;
    writeln!(
        w,
        "%TF.GenerationSoftware,{},{},{}*%",
        field(&sw.vendor),
        field(&sw.application),
        field(&sw.version)
    )
    .unwrap();
    writeln!(w, "%TF.SameCoordinates,Original*%").unwrap();
    // The function's commas are the attribute's own field separators.
    writeln!(w, "%TF.FileFunction,{}*%", layer.function).unwrap();
    writeln!(
        w,
        "%TF.FilePolarity,{}*%",
        if layer.negative {
            "Negative"
        } else {
            "Positive"
        }
    )
    .unwrap();
    writeln!(w, "%FSLAX46Y46*%").unwrap();
    writeln!(w, "%MOMM*%").unwrap();
    writeln!(w, "%LPD*%").unwrap();
    writeln!(w, "G75*").unwrap();
    for (i, (t, f)) in apertures.iter().enumerate() {
        if let Some(f) = f {
            writeln!(w, "%TA.AperFunction,{f}*%").unwrap();
        }
        writeln!(w, "%ADD{}{}*%", 10 + i, aperture_text(*t)).unwrap();
        if f.is_some() {
            writeln!(w, "%TD.AperFunction*%").unwrap();
        }
    }
    let mut st = State {
        aperture: None,
        mode: None,
        dark: true,
    };
    for (item, objs) in &items {
        let attrs = object_attributes(item);
        for a in &attrs {
            writeln!(w, "{a}").unwrap();
        }
        for (dark, o) in objs {
            if *dark != st.dark {
                writeln!(w, "%LP{}*%", if *dark { "D" } else { "C" }).unwrap();
                st.dark = *dark;
            }
            match o {
                Obj::Flash(at, ap) => {
                    st.select(w, number(*ap, item.function.as_deref()));
                    writeln!(w, "X{}Y{}D03*", coord(at.x, *at)?, coord(at.y, *at)?).unwrap();
                }
                Obj::Draw(path, width) => {
                    st.select(
                        w,
                        number(Aperture::Circle(*width), item.function.as_deref()),
                    );
                    st.path(w, path, false)?;
                }
                Obj::Region(c) => {
                    if let Some(f) = &item.function {
                        writeln!(w, "%TA.AperFunction,{f}*%").unwrap();
                    }
                    writeln!(w, "G36*").unwrap();
                    st.path(w, c.edges(), true)?;
                    writeln!(w, "G37*").unwrap();
                    if item.function.is_some() {
                        writeln!(w, "%TD.AperFunction*%").unwrap();
                    }
                }
            }
        }
        if !attrs.is_empty() {
            writeln!(w, "%TD*%").unwrap();
        }
    }
    writeln!(w, "M02*").unwrap();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmt::LIMIT;
    use ryu_board::geom::Arc;
    use ryu_board::shape::{Part, circle, symbol_shape};
    use ryu_board::symbol::{Corners, StandardSymbol};

    fn sw() -> Software {
        Software {
            vendor: "Seiraiyu".into(),
            application: "test".into(),
            version: "1.0.0".into(),
        }
    }

    fn rect_at(x: Nm, y: Nm) -> Shape {
        symbol_shape(&StandardSymbol::Rect {
            w: 1_000_000,
            h: 500_000,
            corners: Corners::SHARP,
        })
        .unwrap()
        .placed(
            &ryu_board::geom::Place::at(Point::new(x, y)),
            Polarity::Positive,
        )
    }

    #[test]
    fn a_layer_has_its_attributes_apertures_and_objects() {
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
        let layer = Layer {
            function: "Copper,L1,Top".into(),
            negative: false,
            items: vec![
                Item {
                    shape: rect_at(5_000_000, 2_000_000),
                    function: Some("SMDPad,CuDef".into()),
                    net: Some("rail:gnd".into()),
                    pad: Some(("R1".into(), "1".into())),
                    component: Some("R1".into()),
                },
                Item {
                    shape: track,
                    function: Some("Conductor".into()),
                    net: Some("sda".into()),
                    ..Item::default()
                },
            ],
        };
        let text = write(&layer, &sw()).unwrap();
        let want_head = "%TF.GenerationSoftware,Seiraiyu,test,1.0.0*%\n\
                         %TF.SameCoordinates,Original*%\n\
                         %TF.FileFunction,Copper,L1,Top*%\n\
                         %TF.FilePolarity,Positive*%\n\
                         %FSLAX46Y46*%\n\
                         %MOMM*%\n\
                         %LPD*%\n\
                         G75*\n\
                         %TA.AperFunction,SMDPad,CuDef*%\n\
                         %ADD10R,1.000000X0.500000*%\n\
                         %TD.AperFunction*%\n\
                         %TA.AperFunction,Conductor*%\n\
                         %ADD11C,0.250000*%\n\
                         %TD.AperFunction*%\n";
        assert!(text.starts_with(want_head), "{text}");
        assert!(
            text.contains(
                "%TO.P,R1,1*%\n%TO.N,rail:gnd*%\n%TO.C,R1*%\nD10*\nX5000000Y2000000D03*\n%TD*%\n"
            ),
            "{text}"
        );
        assert!(text.contains(
            "%TO.N,sda*%\nD11*\nX0Y0D02*\nG01*\nX1000000Y0D01*\nG03*\nX2000000Y1000000I0J1000000D01*\n%TD*%\n"
        ), "{text}");
        assert!(text.ends_with("M02*\n"));
        assert!(!text.contains("CreationDate"));
        // The same layer, the same bytes.
        assert_eq!(write(&layer, &sw()).unwrap(), text);
    }

    #[test]
    fn regions_clear_parts_and_masks() {
        let round = symbol_shape(&ryu_board::symbol::pad::roundrect(
            1_000_000, 600_000, 250_000,
        ))
        .unwrap();
        let mut hole = Shape::regions(
            Polarity::Negative,
            [Region::solid(circle(Point::new(0, 0), 100_000).unwrap())],
        );
        let mut shape = round.clone();
        shape.parts.append(&mut hole.parts);
        let layer = Layer {
            function: "Soldermask,Top".into(),
            negative: true,
            items: vec![Item {
                shape,
                ..Item::default()
            }],
        };
        let text = write(&layer, &sw()).unwrap();
        assert!(text.contains("%TF.FilePolarity,Negative*%"));
        // A rounded rectangle is a region with arcs; the disc a clear flash.
        assert!(text.contains("G36*\n") && text.contains("G37*\n"), "{text}");
        assert!(text.contains("G03*\n"), "{text}");
        assert!(text.contains("%LPC*%\nD10*\nX0Y0D03*\n"), "{text}");
        assert!(text.contains("%ADD10C,0.200000*%"), "{text}");
    }

    #[test]
    fn what_isnt_written_is_refused() {
        let ring = Shape::regions(
            Polarity::Positive,
            [Region::ring(
                circle(Point::new(0, 0), 1_000).unwrap(),
                circle(Point::new(0, 0), 500).unwrap(),
            )],
        );
        let one = |shape: Shape| Layer {
            function: "Legend,Top".into(),
            negative: false,
            items: vec![Item {
                shape,
                ..Item::default()
            }],
        };
        assert_eq!(write(&one(ring), &sw()), Err(Error::Holes));
        let far = rect_at(LIMIT + 1, 0);
        assert!(matches!(write(&one(far), &sw()), Err(Error::OutOfRange(_))));
        let no_width = Shape {
            parts: vec![Part {
                polarity: Polarity::Positive,
                piece: Piece::Stroke(Stroke {
                    path: vec![Edge::line(Point::new(0, 0), Point::new(10, 0))],
                    pen: Pen::Round(0),
                }),
            }],
            ..Shape::default()
        };
        assert!(matches!(
            write(&one(no_width), &sw()),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn square_pens_become_their_hulls() {
        let s = Shape {
            parts: vec![Part {
                polarity: Polarity::Positive,
                piece: Piece::Stroke(Stroke {
                    path: vec![Edge::line(Point::new(0, 0), Point::new(1_000, 0))],
                    pen: Pen::Square(100),
                }),
            }],
            ..Shape::default()
        };
        let layer = Layer {
            function: "Legend,Top".into(),
            negative: false,
            items: vec![Item {
                shape: s,
                ..Item::default()
            }],
        };
        let text = write(&layer, &sw()).unwrap();
        // The hull of two squares is a rectangle: flashed when the hull
        // has only its four corners, else a region (Task 7 reads either
        // back as the hull).
        assert!(
            text.contains("%ADD10R,0.001100X0.000100*%") || text.contains("G36*"),
            "{text}"
        );
    }
}
