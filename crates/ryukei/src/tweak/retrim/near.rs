//! Near boxes (design 669, plan 20.B): the part of a rung's box each
//! working face can need. A working face is carried over its own box and
//! the boxes of the changed faces it touches (shares a vertex with, or
//! is), grown by those faces' reach alone (the same rule as `region_box`,
//! for one face's neighbourhood). Applied in narrowed builds only, so
//! first builds keep their bits.
//!
//! Plan 20.B's Decision 1 grew a near box by the reach of the gaps it
//! touched (changed faces joined where they share a vertex). But
//! `delete_faces` grows each edge-connected set of deleted faces in its
//! own re-trim, so a delete's changed faces are always one gap and that
//! reach is the body's: the near box was the rung's box. The coxswain
//! chose the touched faces' reach instead (2026-11-15).
//!
//! Pinned faces (design 683, plan 21.A): in a narrowed build, a region
//! face left as it is that touches no change is built as its input face,
//! never carried (`pinned`).
//!
//! Adjusted near boxes (design 724, plan 22.A): after a narrowed build's
//! walk fails `Clipped` at a working face, the rung may be built again
//! with that face's near box and its working neighbours' doubled
//! (`room`, `adjusted`). First builds are never adjusted.
//!
//! Faces kept off pins (design 724, plan 22.B): after a narrowed build
//! refuses `Uncertain` at a working face and a pin it shares no edge or
//! vertex with, the rung may be built again with the working face's near
//! box cut back short of the pin (`kept_off`, `cut_short`). A face is
//! adjusted one way only.
//!
//! Released pins (design 919, plan 23.A): after a narrowed build's walk
//! fails at a pinned face, the rung may be built again with that pin
//! released, a working face again, its near box its own joined with its
//! working edge-neighbours' (`released_box`, `near_of`).

use super::region::{Region, face_vertices};
use crate::geom::Box3;
use crate::intersect::face_box;
use crate::topo::{Body, FaceId};
use crate::tweak::engine::Changes;
use std::collections::{BTreeMap, BTreeSet};

/// The changed faces of `region` that working face `f` touches: `f`
/// itself, and those sharing a vertex with it.
pub(crate) fn touched(body: &Body, region: &Region, f: FaceId) -> BTreeSet<FaceId> {
    let mut out = BTreeSet::new();
    if region.changed.contains(&f) {
        out.insert(f);
    }
    for v in face_vertices(body, f) {
        for e in body.vertex_edges(v) {
            for fin in body.edge_fins(e) {
                let g = body.loops[body.fins[fin].loop_].face;
                if region.changed.contains(&g) {
                    out.insert(g);
                }
            }
        }
    }
    out
}

/// The region faces a narrowed build pins (design 683, plan 21.A): each
/// face left as it is (`still`), standing for no other (`absorbing`),
/// that touches no change ([`touched`] is empty). A pin round a vertex
/// where the region less the pins would pinch stays working, until none
/// does: the input region has none, so each new one is at a pin's vertex.
pub(crate) fn pinned(
    body: &Body,
    region: &Region,
    still: &dyn Fn(FaceId) -> bool,
    absorbing: &BTreeSet<FaceId>,
) -> BTreeSet<FaceId> {
    pinned_but(body, region, still, absorbing, &BTreeSet::new())
}

/// [`pinned`] with the faces `released` never pinned (design 919, plan
/// 23.A); a pin round a pinch the release opens stays working too
/// ([`unpinched`]). [`pinned`]'s, bit for bit, where `released` is empty.
pub(crate) fn pinned_but(
    body: &Body,
    region: &Region,
    still: &dyn Fn(FaceId) -> bool,
    absorbing: &BTreeSet<FaceId>,
    released: &BTreeSet<FaceId>,
) -> BTreeSet<FaceId> {
    let pins: BTreeSet<FaceId> = region
        .faces
        .iter()
        .copied()
        .filter(|&f| {
            still(f)
                && !absorbing.contains(&f)
                && !released.contains(&f)
                && touched(body, region, f).is_empty()
        })
        .collect();
    unpinched(body, region, pins)
}

/// `pins` less each pin round a vertex where the region less them would
/// pinch, until none does (plan 21.A's rule: the working body is a sheet,
/// whose boundary can't touch itself at a vertex).
pub(crate) fn unpinched(
    body: &Body,
    region: &Region,
    mut pins: BTreeSet<FaceId>,
) -> BTreeSet<FaceId> {
    loop {
        if pins.is_empty() {
            return pins;
        }
        let rest: BTreeSet<FaceId> = region.faces.difference(&pins).copied().collect();
        let before = pins.len();
        for v in super::region::pinches(body, &rest) {
            for g in super::region::vertex_faces(body, v) {
                pins.remove(&g);
            }
        }
        if pins.len() == before {
            return pins;
        }
    }
}

