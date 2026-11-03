//! Which pairs of faces unfolding and refolding check for crossings.
//!
//! The body given is taken to be sound, as every operation takes it: its
//! faces meet only along their shared edges. A map that carries a set of
//! faces one to one keeps that, so pairs it carries together needn't be
//! checked again. Two kinds of set are carried one to one:
//!
//! - a flange (or a group of faces refolding moves together), by its
//!   rigid motion;
//! - a strip together with the half-slabs beside its two bend lines, by
//!   the fold map. In the bend's cross-section the folded sheet is the
//!   region between a convex path (a line, an arc turning by `θ ≤ π`,
//!   a line) and its offset by `t` away from the turn's centre, which
//!   never meets itself; the near flange lies on the near line's side and
//!   the far flange on the far one's.
//!
//! A face counts as on a bend line's side when its box, in the strip's
//! flat coordinates, ends within `sqrt(2 ε ρ)` of the line (`ε` the
//! linear resolution, `ρ` the neutral radius): there the rigid motion and
//! the fold map differ by at most `ε`. Every other pair is checked.

use crate::geom::{Curve, Frame, Rigid, Surface};
use crate::intersect::face_box;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use std::collections::BTreeMap;

/// What carries a face between the flat and the folded body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Piece {
    /// A rigid motion, shared by every face of the same index.
    Rigid(usize),
    /// Strip `k`'s fold map.
    Strip(usize),
    /// Anything else: always checked.
    Other,
}

/// A strip in the flat, and the rigid pieces either side of it.
#[derive(Clone, Debug)]
pub(super) struct Fold {
    /// [`super::Strip::frame`].
    pub(super) frame: Frame,
    /// The developed length.
    pub(super) length: f64,
    /// The bend's angle (at most `π`).
    pub(super) angle: f64,
    pub(super) near: usize,
    pub(super) far: usize,
}

/// Where a face's flat box lies against a strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Near,
    Far,
    Neither,
}

/// The pairs worth checking, from each face's piece and, for each rigid
/// face beside a strip, how far it reaches across the strip.
pub(super) struct Pairs {
    pieces: BTreeMap<FaceId, Piece>,
    folds: Vec<Option<Fold>>,
    /// For a rigid face and a strip it lies beside, the least and greatest
    /// of its points' `x` in the strip's flat coordinates.
    reach: BTreeMap<(FaceId, usize), [f64; 2]>,
}

impl Pairs {
    /// `flat` is the body in the flat, where the strips' frames are.
    pub(super) fn new(
        flat: &Body,
        pieces: BTreeMap<FaceId, Piece>,
        folds: Vec<Option<Fold>>,
    ) -> Self {
        let mut reach = BTreeMap::new();
        for (&f, &piece) in &pieces {
            let Piece::Rigid(g) = piece else {
                continue;
            };
            for (k, fold) in folds.iter().enumerate() {
                if let Some(fold) = fold
                    && (fold.near == g || fold.far == g)
                {
                    reach.insert((f, k), across(flat, f, &fold.frame));
                }
            }
        }
        Self {
            pieces,
            folds,
            reach,
        }
    }

    fn side(&self, f: FaceId, k: usize) -> Side {
        let (Some(fold), Some([lo, hi])) = (&self.folds[k], self.reach.get(&(f, k))) else {
            return Side::Neither;
        };
        // The fold map is one to one only for turns up to a half (a flat
        // map read from bytes may say anything).
        if !(fold.angle > 0.0 && fold.angle <= core::f64::consts::PI && fold.length > 0.0) {
            return Side::Neither;
        }
        let rho = fold.length / fold.angle;
        let eps = (2.0 * LINEAR_RESOLUTION * rho).sqrt();
        if *hi <= eps {
            Side::Near
        } else if *lo >= fold.length - eps {
            Side::Far
        } else {
            Side::Neither
        }
    }

