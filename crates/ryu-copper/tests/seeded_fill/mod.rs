//! Seeded fill asks, shared by the fill properties and the golden: a
//! 20 mm square, 1 to 12 rectangles and discs of other nets, 0 to 2
//! thermal pads of the pour's net, a clearance from 0.05 mm to 0.5 mm
//! and a least width from 0 to 0.3 mm.

#![allow(dead_code)] // each test file uses a part

use ryu_board::geom::{Nm, Point};
use ryu_board::testing::Lcg;
use ryu_copper::fill::{self, Ask, BOUND, ThermalPad};
use ryu_copper::poly::{self, FillRule, Polygons, Rings};

pub const MM: Nm = 1_000_000;

/// An axis-aligned rectangle.
pub fn rect(x0: Nm, y0: Nm, x1: Nm, y1: Nm) -> Polygons {
    let ring = vec![
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ];
    poly::simplify(&Rings::new(vec![ring], 0), FillRule::NonZero)
}

/// A seeded ask, and the other nets' copper as drawn (ungrown).
pub struct Seeded {
    pub ask: Ask,
    pub others: Vec<Polygons>,
    pub clearance: Nm,
}

pub fn seeded(seed: u64) -> Seeded {
    let mut g = Lcg::new(0x6669_6c6c ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let side = 20 * MM;
    let clearance = g.range(MM / 20, MM / 2);
    let min_width = g.range(0, 3 * MM / 10);
    let mut others = Vec::new();
    for _ in 0..g.range(1, 12) {
        let (x, y) = (g.range(0, side), g.range(0, side));
        if g.coin() {
            let (w, h) = (g.range(MM / 10, 4 * MM), g.range(MM / 10, 4 * MM));
            others.push(rect(x, y, (x + w).min(side), (y + h).min(side)));
        } else {
            // A disc: a 2 nm square grown round.
            let r = g.range(MM / 10, 2 * MM);
            others.push(poly::grow(
                &rect(x - 1, y - 1, x + 1, y + 1),
                r,
                &fill::opts(),
            ));
        }
    }
    let grow = clearance + BOUND + 1;
    let grown: Vec<Polygons> = others
        .iter()
        .map(|o| poly::grow(o, grow, &fill::opts()))
        .collect();
    let mut ask = Ask {
        area: rect(0, 0, side, side),
        obstacles: grown.clone(),
        spoke_obstacles: grown,
        thermals: Vec::new(),
        min_width,
        own: Vec::new(),
        keep_islands: g.coin(),
    };
    for _ in 0..g.range(0, 2) {
        let at = Point::new(
            g.range(2 * MM, side - 2 * MM),
            g.range(2 * MM, side - 2 * MM),
        );
        let half = [g.range(MM / 5, MM), g.range(MM / 5, MM)];
        let pad = rect(
            at.x - half[0],
            at.y - half[1],
            at.x + half[0],
            at.y + half[1],
        );
        let t = ThermalPad {
            at,
            rotation_mdeg: 0,
            half,
            gap: clearance.max(MM / 2),
            spoke: min_width.max(MM / 2),
        };
        ask.obstacles.push(poly::grow(&pad, t.gap, &fill::opts()));
        ask.own.push(pad.body());
        ask.thermals.push(t);
    }
    // A via of the pour's net somewhere.
    let v = Point::new(g.range(0, side), g.range(0, side));
    ask.own
        .push(rect(v.x - MM / 4, v.y - MM / 4, v.x + MM / 4, v.y + MM / 4).body());
    Seeded {
        ask,
        others,
        clearance,
    }
}
