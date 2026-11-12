//! Interference judged on meshes, for pairs with a reference import's
//! body on a side (design 364): a reference is the file's topology, a
//! sheet the kernel's booleans don't take. Each side is meshed at normal
//! display quality; the meshes stand within each side's chord and
//! tolerances of its faces, so what they show is certified to within the
//! pair's reach, and nothing is claimed past it.

use crate::interference::Verdict;
use ryu_session::job::Job;
use ryukei::geom::{Box3, Point3, Rigid, Vector3};
use ryukei::measure::MeshTree;
use ryukei::name::Name;
use ryukei::topo::Body;

/// One body's mesh at normal quality in its own frame, its box tree, each
/// triangle's face, whether it closes, and its reach: the chord plus the
/// largest tolerance of its edges and vertices.
pub struct Prepared {
    pub tree: MeshTree,
    /// The face each triangle belongs to, by name.
    pub faces: Vec<Name>,
    /// No laminar edge and no face left out of the mesh: a winding
    /// number about a point means something.
    pub closed: bool,
    pub reach: f64,
}

impl Prepared {
    /// `body`'s mesh, or the mesher's refusal as text.
    pub fn new(body: &Body) -> Result<Prepared, String> {
        Prepared::new_for(body, &ryukei::work::Work::unlimited()).map_err(|e| format!("{e:?}"))
    }

    /// [`Prepared::new`] under `work`, the mesher's refusal as it gave it
    /// (a cancel among them).
    pub fn new_for(body: &Body, work: &ryukei::work::Work) -> Result<Prepared, ryukei::Error> {
        let diagonal = match crate::inspect::loose_extent(body) {
            Some((lo, hi)) => {
                let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            }
            None => 1.0,
        };
        let params = crate::mesh::Quality::Normal.params(diagonal);
        let m = ryukei::mesh::tessellate_with(body, &params, work)?;
        let mut triangles = Vec::with_capacity(m.triangles.len());
        let mut faces = Vec::with_capacity(m.triangles.len());
        for fm in &m.faces {
            let name = body.face(fm.face).expect("a meshed face is live").name;
            for t in &m.triangles[fm.triangles.start as usize..fm.triangles.end as usize] {
                triangles.push(t.map(|c| m.positions[m.corners[c as usize].position as usize]));
                faces.push(name);
            }
        }
        let laminar = body.edges().any(|e| body.is_laminar(e));
        let tol = body
            .edges()
            .map(|e| body.edge(e).expect("live").tolerance.get())
            .chain(
                body.vertices()
                    .map(|v| body.vertex(v).expect("live").tolerance.get()),
            )
            .fold(0.0, f64::max);
        Ok(Prepared {
            tree: MeshTree::new(triangles),
            faces,
            closed: !laminar && m.faults.is_empty(),
            reach: params.chord + tol,
        })
    }

    /// Its triangles moved by `r`, in a tree of their own, with the same
    /// faces, closure and reach.
    pub fn placed(&self, r: &Rigid) -> Prepared {
        let moved = self
            .tree
            .triangles()
            .iter()
            .map(|t| t.map(|p| r.apply_point(p)))
            .collect();
        Prepared {
            tree: MeshTree::new(moved),
            faces: self.faces.clone(),
            closed: self.closed,
            reach: self.reach,
        }
    }

    /// Its box.
    pub fn bounds(&self) -> Option<Box3> {
        self.tree.bounds().map(|(lo, hi)| Box3::new(lo, hi))
    }
}

/// Pieces a pair's search may visit before it gives up: past it the pair
/// is unchecked.
pub const BUDGET: u64 = 1 << 20;

/// Faces of each side named at most.
const FACES: usize = 64;

/// The pair's reach and its rounding margin: both sides' chords and
/// tolerances, and a part in 10⁹ of the coordinates' size for rounding,
/// which is also the escape certificate's margin.
fn reach(a: &Prepared, b: &Prepared, both: &Box3) -> (f64, f64) {
    let (lo, hi) = (both.min(), both.max());
    let size = [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z]
        .iter()
        .fold(0.0f64, |m, x| m.max(x.abs()));
    let margin = 1e-9 * size.max(1e-3);
    (a.reach + b.reach + margin, margin)
}

/// Whether the prism triangle `t` sweeps from `s0·v` to `s1·v` and the
/// triangle `u` lie apart: some axis among both shapes' face normals (the
/// caps', the three sides', `u`'s) and the cross products of `u`'s edges
/// with the prism's edge directions (`t`'s edges and `v`) on which their
/// projections lie more than `margin` apart along the axis. Both are
/// convex, so these axes suffice; and any axis that separates proves them
/// apart, so one of no length is skipped and one rounded off its true
/// direction is still an axis. The margin, a part in 10⁹ of the pair's
/// size, stands far above the dot products' rounding, so rounding can only
/// withhold the answer, never give it (design 525).
fn prism_misses(
    t: &[Point3; 3],
    v: Vector3,
    s0: f64,
    s1: f64,
    u: &[Point3; 3],
    margin: f64,
) -> bool {
    let prism = [
        t[0] + v * s0,
        t[1] + v * s0,
        t[2] + v * s0,
        t[0] + v * s1,
        t[1] + v * s1,
        t[2] + v * s1,
    ];
    let e = [t[1] - t[0], t[2] - t[1], t[0] - t[2]];
    let f = [u[1] - u[0], u[2] - u[1], u[0] - u[2]];
    let mut axes = [Vector3::ZERO; 17];
    axes[0] = e[0].cross(e[1]);
    axes[1] = v.cross(e[0]);
    axes[2] = v.cross(e[1]);
    axes[3] = v.cross(e[2]);
    axes[4] = f[0].cross(f[1]);
    let mut k = 5;
    for d in [e[0], e[1], e[2], v] {
        for g in f {
            axes[k] = d.cross(g);
            k += 1;
        }
    }
    let span = |pts: &[Point3], a: Vector3| {
        pts.iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                let d = q.to_vector().dot(a);
                (lo.min(d), hi.max(d))
            })
    };
    axes.iter().any(|&a| {
        let len2 = a.dot(a);
        if !(len2 > 0.0 && len2.is_finite()) {
            return false;
        }
        let gap = margin * len2.sqrt();
        let (p0, p1) = span(&prism, a);
        let (q0, q1) = span(u, a);
        p1 + gap < q0 || q1 + gap < p0
    })
}