    /// Whether faces `a` and `b` may meet wrongly after the maps.
    pub(super) fn wanted(&self, a: FaceId, b: FaceId) -> bool {
        let piece = |f| self.pieces.get(&f).copied().unwrap_or(Piece::Other);
        match (piece(a), piece(b)) {
            (Piece::Rigid(x), Piece::Rigid(y)) if x == y => false,
            (Piece::Strip(k), Piece::Strip(l)) if k == l => false,
            (Piece::Strip(k), Piece::Rigid(g)) => !self.beside(b, g, k),
            (Piece::Rigid(g), Piece::Strip(k)) => !self.beside(a, g, k),
            (Piece::Rigid(x), Piece::Rigid(y)) => !self.folds.iter().enumerate().any(|(k, f)| {
                f.as_ref().is_some_and(|f| {
                    (f.near == x
                        && f.far == y
                        && self.side(a, k) == Side::Near
                        && self.side(b, k) == Side::Far)
                        || (f.near == y
                            && f.far == x
                            && self.side(b, k) == Side::Near
                            && self.side(a, k) == Side::Far)
                })
            }),
            _ => true,
        }
    }

    /// Whether rigid face `f` of piece `g` lies on its side of strip `k`.
    /// (A face of the strip is carried by the fold map wherever it runs:
    /// past the bend lines the fold map is the flanges' motions.)
    fn beside(&self, f: FaceId, g: usize, k: usize) -> bool {
        let Some(fold) = &self.folds[k] else {
            return false;
        };
        let side = self.side(f, k);
        (fold.near == g && side == Side::Near) || (fold.far == g && side == Side::Far)
    }
}