/// The faces `adjusts` releases (plan 23.A).
pub(crate) fn released(adjusts: &Adjusts) -> BTreeSet<FaceId> {
    adjusts
        .iter()
        .filter(|(_, a)| **a == Adjust::Released)
        .map(|(&f, _)| f)
        .collect()
}

/// The faces build `which` pins: none in a first build, so it is as
/// before, bit for bit; [`pinned_but`] less the faces `released` in a
/// narrowed one.
pub(crate) fn pins(
    body: &Body,
    region: &Region,
    still: &dyn Fn(FaceId) -> bool,
    absorbing: &BTreeSet<FaceId>,
    which: super::Build,
    released: &BTreeSet<FaceId>,
) -> BTreeSet<FaceId> {
    if !which.narrowed() {
        return BTreeSet::new();
    }
    pinned_but(body, region, still, absorbing, released)
}

/// Working face `f`'s own box: its box joined with the boxes of the
/// changed faces it touches ([`touched`]). Its near box is this grown by
/// their reach ([`near_box`]); a cut keeping it off a pin must leave this
/// inside (design 724, plan 22.B).
pub(crate) fn own_box(body: &Body, region: &Region, f: FaceId) -> Box3 {
    touched(body, region, f)
        .iter()
        .map(|&g| face_box(body, g))
        .fold(face_box(body, f), |a, b| a.union(&b))
}

/// Working face `f`'s near box at `rung` (design 669, plan 20.B): `f`'s
/// box joined with the boxes of the changed faces it touches
/// ([`touched`]), grown by their reach (`reach_of` over them alone;
/// `reach`'s floor, ten resolutions, where it touches none), doubled
/// about its middle at rung 1 as the rung's box is, and kept inside the
/// rung's box `x` (already doubled at rung 1); `x` itself where they
/// don't meet.
pub(crate) fn near_box(
    body: &Body,
    changes: &Changes,
    region: &Region,
    f: FaceId,
    x: &Box3,
    rung: usize,
) -> Box3 {
    let near = touched(body, region, f);
    let b = own_box(body, region, f);
    let r = super::reach_of(body, changes, &|g| near.contains(&g));
    let mut n = b.grow(r);
    if rung == 1 {
        n = super::body::doubled(&n);
    }
    within(&n, x).unwrap_or(*x)
}

/// A working face's near box changed after a narrowed build failed
/// (design 724).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Adjust {
    /// Room (plan 22.A): the near box doubled about its middle this many
    /// times, each inside the rung's box.
    Room(u32),
    /// Kept off a pin (plan 22.B): the near box cut back to this box, its
    /// corners as bits (low x, y, z, then high), so it compares exactly.
    Off([u64; 6]),
    /// Released (design 919, plan 23.A): a pin a narrowed build's walk
    /// failed at, built as a working face again. Its near box is
    /// [`released_box`] ([`near_of`]); it is never pinned again in the
    /// rung, given room or kept off a pin.
    Released,
}

impl Adjust {
    /// `Off` for box `b`.
    pub(crate) fn off(b: &Box3) -> Adjust {
        let (l, h) = (b.min(), b.max());
        Adjust::Off([l.x, l.y, l.z, h.x, h.y, h.z].map(f64::to_bits))
    }
}

/// The box an `Off` holds.
fn unbits(c: &[u64; 6]) -> Box3 {
    use crate::geom::Point3;
    let v = c.map(f64::from_bits);
    Box3::new(Point3::new(v[0], v[1], v[2]), Point3::new(v[3], v[4], v[5]))
}

/// Each working face's adjustment in an adjusted build; empty in a
/// rung's first narrowed build, which is today's, bit for bit.
pub(crate) type Adjusts = BTreeMap<FaceId, Adjust>;

/// Near box `n`, inside the rung's box `x`, as `adjust` leaves it: as it
/// is with none; with `Room(k)`, doubled about its middle and kept inside
/// `x`, `k` times (`body::doubled`, then [`within`], as rung 1 does for
/// every face); with `Off`, the stored box kept inside `x`; with
/// `Released`, as it is: [`near_of`] gives a released face its box.
pub(crate) fn adjusted(n: &Box3, x: &Box3, adjust: Option<Adjust>) -> Box3 {
    match adjust {
        None => *n,
        Some(Adjust::Room(k)) => (0..k).fold(*n, |b, _| {
            within(&super::body::doubled(&b), x).unwrap_or(*x)
        }),
        Some(Adjust::Off(c)) => {
            let b = unbits(&c);
            within(&b, x).unwrap_or(b)
        }
        Some(Adjust::Released) => *n,
    }
}

