//! Certified winding numbers of closed chains of plane curves.
//!
//! A point's winding number about a loop is a sum of the angles its pieces
//! subtend, each piece subdivided until its bound excludes the point.

use super::Vector2;
use super::chain2::{Chain2, Piece2};
use super::intersect2::SLACK_2D;
use crate::math;
use core::f64::consts::TAU;

/// How many times a piece may be halved before its winding contribution
/// counts as uncertain.
const MAX_DEPTH: u32 = 100;

/// The explicit rule for rounding a winding number: the angle sum over
/// `2π` must be within this of an integer.
const WINDING_SLACK: f64 = 0.25;

/// The winding number of a loop about `p`, which must lie off it, or
/// `None` when it can't be certified. The gaps between one segment's end
/// and the next one's start are closed by straight lines.
pub(crate) fn winding(p: Vector2, chains: &[Chain2]) -> Option<i64> {
    let mut sum = 0.0;
    for (i, c) in chains.iter().enumerate() {
        for piece in &c.pieces {
            sum += subtended(p, piece, MAX_DEPTH)?;
        }
        let next = &chains[(i + 1) % chains.len()];
        sum += angle(p, c.end(), next.start());
    }
    let turns = sum / TAU;
    let w = turns.round();
    ((turns - w).abs() <= WINDING_SLACK).then_some(w as i64)
}

/// The signed angle `piece` subtends at `p`: once its grown hull box
/// excludes `p`, the piece lies in a half-plane away from `p`, so the angle
/// between its ends is the angle it sweeps.
fn subtended(p: Vector2, piece: &Piece2, depth: u32) -> Option<f64> {
    let (lo, hi) = piece.hull();
    let grow = Vector2::new(SLACK_2D, SLACK_2D);
    let (lo, hi) = (lo - grow, hi + grow);
    if !(lo.x <= p.x && p.x <= hi.x && lo.y <= p.y && p.y <= hi.y) {
        let first = piece.pts[0].p;
        let last = piece.pts[piece.pts.len() - 1].p;
        return Some(angle(p, first, last));
    }
    if depth == 0 {
        return None;
    }
    let [a, b] = piece.split(0.5);
    Some(subtended(p, &a, depth - 1)? + subtended(p, &b, depth - 1)?)
}

/// The signed angle from `a` to `b` seen from `p`.
fn angle(p: Vector2, a: Vector2, b: Vector2) -> f64 {
    let (u, v) = (a - p, b - p);
    math::atan2(u.cross(v), u.dot(v))
}
