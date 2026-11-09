//! Enclosures of a Bézier patch's jets over boxes small against it, for
//! the mesher's per-triangle bounds (#305, #307, #308): a point value and
//! a Taylor remainder, in place of cutting six interval nets down to
//! each box.
//!
//! [`super::patch_jets`] cuts the patch's homogeneous net and its five
//! derivative nets to the box by interval de Casteljau: for a degree 6 × 10
//! rational patch about 23,000 interval lerps a box, a millisecond on an
//! Opteron 6380. Here each patch is covered by cells, each way a level of
//! its own (the whole of it, halves or quarters, on two grids
//! half a cell apart), so a box takes the smallest cell holding it along
//! each direction, one no wider than half a cell at a level always lying in
//! a cell of that level, and a box long and thin in the parameters (the
//! mesher's planes stretch) a cell long and thin too. A cell keeps, once, its
//! nets of orders 0 to 4 cut to it and centred on it, as `patch_jets`
//! makes them for a box. A box then costs one f64 de Casteljau evaluation
//! of each order 0 to 3 net at its middle, about 7,000 lerps for that
//! patch, and each value net of order 0 to 2 is enclosed about its value
//! there by the lesser of two radii, each a sum of terms:
//!
//! - the coefficients' radius: each interval coefficient `[lo, hi]` is
//!   within `rad` of its midpoint, and Bernstein weights are non-negative
//!   and sum to one, so the polynomial of the midpoints is within the
//!   largest `rad` of every polynomial of the intervals;
//! - rounding: one lerp `fl(a + fl(t fl(b - a)))` with `t` in `[0, 1]` and
//!   `|a|, |b| <= M` is within `5u M + O(u²) <= 6u M` of `a + t (b - a)`
//!   (`u = 2^-53`), errors already in `a` and `b` pass through a convex
//!   combination without growing, and every value stays within
//!   `M (1 + 6u)^k` after `k` levels; a tensor net of degrees `p` and `q`
//!   takes `p` levels along each column and then `q` along the row, so the
//!   value is within `6u (p + q) M (1 + 6u)^(p + q) <= 7u (p + q) M` of the
//!   midpoints' polynomial (any `p + q` under 10¹⁴), `M` the largest
//!   midpoint's magnitude. These two are a net's `err`;
//! - first order, the mean value theorem: along the segment from the
//!   evaluated point `x0` to any `x` of the box (inside the cell, which is
//!   convex), `N(x) - N(x0) = N_u(ξ) Δu + N_v(ξ) Δv`, so each component
//!   moves by at most the cell's bound on `|N_u|` times the box's
//!   half-width `h_u` from `x0`, and likewise along `v`; `N_u` and `N_v` are
//!   the next order's nets, whose cut to the cell holds them (a Bézier
//!   patch lies in its control points' convex hull, and the interval cut
//!   holds the true control points);
//! - or second order, Taylor's theorem on the same segment:
//!   `N(x) - N(x0) = N_u(x0) Δu + N_v(x0) Δv + ½ (N_uu(ξ) Δu² + 2 N_uv(ξ)
//!   Δu Δv + N_vv(ξ) Δv²)`, the gradient evaluated at `x0` (with its own
//!   `err`) and the second derivatives bounded over the cell by the nets
//!   two orders on. The cell-wide bound on `|N_u|` can be several times
//!   its value at a small box, so this one is the tighter as boxes shrink.
//!
//! Both radii hold about the same value, so the lesser does.
//!
//! The quotient rule is then [`super::patch_jets`]'s, in intervals. The
//! bits differ from the cut's (a box's enclosure is the cell's evaluation,
//! not a cut), so meshes of spline faces moved, explained with the golden
//! sections that hold them. A cell whose nets aren't finite leaves its
//! boxes to the cut, as before.

use super::{H, Iv, Net, V3, centre, centred, div3, hom, hull_or_zero, parts, pt, scale3, sub3};
use crate::geom::Vector3;
use crate::geom::nurbs::bezier::BezierPatch;
use crate::interval::{add_up, mul_up};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// The levels of cells along each direction: at level `l` a patch's
/// local `[0, 1]` is split into `2^l` cells on each of two grids half a
/// cell apart, so cell `k` (from 0 to `2^(l + 1) - 2`) covers
/// `[k / 2^(l + 1), (k + 2) / 2^(l + 1)]`. Each direction takes its own
/// level, so a box long and thin in the parameters (the mesher's planes
/// stretch) takes a cell long and thin too.
///
/// Three levels, not four: eighths built four times the cells for
/// linkrods' mass, twice its memory and more time than they saved.
const LEVELS: usize = 3;