/// The least and greatest `x` of a face's points in `frame`. On a ruled
/// surface (a plane, a cylinder, a cone, an extrusion) `x` is linear along
/// each ruling, so it is greatest and least on the face's boundary: from
/// its edges' curves, moved into the frame, over their intervals, and its
/// lone vertices. Any other face takes its box's corners.
fn across(b: &Body, f: FaceId, frame: &Frame) -> [f64; 2] {
    let to_local = Rigid::from_frame(frame).inverse();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let ruled = matches!(
        b.surfaces[b.faces[f].surface],
        Surface::Plane(_) | Surface::Cylinder(_) | Surface::Cone(_) | Surface::Extrusion(_)
    );
    if ruled {
        for l in b.face_loops(f) {
            if let Some(v) = b.loops[l].vertex {
                let x = frame.to_local(b.vertices[v].point).x;
                lo = lo.min(x);
                hi = hi.max(x);
            }
            for fin in b.loop_fins(l) {
                let e = &b.edges[b.fins[fin].edge];
                let [a, c] = x_range(&b.curves[e.curve].transformed(&to_local), e.interval);
                let t = e.tolerance.get();
                lo = lo.min(a - t);
                hi = hi.max(c + t);
            }
        }
    } else {
        for c in face_box(b, f).corners() {
            let x = frame.to_local(c).x;
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    [lo, hi]
}

/// The least and greatest `x` along a curve over an interval: a circle's
/// arc exactly, from its ends and the angles where `x` turns (`x(t) = c +
/// a cos t + b sin t` turns at `atan2(b, a) + kπ`), grown by a part in
/// 10¹² of its size for rounding; any other curve by its box.
fn x_range(c: &Curve, [t0, t1]: [f64; 2]) -> [f64; 2] {
    let Curve::Circle(k) = c else {
        let bx = c.bounds([t0, t1]);
        return [bx.min().x, bx.max().x];
    };
    let f = k.frame();
    let (a, b) = (k.radius() * f.x().get().x, k.radius() * f.y().get().x);
    let x = |t: f64| {
        let (s, co) = math::sin_cos(t);
        f.origin().x + a * co + b * s
    };
    let (mut lo, mut hi) = (x(t0).min(x(t1)), x(t0).max(x(t1)));
    let turn = math::atan2(b, a);
    let pi = core::f64::consts::PI;
    let mut t = turn + pi * ((t0 - turn) / pi).ceil();
    while t < t1 {
        lo = lo.min(x(t));
        hi = hi.max(x(t));
        t += pi;
    }
    let slack = 1e-12 * (f.origin().x.abs() + k.radius());
    [lo - slack, hi + slack]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Vector3};
    use crate::name::OpKey;
    use crate::primitive::block;
    use core::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn an_arcs_reach_is_exact() {
        let c = Curve::Circle(Circle::new(Frame::WORLD, 2.0).unwrap());
        // A quarter from the top round to the left: x from -2 to 0.
        let [lo, hi] = x_range(&c, [FRAC_PI_2, PI]);
        assert!((lo + 2.0).abs() < 1e-11 && hi.abs() < 1e-11, "{lo} {hi}");
        // Past the right: x reaches 2 inside the interval.
        let [_, hi] = x_range(&c, [-0.5, 0.5]);
        assert!((hi - 2.0).abs() < 1e-11);
        // The whole ring's box agrees with Curve::bounds.
        let [lo, hi] = x_range(&c, [0.0, 2.0 * PI]);
        assert!((lo + 2.0).abs() < 1e-11 && (hi - 2.0).abs() < 1e-11);
    }

    #[test]
    fn pairs_a_map_carries_one_to_one_are_skipped() {
        // A block reaching from 20 mm before the near bend line to it.
        let near = block(
            OpKey(1),
            0.02,
            0.01,
            0.001,
            &Rigid::translation(Vector3::new(-0.02, 0.0, 0.0)),
        )
        .unwrap()
        .0;
        let fold = Fold {
            frame: Frame::WORLD,
            length: 0.01,
            angle: FRAC_PI_2,
            near: 0,
            far: 1,
        };
        let n: Vec<FaceId> = near.faces().collect();
        let pieces: BTreeMap<FaceId, Piece> = n.iter().map(|&f| (f, Piece::Rigid(0))).collect();
        let p = Pairs::new(&near, pieces, vec![Some(fold.clone())]);
        // One motion: never checked.
        assert!(!p.wanted(n[0], n[1]));
        // Every face of the near block ends at the near line: on its side.
        for &f in &n {
            assert_eq!(p.side(f, 0), Side::Near);
        }
        // Moved past the line by more than sqrt(2 ε ρ), a face is checked
        // against the strip.
        let past = block(
            OpKey(1),
            0.02,
            0.01,
            0.001,
            &Rigid::translation(Vector3::new(-0.0199, 0.0, 0.0)),
        )
        .unwrap()
        .0;
        let pieces: BTreeMap<FaceId, Piece> = past.faces().map(|f| (f, Piece::Rigid(0))).collect();
        let p = Pairs::new(&past, pieces, vec![Some(fold.clone())]);
        let plus_x = past
            .faces()
            .max_by(|&a, &b| {
                let x = |f| face_box(&past, f).min().x;
                x(a).total_cmp(&x(b))
            })
            .unwrap();
        assert_eq!(p.side(plus_x, 0), Side::Neither);
        // A strip face against a near face on its side: skipped; against
        // a face of another piece: checked.
        let mut pieces: BTreeMap<FaceId, Piece> = BTreeMap::new();
        pieces.insert(n[0], Piece::Rigid(0));
        pieces.insert(n[1], Piece::Strip(0));
        pieces.insert(n[2], Piece::Rigid(5));
        let p = Pairs::new(&near, pieces, vec![Some(fold.clone())]);
        assert!(!p.wanted(n[1], n[0]));
        assert!(p.wanted(n[1], n[2]));
        // Past a half turn the fold map isn't one to one: checked.
        let wide = Fold { angle: 4.0, ..fold };
        let mut pieces: BTreeMap<FaceId, Piece> = BTreeMap::new();
        pieces.insert(n[0], Piece::Rigid(0));
        pieces.insert(n[1], Piece::Strip(0));
        let p = Pairs::new(&near, pieces, vec![Some(wide)]);
        assert!(p.wanted(n[1], n[0]));
    }
}