/// Whether no point of piece `t` can be deep in the closed mesh `inside`
/// (design 525, the escape certificate): for some unit direction `v`, no
/// triangle of `inside` meets the prism `t ⊕ [0.9·reach, far]·v`, where
/// `far` takes all of `t` out past `inside`'s box grown by `reach`. Then a
/// ray along `v` from a point of `t` farther than `reach` from the mesh
/// crosses none of it: within `0.9·reach·|v|` of its start (under `reach`,
/// though `v`'s rounded length may pass 1), then inside the prism, then
/// outside the box. So the point winds zero, and every point of `t` is
/// within `reach` of the mesh or outside it. The directions tried are
/// `t`'s inward normal, its outward one, then (where it isn't parallel to
/// `t`'s) the normal `m` of `inside`'s triangle `nearest` both ways, and
/// `t`'s normal across `m` both ways: a wall whose edge stands on the
/// other side's flush face grazes that face along its own normal, but
/// leaves it along the face's; a pin in a hole leaves it only along the
/// hole, across both (departures from the design's wording, 29.A; the
/// proof holds for any direction). A
/// degenerate piece has no normal and no certificate. `found` is the box
/// query's buffer, kept by the caller.
fn escapes(
    inside: &MeshTree,
    t: &[Point3; 3],
    reach: f64,
    margin: f64,
    found: &mut Vec<u32>,
    nearest: u32,
) -> bool {
    let Some((lo, hi)) = inside.bounds() else {
        return false;
    };
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    let len = n.length();
    if !(len > 0.0 && len.is_finite()) {
        return false;
    }
    let grown = Box3::new(lo, hi).grow(reach);
    let (glo, ghi) = (grown.min(), grown.max());
    let mid = Point3::new(
        (glo.x + ghi.x) / 2.0,
        (glo.y + ghi.y) / 2.0,
        (glo.z + ghi.z) / 2.0,
    );
    let out = t.iter().map(|q| q.distance(mid)).fold(0.0, f64::max);
    // |q + far·v − mid| ≥ far − out = reach + diagonal: past the grown
    // box's half diagonal for every point q of t, and for every s ≥ far.
    let far = reach + (ghi - glo).length() + out;
    let near = 0.9 * reach;
    let n = n * (1.0 / len);
    let u = &inside.triangles()[nearest as usize];
    let m = (u[1] - u[0]).cross(u[2] - u[0]);
    let l = m.length();
    // The face's normal, unless it has none or lies within about a
    // thousandth of a radian of `t`'s (then it adds nothing).
    let m = (l > 0.0 && l.is_finite() && (m * (1.0 / l)).dot(n).abs() < 1.0 - 1e-6)
        .then(|| m * (1.0 / l));
    // Along both faces: out of a hole along its axis.
    let w = m.map(|m| {
        let w = n.cross(m);
        w * (1.0 / w.length())
    });
    let dirs = [
        Some(n * -1.0),
        Some(n),
        m,
        m.map(|m| m * -1.0),
        w,
        w.map(|w| w * -1.0),
    ];
    for v in dirs.into_iter().flatten() {
        let ends = [
            t[0] + v * near,
            t[1] + v * near,
            t[2] + v * near,
            t[0] + v * far,
            t[1] + v * far,
            t[2] + v * far,
        ];
        let b = Box3::from_points(ends).expect("six points").grow(margin);
        inside.in_box(b.min(), b.max(), found);
        let tris = inside.triangles();
        if found
            .iter()
            .all(|&k| prism_misses(t, v, near, far, &tris[k as usize], margin))
        {
            return true;
        }
    }
    false
}

/// Why a pair's search stopped before it decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// Past its budget: the pair is unchecked.
    OverBudget,
    /// The request was cancelled: the pair answers `cancelled`.
    Cancelled,
}

/// Pieces between looks at the request's cancel.
pub const CANCEL_EVERY: u64 = 4096;

/// A pair's search: the pieces it has visited, how many it may, and the
/// request whose cancel stops it, looked at on the first piece and every
/// [`CANCEL_EVERY`] after.
pub struct Search<'j> {
    pub spent: u64,
    budget: u64,
    job: &'j Job,
}

impl<'j> Search<'j> {
    /// [`BUDGET`] pieces, stopped by `job`'s cancel.
    pub fn new(job: &'j Job) -> Self {
        Search {
            spent: 0,
            budget: BUDGET,
            job,
        }
    }

    /// Another budget (tests).
    pub fn with_budget(mut self, budget: u64) -> Self {
        self.budget = budget;
        self
    }

    /// One piece more.
    fn charge(&mut self) -> Result<(), Halt> {
        self.spent += 1;
        if self.spent > self.budget {
            return Err(Halt::OverBudget);
        }
        if self.spent % CANCEL_EVERY == 1 && self.job.is_cancelled() {
            return Err(Halt::Cancelled);
        }
        Ok(())
    }
}