/// Cell origins along a direction at level `l`.
const fn steps(l: usize) -> usize {
    (2 << l) - 1
}

/// Where a level's cells start in a direction's list of every level's.
const fn first(l: usize) -> usize {
    // The sum of `steps` below `l`: `2^(l + 1) - 2 - l`.
    (2 << l) - 2 - l
}

/// Cells along one direction, every level.
const ALONG: usize = first(LEVELS);

/// A cell along one direction: its level and its origin's index.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    level: usize,
    k: usize,
}

impl Span {
    /// Half a cell at this level, a power of two.
    fn half(self) -> f64 {
        1.0 / (2u64 << self.level) as f64
    }
    /// Cells across `[0, 1]`, a power of two.
    fn cells(self) -> f64 {
        (1u64 << self.level) as f64
    }
    /// The cell's local range.
    fn range(self) -> [f64; 2] {
        let h = self.half();
        [self.k as f64 * h, (self.k + 2) as f64 * h]
    }
    fn index(self) -> usize {
        first(self.level) + self.k
    }
}

/// The unit roundoff of f64.
const U: f64 = 1.0 / 9_007_199_254_740_992.0;

/// One net cut to a cell, ready to evaluate: midpoints row-major, and per
/// component the largest radius plus the rounding bound of evaluating it.
struct Values {
    rows: usize,
    cols: usize,
    mids: Vec<[f64; 4]>,
    err: [f64; 4],
}

/// The nets a cell keeps, by their orders `(i, j)` (`i` along `u`):
/// values of orders 0 to 3 are evaluated at a box's middle, and orders 1
/// to 4 bound the remainders.
const ORDERS: [(usize, usize); 15] = [
    (0, 0),
    (1, 0),
    (0, 1),
    (2, 0),
    (1, 1),
    (0, 2),
    (3, 0),
    (2, 1),
    (1, 2),
    (0, 3),
    (4, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 4),
];

/// The index in [`ORDERS`] of order `(i, j)`, `i + j <= 4`.
const fn idx(i: usize, j: usize) -> usize {
    let n = i + j;
    n * (n + 1) / 2 + j
}

/// Nets evaluated at a box's middle: orders 0 to 3.
const EVALUATED: usize = 10;

/// One cell: its centre, its nets of orders 0 to 3 ready to evaluate
/// (homogeneous, centred on `at`, in the surface's parameters), and every
/// net's componentwise magnitude bound over the cell (`mags[0]` unused).
struct Cell {
    at: Vector3,
    /// Every weight of the patch is 1: the homogeneous weight is exactly
    /// 1 and its derivatives exactly 0, so the point's jets are the first
    /// three components' own enclosures, with no quotient rule.
    polynomial: bool,
    values: [Values; EVALUATED],
    mags: [[f64; 4]; 15],
}

/// The cells every surface's patches have built, by the surface's id
/// (process-wide, never reused), the patch's index and the cell's: `None`
/// where a cell's nets aren't finite (the cut serves its boxes). A cell is
/// a function of its key alone, so a hit and a build give the same bits
/// on any thread. Kept on the patches, a surface of thousands of patches
/// held gigabytes of cells as long as it lived (KiCad's Smolhaj model,
/// 4.3 GB); here each shard keeps two generations of at most
/// [`SHARD_BYTES`] each.
struct Table {
    shards: [Mutex<Shard>; SHARDS],
}

/// A shard's cells in two generations: `young` takes what is built or
/// found in `old`; when `young` passes the limit it becomes `old` and the
/// old one goes, so only cells no box asked for during a whole generation
/// are dropped (emptying the shard instead built linkrods' cells again
/// and again: twice its mass's time).
#[derive(Default)]
struct Shard {
    young: HashMap<Key, Option<Arc<Cell>>>,
    old: HashMap<Key, Option<Arc<Cell>>>,
    bytes: usize,
}

type Key = (u64, u32, u32);