/// Room for the working faces `clipped` (design 724, plan 22.A): those
/// whose piece a narrowed build's walk found clipped, or whose clipped
/// side that piece reached. `adjusts` with each of them and every face of
/// `working` sharing a vertex with one in the input given one more
/// doubling. `near` gives a face's near box before any adjustment. `None`
/// where none of their near boxes would grow (each already the rung's
/// box `x`): that build would repeat the last.
/// `None` too where any of them is kept off a pin (a face is adjusted
/// one way only).
pub(crate) fn room(
    body: &Body,
    working: &BTreeSet<FaceId>,
    clipped: &BTreeSet<FaceId>,
    x: &Box3,
    adjusts: &Adjusts,
    near: &dyn Fn(FaceId) -> Box3,
) -> Option<Adjusts> {
    let mut faces = clipped.clone();
    for &f in clipped {
        for v in face_vertices(body, f) {
            faces.extend(
                super::region::vertex_faces(body, v)
                    .into_iter()
                    .filter(|g| working.contains(g)),
            );
        }
    }
    let mut out = adjusts.clone();
    let mut grows = false;
    for g in faces {
        let k = match adjusts.get(&g) {
            Some(Adjust::Room(k)) => *k,
            // A face is adjusted one way only (design 724): room asked
            // for a face kept off a pin ends the adjustments.
            Some(Adjust::Off(_)) => return None,
            // A released face's box follows its neighbours' (plan 23.A):
            // it is given no room of its own.
            Some(Adjust::Released) => continue,
            None => 0,
        };
        let n = near(g);
        let now = adjusted(&n, x, adjusts.get(&g).copied());
        grows |= adjusted(&n, x, Some(Adjust::Room(k + 1))) != now;
        out.insert(g, Adjust::Room(k + 1));
    }
    grows.then_some(out)
}

/// One coordinate of `p`: x, y or z for `k` 0, 1 or 2.
fn axis(p: crate::geom::Point3, k: usize) -> f64 {
    [p.x, p.y, p.z][k]
}

/// `p` with its `k` coordinate set to `v`.
fn with_axis(p: crate::geom::Point3, k: usize, v: f64) -> crate::geom::Point3 {
    let mut c = [p.x, p.y, p.z];
    c[k] = v;
    crate::geom::Point3::new(c[0], c[1], c[2])
}

/// Near box `n` cut back on one side so the sheet carried over it ends
/// short of pin box `pin` by `gap` (design 724, plan 22.B). `sheet` is
/// the box of the sheet carried over `n` in the build that failed: a
/// sheet's parameter rect is grown past its near box's corners, so each
/// side is also cut back by that side's overshoot. A side is valid where
/// the cut leaves `must` (the face's [`own_box`]) inside and the box not
/// empty. Of the valid sides, the one removing the least length along
/// its own axis; the first on a tie, in the order x low, x high, y low,
/// y high, z low, z high. `None` where no side is valid: the pin's box
/// lies within what the face must keep on every axis.
pub(crate) fn cut_short(n: &Box3, sheet: &Box3, must: &Box3, pin: &Box3, gap: f64) -> Option<Box3> {
    let mut best: Option<(f64, Box3)> = None;
    let mut take = |loss: f64, b: Box3| {
        if best.as_ref().is_none_or(|(l, _)| loss < *l) {
            best = Some((loss, b));
        }
    };
    for k in 0..3 {
        let (nl, nh) = (axis(n.min(), k), axis(n.max(), k));
        let (sl, sh) = (axis(sheet.min(), k), axis(sheet.max(), k));
        let (ml, mh) = (axis(must.min(), k), axis(must.max(), k));
        let (pl, ph) = (axis(pin.min(), k), axis(pin.max(), k));
        // The low side raised past the pin's high side.
        let lo = ph + gap + (nl - sl).max(0.0);
        if lo > nl && lo <= ml && lo < nh {
            take(lo - nl, Box3::new(with_axis(n.min(), k, lo), n.max()));
        }
        // The high side lowered short of the pin's low side.
        let hi = pl - gap - (sh - nh).max(0.0);
        if hi < nh && hi >= mh && hi > nl {
            take(nh - hi, Box3::new(n.min(), with_axis(n.max(), k, hi)));
        }
    }
    best.map(|(_, b)| b)
}

