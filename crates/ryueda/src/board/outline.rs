//! A board's outline and cutouts as `ryu-board` contours, and whether they
//! are sound: closed, simple, in range, every cutout inside the outline
//! and apart from the others.
//!
//! Straight contours are judged exactly (integer orientation tests). A
//! contour with arcs is judged on its ring flattened at
//! [`FLATTEN_NM`]: the explicit tolerance rule, so a contour within that
//! of crossing itself is refused, never accepted.

use super::file::{Board, Contour as FileContour, Op};
use crate::error::Code;
use crate::fault::Fault;
use ryu_board::geom::{Arc, COORD_LIMIT, Contour, Edge, Point, Region};
use ryu_board::model::Polarity;
use ryu_board::shape::Shape;
use ryu_copper::poly::{self, FillRule, FlattenOptions, Rings, flatten_shape};

/// How finely a contour with arcs is flattened to be judged, in nm.
pub const FLATTEN_NM: i64 = 10;

fn pt(p: [i64; 2]) -> Point {
    Point::new(p[0], p[1])
}

/// Why a file's contour isn't a `ryu-board` contour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bad {
    OutOfRange,
    Invalid(&'static str),
}

/// The file's contour as `ryu-board` edges, or why it can't be one.
pub fn contour(c: &FileContour) -> Result<Contour, Bad> {
    if c.points()
        .iter()
        .any(|p| p[0].abs() > COORD_LIMIT || p[1].abs() > COORD_LIMIT)
    {
        return Err(Bad::OutOfRange);
    }
    let mut at = pt(c.start);
    let mut edges = Vec::with_capacity(c.edges.len());
    for e in &c.edges {
        let edge = match e {
            Op::Line { to } => {
                if pt(*to) == at {
                    return Err(Bad::Invalid("an edge of no length"));
                }
                Edge::line(at, pt(*to))
            }
            Op::Arc { mid, to } => match Arc::through(at, pt(*mid), pt(*to)) {
                Some(a) => Edge::Arc(a),
                None => return Err(Bad::Invalid("an arc through three points in a line")),
            },
        };
        at = edge.end();
        edges.push(edge);
    }
    if edges.is_empty() {
        return Err(Bad::Invalid("no edges"));
    }
    if at != pt(c.start) {
        return Err(Bad::Invalid(
            "not closed: the last edge doesn't end at the start",
        ));
    }
    Contour::new(edges).map_err(|_| Bad::Invalid("not closed"))
}

/// The contour's ring: its corners when it is straight, else its
/// flattening at [`FLATTEN_NM`].
fn ring(c: &Contour) -> Vec<Point> {
    if c.edges().iter().all(|e| matches!(e, Edge::Segment(_))) {
        return c.edges().iter().map(Edge::start).collect();
    }
    let shape = Shape::regions(Polarity::Positive, [Region::solid(ccw(c.clone()))]);
    flatten_shape(&shape, &FlattenOptions::new(FLATTEN_NM))
        .into_iter()
        .flat_map(|(_, r)| r.rings)
        .next()
        .unwrap_or_default()
}

/// Whether a ring is simple: [`crate::footprint::simple`]'s exact tests.
fn simple(r: &[Point]) -> bool {
    let pts: Vec<[i64; 2]> = r.iter().map(|p| [p.x, p.y]).collect();
    crate::footprint::simple(&pts)
}

fn polygons(r: &[Point]) -> poly::Polygons {
    poly::simplify(&Rings::new(vec![r.to_vec()], FLATTEN_NM), FillRule::NonZero)
}

/// A contour counterclockwise, as a region's outer contour must be.
pub fn ccw(c: Contour) -> Contour {
    if c.area2() < 0.0 { c.reversed() } else { c }
}

/// What is wrong with the board's outline and cutouts, each fault with
/// its JSON pointer.
pub fn judge(b: &Board) -> Vec<Fault> {
    let mut out = Vec::new();
    let fault = |code: Code, ptr: &str, why: &str| Fault::error(code, why).with("pointer", ptr);
    let mut judged: Vec<(String, Vec<Point>)> = Vec::new();
    let all = std::iter::once(("/outline".to_owned(), &b.outline)).chain(
        b.cutouts
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("/cutouts/{i}"), c)),
    );
    for (ptr, c) in all {
        match contour(c) {
            Err(Bad::OutOfRange) => {
                out.push(fault(Code::BoardOutOfRange, &ptr, "a point past ±2^40 nm"))
            }
            Err(Bad::Invalid(why)) => out.push(fault(Code::BoardOutlineInvalid, &ptr, why)),
            Ok(k) => {
                let r = ring(&k);
                if r.len() < 3 || !simple(&r) {
                    out.push(fault(
                        Code::BoardOutlineInvalid,
                        &ptr,
                        "the contour crosses or touches itself",
                    ));
                } else {
                    judged.push((ptr, r));
                }
            }
        }
    }
    // Cutouts against the outline and each other, when all are sound.
    if out.is_empty() && judged.len() > 1 {
        let outline = polygons(&judged[0].1);
        let cuts: Vec<poly::Polygons> = judged[1..].iter().map(|(_, r)| polygons(r)).collect();
        for i in 1..judged.len() {
            let c = &cuts[i - 1];
            if !poly::difference(c, &outline).is_empty() {
                out.push(fault(
                    Code::BoardOutlineInvalid,
                    &judged[i].0,
                    "a cutout reaches outside the outline",
                ));
            } else if touches(&judged[0].1, &judged[i].1) {
                out.push(fault(
                    Code::BoardOutlineInvalid,
                    &judged[i].0,
                    "a cutout touches the outline",
                ));
            }
            for j in (i + 1)..judged.len() {
                let d = &cuts[j - 1];
                if !poly::intersection(c, d).is_empty() || touches(&judged[i].1, &judged[j].1) {
                    out.push(fault(
                        Code::BoardOutlineInvalid,
                        &judged[j].0,
                        &format!("a cutout meets the one at {}", judged[i].0),
                    ));
                }
            }
        }
    }
    out
}