impl Shard {
    /// Keeps a cell in `young`, turning the generations over first when
    /// it would pass the limit.
    fn keep(&mut self, key: Key, cell: Option<Arc<Cell>>) {
        let size = cell.as_ref().map_or(64, |c| c.bytes() + 64);
        if self.bytes + size > CAP.load(Ordering::Relaxed) {
            self.old = core::mem::take(&mut self.young);
            self.bytes = 0;
        }
        self.bytes += size;
        self.young.insert(key, cell);
    }
}

/// How many locks the table is split under.
const SHARDS: usize = 16;

/// The most bytes of cells a shard's young generation holds: 8 MiB, so
/// at most 256 MiB in all, both generations of every shard. linkrods'
/// cells fit (its peak RSS 335 MB uncapped); KiCad's Smolhaj model's
/// don't, and are built again once a generation has passed them by.
const SHARD_BYTES: usize = 8 << 20;

/// The limit in force: [`SHARD_BYTES`], but tests lower it to force
/// evictions (which change nothing but the time).
static CAP: AtomicUsize = AtomicUsize::new(SHARD_BYTES);

static TABLE: LazyLock<Table> = LazyLock::new(|| Table {
    shards: core::array::from_fn(|_| Mutex::new(Shard::default())),
});

impl Cell {
    /// About what the cell holds, for the table's limit.
    fn bytes(&self) -> usize {
        core::mem::size_of::<Self>()
            + self
                .values
                .iter()
                .map(|v| v.mids.len() * core::mem::size_of::<[f64; 4]>())
                .sum::<usize>()
    }
}

/// The cell `(ks, kt)` of the surface's patch `index`, from the table or
/// built and kept there.
fn cell(surface: u64, index: usize, patch: &BezierPatch, ks: Span, kt: Span) -> Option<Arc<Cell>> {
    let key = (
        surface,
        index as u32,
        (ks.index() * ALONG + kt.index()) as u32,
    );
    // The cells this thread asked for last: a mesher's boxes come one
    // face at a time, neighbours after neighbours, so most find their cell
    // here without the table's lock and hash (#308). A cell is a function
    // of its key, so a hit here is the table's cell, bit for bit.
    let recent = |found: Option<&Option<Arc<Cell>>>| found.cloned();
    if let Some(c) = RECENT.with(|r| recent(r.borrow().iter().find(|e| e.0 == key).map(|e| &e.1))) {
        return c;
    }
    let found = table_cell(key, patch, ks, kt);
    RECENT.with(|r| {
        let mut r = r.borrow_mut();
        if r.len() == RECENT_CELLS {
            r.pop_back();
        }
        r.push_front((key, found.clone()));
    });
    found
}

/// How many cells each thread keeps of those it asked for last.
const RECENT_CELLS: usize = 4;