/// Working face `f` kept off a pin (design 724, plan 22.B): `adjusts`
/// with `f`'s near box, `now` as adjusted so far, cut back by
/// [`cut_short`] (`sheet`, `must`, `pin` and `gap` as there). `None`
/// where no side can be cut. `None` too where `f` was given room.
pub(crate) fn kept_off(
    f: FaceId,
    now: &Box3,
    sheet: &Box3,
    must: &Box3,
    pin: &Box3,
    gap: f64,
    adjusts: &Adjusts,
) -> Option<Adjusts> {
    // A face is adjusted one way only (design 724).
    if matches!(adjusts.get(&f), Some(Adjust::Room(_))) {
        return None;
    }
    let cut = cut_short(now, sheet, must, pin, gap)?;
    let mut out = adjusts.clone();
    out.insert(f, Adjust::off(&cut));
    Some(out)
}

/// Where boxes `a` and `x` overlap, or `None`.
fn within(a: &Box3, x: &Box3) -> Option<Box3> {
    use crate::geom::Point3;
    let lo = Point3::new(
        a.min().x.max(x.min().x),
        a.min().y.max(x.min().y),
        a.min().z.max(x.min().z),
    );
    let hi = Point3::new(
        a.max().x.min(x.max().x),
        a.max().y.min(x.max().y),
        a.max().z.min(x.max().z),
    );
    (lo.x <= hi.x && lo.y <= hi.y && lo.z <= hi.z).then(|| Box3::new(lo, hi))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::region::region;
    use super::super::tests::block;
    use super::*;
    use crate::blend::{BlendKind, EdgeBlend, blend};
    use crate::geom::Surface;
    use crate::name::OpKey;

    /// The test block with every edge rounded 0.05, and its two corner
    /// balls nearest the origin and farthest from it.
    pub(crate) fn rounded() -> (Body, FaceId, FaceId) {
        let b = block();
        let edges: Vec<EdgeBlend> = b
            .edges()
            .map(|e| EdgeBlend {
                edge: b.edges[e].name,
                kind: BlendKind::Fillet { radius: 0.05 },
            })
            .collect();
        let (r, _) = blend(OpKey(0x20b0), &b, &edges).unwrap();
        let balls: Vec<FaceId> = r
            .faces()
            .filter(|&f| matches!(r.surfaces[r.faces[f].surface], Surface::Sphere(_)))
            .collect();
        assert_eq!(balls.len(), 8);
        let key = |f: &FaceId| {
            let m = crate::intersect::face_box(&r, *f).min();
            m.x + m.y + m.z
        };
        let low = *balls
            .iter()
            .min_by(|a, b| key(a).total_cmp(&key(b)))
            .unwrap();
        let high = *balls
            .iter()
            .max_by(|a, b| key(a).total_cmp(&key(b)))
            .unwrap();
        (r, low, high)
    }

    #[test]
    fn reach_of_every_face_is_the_bodys_reach() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        assert_eq!(
            super::super::reach_of(&r, &changes, &|_| true).to_bits(),
            super::super::reach(&r, &changes).to_bits()
        );
        // One ball alone: as wide as its box.
        let own = crate::intersect::face_box(&r, low);
        assert_eq!(
            super::super::reach_of(&r, &changes, &|f| f == low).to_bits(),
            own.min().distance(own.max()).to_bits()
        );
    }

    #[test]
    fn a_face_at_one_end_of_a_long_set_is_carried_past_that_end_only() {
        // One connected set spanning the block, as `delete_faces` re-trims
        // it: every blend face off the x = 0 end, and the low corner's
        // ball. The x = 0 side touches only that ball.
        let (r, low, _) = rounded();
        let blendy = |f: FaceId| !matches!(r.surfaces[r.faces[f].surface], Surface::Plane(_));
        let deleted: BTreeSet<FaceId> = r
            .faces()
            .filter(|&f| {
                blendy(f) && (f == low || crate::intersect::face_box(&r, f).min().x > 0.04)
            })
            .collect();
        let changes = Changes {
            deleted: deleted.clone(),
            ..Default::default()
        };
        let region = region(&r, &deleted);
        let end = r
            .faces()
            .find(|&f| !blendy(f) && crate::intersect::face_box(&r, f).max().x < 1e-9)
            .unwrap();
        assert!(region.faces.contains(&end));
        assert_eq!(touched(&r, &region, end), BTreeSet::from([low]));
        let x = rung_box(&r, &changes, &region);
        assert!(x.max().x > 1.5, "{x:?}");
        let n = near_box(&r, &changes, &region, end, &x, 0);
        // The ball's box (its sphere's, 0.1 deep) grown by its diagonal:
        // about 0.27, never the rung's 1.5.
        assert!(n.max().x < 0.3, "{n:?}");
        assert!(holds(&n, &crate::intersect::face_box(&r, end)));
        assert!(holds(&n, &crate::intersect::face_box(&r, low)));
    }

    /// The region's box for `changes`, as `retrim_with` makes it at rung 0.
    fn rung_box(r: &Body, changes: &Changes, region: &Region) -> Box3 {
        super::super::body::region_box(r, region, super::super::reach(r, changes))
    }

    fn holds(outer: &Box3, inner: &Box3) -> bool {
        let (a, b) = (outer.min(), outer.max());
        let (c, d) = (inner.min(), inner.max());
        a.x <= c.x && a.y <= c.y && a.z <= c.z && d.x <= b.x && d.y <= b.y && d.z <= b.z
    }

    #[test]
    fn a_face_beside_a_deleted_corner_is_carried_past_it_not_across_the_body() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let x = rung_box(&r, &changes, &region);
        let ball = crate::intersect::face_box(&r, low);
        let far = crate::intersect::face_box(&r, high);
        let mut beside = 0;
        for &f in region.faces.iter().filter(|f| !changes.deleted.contains(f)) {
            let n = near_box(&r, &changes, &region, f, &x, 0);
            // Inside the rung's box, holding the face's own box.
            assert!(holds(&x, &n), "{f:?}");
            assert!(holds(&n, &crate::intersect::face_box(&r, f)), "{f:?}");
            // A face round the low corner holds its ball, and stops well
            // short of the far corner's.
            if crate::intersect::face_box(&r, f).intersects(&ball.grow(1e-9)) {
                beside += 1;
                assert!(holds(&n, &ball), "{f:?}");
                assert!(!n.intersects(&far), "{f:?}: {n:?}");
            }
        }
        assert!(beside >= 3, "{beside}");
    }

    /// Plan 22.B: a near box is the face's own box (its box and its
    /// changes' boxes) grown by those changes' reach, inside the rung's
    /// box; the own box holds the face's box and every change it touches.
    #[test]
    fn a_near_box_is_its_own_box_grown_by_its_reach() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let x = rung_box(&r, &changes, &region);
        for &f in region.faces.iter().filter(|f| !changes.deleted.contains(f)) {
            let own = own_box(&r, &region, f);
            let near = touched(&r, &region, f);
            let reach = super::super::reach_of(&r, &changes, &|g| near.contains(&g));
            assert_eq!(
                near_box(&r, &changes, &region, f, &x, 0),
                within(&own.grow(reach), &x).unwrap_or(x),
                "{f:?}"
            );
            assert!(holds(&own, &crate::intersect::face_box(&r, f)));
            for g in near {
                assert!(holds(&own, &crate::intersect::face_box(&r, g)));
            }
        }
    }

    #[test]
    fn a_face_touching_no_gap_keeps_its_own_box() {
        let (r, low, _) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low]),
            ..Default::default()
        };
        // Rung 2's region: grown by a ring.
        let region = region(&r, &BTreeSet::from([low])).grown(&r);
        let x = rung_box(&r, &changes, &region);
        let lone = region
            .faces
            .iter()
            .copied()
            .find(|&f| {
                face_vertices(&r, f)
                    .iter()
                    .all(|&v| !face_vertices(&r, low).contains(&v))
            })
            .expect("a face of the ring touching no gap");
        let n = near_box(&r, &changes, &region, lone, &x, 2);
        let own = crate::intersect::face_box(&r, lone);
        assert_eq!(n, own.grow(10.0 * crate::tol::LINEAR_RESOLUTION));
    }

    #[test]
    fn rung_one_doubles_a_near_box_inside_its_doubled_box() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let x = super::super::body::doubled(&rung_box(&r, &changes, &region));
        let f = region
            .faces
            .iter()
            .copied()
            .find(|f| !changes.deleted.contains(f))
            .unwrap();
        let (n0, n1) = (
            near_box(&r, &changes, &region, f, &x, 0),
            near_box(&r, &changes, &region, f, &x, 1),
        );
        assert!(holds(&n1, &n0));
        assert!(holds(&x, &n1));
    }

    /// Plan 22.A: room doubles a near box about its middle, inside the
    /// rung's box, once per step; no adjustment leaves it.
    #[test]
    fn room_doubles_a_near_box_inside_the_rungs_box() {
        let (r, low, high) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low, high]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low, high]));
        let x = rung_box(&r, &changes, &region);
        let n = region
            .faces
            .iter()
            .filter(|f| !changes.deleted.contains(f))
            .map(|&f| near_box(&r, &changes, &region, f, &x, 0))
            .find(|n| *n != x)
            .expect("a near box short of the rung's box");
        assert_eq!(adjusted(&n, &x, None), n);
        assert_eq!(adjusted(&n, &x, Some(Adjust::Room(0))), n);
        let one = adjusted(&n, &x, Some(Adjust::Room(1)));
        assert_eq!(one, within(&super::super::body::doubled(&n), &x).unwrap());
        assert!(
            one != n && holds(&one, &n) && holds(&x, &one),
            "{n:?} {one:?}"
        );
        let two = adjusted(&n, &x, Some(Adjust::Room(2)));
        assert!(holds(&two, &one) && holds(&x, &two));
        // At the rung's box, room ends.
        assert_eq!(adjusted(&x, &x, Some(Adjust::Room(3))), x);
    }

    /// Plan 22.A: room for a clipped face goes to it and to every working
    /// face sharing a vertex with it, one more doubling each; never to a
    /// deleted face.
    #[test]
    fn room_goes_to_the_clipped_face_and_its_working_neighbours() {
        let (r, low, _) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low]));
        let x = rung_box(&r, &changes, &region);
        let working: BTreeSet<FaceId> =
            region.faces.iter().copied().filter(|&f| f != low).collect();
        let near = |g: FaceId| near_box(&r, &changes, &region, g, &x, 0);
        let f = *working.iter().next().unwrap();
        let mut want: BTreeSet<FaceId> = face_vertices(&r, f)
            .into_iter()
            .flat_map(|v| super::super::region::vertex_faces(&r, v))
            .filter(|g| working.contains(g))
            .collect();
        want.insert(f);
        let once = room(
            &r,
            &working,
            &BTreeSet::from([f]),
            &x,
            &Adjusts::new(),
            &near,
        )
        .expect("room to give");
        assert_eq!(once.keys().copied().collect::<BTreeSet<_>>(), want);
        assert!(once.values().all(|&a| a == Adjust::Room(1)));
        assert!(!once.contains_key(&low), "a deleted face gets no room");
        let full = want
            .iter()
            .all(|&g| adjusted(&near(g), &x, once.get(&g).copied()) == x);
        match room(&r, &working, &BTreeSet::from([f]), &x, &once, &near) {
            Some(twice) => {
                assert!(!full);
                for g in &want {
                    assert_eq!(twice[g], Adjust::Room(2));
                }
            }
            None => assert!(full),
        }
    }

    /// Plan 22.A: where every near box named is already the rung's box,
    /// room would repeat the build: none is given.
    #[test]
    fn no_room_where_every_near_box_is_the_rungs_box() {
        let (r, low, _) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low]));
        let x = rung_box(&r, &changes, &region);
        let working: BTreeSet<FaceId> =
            region.faces.iter().copied().filter(|&f| f != low).collect();
        let f = *working.iter().next().unwrap();
        assert_eq!(
            room(
                &r,
                &working,
                &BTreeSet::from([f]),
                &x,
                &Adjusts::new(),
                &|_| x
            ),
            None
        );
    }

    fn cube(l: [f64; 3], h: [f64; 3]) -> Box3 {
        Box3::new(
            crate::geom::Point3::new(l[0], l[1], l[2]),
            crate::geom::Point3::new(h[0], h[1], h[2]),
        )
    }

    /// Plan 22.B: a near box is cut back short of a pin, by the gap, on
    /// the side losing least (the first on a tie: x low, x high, y low,
    /// …), and keeps what the face must keep.
    #[test]
    fn a_near_box_is_cut_short_of_a_pin_on_the_side_losing_least() {
        let n = cube([0.0; 3], [1.0; 3]);
        let must = cube([0.1; 3], [0.4, 0.5, 0.5]);
        let gap = 1e-6;
        // Over the high x and high z ends: z high loses 0.1, x high 0.3.
        let pin = cube([0.7, 0.2, 0.9], [1.2, 0.4, 1.2]);
        let c = cut_short(&n, &n, &must, &pin, gap).expect("a cut");
        assert_eq!(c, cube([0.0; 3], [1.0, 1.0, 0.9 - gap]));
        assert!(holds(&n, &c) && holds(&c, &must));
        // Below the low y end: y low is raised past it.
        let below = cube([0.6, -0.5, 0.6], [0.9, 0.05, 0.9]);
        let c = cut_short(&n, &n, &must, &below, gap).expect("a cut");
        assert_eq!(c, cube([0.0, 0.05 + gap, 0.0], [1.0; 3]));
        // Over the high x and high y ends equally: x first.
        let corner = cube([0.8, 0.8, 0.0], [1.5, 1.5, 0.2]);
        let c = cut_short(&n, &n, &must, &corner, gap).expect("a cut");
        assert_eq!(c, cube([0.0; 3], [0.8 - gap, 1.0, 1.0]));
    }

    /// Plan 22.B: a sheet carried past its near box is cut back by that
    /// overshoot too; a pin inside what the face must keep gets no cut.
    #[test]
    fn a_cut_takes_the_sheets_overshoot_and_never_what_the_face_keeps() {
        let n = cube([0.0; 3], [1.0; 3]);
        let must = cube([0.1; 3], [0.4, 0.5, 0.5]);
        let gap = 1e-6;
        let pin = cube([0.7, 0.2, 0.9], [1.2, 0.4, 1.2]);
        let sheet = cube([-0.05; 3], [1.0, 1.0, 1.02]);
        let c = cut_short(&n, &sheet, &must, &pin, gap).expect("a cut");
        assert_eq!(c.max().z, 0.9 - gap - (1.02 - 1.0));
        assert!(holds(&c, &must));
        // Inside what it keeps: no side is valid.
        let inside = cube([0.2; 3], [0.3; 3]);
        assert_eq!(cut_short(&n, &n, &must, &inside, gap), None);
        // Over the whole near box: no side is valid either.
        let over = cube([-1.0; 3], [2.0; 3]);
        assert_eq!(cut_short(&n, &n, &must, &over, gap), None);
    }

    /// Plan 22.B: a face kept off a pin has its near box cut to the stored
    /// box; a second pin, kept off from that box, tightens it.
    #[test]
    fn a_face_kept_off_a_pin_is_cut_and_a_second_pin_tightens_it() {
        let b = block();
        let f = b.faces().next().unwrap();
        let n = cube([0.0; 3], [1.0; 3]);
        let must = cube([0.1; 3], [0.4, 0.5, 0.5]);
        let gap = 1e-6;
        let pin = cube([0.7, 0.2, 0.9], [1.2, 0.4, 1.2]);
        let once = kept_off(f, &n, &n, &must, &pin, gap, &Adjusts::new()).expect("a cut");
        let c = cut_short(&n, &n, &must, &pin, gap).unwrap();
        assert_eq!(once, Adjusts::from([(f, Adjust::off(&c))]));
        assert_eq!(adjusted(&n, &n, once.get(&f).copied()), c);
        // Within a smaller rung's box, inside it.
        let x = cube([0.0; 3], [0.95; 3]);
        assert_eq!(
            adjusted(&n, &x, once.get(&f).copied()),
            within(&c, &x).unwrap()
        );
        let second = cube([0.8, 0.1, 0.1], [1.5, 0.3, 0.3]);
        let twice = kept_off(f, &c, &c, &must, &second, gap, &once).expect("tighter");
        let t = adjusted(&n, &n, twice.get(&f).copied());
        assert_eq!(t, cube([0.0; 3], [0.8 - gap, 1.0, 0.9 - gap]));
        assert!(holds(&c, &t) && t != c);
        // Nothing to cut: no ask.
        let inside = cube([0.2; 3], [0.3; 3]);
        assert_eq!(
            kept_off(f, &n, &n, &must, &inside, gap, &Adjusts::new()),
            None
        );
    }

    /// Design 724: a face is adjusted one way only. Room asked for a face
    /// kept off a pin, or for one of its working neighbours kept off one,
    /// ends the adjustments; so does a cut asked for a face given room.
    #[test]
    fn a_face_is_adjusted_one_way_only() {
        let (r, low, _) = rounded();
        let changes = Changes {
            deleted: BTreeSet::from([low]),
            ..Default::default()
        };
        let region = region(&r, &BTreeSet::from([low]));
        let x = rung_box(&r, &changes, &region);
        let working: BTreeSet<FaceId> =
            region.faces.iter().copied().filter(|&f| f != low).collect();
        let near = |g: FaceId| near_box(&r, &changes, &region, g, &x, 0);
        let f = *working.iter().next().unwrap();
        let clipped = BTreeSet::from([f]);
        assert!(room(&r, &working, &clipped, &x, &Adjusts::new(), &near).is_some());
        let off = Adjusts::from([(f, Adjust::off(&near(f)))]);
        assert_eq!(room(&r, &working, &clipped, &x, &off, &near), None);
        let g = face_vertices(&r, f)
            .into_iter()
            .flat_map(|v| super::super::region::vertex_faces(&r, v))
            .find(|g| *g != f && working.contains(g))
            .expect("a working neighbour");
        let off = Adjusts::from([(g, Adjust::off(&near(g)))]);
        assert_eq!(room(&r, &working, &clipped, &x, &off, &near), None);
        let n = cube([0.0; 3], [1.0; 3]);
        let must = cube([0.1; 3], [0.4, 0.5, 0.5]);
        let pin = cube([0.7, 0.2, 0.9], [1.2, 0.4, 1.2]);
        let roomy = Adjusts::from([(f, Adjust::Room(1))]);
        assert_eq!(kept_off(f, &n, &n, &must, &pin, 1e-6, &roomy), None);
    }

    /// The rounded block's low ball deleted, its region grown by a ring
    /// (rung 2's). Every pin is left as it is and touches no change; the
    /// ball's neighbours are never pinned; the region less its pins has
    /// no pinch.
    #[test]
    fn ring_faces_touching_no_change_are_pinned() {
        let (r, low, _) = rounded();
        let ring = region(&r, &BTreeSet::from([low])).grown(&r);
        let still = |f: FaceId| ring.faces.contains(&f) && f != low;
        let pins = pinned(&r, &ring, &still, &BTreeSet::new());
        assert!(!pins.is_empty(), "rung 2's ring pins nothing");
        for &f in &pins {
            assert!(still(f), "{f:?}");
            assert!(touched(&r, &ring, f).is_empty(), "{f:?}");
        }
        assert!(!pins.contains(&low));
        for &f in &ring.faces {
            if f != low && !touched(&r, &ring, f).is_empty() {
                assert!(!pins.contains(&f), "{f:?} touches the ball");
            }
        }
        let rest: BTreeSet<FaceId> = ring.faces.difference(&pins).copied().collect();
        assert!(super::super::region::pinches(&r, &rest).is_empty());
        // A face standing for others is never pinned.
        let some = *pins.iter().next().unwrap();
        let fewer = pinned(&r, &ring, &still, &BTreeSet::from([some]));
        assert!(!fewer.contains(&some));
    }

    #[test]
    fn first_builds_pin_nothing() {
        use super::super::Build;
        let (r, low, _) = rounded();
        let ring = region(&r, &BTreeSet::from([low])).grown(&r);
        let still = |f: FaceId| ring.faces.contains(&f) && f != low;
        let none = BTreeSet::new();
        assert!(pins(&r, &ring, &still, &none, Build::First, &none).is_empty());
        let want = pinned(&r, &ring, &still, &none);
        for which in [Build::Retry, Build::Only] {
            assert_eq!(pins(&r, &ring, &still, &none, which, &none), want);
        }
    }

    /// Design 919, plan 23.A: a released face is never pinned, and a
    /// first build pins nothing whatever is released. Released nothing,
    /// the pins are `pinned`'s, bit for bit.
    #[test]
    fn a_released_face_is_never_pinned() {
        use super::super::Build;
        use super::super::tests::{face_of, raised};
        use crate::name::FaceRole;
        let b = block();
        let changes = raised(&b, 0.2);
        let changed = changes.motion.keys().copied().collect();
        let whole = region(&b, &changed).grown(&b);
        let still = |f: FaceId| whole.faces.contains(&f) && !changes.motion.contains_key(&f);
        let none = BTreeSet::new();
        let bottom = face_of(&b, FaceRole::MinusZ);
        let pins = pinned(&b, &whole, &still, &none);
        assert_eq!(pins, BTreeSet::from([bottom]));
        assert_eq!(pinned_but(&b, &whole, &still, &none, &none), pins);
        let released = BTreeSet::from([bottom]);
        assert!(pinned_but(&b, &whole, &still, &none, &released).is_empty());
        assert!(super::pins(&b, &whole, &still, &none, Build::First, &none).is_empty());
        assert!(super::pins(&b, &whole, &still, &none, Build::First, &released).is_empty());
        assert_eq!(
            super::pins(&b, &whole, &still, &none, Build::Only, &none),
            pins
        );
        assert!(super::pins(&b, &whole, &still, &none, Build::Only, &released).is_empty());
        // `unpinched` keeps pins that pinch nothing.
        assert_eq!(unpinched(&b, &whole, pins.clone()), pins);
        // `released` reads the released faces off the adjustments.
        let a = Adjusts::from([
            (bottom, Adjust::Released),
            (face_of(&b, FaceRole::PlusX), Adjust::Room(1)),
        ]);
        assert_eq!(super::released(&a), BTreeSet::from([bottom]));
    }

    /// `unpinched` releases the pins round a pinch the release opens: the
    /// rounded block's ring less one pin, every pin it leaves pinching
    /// nothing.
    #[test]
    fn a_release_unpinches_its_neighbours() {
        let (r, low, _) = rounded();
        let ring = region(&r, &BTreeSet::from([low])).grown(&r);
        let still = |f: FaceId| ring.faces.contains(&f) && f != low;
        let none = BTreeSet::new();
        let pins = pinned(&r, &ring, &still, &none);
        for &p in &pins {
            let fewer = pinned_but(&r, &ring, &still, &none, &BTreeSet::from([p]));
            assert!(!fewer.contains(&p), "{p:?} pinned though released");
            assert!(fewer.is_subset(&pins));
            let rest: BTreeSet<FaceId> = ring.faces.difference(&fewer).copied().collect();
            assert!(super::super::region::pinches(&r, &rest).is_empty(), "{p:?}");
        }
    }
}
