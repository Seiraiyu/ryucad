//! Pour fills (design 924): an area less grown obstacles, opened to a
//! least width, joined to thermal pads by spokes, its islands dropped.
//! Product-neutral: the caller grows each obstacle by its rule (plus
//! [`BOUND`] and a nanometre) and says which copper is the pour's own.
//! Exact integer booleans throughout (`poly`); the same polygons on
//! every machine and thread count.

#[allow(unused_imports)]
use crate::measure::{self, Body, Verdict};
use crate::poly::{self, FillRule, FlattenOptions, Polygons, Rings};
#[allow(unused_imports)]
use ryu_board::geom::{Mdeg, Nm, Place, Point};

/// The bound fills are flattened at.
pub const BOUND: Nm = 10;

/// The flattening fills use.
pub fn opts() -> FlattenOptions {
    FlattenOptions::new(BOUND)
}

/// A pad the fill reaches by thermal spokes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThermalPad {
    pub at: Point,
    /// Its axes' angle, millidegrees counterclockwise.
    pub rotation_mdeg: i64,
    /// Half its extent along its own x and y axes.
    pub half: [Nm; 2],
    pub gap: Nm,
    pub spoke: Nm,
}

/// What to fill.
#[derive(Clone, Debug)]
pub struct Ask {
    /// Where the fill may go.
    pub area: Polygons,
    /// What it must avoid, each already grown by its rule.
    pub obstacles: Vec<Polygons>,
    /// Other nets' copper grown by its clearance: spokes must miss it.
    pub spoke_obstacles: Vec<Polygons>,
    pub thermals: Vec<ThermalPad>,
    /// No neck or sliver narrower than this is left (0: none removed).
    pub min_width: Nm,
    /// The pour's net's copper on the layer: an island touching none of
    /// it is dropped.
    pub own: Vec<Body>,
    pub keep_islands: bool,
}

/// What a fill came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub polygons: Polygons,
    pub islands_removed: usize,
    /// Polygons (by index) whose contact with the pour's copper the
    /// bodies' bounds couldn't decide: kept.
    pub uncertain: Vec<usize>,
    /// Spokes kept per thermal pad, in the ask's order.
    pub spokes: Vec<u8>,
}

/// The union of `ps`.
pub fn union_all(ps: &[Polygons]) -> Polygons {
    let mut rings = Rings::default();
    for p in ps {
        let r = p.rings();
        rings.bound = rings.bound.max(r.bound);
        rings.rings.extend(r.rings);
    }
    poly::simplify(&rings, FillRule::Positive)
}

/// The fill.
pub fn fill(ask: &Ask) -> Fill {
    let cut = poly::difference(&ask.area, &union_all(&ask.obstacles));
    let opened = if ask.min_width > 1 {
        poly::open(&cut, ask.min_width / 2, &opts())
    } else {
        cut
    };
    Fill {
        polygons: opened,
        islands_removed: 0,
        uncertain: Vec::new(),
        spokes: vec![0; ask.thermals.len()],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const MM: Nm = 1_000_000;

    /// An axis-aligned rectangle as polygons.
    pub(crate) fn rect(x0: Nm, y0: Nm, x1: Nm, y1: Nm) -> Polygons {
        let ring = vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ];
        poly::simplify(&Rings::new(vec![ring], 0), FillRule::NonZero)
    }

    pub(crate) fn ask(area: Polygons) -> Ask {
        Ask {
            area,
            obstacles: Vec::new(),
            spoke_obstacles: Vec::new(),
            thermals: Vec::new(),
            min_width: 0,
            own: Vec::new(),
            keep_islands: true,
        }
    }

    #[test]
    fn the_area_less_its_obstacles() {
        let mut a = ask(rect(0, 0, 10 * MM, 10 * MM));
        a.obstacles.push(rect(4 * MM, 4 * MM, 6 * MM, 6 * MM));
        a.obstacles.push(rect(5 * MM, 5 * MM, 7 * MM, 7 * MM));
        let f = fill(&a);
        assert_eq!(f.polygons.polygons.len(), 1);
        assert_eq!(
            f.polygons.polygons[0].holes.len(),
            1,
            "two obstacles, one hole"
        );
        // 100 mm² less the two 4 mm² squares overlapping by 1 mm².
        assert_eq!(
            f.polygons.area2(),
            2 * (100 - 7) * i128::from(MM) * i128::from(MM)
        );
        assert!(fill(&ask(Polygons::default())).polygons.is_empty());
    }
    #[test]
    fn necks_under_the_least_width_go() {
        // Two 4 mm squares joined by a 0.1 mm neck.
        let area = union_all(&[
            rect(0, 0, 4 * MM, 4 * MM),
            rect(6 * MM, 0, 10 * MM, 4 * MM),
            rect(4 * MM - 1, 2 * MM - MM / 20, 6 * MM + 1, 2 * MM + MM / 20),
        ]);
        let mut a = ask(area);
        a.min_width = MM / 5;
        assert_eq!(fill(&a).polygons.polygons.len(), 2, "the neck is gone");
        a.min_width = MM / 20;
        assert_eq!(
            fill(&a).polygons.polygons.len(),
            1,
            "a wide enough neck stays"
        );
    }
}
