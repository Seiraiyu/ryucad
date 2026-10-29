//! Polygons as ODB++ writes them in surfaces, profiles and package and pin
//! outlines: `OB x y I|H`, then `OS x y` segments and `OC x y xc yc cw`
//! arcs, then `OE`, islands first and each island's holes after it.
//!
//! Islands run clockwise and holes counterclockwise in ODB++; the model
//! wants outer contours counterclockwise and holes clockwise, so each
//! closed polygon is turned that way.

use ryu_board::geom::{Arc, Contour, Dir, Edge, Point, Region};

use super::text::{Bad, Token};
use super::units::Nums;

/// An arc's written end more than this far off its circle is a warning.
pub const ARC_SLACK: f64 = 1_000.0;

/// Polygons being read.
#[derive(Clone, Debug, Default)]
pub struct Outline {
    pub regions: Vec<Region>,
    open: Option<(bool, Point, Vec<Edge>)>,
    /// Arcs whose written end is more than [`ARC_SLACK`] off their circle.
    pub inconsistent: usize,
}

fn point(t: &[Token], n: usize, nums: &mut Nums, line: u32) -> Result<Point, Bad> {
    match (t.get(n), t.get(n + 1)) {
        (Some(x), Some(y)) => nums.point(x, y, line),
        _ => Err(Bad::new(line, 1, format!("{} without a point", t[0].text))),
    }
}

/// `Y` (clockwise) or `N`, in either case.
pub fn dir(t: Option<&Token>, line: u32) -> Result<Dir, Bad> {
    match t.map(|t| t.text) {
        Some("Y") | Some("y") => Ok(Dir::Cw),
        Some("N") | Some("n") => Ok(Dir::Ccw),
        Some(other) => Err(Bad::new(
            line,
            t.unwrap().column,
            format!("direction {other}: not Y or N"),
        )),
        None => Err(Bad::new(line, 1, "direction missing")),
    }
}

impl Outline {
    /// Whether a polygon is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// An arc, noting it when its end is off its circle.
    pub fn arc(&mut self, start: Point, end: Point, centre: Point, dir: Dir) -> Arc {
        let arc = Arc {
            start,
            end,
            centre,
            dir,
        };
        if arc.end_error() > ARC_SLACK {
            self.inconsistent += 1;
        }
        arc
    }

    /// One `OB`, `OS`, `OC` or `OE` line, by its fields.
    pub fn line(&mut self, t: &[Token], nums: &mut Nums, line: u32) -> Result<(), Bad> {
        let bad = |m: &str| Bad::new(line, 1, m);
        match t.first().map_or("", |t| t.text) {
            "OB" => {
                if self.open.is_some() {
                    return Err(bad("OB inside an open polygon"));
                }
                let p = point(t, 1, nums, line)?;
                let island = match t.get(3).map(|t| t.text) {
                    Some("I") => true,
                    Some("H") => false,
                    _ => return Err(bad("OB without I or H")),
                };
                if !island && self.regions.is_empty() {
                    return Err(bad("a hole before any island"));
                }
                self.open = Some((island, p, Vec::new()));
            }
            head @ ("OS" | "OC") => {
                let Some(from) = self
                    .open
                    .as_ref()
                    .map(|(_, p, e)| e.last().map_or(*p, Edge::end))
                else {
                    return Err(bad(&format!("{head} outside a polygon")));
                };
                let to = point(t, 1, nums, line)?;
                let edge = if head == "OS" {
                    Edge::line(from, to)
                } else {
                    let centre = point(t, 3, nums, line)?;
                    let d = dir(t.get(5), line)?;
                    Edge::Arc(self.arc(from, to, centre, d))
                };
                self.open.as_mut().unwrap().2.push(edge);
            }
            "OE" => {
                let Some((island, start, edges)) = self.open.take() else {
                    return Err(bad("OE without OB"));
                };
                if edges.last().map_or(start, Edge::end) != start {
                    return Err(bad("a polygon that doesn't close"));
                }
                let c = Contour::new(edges).map_err(|_| bad("a polygon with no edges"))?;
                let want = if island { Dir::Ccw } else { Dir::Cw };
                let c = if c.orientation() == want {
                    c
                } else {
                    c.reversed()
                };
                if island {
                    self.regions.push(Region::solid(c));
                } else {
                    self.regions.last_mut().unwrap().holes.push(c);
                }
            }
            other => return Err(bad(&format!("{other}: not OB, OS, OC or OE"))),
        }
        Ok(())
    }

    /// The regions, once every polygon has closed.
    pub fn finish(self, line: u32) -> Result<Vec<Region>, Bad> {
        if self.open.is_some() {
            return Err(Bad::new(line, 1, "a polygon without OE"));
        }
        Ok(self.regions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odb::text::tokens;
    use crate::odb::units::Units;

    fn read(lines: &str) -> Result<Outline, Bad> {
        let mut o = Outline::default();
        let mut nums = Nums::new(Units::Inch);
        for (i, l) in lines.lines().enumerate() {
            o.line(&tokens(l).unwrap(), &mut nums, i as u32 + 1)?;
        }
        Ok(o)
    }

    #[test]
    fn islands_and_holes_turn_the_models_way() {
        let o = read("OB 0 0 I\nOS 0 1\nOS 1 1\nOS 1 0\nOS 0 0\nOE\nOB 0.25 0.25 H\nOS 0.75 0.25\nOS 0.75 0.75\nOS 0.25 0.75\nOS 0.25 0.25\nOE\nOB 2 0 I\nOC 2 0 2.5 0 Y\nOE").unwrap();
        let r = o.finish(13).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].outer.orientation(), Dir::Ccw);
        assert_eq!(r[0].holes[0].orientation(), Dir::Cw);
        let inch = 25_400_000f64;
        assert!((r[0].area2() - 2.0 * 0.75 * inch * inch).abs() < 1.0);
        let Edge::Arc(a) = r[1].outer.edges()[0] else {
            panic!()
        };
        assert!(a.is_full());
    }

    #[test]
    fn faults() {
        let m = |t: &str| read(t).unwrap_err().message;
        assert_eq!(
            m("OB 0 0 I\nOS 1 0\nOS 1 1\nOE"),
            "a polygon that doesn't close"
        );
        assert_eq!(m("OB 0 0 H"), "a hole before any island");
        assert_eq!(m("OS 1 1"), "OS outside a polygon");
        assert_eq!(m("OE"), "OE without OB");
        assert_eq!(m("OB 0 0 I\nOB 0 0 I"), "OB inside an open polygon");
        assert_eq!(m("OB 0 0 X"), "OB without I or H");
        assert_eq!(m("OB 0 0 I\nOC 1 1 0 1 Q"), "direction Q: not Y or N");
        assert_eq!(m("OB 0"), "OB without a point");
        assert_eq!(
            read("OB 0 0 I").unwrap().finish(2).unwrap_err().message,
            "a polygon without OE"
        );
        // An end 1.27 µm off the unit circle, which the next arc starts
        // from, so its radius is off by as much at its own end.
        let o = read("OB 1 0 I\nOC -1 0.01 0 0 N\nOC 1 0 0 0 N\nOE").unwrap();
        assert_eq!(o.inconsistent, 2);
        let o = read("OB 1 0 I\nOC -1 0.00003 0 0 N\nOC 1 0 0 0 N\nOE").unwrap();
        assert_eq!(o.inconsistent, 0);
    }
}