/// A point of a triangle of `tris` (those whose boxes come within `reach`
/// of `inside`'s box) whose winding in `inside` isn't zero and that lies
/// farther than `reach` from it, found by quartering triangles. A piece
/// is dropped:
/// - where its corners all lie within `reach` of the triangle of
///   `inside` nearest its middle (the distance to a triangle is convex,
///   so every point of it does: flush faces end at once);
/// - where a ball round its middle holding it misses `inside`'s mesh
///   and the middle winds zero (the piece is wholly outside);
/// - once it is an eighth of `reach` across (what it may still hold lies
///   within about the reach of the mesh: not deep).
/// - where it escapes ([`escapes`]): no triangle of `inside` meets its
///   prism along its normal either way, so no point of it is deep
///   (design 525).
///
/// A piece wholly inside (its ball misses the mesh, its middle winds) is
/// deep when its middle lies farther than `reach`. [`Halt::OverBudget`]
/// past the search's budget, [`Halt::Cancelled`] on a cancel.
pub fn deep_point(
    inside: &MeshTree,
    tris: &[[Point3; 3]],
    reach: f64,
    margin: f64,
    search: &mut Search,
) -> Result<Option<Point3>, Halt> {
    let Some((lo, hi)) = inside.bounds() else {
        return Ok(None);
    };
    let near = Box3::new(lo, hi).grow(reach);
    let mut found = Vec::new();
    let mut stack: Vec<[Point3; 3]> = tris
        .iter()
        .filter(|t| Box3::from_points(t.iter().copied()).is_some_and(|b| b.intersects(&near)))
        .copied()
        .collect();
    while let Some(t) = stack.pop() {
        search.charge()?;
        let c = Point3::new(
            (t[0].x + t[1].x + t[2].x) / 3.0,
            (t[0].y + t[1].y + t[2].y) / 3.0,
            (t[0].z + t[1].z + t[2].z) / 3.0,
        );
        let r = t.iter().map(|p| p.distance(c)).fold(0.0, f64::max);
        let (d, _, k) = inside.nearest_point(c).expect("a mesh");
        if d + r <= reach {
            continue;
        }
        if t.iter().all(|&p| inside.to_triangle(p, k) <= reach) {
            continue;
        }
        match inside.winding(c) {
            Some(0) if d > r => continue,
            Some(w) if w != 0 && d > reach => return Ok(Some(c)),
            _ => {}
        }
        if r < reach / 8.0 {
            continue;
        }
        if escapes(inside, &t, reach, margin, &mut found, k) {
            continue;
        }
        let m = |p: Point3, q: Point3| {
            Point3::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0, (p.z + q.z) / 2.0)
        };
        let (ab, bc, ca) = (m(t[0], t[1]), m(t[1], t[2]), m(t[2], t[0]));
        stack.extend([[t[0], ab, ca], [ab, t[1], bc], [ca, bc, t[2]], [ab, bc, ca]]);
    }
    Ok(None)
}