/// Whether two rings share a point: any two of their edges meet (exact).
fn touches(a: &[Point], b: &[Point]) -> bool {
    let (n, m) = (a.len(), b.len());
    let p = |q: Point| [q.x, q.y];
    (0..n).any(|i| {
        (0..m).any(|j| {
            crate::footprint::segments_meet(p(a[i]), p(a[(i + 1) % n]), p(b[j]), p(b[(j + 1) % m]))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;

    fn board(outline: FileContour, cutouts: Vec<FileContour>) -> Board {
        Board {
            name: "b".into(),
            design: Id::parse("d", "design").unwrap(),
            preset: "jlc-2l".into(),
            outline,
            cutouts,
            stackup: None,
        }
    }

    fn square(x: i64, y: i64, s: i64) -> FileContour {
        FileContour {
            start: [x, y],
            edges: vec![
                Op::Line { to: [x + s, y] },
                Op::Line { to: [x + s, y + s] },
                Op::Line { to: [x, y + s] },
                Op::Line { to: [x, y] },
            ],
        }
    }

    fn codes(b: &Board) -> Vec<(Code, String)> {
        judge(b)
            .into_iter()
            .map(|f| {
                (
                    f.error.code,
                    f.error.details["pointer"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn a_rectangle_with_a_cutout_is_sound() {
        let b = board(
            FileContour::rect(20_000_000, 10_000_000),
            vec![square(1_000_000, 1_000_000, 2_000_000)],
        );
        assert!(judge(&b).is_empty(), "{:?}", judge(&b));
    }

    #[test]
    fn open_crossed_and_far_contours_are_faults() {
        let mut open = FileContour::rect(10, 10);
        open.edges.pop();
        assert_eq!(
            codes(&board(open, vec![])),
            vec![(Code::BoardOutlineInvalid, "/outline".into())]
        );
        let bow = FileContour {
            start: [0, 0],
            edges: vec![
                Op::Line { to: [10, 10] },
                Op::Line { to: [10, 0] },
                Op::Line { to: [0, 10] },
                Op::Line { to: [0, 0] },
            ],
        };
        assert_eq!(
            codes(&board(bow, vec![])),
            vec![(Code::BoardOutlineInvalid, "/outline".into())]
        );
        let far = square(0, 0, COORD_LIMIT + 1);
        assert_eq!(
            codes(&board(far, vec![])),
            vec![(Code::BoardOutOfRange, "/outline".into())]
        );
    }

    #[test]
    fn cutouts_must_be_inside_and_apart() {
        let o = FileContour::rect(10_000, 10_000);
        let out = square(9_000, 1_000, 2_000);
        assert_eq!(
            codes(&board(o.clone(), vec![out])),
            vec![(Code::BoardOutlineInvalid, "/cutouts/0".into())]
        );
        let a = square(1_000, 1_000, 2_000);
        let b = square(2_000, 2_000, 2_000);
        assert_eq!(
            codes(&board(o.clone(), vec![a.clone(), b])),
            vec![(Code::BoardOutlineInvalid, "/cutouts/1".into())]
        );
        // Touching at the edge is meeting, to the nanometre.
        let c = square(3_000, 1_000, 2_000);
        assert_eq!(codes(&board(o.clone(), vec![a.clone(), c])).len(), 1);
        // And touching the outline from inside.
        let d = square(8_000, 1_000, 2_000);
        assert_eq!(
            codes(&board(o, vec![d])),
            vec![(Code::BoardOutlineInvalid, "/cutouts/0".into())]
        );
    }

    #[test]
    fn rounded_outlines_are_judged_on_their_flattening() {
        // A 10 x 10 mm board whose right side bulges by an arc.
        let o = FileContour {
            start: [0, 0],
            edges: vec![
                Op::Line {
                    to: [10_000_000, 0],
                },
                Op::Arc {
                    mid: [12_000_000, 5_000_000],
                    to: [10_000_000, 10_000_000],
                },
                Op::Line {
                    to: [0, 10_000_000],
                },
                Op::Line { to: [0, 0] },
            ],
        };
        assert!(judge(&board(o, vec![])).is_empty());
        let collinear = FileContour {
            start: [0, 0],
            edges: vec![
                Op::Arc {
                    mid: [5, 0],
                    to: [10, 0],
                },
                Op::Line { to: [0, 0] },
            ],
        };
        assert_eq!(
            codes(&board(collinear, vec![])),
            vec![(Code::BoardOutlineInvalid, "/outline".into())]
        );
    }
}
