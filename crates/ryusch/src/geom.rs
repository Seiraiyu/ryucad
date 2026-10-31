//! Integer geometry: nanometres, y up, as ryueda's symbols are drawn.
//! Every placement, route and label sits on the 50 mil grid.

use serde::{Deserialize, Serialize};

/// A length in nanometres.
pub type Nm = i64;

/// `[x, y]` in nanometres.
pub type Point = [Nm; 2];

/// 50 mil, KiCad's and Altium's schematic grid.
pub const GRID: Nm = 1_270_000;

/// The nearest grid point to `v`, halves away from zero.
pub fn snap(v: Nm) -> Nm {
    let h = GRID / 2;
    if v >= 0 {
        (v + h) / GRID * GRID
    } else {
        -((-v + h) / GRID * GRID)
    }
}

/// Whether `p` lies on the grid.
pub fn on_grid(p: Point) -> bool {
    p[0] % GRID == 0 && p[1] % GRID == 0
}

/// An axis-aligned box, its corners included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    /// The box with corners `a` and `b`, either way round.
    pub fn new(a: Point, b: Point) -> Rect {
        Rect {
            min: [a[0].min(b[0]), a[1].min(b[1])],
            max: [a[0].max(b[0]), a[1].max(b[1])],
        }
    }

    /// The box holding every point; `None` for none.
    pub fn of(points: impl IntoIterator<Item = Point>) -> Option<Rect> {
        let mut it = points.into_iter();
        let first = it.next()?;
        Some(it.fold(Rect::new(first, first), |r, p| r.with(p)))
    }

    pub fn with(self, p: Point) -> Rect {
        Rect {
            min: [self.min[0].min(p[0]), self.min[1].min(p[1])],
            max: [self.max[0].max(p[0]), self.max[1].max(p[1])],
        }
    }

    pub fn union(self, o: Rect) -> Rect {
        self.with(o.min).with(o.max)
    }

    /// Grown by `d` every way (shrunk for a negative `d`).
    pub fn grow(self, d: Nm) -> Rect {
        Rect {
            min: [self.min[0] - d, self.min[1] - d],
            max: [self.max[0] + d, self.max[1] + d],
        }
    }

    pub fn shift(self, by: Point) -> Rect {
        Rect {
            min: [self.min[0] + by[0], self.min[1] + by[1]],
            max: [self.max[0] + by[0], self.max[1] + by[1]],
        }
    }

    pub fn width(&self) -> Nm {
        self.max[0] - self.min[0]
    }

    pub fn height(&self) -> Nm {
        self.max[1] - self.min[1]
    }

    pub fn center(&self) -> Point {
        [
            (self.min[0] + self.max[0]) / 2,
            (self.min[1] + self.max[1]) / 2,
        ]
    }

    /// Whether the boxes share any area: boxes that only touch don't.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.min[0] < o.max[0]
            && o.min[0] < self.max[0]
            && self.min[1] < o.max[1]
            && o.min[1] < self.max[1]
    }

    /// Whether `p` is inside or on the edge.
    pub fn holds(&self, p: Point) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }

    /// Whether `p` is strictly inside.
    pub fn holds_inside(&self, p: Point) -> bool {
        self.min[0] < p[0] && p[0] < self.max[0] && self.min[1] < p[1] && p[1] < self.max[1]
    }

    /// Whether `o` lies wholly inside, edges included.
    pub fn contains(&self, o: &Rect) -> bool {
        self.holds(o.min) && self.holds(o.max)
    }

    /// The area, in grid squares (for density).
    pub fn area(&self) -> i128 {
        i128::from(self.width()) * i128::from(self.height())
    }
}

/// Which way a pin points from its body to its end, or a label from its
/// anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Up, Side::Down];

    /// One grid step that way.
    pub fn step(self) -> Point {
        match self {
            Side::Left => [-GRID, 0],
            Side::Right => [GRID, 0],
            Side::Up => [0, GRID],
            Side::Down => [0, -GRID],
        }
    }

    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }

    pub fn horizontal(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }

    /// Turned a quarter counterclockwise.
    pub fn turned(self) -> Side {
        match self {
            Side::Right => Side::Up,
            Side::Up => Side::Left,
            Side::Left => Side::Down,
            Side::Down => Side::Right,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Up => "up",
            Side::Down => "down",
        }
    }
}

/// Where a part sits: its origin's place, then a counterclockwise turn
/// of 0, 90, 180 or 270 degrees, after a mirror across its own y axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Place {
    pub at: Point,
    /// Degrees: 0, 90, 180 or 270.
    pub rotation: u16,
    pub mirror: bool,
}

impl Place {
    pub fn new(at: Point, rotation: u16, mirror: bool) -> Place {
        Place {
            at,
            rotation: rotation % 360,
            mirror,
        }
    }

    /// `p` in the part's own frame, placed.
    pub fn apply(&self, p: Point) -> Point {
        let r = self.turn(p);
        [r[0] + self.at[0], r[1] + self.at[1]]
    }

    /// `p` mirrored and turned, not moved.
    pub fn turn(&self, p: Point) -> Point {
        let [x, y] = if self.mirror { [-p[0], p[1]] } else { p };
        match self.rotation {
            90 => [-y, x],
            180 => [-x, -y],
            270 => [y, -x],
            _ => [x, y],
        }
    }

    /// A side in the part's frame, placed.
    pub fn side(&self, s: Side) -> Side {
        let s = match (self.mirror, s) {
            (true, Side::Left) => Side::Right,
            (true, Side::Right) => Side::Left,
            (_, s) => s,
        };
        (0..self.rotation / 90).fold(s, |s, _| s.turned())
    }

    /// A box in the part's frame, placed.
    pub fn rect(&self, r: Rect) -> Rect {
        Rect::new(self.apply(r.min), self.apply(r.max))
    }
}

/// `text` as decimal millimetres, exactly: `1270000` is `1.27`.
pub fn mm(v: Nm) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    let (whole, frac) = (a / 1_000_000, a % 1_000_000);
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        let f = format!("{frac:06}");
        format!("{sign}{whole}.{}", f.trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_mirror_then_turn() {
        let p = Place::new([10, 20], 90, false);
        assert_eq!(p.apply([1, 0]), [10, 21]);
        assert_eq!(p.side(Side::Right), Side::Up);
        let m = Place::new([0, 0], 90, true);
        assert_eq!(m.apply([1, 0]), [0, -1]);
        assert_eq!(m.side(Side::Right), Side::Down);
        assert_eq!(Place::new([0, 0], 270, false).side(Side::Up), Side::Right);
        assert_eq!(
            Place::new([5, 5], 180, false).rect(Rect::new([0, 0], [2, 1])),
            Rect::new([3, 4], [5, 5])
        );
    }

    #[test]
    fn grids_boxes_and_millimetres() {
        assert_eq!(snap(1_900_000), GRID);
        assert_eq!(snap(-1_900_000), -GRID);
        assert_eq!(snap(635_000), GRID);
        assert!(on_grid([GRID * 3, -GRID]));
        let a = Rect::new([0, 0], [10, 10]);
        assert!(!a.overlaps(&Rect::new([10, 0], [20, 10])));
        assert!(a.overlaps(&Rect::new([9, 9], [20, 20])));
        assert!(a.holds([10, 10]) && !a.holds_inside([10, 5]));
        assert_eq!(mm(1_270_000), "1.27");
        assert_eq!(mm(-5_000_000), "-5");
        assert_eq!(mm(1), "0.000001");
    }
}