/// A point inside both closed meshes and farther than `reach` from each,
/// found among `own`'s triangles' centroids stepped inwards (against the
/// triangle's front, the side its corners wind counterclockwise about) by
/// twice `reach`: kept where `own` winds it non-zero and it lies farther
/// than `reach` from `own`'s mesh, a certificate where `other` winds it
/// non-zero too and it lies farther than `reach` from `other`'s mesh. So
/// two closed sides whose surfaces coincide, or nearly (a part placed twice
/// at one pose, which no surface point of either certifies), interfere.
/// Only points inside `other`'s box grown by `reach` are judged; each
/// costs a piece of the search's budget: [`Halt::OverBudget`] past it,
/// [`Halt::Cancelled`] on a cancel.
pub fn interior_point(
    own: &MeshTree,
    other: &MeshTree,
    reach: f64,
    search: &mut Search,
) -> Result<Option<Point3>, Halt> {
    let Some((lo, hi)) = other.bounds() else {
        return Ok(None);
    };
    let near = Box3::new(lo, hi).grow(reach);
    for t in own.triangles() {
        // Divided by its own length, not `normalize`d: a small triangle's
        // cross product falls under the resolution.
        let n = (t[1] - t[0]).cross(t[2] - t[0]);
        let len = n.length();
        if !(len > 0.0 && len.is_finite()) {
            continue;
        }
        let c = Point3::new(
            (t[0].x + t[1].x + t[2].x) / 3.0,
            (t[0].y + t[1].y + t[2].y) / 3.0,
            (t[0].z + t[1].z + t[2].z) / 3.0,
        );
        let p = c - n * (2.0 * reach / len);
        if !near.intersects(&Box3::from_point(p)) {
            continue;
        }
        search.charge()?;
        if !matches!(own.winding(p), Some(w) if w != 0) {
            continue;
        }
        if own.nearest_point(p).is_none_or(|(d, _, _)| d <= reach) {
            continue;
        }
        if !matches!(other.winding(p), Some(w) if w != 0) {
            continue;
        }
        if other.nearest_point(p).is_some_and(|(d, _, _)| d > reach) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// `a` against `b` placed at `rel` in `a`'s frame, on their meshes, under
/// `search`'s budget and cancel.
pub fn verdict(
    a: &Prepared,
    b: &Prepared,
    rel: &Rigid,
    search: &mut Search,
) -> Result<Verdict, Halt> {
    let bp = b.placed(rel);
    let (Some(ba), Some(bb)) = (a.bounds(), bp.bounds()) else {
        return Ok(Verdict::Clear);
    };
    let (reach, margin) = reach(a, b, &ba.union(&bb));
    let nearest = a
        .tree
        .nearest(&bp.tree)
        .map_or(f64::INFINITY, |n| n.distance);
    let mut deep = None;
    if a.closed {
        deep = deep_point(&a.tree, bp.tree.triangles(), reach, margin, search)?;
    }
    if deep.is_none() && bp.closed {
        deep = deep_point(&bp.tree, a.tree.triangles(), reach, margin, search)?;
    }
    if deep.is_none() && a.closed && bp.closed && nearest <= reach {
        deep = interior_point(&a.tree, &bp.tree, reach, search)?;
        if deep.is_none() {
            deep = interior_point(&bp.tree, &a.tree, reach, search)?;
        }
    }
    if deep.is_some() {
        let (mut faces_a, mut faces_b) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for (i, j) in a.tree.within(&bp.tree, reach) {
            if faces_a.len() < FACES {
                faces_a.insert(a.faces[i as usize]);
            }
            if faces_b.len() < FACES {
                faces_b.insert(b.faces[j as usize]);
            }
        }
        let (lo, hi) = (ba.min().max(bb.min()), ba.max().min(bb.max()));
        return Ok(Verdict::Approximate {
            min: [lo.x, lo.y, lo.z],
            max: [hi.x, hi.y, hi.z],
            open: !a.closed || !b.closed,
            faces_a: faces_a.into_iter().collect(),
            faces_b: faces_b.into_iter().collect(),
        });
    }
    Ok(if nearest <= reach {
        Verdict::Touching
    } else {
        Verdict::Clear
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::Rigid;
    use ryukei::name::OpKey;
    use std::sync::{Arc, atomic::AtomicBool};

    pub(crate) fn block(side: f64) -> Body {
        ryukei::primitive::block(OpKey::from_bytes(b"b"), side, side, side, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    #[test]
    fn a_solid_block_is_prepared_closed_with_its_reach() {
        let p = Prepared::new(&block(0.01)).unwrap();
        assert!(p.closed);
        let diagonal = (3.0f64).sqrt() * 0.01;
        let chord = crate::mesh::Quality::Normal.params(diagonal).chord;
        // The chord and the block's tolerances (the resolution).
        let tol = ryukei::tol::LINEAR_RESOLUTION;
        assert!((p.reach - chord - tol).abs() < 1e-15, "{} {chord}", p.reach);
        assert_eq!(p.tree.triangles().len(), p.faces.len());
        assert_eq!(p.tree.triangles().len(), 12);
    }

    #[test]
    fn a_sheet_is_prepared_open() {
        let b = block(0.01);
        let top = b.faces().next().unwrap();
        let name = b.face(top).unwrap().name;
        let (sheet, _) = ryukei::sheet::extract(OpKey::from_bytes(b"s"), &b, &[name]).unwrap();
        let p = Prepared::new(&sheet).unwrap();
        assert!(!p.closed);
        assert_eq!(p.tree.triangles().len(), 2);
    }
    /// `verdict` with no cancel and the whole budget, and what it spent.
    fn judged(a: &Prepared, b: &Prepared, rel: &Rigid) -> (Verdict, u64) {
        let job = Job::default();
        let mut s = Search::new(&job);
        let v = verdict(a, b, rel, &mut s).unwrap();
        (v, s.spent)
    }

    fn judge(a: &Prepared, b: &Prepared, rel: &Rigid) -> Verdict {
        judged(a, b, rel).0
    }

    #[test]
    fn a_cancelled_search_stops_and_a_spent_one_is_over_budget() {
        let a = Prepared::new(&block(0.01)).unwrap();
        let b = Prepared::new(&block(0.01)).unwrap();
        let overlapping = at(0.005, 0.002, 0.0);
        let job = Job::new("t", "p", "interference", Arc::new(AtomicBool::new(true)));
        let mut s = Search::new(&job);
        assert_eq!(verdict(&a, &b, &overlapping, &mut s), Err(Halt::Cancelled));
        assert_eq!(s.spent, 1, "the first piece looks");
        let none = Job::default();
        let mut s = Search::new(&none).with_budget(1);
        assert_eq!(
            verdict(&a, &b, &at(0.01, 0.0, 0.0), &mut s),
            Err(Halt::OverBudget)
        );
        assert_eq!(s.spent, 2);
    }
    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(ryukei::geom::Vector3::new(x, y, z))
    }

    fn pin(r: f64, h: f64) -> Body {
        ryukei::primitive::cylinder(OpKey::from_bytes(b"p"), r, h, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    #[test]
    fn blocks_apart_touching_and_overlapping() {
        let a = Prepared::new(&block(0.01)).unwrap();
        let b = Prepared::new(&block(0.01)).unwrap();
        assert_eq!(judge(&a, &b, &at(0.011, 0.0, 0.0)), Verdict::Clear);
        assert_eq!(judge(&a, &b, &at(0.01, 0.0, 0.0)), Verdict::Touching);
        let v = judge(&a, &b, &at(0.005, 0.002, 0.0));
        let Verdict::Approximate {
            min,
            max,
            open,
            faces_a,
            faces_b,
        } = v
        else {
            panic!("{v:?}")
        };
        assert!(!open && !faces_a.is_empty() && !faces_b.is_empty());
        assert!(
            (min[0] - 0.005).abs() < 1e-9 && (max[0] - 0.01).abs() < 1e-9,
            "{min:?} {max:?}"
        );
    }

    #[test]
    fn coincident_and_nearly_coincident_blocks_interfere() {
        let a = Prepared::new(&block(0.01)).unwrap();
        let b = Prepared::new(&block(0.01)).unwrap();
        for rel in [Rigid::IDENTITY, at(1e-6, 0.0, 0.0), at(0.0, -1e-6, 1e-6)] {
            let v = judge(&a, &b, &rel);
            assert!(
                matches!(v, Verdict::Approximate { open: false, .. }),
                "{rel:?}: {v:?}"
            );
        }
        // A block standing on another, flush, still only touches.
        assert_eq!(judge(&a, &b, &at(0.0, 0.0, 0.01)), Verdict::Touching);
    }

    #[test]
    fn a_pin_through_a_plate_interferes_though_no_vertex_is_deep() {
        // The plate's top and bottom are two triangles each, the pin's
        // vertices all on its rims above and below the plate.
        let plate = ryukei::primitive::block(
            OpKey::from_bytes(b"pl"),
            0.04,
            0.04,
            0.005,
            &Rigid::IDENTITY,
        )
        .unwrap()
        .0;
        let a = Prepared::new(&plate).unwrap();
        let b = Prepared::new(&pin(0.003, 0.02)).unwrap();
        let v = judge(&a, &b, &at(0.02, 0.02, -0.0075));
        assert!(
            matches!(v, Verdict::Approximate { open: false, .. }),
            "{v:?}"
        );
    }

    #[test]
    fn one_inside_another_interferes_and_a_sheet_through_a_block_is_open() {
        let a = Prepared::new(&block(0.01)).unwrap();
        let small = Prepared::new(&block(0.002)).unwrap();
        let v = judge(&a, &small, &at(0.004, 0.004, 0.004));
        assert!(matches!(v, Verdict::Approximate { .. }), "{v:?}");
        let b = block(0.01);
        let top = b.faces().next().unwrap();
        let (sheet, _) =
            ryukei::sheet::extract(OpKey::from_bytes(b"s"), &b, &[b.face(top).unwrap().name])
                .unwrap();
        let s = Prepared::new(&sheet).unwrap();
        // The square sheet moved to lie across the block's middle.
        let box3 = s.bounds().unwrap();
        let mid = 0.005 - box3.min().z;
        let v = judge(&a, &s, &at(0.002, 0.002, mid));
        assert!(
            matches!(v, Verdict::Approximate { open: true, .. }),
            "{v:?}"
        );
        // Two sheets crossing only touch.
        let v = judge(&s, &s, &at(0.002, 0.002, 0.0));
        assert!(matches!(v, Verdict::Touching | Verdict::Clear), "{v:?}");
    }

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    /// A millimetre triangle in z = 0 swept down from 10 µm to 1 cm.
    fn prism_case() -> ([Point3; 3], ryukei::geom::Vector3, f64, f64, f64) {
        let t = [p(0.0, 0.0, 0.0), p(0.001, 0.0, 0.0), p(0.0, 0.001, 0.0)];
        (
            t,
            ryukei::geom::Vector3::new(0.0, 0.0, -1.0),
            1e-5,
            0.01,
            1e-12,
        )
    }

    fn moved(t: &[Point3; 3], x: f64, y: f64, z: f64) -> [Point3; 3] {
        t.map(|q| p(q.x + x, q.y + y, q.z + z))
    }

    #[test]
    fn a_prism_misses_only_what_lies_apart_by_more_than_the_margin() {
        let (t, v, s0, s1, mu) = prism_case();
        // Above the swept triangle (the side the prism leaves from).
        assert!(prism_misses(
            &t,
            v,
            s0,
            s1,
            &moved(&t, 0.0, 0.0, 0.0005),
            mu
        ));
        // Just above the prism's top cap (z = -s0), by half the margin and
        // by twice it.
        assert!(!prism_misses(
            &t,
            v,
            s0,
            s1,
            &moved(&t, 0.0, 0.0, -s0 + 0.5e-12),
            mu
        ));
        assert!(prism_misses(
            &t,
            v,
            s0,
            s1,
            &moved(&t, 0.0, 0.0, -s0 + 2e-12),
            mu
        ));
        // Standing across the prism.
        let across = [
            p(0.0002, 0.0002, -0.005),
            p(0.0003, 0.0002, -0.005),
            p(0.0002, 0.0002, 0.005),
        ];
        assert!(!prism_misses(&t, v, s0, s1, &across, mu));
        // Parallel edges (zero-length axes skipped): a copy inside it, and
        // one beside its slanted side.
        assert!(!prism_misses(
            &t,
            v,
            s0,
            s1,
            &moved(&t, 0.0, 0.0, -0.005),
            mu
        ));
        assert!(prism_misses(
            &t,
            v,
            s0,
            s1,
            &moved(&t, 0.002, 0.0, -0.005),
            mu
        ));
        // Past its far end.
        assert!(prism_misses(&t, v, s0, s1, &moved(&t, 0.0, 0.0, -0.02), mu));
    }

    #[test]
    fn a_piece_under_a_flush_block_escapes_and_one_inside_it_does_not() {
        // A 48 × 5 × 8.5 mm block standing on z = 1.6 mm.
        let comp = ryukei::primitive::block(
            OpKey::from_bytes(b"c"),
            0.048,
            0.005,
            0.0085,
            &at(0.006, 0.0075, 0.0016),
        )
        .unwrap()
        .0;
        let c = Prepared::new(&comp).unwrap();
        let reach = 2.0 * c.reach;
        let margin = 1e-12;
        let mut found = Vec::new();
        // A board-wide piece on z = 1.6 mm, its front up (counterclockwise
        // seen from +z), under the block and past its footprint's edges.
        let wide = [
            p(-0.01, -0.01, 0.0016),
            p(0.07, -0.01, 0.0016),
            p(-0.01, 0.03, 0.0016),
        ];
        assert!(escapes(
            &c.tree,
            &wide,
            reach,
            margin,
            &mut found,
            nearest(&c.tree, &wide)
        ));
        // The same piece wound the other way still escapes (a sheet's
        // triangles carry no front: both directions are tried).
        let back = [wide[0], wide[2], wide[1]];
        assert!(escapes(
            &c.tree,
            &back,
            reach,
            margin,
            &mut found,
            nearest(&c.tree, &back)
        ));
        // Inside the block: its bottom face below, its top above.
        let inside = [
            p(0.01, 0.008, 0.005),
            p(0.02, 0.008, 0.005),
            p(0.01, 0.012, 0.005),
        ];
        assert!(!escapes(
            &c.tree,
            &inside,
            reach,
            margin,
            &mut found,
            nearest(&c.tree, &inside)
        ));
        // A degenerate piece has no normal and no certificate.
        let line = [
            p(0.0, 0.0, 0.0016),
            p(0.001, 0.0, 0.0016),
            p(0.002, 0.0, 0.0016),
        ];
        assert!(!escapes(
            &c.tree,
            &line,
            reach,
            margin,
            &mut found,
            nearest(&c.tree, &line)
        ));
    }

    #[test]
    fn a_block_flush_on_a_plate_is_decided_in_few_pieces() {
        let plate = Prepared::new(
            &ryukei::primitive::block(
                OpKey::from_bytes(b"pl"),
                0.06,
                0.02,
                0.0016,
                &Rigid::IDENTITY,
            )
            .unwrap()
            .0,
        )
        .unwrap();
        let comp = Prepared::new(
            &ryukei::primitive::block(
                OpKey::from_bytes(b"c"),
                0.048,
                0.005,
                0.0085,
                &Rigid::IDENTITY,
            )
            .unwrap()
            .0,
        )
        .unwrap();
        let on = at(0.006, 0.0075, 0.0016);
        let (v, spent) = judged(&plate, &comp, &on);
        println!("flush block on a plate: {spent} pieces");
        assert_eq!(v, Verdict::Touching);
        assert!(spent < 1 << 10, "{spent}");
    }

    /// The index of `inside`'s triangle nearest `t`'s middle.
    fn nearest(inside: &MeshTree, t: &[Point3; 3]) -> u32 {
        let c = Point3::new(
            (t[0].x + t[1].x + t[2].x) / 3.0,
            (t[0].y + t[1].y + t[2].y) / 3.0,
            (t[0].z + t[1].z + t[2].z) / 3.0,
        );
        inside.nearest_point(c).unwrap().2
    }

    #[test]
    fn a_wall_on_a_face_escapes_along_it_only_past_the_margin() {
        // A wall piece in x = 2 mm, its top edge on block A's bottom face
        // (z = 0): its own normal (±x) grazes that face, and the face's
        // normal leads down, out of A. Block B stands beside that downward
        // prism, `gap` off its plane: apart by half the margin it must be
        // withheld (the exact shapes miss, but not by the margin), by twice
        // it granted.
        let margin = 1e-12;
        let reach = 1e-5;
        let wall = [
            p(0.002, 0.001, 0.0),
            p(0.002, 0.003, 0.0),
            p(0.002, 0.002, -0.001),
        ];
        let cube = |at: Rigid| {
            ryukei::primitive::block(OpKey::from_bytes(b"q"), 0.01, 0.01, 0.01, &at)
                .unwrap()
                .0
        };
        let a = Prepared::new(&cube(Rigid::IDENTITY)).unwrap();
        let mut found = Vec::new();
        // A alone: the wall escapes downward, along A's face's normal.
        assert!(escapes(
            &a.tree,
            &wall,
            reach,
            margin,
            &mut found,
            nearest(&a.tree, &wall)
        ));
        for (gap, granted) in [(0.5 * margin, false), (2.0 * margin, true)] {
            let b = Prepared::new(&cube(at(0.002 + gap, 0.0, -0.012))).unwrap();
            let both = MeshTree::new(
                a.tree
                    .triangles()
                    .iter()
                    .chain(b.tree.triangles())
                    .copied()
                    .collect(),
            );
            let k = nearest(&both, &wall);
            assert!(k < 12, "the nearest is A's bottom");
            assert_eq!(
                escapes(&both, &wall, reach, margin, &mut found, k),
                granted,
                "{gap}"
            );
        }
    }

    use ryukei::boolean::{BooleanOp, Tool};
    use ryukei::name::ToolKey;

    /// 9.C's plate: 60 × 20 × 1.6 mm, drilled with 38 holes of 1 mm on a
    /// 2.54 mm pitch in two rows of 19, under a 48 × 5 mm footprint at
    /// (6, 7.5) mm; placed by `g`.
    pub(crate) fn drilled_plate(g: &Rigid) -> Body {
        let plate = ryukei::primitive::block(OpKey::from_bytes(b"plate"), 0.06, 0.02, 0.0016, g)
            .unwrap()
            .0;
        let hole = pin(0.0005, 0.004);
        let mut places = Vec::new();
        for i in 0..19 {
            for y in [0.01 - 0.00127, 0.01 + 0.00127] {
                places.push(g.then_after(&at(0.006 + 0.00127 + i as f64 * 0.00254, y, -0.001)));
            }
        }
        let tools: Vec<Tool> = places
            .iter()
            .enumerate()
            .map(|(k, place)| Tool {
                key: ToolKey(OpKey::from_bytes(format!("hole{k}").as_bytes()).0),
                body: &hole,
                place: *place,
            })
            .collect();
        ryukei::boolean::boolean(
            OpKey::from_bytes(b"drill"),
            BooleanOp::Subtract,
            &plate,
            &tools,
        )
        .unwrap()
        .0
    }

    pub(crate) fn header() -> Body {
        ryukei::primitive::block(
            OpKey::from_bytes(b"hdr"),
            0.048,
            0.005,
            0.0085,
            &Rigid::IDENTITY,
        )
        .unwrap()
        .0
    }

    /// Where the header stands flush on the plate, in the plate's frame.
    fn on_plate() -> Rigid {
        at(0.006, 0.0075, 0.0016)
    }

    #[test]
    fn a_header_flush_on_a_drilled_plate_is_decided_both_ways() {
        let plate = Prepared::new(&drilled_plate(&Rigid::IDENTITY)).unwrap();
        let hdr = Prepared::new(&header()).unwrap();
        let (v, spent) = judged(&plate, &hdr, &on_plate());
        println!("plate, header: {spent} pieces");
        assert_eq!(v, Verdict::Touching);
        assert!(spent < 1 << 14, "{spent}");
        let (v, spent) = judged(&hdr, &plate, &on_plate().inverse());
        println!("header, plate: {spent} pieces");
        assert_eq!(v, Verdict::Touching);
        assert!(spent < 1 << 14, "{spent}");
    }

    fn united(key: &[u8], body: &Body, parts: &[(Body, Rigid)]) -> Body {
        let tools: Vec<Tool> = parts
            .iter()
            .enumerate()
            .map(|(k, (b, place))| Tool {
                key: ToolKey(OpKey::from_bytes(format!("{k}").as_bytes()).0),
                body: b,
                place: *place,
            })
            .collect();
        ryukei::boolean::boolean(OpKey::from_bytes(key), BooleanOp::Unite, body, &tools)
            .unwrap()
            .0
    }

    /// The header with 38 square pins of 0.64 mm through its holes, from
    /// 3 mm under the plate's bottom to its top.
    fn pinned_header() -> Body {
        let mut pins = Vec::new();
        for i in 0..19 {
            for y in [0.0025 - 0.00127, 0.0025 + 0.00127] {
                let pin = ryukei::primitive::block(
                    OpKey::from_bytes(format!("pin{i}{y}").as_bytes()),
                    0.00064,
                    0.00064,
                    0.0131,
                    &Rigid::IDENTITY,
                )
                .unwrap()
                .0;
                pins.push((
                    pin,
                    at(0.00127 + i as f64 * 0.00254 - 0.00032, y - 0.00032, -0.0046),
                ));
            }
        }
        united(b"pinned", &header(), &pins)
    }

    /// A 10 × 10 × 1.5 mm body on 0.2 mm feet, four a side, flush on the
    /// plate where its feet stand (its own frame's z = 0).
    fn gull_wing() -> Body {
        let body = ryukei::primitive::block(
            OpKey::from_bytes(b"smd"),
            0.01,
            0.01,
            0.0015,
            &at(0.0, 0.0, 0.0002),
        )
        .unwrap()
        .0;
        let mut feet = Vec::new();
        for k in 0..4 {
            let y = 0.0015 + k as f64 * 0.0022;
            for x in [-0.001, 0.0095] {
                let foot = ryukei::primitive::block(
                    OpKey::from_bytes(format!("foot{k}{x}").as_bytes()),
                    0.0015,
                    0.0003,
                    0.0002,
                    &Rigid::IDENTITY,
                )
                .unwrap()
                .0;
                feet.push((foot, at(x, y, 0.0)));
            }
        }
        united(b"gull", &body, &feet)
    }

    #[test]
    fn pins_through_holes_and_gull_wing_feet_only_touch() {
        let plate = Prepared::new(&drilled_plate(&Rigid::IDENTITY)).unwrap();
        let pinned = Prepared::new(&pinned_header()).unwrap();
        let smd = Prepared::new(&gull_wing()).unwrap();
        for (what, other, rel) in [
            ("pins", &pinned, on_plate()),
            ("gull wing", &smd, at(0.02, 0.005, 0.0016)),
        ] {
            let (v, spent) = judged(&plate, other, &rel);
            println!("plate, {what}: {spent} pieces");
            assert_eq!(v, Verdict::Touching, "{what}");
            assert!(spent < 1 << 15, "{what}: {spent}");
            let (v, spent) = judged(other, &plate, &rel.inverse());
            println!("{what}, plate: {spent} pieces");
            assert_eq!(v, Verdict::Touching, "{what}, swapped");
            assert!(spent < 1 << 15, "{what}, swapped: {spent}");
        }
    }

    #[test]
    fn a_header_sunk_raised_or_apart_is_judged_by_its_depth() {
        let plate = Prepared::new(&drilled_plate(&Rigid::IDENTITY)).unwrap();
        let hdr = Prepared::new(&header()).unwrap();
        let rho = plate.reach + hdr.reach;
        for (dz, sunk) in [
            (-2.0 * rho, true),
            (-0.25 * rho, false),
            (0.5 * rho, false),
            (2.0 * rho, false),
        ] {
            let rel = at(0.006, 0.0075, 0.0016 + dz);
            for (v, side) in [
                (judge(&plate, &hdr, &rel), "plate first"),
                (judge(&hdr, &plate, &rel.inverse()), "header first"),
            ] {
                match (sunk, dz > rho) {
                    (true, _) => assert!(
                        matches!(v, Verdict::Approximate { open: false, .. }),
                        "{dz} {side}: {v:?}"
                    ),
                    (false, true) => assert_eq!(v, Verdict::Clear, "{dz} {side}"),
                    (false, false) => assert_eq!(v, Verdict::Touching, "{dz} {side}"),
                }
            }
        }
    }

    #[test]
    fn a_flush_sheet_only_touches() {
        // The header's bottom face as a sheet (open: no front to trust).
        let h = header();
        let prepared = Prepared::new(&h).unwrap();
        let bottom = prepared
            .tree
            .triangles()
            .iter()
            .position(|t| t.iter().all(|q| q.z == 0.0))
            .map(|k| prepared.faces[k])
            .expect("a face on z = 0");
        let (sheet, _) = ryukei::sheet::extract(OpKey::from_bytes(b"sh"), &h, &[bottom]).unwrap();
        let s = Prepared::new(&sheet).unwrap();
        assert!(!s.closed);
        let plate = Prepared::new(&drilled_plate(&Rigid::IDENTITY)).unwrap();
        let (v, spent) = judged(&plate, &s, &on_plate());
        assert!(matches!(v, Verdict::Touching), "{v:?}");
        assert!(spent < 1 << 14, "{spent}");
        let (v, _) = judged(&s, &plate, &on_plate().inverse());
        assert!(matches!(v, Verdict::Touching), "{v:?}");
    }

    #[test]
    fn a_flush_header_on_a_turned_plate_is_decided() {
        let mut g = ryukei_testkit::Lcg::new(525);
        let hdr = header();
        for k in 0..3 {
            let turn =
                Rigid::rotation(ryukei::geom::UnitVector3::Z, g.next_f64(0.0, 6.2)).then_after(
                    &Rigid::rotation(ryukei::geom::UnitVector3::X, g.next_f64(-0.5, 0.5)),
                );
            let pose = at(
                g.next_f64(-0.05, 0.05),
                g.next_f64(-0.05, 0.05),
                g.next_f64(-0.05, 0.05),
            )
            .then_after(&turn);
            let plate = Prepared::new(&drilled_plate(&pose)).unwrap();
            let h = Prepared::new(&hdr).unwrap();
            let rel = pose.then_after(&on_plate());
            let (v, spent) = judged(&plate, &h, &rel);
            assert_eq!(v, Verdict::Touching, "pose {k}");
            assert!(spent < 1 << 15, "pose {k}: {spent}");
            let (v, _) = judged(&h, &plate, &rel.inverse());
            assert_eq!(v, Verdict::Touching, "pose {k}, swapped");
        }
    }

    #[test]
    fn a_piece_in_a_hole_escapes_along_it_only_past_the_margin() {
        // A wall piece in x = 2 mm, in a tube of blocks open only upwards:
        // its own normal (±x) and the nearest face's (±y, block A's at
        // y = 0.95 mm) meet blocks, and so does down; up, along both
        // faces (the hole's axis), block B stands beside the prism, `gap`
        // off its plane: withheld at half the margin, granted at twice it.
        let margin = 1e-12;
        let reach = 1e-5;
        let mm = 1e-3;
        let wall = [
            p(2.0 * mm, mm, mm),
            p(2.0 * mm, 3.0 * mm, mm),
            p(2.0 * mm, 2.0 * mm, 2.0 * mm),
        ];
        let cube = |lo: [f64; 3], hi: [f64; 3]| {
            ryukei::primitive::block(
                OpKey::from_bytes(b"q"),
                (hi[0] - lo[0]) * mm,
                (hi[1] - lo[1]) * mm,
                (hi[2] - lo[2]) * mm,
                &at(lo[0] * mm, lo[1] * mm, lo[2] * mm),
            )
            .unwrap()
            .0
        };
        let tube = [
            cube([0.0, -1.0, 0.0], [4.0, 0.95, 3.0]), // A, -y
            cube([0.0, 4.0, 0.0], [4.0, 5.0, 3.0]),   // +y
            cube([0.0, 0.95, 0.0], [1.0, 4.0, 3.0]),  // -x
            cube([3.0, 0.95, 0.0], [4.0, 4.0, 3.0]),  // +x
            cube([0.0, 0.95, -1.0], [4.0, 4.0, 0.5]), // below
        ];
        let mut tris: Vec<[Point3; 3]> = Vec::new();
        for b in &tube {
            tris.extend(Prepared::new(b).unwrap().tree.triangles());
        }
        let face = tris
            .iter()
            .position(|t| t.iter().all(|q| (q.y - 0.95 * mm).abs() < 1e-12))
            .expect("A's face at y = 0.95 mm") as u32;
        let mut found = Vec::new();
        let open = MeshTree::new(tris.clone());
        assert!(escapes(&open, &wall, reach, margin, &mut found, face));
        for (gap, granted) in [(0.5 * margin, false), (2.0 * margin, true)] {
            let b = cube([2.0 + gap / mm, 0.95, 2.1], [2.5, 4.0, 3.0]);
            let mut all = tris.clone();
            all.extend(Prepared::new(&b).unwrap().tree.triangles());
            let m = MeshTree::new(all);
            assert_eq!(
                escapes(&m, &wall, reach, margin, &mut found, face),
                granted,
                "{gap}"
            );
        }
    }

    #[test]
    fn a_header_sunk_less_than_the_reach_is_decided_in_few_pieces() {
        // The header's top 0.7 of the reach into the plate's bottom: an
        // overlap no deeper than the reach, so touching, and the pieces
        // crossing either surface escape past it (the prism starts at
        // 0.9 of the reach, not half).
        let plate = Prepared::new(&drilled_plate(&Rigid::IDENTITY)).unwrap();
        let hdr = Prepared::new(&header()).unwrap();
        let rho = plate.reach + hdr.reach;
        let rel = at(0.006, 0.0075, -0.0085 + 0.7 * rho);
        let (v, spent) = judged(&plate, &hdr, &rel);
        println!("plate, sunk header: {spent} pieces");
        assert_eq!(v, Verdict::Touching);
        assert!(spent < 1 << 15, "{spent}");
        let (v, spent) = judged(&hdr, &plate, &rel.inverse());
        println!("sunk header, plate: {spent} pieces");
        assert_eq!(v, Verdict::Touching);
        assert!(spent < 1 << 15, "{spent}");
    }
}