thread_local! {
    static RECENT: core::cell::RefCell<std::collections::VecDeque<(Key, Option<Arc<Cell>>)>> =
        const { core::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// [`cell`] from the process-wide table.
fn table_cell(key: Key, patch: &BezierPatch, ks: Span, kt: Span) -> Option<Arc<Cell>> {
    // SipHash-free spread: the id and indices mixed by a fixed odd
    // multiplier, so neighbouring cells take different shards.
    let mix =
        (key.0 ^ (u64::from(key.1) << 20) ^ u64::from(key.2)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let shard = &TABLE.shards[(mix >> 60) as usize % SHARDS];
    {
        let mut s = shard.lock().expect("unpoisoned");
        if let Some(c) = s.young.get(&key) {
            return c.clone();
        }
        if let Some(c) = s.old.remove(&key) {
            s.keep(key, c.clone());
            return c;
        }
    }
    let built = build(patch, ks, kt).map(Arc::new);
    shard.lock().expect("unpoisoned").keep(key, built.clone());
    built
}

/// The smallest cell at a level below [`LEVELS`] holding `[s0, s1]`
/// (local, inside `[0, 1]`), if one does: `s0 / half` is exact (`half` a
/// power of two).
fn step(s0: f64, s1: f64) -> Option<Span> {
    (0..LEVELS).rev().find_map(|level| {
        let span = Span { level, k: 0 };
        let half = span.half();
        let k = ((s0 / half) as usize).min(steps(level) - 1);
        let span = Span { level, k };
        let [lo, hi] = span.range();
        (lo <= s0 && s1 <= hi).then_some(span)
    })
}

/// The patch's jets over the local box `[s, t]` as [`super::patch_jets`]
/// gives them (relative to the returned centre), by a cell's evaluation;
/// `None` where the box isn't inside one cell, or the cell or the result
/// isn't finite.
pub(super) fn jets(
    surface: u64,
    index: usize,
    patch: &BezierPatch,
    s: [f64; 2],
    t: [f64; 2],
) -> Option<(Vector3, [V3; 6])> {
    let (ks, kt) = (step(s[0], s[1])?, step(t[0], t[1])?);
    let cell = cell(surface, index, patch, ks, kt)?;
    let jets = enclose(&cell, patch.range, [s, t], [ks, kt], cell.polynomial)?;
    Some((cell.at, jets))
}

/// The patch's point and derivatives to third order over the local box
/// `[s, t]`, by [`crate::geom::nurbs::third::ORDERS`] (this module's first
/// ten [`ORDERS`]), relative to the returned centre as [`jets`] gives its
/// six: orders 0 to 2 as [`enclose`] takes them, order 3 within the mean
/// value form, then the quotient rule as [`super::patch_jet3`] takes it.
/// `None` where the box isn't inside one cell, or the cell or the result
/// isn't finite (#254: the fit's third-order bound cut ten nets to every
/// span's box).
pub(super) fn jets3(
    surface: u64,
    index: usize,
    patch: &BezierPatch,
    s: [f64; 2],
    t: [f64; 2],
) -> Option<(Vector3, [V3; 10])> {
    let (ks, kt) = (step(s[0], s[1])?, step(t[0], t[1])?);
    let cell = cell(surface, index, patch, ks, kt)?;
    let jets = enclose3(&cell, patch.range, [s, t], [ks, kt], cell.polynomial)?;
    Some((cell.at, jets))
}

/// [`jets3`]'s enclosures from a cell.
fn enclose3(
    cell: &Cell,
    [ru, rv]: [[f64; 2]; 2],
    [s, t]: [[f64; 2]; 2],
    [ks, kt]: [Span; 2],
    polynomial: bool,
) -> Option<[V3; 10]> {
    let (su, hu) = place(s, ks, ru);
    let (sv, hv) = place(t, kt, rv);
    let at_point: [[f64; 4]; EVALUATED] =
        core::array::from_fn(|k| evaluate(&cell.values[k], su, sv));
    let comps = if polynomial { 3 } else { 4 };
    let mut out: [H; EVALUATED] = [[pt(0.0); 4]; EVALUATED];
    for (k, o) in out.iter_mut().enumerate() {
        let (i, j) = ORDERS[k];
        let (ku, kv) = (idx(i + 1, j), idx(i, j + 1));
        for c in 0..comps {
            let value = at_point[k][c];
            let r = if i + j < 3 {
                let (kuu, kuv, kvv) = (idx(i + 2, j), idx(i + 1, j + 1), idx(i, j + 2));
                radius(
                    cell,
                    [k, ku, kv, kuu, kuv, kvv],
                    c,
                    [at_point[ku][c], at_point[kv][c]],
                    [hu, hv],
                )
            } else {
                first_radius(cell, [k, ku, kv], c, [hu, hv])
            };
            if !(r.is_finite() && value.is_finite()) {
                return None;
            }
            o[c] = pt(value) + Iv::new(-r, r).expect("ordered");
        }
    }
    let finite = |jets: &[V3; 10]| {
        jets.iter()
            .flatten()
            .all(|x| x.lo().is_finite() && x.hi().is_finite())
    };
    if polynomial {
        let jets = out.map(|o| [o[0], o[1], o[2]]);
        return finite(&jets).then_some(jets);
    }
    let mut a = [[pt(0.0); 3]; 10];
    let mut w = [pt(0.0); 10];
    for k in 0..10 {
        let (ak, wk) = parts(out[k]);
        a[k] = ak;
        w[k] = wk;
    }
    a[0] = div3(a[0], w[0]);
    let jets =
        crate::geom::nurbs::third::quotient_from(a, w, |x, y, c| scale3(x, y * pt(c)), div3, sub3);
    finite(&jets).then_some(jets)
}

/// A box's middle in its cell's own `[0, 1]`, and its half-width from the
/// evaluated point in the surface's parameter, rounded up. `mid - c0`
/// rounds by at most 2^-54 and `* cells` is exact, so the point
/// evaluated is within 2^-54 of the middle in the patch's local
/// parameters, which `4 U` covers.
fn place([a, b]: [f64; 2], span: Span, [r0, r1]: [f64; 2]) -> (f64, f64) {
    let mid = 0.5 * (a + b);
    let c0 = span.range()[0];
    let sigma = ((mid - c0) * span.cells()).clamp(0.0, 1.0);
    // The local half-width from the evaluated point, then in the
    // surface's parameter: `(r1 - r0) h`, rounded up.
    let h = ((pt(b) - pt(mid)).hi().max((pt(mid) - pt(a)).hi()) + 4.0 * U).max(0.0);
    let width = (pt(r1) - pt(r0)).hi();
    (sigma, (pt(h) * pt(width)).hi())
}

/// The jets over the local box `[s, t]` from its cells `[ks, kt]` of a
/// patch over `range`, by the quotient rule unless `polynomial` (which the
/// cell must be: tests compare both ways).
fn enclose(
    cell: &Cell,
    [ru, rv]: [[f64; 2]; 2],
    [s, t]: [[f64; 2]; 2],
    [ks, kt]: [Span; 2],
    polynomial: bool,
) -> Option<[V3; 6]> {
    let (su, hu) = place(s, ks, ru);
    let (sv, hv) = place(t, kt, rv);
    // Every evaluated net's value at the point, and its own error.
    let at_point: [[f64; 4]; EVALUATED] =
        core::array::from_fn(|k| evaluate(&cell.values[k], su, sv));
    // A polynomial patch's weight is 1 exactly and every derivative of it
    // 0, so its jets are the first three components' enclosures: no
    // weight's radii and no quotient rule, which in intervals only
    // widened them (dividing by an enclosure of 1, less the point times an
    // enclosure of 0). 98 of the 100 spline faces of KiCad's Smolhaj
    // model are so (#308).
    let comps = if polynomial { 3 } else { 4 };
    let mut out: [H; 6] = [[pt(0.0); 4]; 6];
    for (k, o) in out.iter_mut().enumerate() {
        let (i, j) = ORDERS[k];
        let (ku, kv) = (idx(i + 1, j), idx(i, j + 1));
        let (kuu, kuv, kvv) = (idx(i + 2, j), idx(i + 1, j + 1), idx(i, j + 2));
        for c in 0..comps {
            let value = at_point[k][c];
            let r = radius(
                cell,
                [k, ku, kv, kuu, kuv, kvv],
                c,
                [at_point[ku][c], at_point[kv][c]],
                [hu, hv],
            );
            if !(r.is_finite() && value.is_finite()) {
                return None;
            }
            o[c] = pt(value) + Iv::new(-r, r).expect("ordered");
        }
    }
    let finite = |jets: &[V3; 6]| {
        jets.iter()
            .flatten()
            .all(|x| x.lo().is_finite() && x.hi().is_finite())
    };
    if polynomial {
        let jets = out.map(|o| [o[0], o[1], o[2]]);
        return finite(&jets).then_some(jets);
    }
    let [a0, au, av, auu, auv, avv] = out.map(parts);
    let (a, w) = a0;
    let ((au, wu), (av, wv)) = (au, av);
    let ((auu, wuu), (auv, wuv), (avv, wvv)) = (auu, auv, avv);
    // The quotient rule, as `patch_jets` takes it.
    let c = div3(a, w);
    let su = div3(sub3(au, scale3(c, wu)), w);
    let sv = div3(sub3(av, scale3(c, wv)), w);
    let suu = div3(sub3(sub3(auu, scale3(su, pt(2.0) * wu)), scale3(c, wuu)), w);
    let svv = div3(sub3(sub3(avv, scale3(sv, pt(2.0) * wv)), scale3(c, wvv)), w);
    let suv = div3(
        sub3(
            sub3(sub3(auv, scale3(sv, wu)), scale3(su, wv)),
            scale3(c, wuv),
        ),
        w,
    );
    let jets = [c, su, sv, suu, suv, svv];
    finite(&jets).then_some(jets)
}

/// The radius about a net's value at the evaluated point that holds its
/// values over the box, for component `c` of the net `n[0]` (`n` its
/// nets along `u` and `v`, then its second-order ones), the gradient's
/// values `g` at the point and the box's half-widths `h` (upper ends
/// [`jets`] rounded up, so at least `TINY`): the lesser of
/// the mean value form and the second-order Taylor form (see the module's
/// notes).
///
/// Every term is non-negative, so only upper ends are taken, by
/// [`add_up`] and [`mul_up`], in the order the interval expressions
///
/// ```text
/// first  = err + |N_u| hu + |N_v| hv
/// second = err + (|g_u| + err_u) hu + (|g_v| + err_v) hv
///        + 0.5 (|N_uu| hu hu + 2 |N_uv| hu hv + |N_vv| hv hv)
/// ```
///
/// took them in point intervals, so the same floats for a third of the
/// work: those expressions computed both ends of every step, three
/// quarters of mass's fine mesh of KiCad's Smolhaj model (#308).
fn radius(cell: &Cell, n: [usize; 6], c: usize, g: [f64; 2], [hu, hv]: [f64; 2]) -> f64 {
    let [k, ku, kv, kuu, kuv, kvv] = n;
    let m = |k: usize| cell.mags[k][c];
    let err = cell.values[k].err[c];
    // First order: the mean value form, `|N_u|` and `|N_v|` over the
    // cell.
    let first = add_up(add_up(err, mul_up(m(ku), hu)), mul_up(m(kv), hv));
    // Second order: the gradient at the point (its own errors added), and
    // half the Hessian's bound over the cell.
    let gu = add_up(g[0].abs(), cell.values[ku].err[c]);
    let gv = add_up(g[1].abs(), cell.values[kv].err[c]);
    let rest = add_up(
        add_up(
            mul_up(mul_up(m(kuu), hu), hu),
            mul_up(mul_up(mul_up(2.0, m(kuv)), hu), hv),
        ),
        mul_up(mul_up(m(kvv), hv), hv),
    );
    let second = add_up(
        add_up(add_up(err, mul_up(gu, hu)), mul_up(gv, hv)),
        mul_up(0.5, rest),
    );
    // Both hold, about the same value: the lesser does.
    first.min(second)
}

/// The mean value form alone (the first of [`radius`]'s two), for a
/// third-order net, whose second-order form would read fifth-order nets
/// a cell doesn't keep.
fn first_radius(cell: &Cell, [k, ku, kv]: [usize; 3], c: usize, [hu, hv]: [f64; 2]) -> f64 {
    let m = |k: usize| cell.mags[k][c];
    add_up(
        add_up(cell.values[k].err[c], mul_up(m(ku), hu)),
        mul_up(m(kv), hv),
    )
}

/// A net's components at local `(su, sv)` by f64 de Casteljau,
/// `a + t (b - a)` at each level: along each column (`u`) then along the
/// row. An empty net (a derivative of a constant) is zero.
fn evaluate(v: &Values, su: f64, sv: f64) -> [f64; 4] {
    if v.mids.is_empty() {
        return [0.0; 4];
    }
    // The same arithmetic in either size: a small net (bicubic and
    // below, most of what files hold) takes stack rows of 8 rather than
    // clearing rows for the largest degree on every evaluation (#308).
    if v.rows <= SMALL && v.cols <= SMALL {
        evaluate_in::<SMALL>(v, su, sv)
    } else {
        evaluate_in::<{ crate::geom::nurbs::MAX_DEGREE + 1 }>(v, su, sv)
    }
}

/// The largest net [`evaluate`] takes in its small rows.
const SMALL: usize = 8;

/// [`evaluate`] with rows of `N`, at least the net's rows and columns.
fn evaluate_in<const N: usize>(v: &Values, su: f64, sv: f64) -> [f64; 4] {
    let mut column = [[0.0f64; 4]; N];
    let mut row = [[0.0f64; 4]; N];
    let lerp = |a: [f64; 4], b: [f64; 4], t: f64| -> [f64; 4] {
        core::array::from_fn(|k| a[k] + t * (b[k] - a[k]))
    };
    for (j, r) in row.iter_mut().enumerate().take(v.cols) {
        for (i, c) in column.iter_mut().enumerate().take(v.rows) {
            *c = v.mids[i * v.cols + j];
        }
        for level in 1..v.rows {
            for i in 0..v.rows - level {
                column[i] = lerp(column[i], column[i + 1], su);
            }
        }
        *r = column[0];
    }
    for level in 1..v.cols {
        for j in 0..v.cols - level {
            row[j] = lerp(row[j], row[j + 1], sv);
        }
    }
    row[0]
}

/// A cell's nets: the patch's homogeneous net cut to the cell gives its
/// centre, the whole net centred there is differenced up to third order
/// (as `patch_jets` and `patch_jet3` difference it, before the cut), and
/// each net is cut to the cell.
fn build(patch: &BezierPatch, ks: Span, kt: Span) -> Option<Cell> {
    let [p, q] = patch.degrees;
    let net = Net {
        pts: patch.pts.iter().map(|h| hom(h.p, h.w)).collect(),
        rows: p + 1,
        cols: q + 1,
    };
    let (s, t) = (ks.range(), kt.range());
    let at = centre(&net.part(s, t).pts);
    if !(at.x.is_finite() && at.y.is_finite() && at.z.is_finite()) {
        return None;
    }
    let whole = Net {
        pts: centred(&net.pts, at),
        rows: net.rows,
        cols: net.cols,
    };
    let [ru, rv] = patch.range;
    let iu = pt(1.0).checked_div(pt(ru[1]) - pt(ru[0]))?;
    let iv = pt(1.0).checked_div(pt(rv[1]) - pt(rv[0]))?;
    let du = |n: &Net| n.diff_u(pt(n.rows.saturating_sub(1) as f64) * iu);
    let dv = |n: &Net| n.diff_v(pt(n.cols.saturating_sub(1) as f64) * iv);
    // Each order from the one before it along `u`, else along `v`, over
    // the whole patch, then every net cut to the cell.
    let mut whole_nets: Vec<Net> = Vec::with_capacity(ORDERS.len());
    for &(i, j) in &ORDERS {
        let n = if i == 0 && j == 0 {
            Net {
                pts: whole.pts.clone(),
                rows: whole.rows,
                cols: whole.cols,
            }
        } else if i > 0 {
            du(&whole_nets[idx(i - 1, j)])
        } else {
            dv(&whole_nets[idx(i, j - 1)])
        };
        whole_nets.push(n);
    }
    let nets: Vec<Net> = whole_nets.iter().map(|n| n.part(s, t)).collect();
    // Each net's componentwise magnitude bound over the cell.
    let mut mags = [[0.0f64; 4]; 15];
    for (m, n) in mags.iter_mut().zip(&nets) {
        *m = hull_or_zero(&n.pts).map(super::mag);
    }
    if mags.iter().flatten().any(|x| !x.is_finite()) {
        return None;
    }
    let values: [Values; EVALUATED] = core::array::from_fn(|k| values(&nets[k]));
    if values.iter().any(|v| v.err.iter().any(|e| !e.is_finite())) {
        return None;
    }
    let polynomial = patch.pts.iter().all(|h| h.w == 1.0);
    Some(Cell {
        at,
        polynomial,
        values,
        mags,
    })
}

/// Midpoints of a net's interval coefficients, and per component the
/// largest radius plus `7u (p + q) M`, rounded up.
fn values(n: &Net) -> Values {
    let mut err = [0.0f64; 4];
    let mut mids = Vec::with_capacity(n.pts.len());
    let mut rad = [0.0f64; 4];
    let mut big = [0.0f64; 4];
    for h in &n.pts {
        let m: [f64; 4] = core::array::from_fn(|k| 0.5 * (h[k].lo() + h[k].hi()));
        for k in 0..4 {
            let r = (pt(h[k].hi()) - pt(m[k]))
                .hi()
                .max((pt(m[k]) - pt(h[k].lo())).hi());
            rad[k] = rad[k].max(r);
            big[k] = big[k].max(m[k].abs());
        }
        mids.push(m);
    }
    let levels = (n.rows.saturating_sub(1) + n.cols.saturating_sub(1)) as f64;
    for k in 0..4 {
        err[k] = (pt(rad[k]) + pt(7.0 * U) * pt(levels) * pt(big[k])).hi();
    }
    Values {
        rows: n.rows,
        cols: n.cols,
        mids,
        err,
    }
}

#[cfg(test)]
mod tests;
