//! Boards drawn by hand for the copper and DRC tests: rectangular pads,
//! tracks, vias and holes where a test puts them, on a rectangular board.

use super::file::{CopperLayer, Track};
use super::shapes::{self, BoardShapes, HoleInfo, HoleOf, PadInfo, TrackInfo, ViaInfo};
use crate::id::Id;
use ryu_board::geom::{Contour, Edge, Pen, Point, Region, Stroke};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape};
use ryu_copper::layout::HoleShape;

pub const MM: i64 = 1_000_000;
pub const UM: i64 = 1_000;

/// An axis-aligned rectangle `w` by `h` centred on `at`.
pub fn rect(at: Point, w: i64, h: i64) -> Shape {
    let (x0, y0, x1, y1) = (at.x - w / 2, at.y - h / 2, at.x + w / 2, at.y + h / 2);
    let p = [
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ];
    let c = Contour::new((0..4).map(|i| Edge::line(p[i], p[(i + 1) % 4])).collect()).unwrap();
    Shape::regions(Polarity::Positive, [Region::solid(c)])
}

/// A board `w` by `h` with its bottom left at the origin, board `b`.
pub fn board(w: i64, h: i64) -> BoardShapes {
    let outline = rect(Point::new(w / 2, h / 2), w, h);
    let mut edges = Shape::default();
    if let Piece::Region(r) = &outline.parts[0].piece {
        edges.parts.push(Part {
            polarity: Polarity::Positive,
            piece: Piece::Stroke(Stroke {
                path: r.outer.edges().to_vec(),
                pen: Pen::Round(0),
            }),
        });
    }
    BoardShapes {
        outline,
        edges,
        copper_layers: 2,
        ..BoardShapes::default()
    }
}

fn id(s: &str, what: &str) -> Id {
    Id::parse(s, what).unwrap()
}

impl BoardShapes {
    /// A surface pad on top, its mask opening the pad itself.
    pub fn smd(
        &mut self,
        occ: &str,
        pad: &str,
        at: (i64, i64),
        size: (i64, i64),
        net: Option<&str>,
    ) -> &mut Self {
        let copper = rect(Point::new(at.0, at.1), size.0, size.1);
        self.pads.push(PadInfo {
            reference: format!("b:{occ}/{pad}"),
            occurrence: occ.into(),
            number: Some(pad.into()),
            pad: pad.into(),
            at: Point::new(at.0, at.1),
            rotation_mdeg: 0,
            size: [size.0, size.1],
            top: true,
            bottom: false,
            inner: false,
            net: net.map(Into::into),
            mask: [Some(copper.clone()), None],
            copper,
        });
        self
    }

    /// A square through pad on both sides with a plated round drill.
    pub fn through(
        &mut self,
        occ: &str,
        pad: &str,
        at: (i64, i64),
        side: i64,
        drill: i64,
        net: Option<&str>,
    ) -> &mut Self {
        let copper = rect(Point::new(at.0, at.1), side, side);
        self.pads.push(PadInfo {
            reference: format!("b:{occ}/{pad}"),
            occurrence: occ.into(),
            number: Some(pad.into()),
            pad: pad.into(),
            at: Point::new(at.0, at.1),
            rotation_mdeg: 0,
            size: [side, side],
            top: true,
            bottom: true,
            inner: true,
            net: net.map(Into::into),
            mask: [Some(copper.clone()), Some(copper.clone())],
            copper,
        });
        self.holes.push(HoleInfo {
            owner: format!("b:{occ}/{pad}"),
            shape: HoleShape::Round {
                at: Point::new(at.0, at.1),
                d: drill,
            },
            plated: true,
            of: HoleOf::Pad {
                occ: occ.into(),
                pad: pad.into(),
            },
            span: (0, self.n() - 1),
        });
        self
    }

    pub fn track(
        &mut self,
        tid: &str,
        net: &str,
        layer: CopperLayer,
        width: i64,
        pts: &[(i64, i64)],
    ) -> &mut Self {
        let t = Track {
            net: net.into(),
            layer,
            width_nm: width,
            points: pts.iter().map(|p| [p.0, p.1]).collect(),
        };
        self.tracks.push(TrackInfo {
            id: id(tid, "track"),
            net: net.into(),
            layer,
            width_nm: width,
            shape: shapes::track_shape(&t),
        });
        self
    }

    pub fn via(
        &mut self,
        vid: &str,
        net: &str,
        at: (i64, i64),
        drill: i64,
        diameter: i64,
    ) -> &mut Self {
        let p = Point::new(at.0, at.1);
        self.vias.push(ViaInfo {
            id: id(vid, "via"),
            from: 0,
            to: self.n() - 1,
            net: net.into(),
            at: p,
            drill_nm: drill,
            diameter_nm: diameter,
            shape: shapes::disc(p, diameter),
        });
        self.holes.push(HoleInfo {
            owner: format!("b:via/{vid}"),
            shape: HoleShape::Round { at: p, d: drill },
            plated: true,
            of: HoleOf::Via(id(vid, "via")),
            span: (0, self.n() - 1),
        });
        self
    }

    pub fn hole(&mut self, hid: &str, at: (i64, i64), drill: i64) -> &mut Self {
        self.holes.push(HoleInfo {
            owner: format!("b:hole/{hid}"),
            shape: HoleShape::Round {
                at: Point::new(at.0, at.1),
                d: drill,
            },
            plated: false,
            of: HoleOf::Hole(id(hid, "hole")),
            span: (0, self.n() - 1),
        });
        self
    }

    /// The same board on `n` copper layers: through pads', holes' and
    /// vias' spans made whole again (call it before adding blind vias).
    pub fn layers(&mut self, n: usize) -> &mut Self {
        self.copper_layers = n;
        for h in &mut self.holes {
            h.span = (0, n - 1);
        }
        for v in &mut self.vias {
            v.from = 0;
            v.to = n - 1;
        }
        self
    }

    /// A via over layers `span.0..=span.1`.
    pub fn blind(
        &mut self,
        vid: &str,
        net: &str,
        at: (i64, i64),
        drill: i64,
        diameter: i64,
        span: (usize, usize),
    ) -> &mut Self {
        self.via(vid, net, at, drill, diameter);
        let v = self.vias.last_mut().unwrap();
        v.from = span.0;
        v.to = span.1;
        self.holes.last_mut().unwrap().span = span;
        self
    }

    /// A rectangular courtyard `size` centred on `at`.
    pub fn courtyard(
        &mut self,
        occ: &str,
        at: (i64, i64),
        size: (i64, i64),
        open: bool,
    ) -> &mut Self {
        self.courtyards.push(super::shapes::CourtyardInfo {
            owner: format!("b:{occ}"),
            occurrence: occ.into(),
            shape: rect(Point::new(at.0, at.1), size.0, size.1),
            open,
            bottom: false,
        });
        self
    }

    /// A part's designator `refdes`, 1 mm high, centred on `at`, on
    /// `layer` (`silk.top` or `silk.bottom`).
    pub fn designator(&mut self, refdes: &str, at: (i64, i64), layer: &str) -> &mut Self {
        let shape = super::text::centred(refdes, [at.0, at.1], 0, [MM, MM], 150 * UM).unwrap();
        self.designators.push(super::shapes::DesignatorInfo {
            owner: format!("b:{refdes}"),
            refdes: refdes.into(),
            layer: layer.into(),
            shape,
        });
        self
    }
}
