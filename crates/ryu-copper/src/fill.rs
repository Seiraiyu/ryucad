//! Pour fills (design 924): an area less grown obstacles, opened to a
//! least width, joined to thermal pads by spokes, its islands dropped.
//! Product-neutral: the caller grows each obstacle by its rule (plus
//! [`BOUND`] and a nanometre) and says which copper is the pour's own.
//! Exact integer booleans throughout (`poly`); the same polygons on
//! every machine and thread count.

use crate::measure::{self, Body, Verdict};
use crate::poly::{self, FillRule, FlattenOptions, Op, Polygons, Rings};
use crate::tree;
use rayon::prelude::*;
use ryu_board::geom::{Mdeg, Nm, Place, Point, Rect};

/// The bound fills are flattened at (ryueda 7.E, question 1305: 1 µm;
/// 7.D's 10 nm put a plane's time in its round corners' vertices, 9.9 s
/// against 0.64 s on the bars' board). Obstacles grow by their rule plus
/// this and a nanometre, so a fill is exactly clear of copper by its rule
/// either way.
pub const BOUND: Nm = 1_000;

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

/// Every ring of `ps`, as one operand.
fn all_rings(ps: &[Polygons]) -> Rings {
    let mut rings = Rings::default();
    for p in ps {
        let r = p.rings();
        rings.bound = rings.bound.max(r.bound);
        rings.rings.extend(r.rings);
    }
    rings
}

/// The union of `ps`.
pub fn union_all(ps: &[Polygons]) -> Polygons {
    poly::simplify(&all_rings(ps), FillRule::Positive)
}

/// A thermal pad's four spokes, along its axes from its centre out past
/// its gap by the least width.
fn spokes(t: &ThermalPad, min_width: Nm) -> Vec<Polygons> {
    let w = t.spoke / 2;
    (0..4)
        .map(|k| {
            let length = t.half[k % 2] + t.gap + min_width.max(t.spoke);
            let place = Place {
                at: t.at,
                rot: Mdeg::new(t.rotation_mdeg + 90_000 * k as i64),
                mirror: false,
                scale: None,
            };
            let ring: Vec<Point> = [(0, -w), (length, -w), (length, w), (0, w)]
                .iter()
                .map(|&(x, y)| place.point(Point::new(x, y)))
                .collect();
            // A turn off the axes rounds each corner to the nanometre.
            poly::simplify(&Rings::new(vec![ring], 1), FillRule::NonZero)
        })
        .collect()
}

/// The area less `obstacles`, opened by half `min_width`. The opening's
/// shrink goes through the difference, (A - U O) shrunk by d being A
/// shrunk by d less each O grown by d: the area is simple and the
/// obstacles small and grown in parallel, so no plane-sized polygon is
/// shrunk. One boolean for the difference (a union first would be a
/// second pass over a plane's thousands of obstacles).
fn opened_of(area: &Polygons, obstacles: &[&Polygons], min_width: Nm) -> Polygons {
    let d = min_width / 2;
    if min_width > 1 {
        let grown: Vec<Polygons> = obstacles
            .par_iter()
            .map(|o| poly::grow(o, d, &opts()))
            .collect();
        let shrunk = poly::boolean(
            &poly::shrink(area, d, &opts()).rings(),
            FillRule::NonZero,
            &all_rings(&grown),
            FillRule::NonZero,
            Op::Difference,
        );
        poly::grow(&shrunk, d, &opts())
    } else {
        let mut rings = Rings::default();
        for o in obstacles {
            let r = o.rings();
            rings.bound = rings.bound.max(r.bound);
            rings.rings.extend(r.rings);
        }
        poly::boolean(
            &area.rings(),
            FillRule::NonZero,
            &rings,
            FillRule::NonZero,
            Op::Difference,
        )
    }
}

/// Whether a spoke is kept: it reaches the opened fill (cut to the
/// spoke's box: a nanometre's rounding at the box's edges, outside the
/// spoke, not a boolean of the whole plane) and misses, exactly, every
/// other net's copper whose box meets it.
fn spoke_kept(s: &Polygons, opened: &Rings, blocked: &[(&Polygons, Option<Rect>)]) -> bool {
    let Some(b) = s.bounds() else {
        return false;
    };
    let near = poly::clip(opened, tree::grown(&b, 2));
    let reaches = !poly::boolean(
        &s.rings(),
        FillRule::NonZero,
        &near,
        FillRule::NonZero,
        Op::Intersection,
    )
    .is_empty();
    reaches
        && blocked
            .iter()
            .filter(|(_, ob)| ob.is_some_and(|ob| tree::meets(&ob, &b)))
            .all(|(o, _)| poly::intersection(s, o).is_empty())
}

/// The fill.
pub fn fill(ask: &Ask) -> Fill {
    let obstacles: Vec<&Polygons> = ask.obstacles.iter().collect();
    let opened = opened_of(&ask.area, &obstacles, ask.min_width);
    let opened_rings = opened.rings();
    let blocked: Vec<(&Polygons, Option<Rect>)> = ask
        .spoke_obstacles
        .iter()
        .map(|o| (o, o.bounds()))
        .collect();
    let mut kept = opened_rings.clone();
    let mut counts = Vec::with_capacity(ask.thermals.len());
    for t in &ask.thermals {
        let mut n = 0u8;
        for s in spokes(t, ask.min_width) {
            if spoke_kept(&s, &opened_rings, &blocked) {
                let r = s.rings();
                kept.bound = kept.bound.max(r.bound);
                kept.rings.extend(r.rings);
                n += 1;
            }
        }
        counts.push(n);
    }
    // The fill and its spokes joined and clipped to the area in one
    // boolean: the opening's grow and the spokes may reach past it (by a
    // flattening bound, or a spoke's length near its edge).
    let joined = poly::boolean(
        &kept,
        FillRule::NonZero,
        &ask.area.rings(),
        FillRule::NonZero,
        Op::Intersection,
    );
    let mut out = Vec::new();
    let mut uncertain = Vec::new();
    let mut removed = 0;
    for p in &joined.polygons {
        let one = Polygons {
            polygons: vec![p.clone()],
            bound: joined.bound,
        };
        match own_contact(&ask.own, &one.body()) {
            Verdict::Yes => out.push(p.clone()),
            Verdict::Uncertain => {
                uncertain.push(out.len());
                out.push(p.clone());
            }
            Verdict::No if ask.keep_islands => out.push(p.clone()),
            Verdict::No => removed += 1,
        }
    }
    Fill {
        polygons: Polygons {
            polygons: out,
            bound: joined.bound,
        },
        islands_removed: removed,
        uncertain,
        spokes: counts,
    }
}

/// Whether a body touches any of the pour's own copper.
fn own_contact(own: &[Body], body: &Body) -> Verdict {
    own.iter()
        .map(|o| measure::touches(o, body))
        .fold(Verdict::No, |acc, v| match (acc, v) {
            (Verdict::Yes, _) | (_, Verdict::Yes) => Verdict::Yes,
            (Verdict::Uncertain, _) | (_, Verdict::Uncertain) => Verdict::Uncertain,
            _ => Verdict::No,
        })
}

/// The tiles' side: fills by tile are cut into squares of it aligned to
/// the origin, each filled from what lies within reach of it (ryueda
/// 7.E, #1067).
pub const TILE: Nm = 8_000_000;

/// How far a tile's seam can move its fill's outline: the exact
/// boolean's snap of the crossings to the nanometre. Callers grow
/// obstacles by this more for tiled fills.
pub const TILE_SNAP: Nm = 1;

/// One tile's fill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileFill {
    /// Its lower-left corner (a multiple of the side).
    pub at: Point,
    /// The hash of what its window's fill and spoke choices read.
    pub key_a: [u8; 16],
    /// The hash of everything its fill read: `key_a` and the kept spokes
    /// reaching it.
    pub key: [u8; 16],
    /// Its polygons, inside the tile, before islands are dropped.
    pub fill: Polygons,
    /// Each of its thermal pads' spoke choices (by the pad's hash), one
    /// bit a spoke.
    pub choices: Vec<([u8; 16], u8)>,
    /// Its polygons (by index) dropped as islands.
    pub dropped: Vec<usize>,
}

/// A fill by tiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tiled {
    pub side: Nm,
    pub tiles: Vec<TileFill>,
    /// Regions (joined across seams) dropped as islands.
    pub islands_removed: usize,
    /// Pieces (tile, polygon) whose contact with the pour's copper the
    /// bounds couldn't decide: kept.
    pub uncertain: Vec<(usize, usize)>,
    /// Spokes kept per thermal pad, in the ask's order.
    pub spokes: Vec<u8>,
}

impl Tiled {
    /// Every kept piece, tile by tile (not merged across seams).
    pub fn polygons(&self) -> Polygons {
        let mut out = Polygons::default();
        for t in &self.tiles {
            out.bound = out.bound.max(t.fill.bound);
            for (i, p) in t.fill.polygons.iter().enumerate() {
                if !t.dropped.contains(&i) {
                    out.polygons.push(p.clone());
                }
            }
        }
        out
    }

    /// Every kept piece with its tile and index, in tile order.
    pub fn pieces(&self) -> impl Iterator<Item = (usize, usize, &poly::Polygon)> {
        self.tiles.iter().enumerate().flat_map(|(ti, t)| {
            t.fill
                .polygons
                .iter()
                .enumerate()
                .filter(move |(i, _)| !t.dropped.contains(i))
                .map(move |(i, p)| (ti, i, p))
        })
    }
}

static TILES_FILLED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many tiles' fills have been computed rather than reused, in this
/// process: for tests and bars.
pub fn tiles_filled() -> usize {
    TILES_FILLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn rect_polys(r: &Rect) -> Polygons {
    let ring = vec![
        r.min,
        Point::new(r.max.x, r.min.y),
        r.max,
        Point::new(r.min.x, r.max.y),
    ];
    poly::simplify(&Rings::new(vec![ring], 0), FillRule::NonZero)
}

/// A 128-bit fingerprint of a tile's inputs: two independent 64-bit
/// multiply-xorshift streams over their integers. Not cryptographic (no
/// one chooses a board to collide with another); the same on every
/// machine.
#[derive(Clone, Copy)]
struct Print(u64, u64);

impl Print {
    fn new(tag: &str) -> Print {
        let mut p = Print(0x9e37_79b9_7f4a_7c15, 0xc2b2_ae3d_27d4_eb4f);
        for b in tag.bytes() {
            p.word(u64::from(b));
        }
        p
    }

    fn word(&mut self, v: u64) {
        let mix = |h: u64, v: u64, k: u64| {
            let x = (h ^ v).wrapping_mul(k);
            x ^ (x >> 29)
        };
        self.0 = mix(self.0, v, 0xff51_afd7_ed55_8ccd);
        self.1 = mix(self.1.rotate_left(17), v, 0xc4ce_b9fe_1a85_ec53);
    }

    fn int(&mut self, v: i64) {
        self.word(v as u64);
    }

    fn print(&mut self, p: &[u8; 16]) {
        self.word(u64::from_le_bytes(p[..8].try_into().unwrap()));
        self.word(u64::from_le_bytes(p[8..].try_into().unwrap()));
    }

    fn finish(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&self.0.to_le_bytes());
        out[8..].copy_from_slice(&self.1.to_le_bytes());
        out
    }
}

fn hash_rings(h: &mut Print, p: &Polygons) {
    h.int(p.bound);
    for poly in &p.polygons {
        for r in poly.rings() {
            h.int(r.len() as i64);
            for q in r {
                h.int(q.x);
                h.int(q.y);
            }
        }
        h.int(-1);
    }
}

fn thermal_hash(t: &ThermalPad) -> [u8; 16] {
    let mut h = Print::new("thermal");
    for v in [
        t.at.x,
        t.at.y,
        t.rotation_mdeg,
        t.half[0],
        t.half[1],
        t.gap,
        t.spoke,
    ] {
        h.int(v);
    }
    h.finish()
}

/// The fill by tiles of `side`, each filled from the ask within reach of
/// it, in parallel; `prev`'s tiles are reused where their inputs hash the
/// same. Spokes are chosen in the tile holding their pad's centre, from
/// that tile's window; islands are found across seams by the pieces'
/// contacts.
pub fn fill_tiled(ask: &Ask, side: Nm, prev: Option<&Tiled>) -> Tiled {
    let side = side.max(1);
    let prev = prev.filter(|p| p.side == side);
    let longest = ask
        .thermals
        .iter()
        .map(|t| t.half[0].max(t.half[1]) + t.gap + t.spoke.max(ask.min_width))
        .max()
        .unwrap_or(0);
    // A tile's fill reads what lies within the least width (the opening),
    // two spokes' lengths (its pads' spokes, chosen against its window)
    // and the bounds of it.
    let reach = ask.min_width + 2 * longest + 4 * BOUND + 2;
    let Some(bounds) = ask.area.bounds() else {
        return Tiled {
            side,
            tiles: Vec::new(),
            islands_removed: 0,
            uncertain: Vec::new(),
            spokes: vec![0; ask.thermals.len()],
        };
    };
    let lo = |v: Nm| v.div_euclid(side);
    let corners: Vec<Point> = (lo(bounds.min.y)..=lo(bounds.max.y))
        .flat_map(|j| {
            (lo(bounds.min.x)..=lo(bounds.max.x)).map(move |i| Point::new(i * side, j * side))
        })
        .collect();
    let tile_rect = |at: Point| Rect {
        min: at,
        max: at.offset(side, side),
    };
    let obstacle_boxes: Vec<Option<Rect>> = ask.obstacles.iter().map(Polygons::bounds).collect();
    let blocked: Vec<(&Polygons, Option<Rect>)> = ask
        .spoke_obstacles
        .iter()
        .map(|o| (o, o.bounds()))
        .collect();
    let tile_of = |p: Point| Point::new(lo(p.x) * side, lo(p.y) * side);
    let prev_at: std::collections::BTreeMap<(i64, i64), &TileFill> = prev
        .map(|p| p.tiles.iter().map(|t| ((t.at.x, t.at.y), t)).collect())
        .unwrap_or_default();
    // Phase A: each tile's window cut and opened, its pads' spokes chosen.
    struct A {
        at: Point,
        key_a: [u8; 16],
        area: Polygons,
        opened: Option<Polygons>,
        choices: Vec<([u8; 16], u8)>,
    }
    let phase_a = |at: Point| -> Option<A> {
        let t = tile_rect(at);
        let w = tree::grown(&t, reach);
        let area = poly::intersection(&ask.area, &rect_polys(&t));
        if area.is_empty() {
            return None;
        }
        let window_area = poly::intersection(&ask.area, &rect_polys(&w));
        let obstacles: Vec<&Polygons> = ask
            .obstacles
            .iter()
            .zip(&obstacle_boxes)
            .filter(|(_, b)| b.is_some_and(|b| tree::meets(&b, &w)))
            .map(|(o, _)| o)
            .collect();
        let mine: Vec<&ThermalPad> = ask
            .thermals
            .iter()
            .filter(|th| tile_of(th.at) == at)
            .collect();
        let mut h = Print::new("ryu-copper.tile-a.1");
        h.int(side);
        h.int(ask.min_width);
        h.int(BOUND);
        hash_rings(&mut h, &window_area);
        for o in &obstacles {
            hash_rings(&mut h, o);
        }
        h.int(-2);
        for (o, b) in &blocked {
            if b.is_some_and(|b| tree::meets(&b, &w)) {
                hash_rings(&mut h, o);
            }
        }
        h.int(-3);
        for th in &mine {
            h.print(&thermal_hash(th));
        }
        let key_a = h.finish();
        if let Some(old) = prev_at.get(&(at.x, at.y))
            && old.key_a == key_a
        {
            return Some(A {
                at,
                key_a,
                area,
                opened: None,
                choices: old.choices.clone(),
            });
        }
        let opened = opened_of(&window_area, &obstacles, ask.min_width);
        let rings = opened.rings();
        let choices = mine
            .iter()
            .map(|th| {
                let mut bits = 0u8;
                for (k, s) in spokes(th, ask.min_width).iter().enumerate() {
                    if spoke_kept(s, &rings, &blocked) {
                        bits |= 1 << k;
                    }
                }
                (thermal_hash(th), bits)
            })
            .collect();
        Some(A {
            at,
            key_a,
            area,
            opened: Some(opened),
            choices,
        })
    };
    let a: Vec<A> = corners.par_iter().filter_map(|at| phase_a(*at)).collect();
    // Every kept spoke, with its box.
    let chosen: std::collections::BTreeMap<[u8; 16], u8> =
        a.iter().flat_map(|x| x.choices.iter().cloned()).collect();
    let mut counts = Vec::with_capacity(ask.thermals.len());
    let mut kept_spokes: Vec<(Polygons, Rect)> = Vec::new();
    for th in &ask.thermals {
        let bits = chosen.get(&thermal_hash(th)).copied().unwrap_or(0);
        counts.push(bits.count_ones() as u8);
        for (k, s) in spokes(th, ask.min_width).into_iter().enumerate() {
            if bits & (1 << k) != 0
                && let Some(b) = s.bounds()
            {
                kept_spokes.push((s, b));
            }
        }
    }
    // Phase B: each tile's opened fill and the kept spokes reaching it,
    // clipped to its share of the area.
    let tiles: Vec<TileFill> = a
        .into_par_iter()
        .map(|x| {
            let t = tile_rect(x.at);
            let mine: Vec<&(Polygons, Rect)> = kept_spokes
                .iter()
                .filter(|(_, b)| tree::meets(b, &t))
                .collect();
            let mut h = Print::new("ryu-copper.tile.1");
            h.print(&x.key_a);
            for (s, _) in &mine {
                hash_rings(&mut h, s);
            }
            let key = h.finish();
            if let Some(old) = prev_at.get(&(x.at.x, x.at.y))
                && old.key == key
            {
                return TileFill {
                    at: x.at,
                    key_a: x.key_a,
                    key,
                    fill: old.fill.clone(),
                    choices: x.choices,
                    dropped: Vec::new(),
                };
            }
            let opened = match x.opened {
                Some(o) => o,
                None => {
                    // Its spokes' choices held but a neighbour's spoke into
                    // it changed: its window is filled again.
                    let w = tree::grown(&t, reach);
                    let window_area = poly::intersection(&ask.area, &rect_polys(&w));
                    let obstacles: Vec<&Polygons> = ask
                        .obstacles
                        .iter()
                        .zip(&obstacle_boxes)
                        .filter(|(_, b)| b.is_some_and(|b| tree::meets(&b, &w)))
                        .map(|(o, _)| o)
                        .collect();
                    opened_of(&window_area, &obstacles, ask.min_width)
                }
            };
            TILES_FILLED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut kept = opened.rings();
            for (s, _) in &mine {
                let r = s.rings();
                kept.bound = kept.bound.max(r.bound);
                kept.rings.extend(r.rings);
            }
            let fill = poly::boolean(
                &kept,
                FillRule::NonZero,
                &x.area.rings(),
                FillRule::NonZero,
                Op::Intersection,
            );
            TileFill {
                at: x.at,
                key_a: x.key_a,
                key,
                fill,
                choices: x.choices,
                dropped: Vec::new(),
            }
        })
        .collect();
    let mut out = Tiled {
        side,
        tiles,
        islands_removed: 0,
        uncertain: Vec::new(),
        spokes: counts,
    };
    islands_across(&mut out, ask);
    out
}

/// Joins the tiles' pieces into regions by their contacts across seams,
/// and drops the regions no copper of the pour's net touches (unless the
/// ask keeps islands).
fn islands_across(t: &mut Tiled, ask: &Ask) {
    let mut pieces: Vec<(usize, usize, Body, Rect)> = Vec::new();
    for (ti, tile) in t.tiles.iter().enumerate() {
        for (i, p) in tile.fill.polygons.iter().enumerate() {
            let one = Polygons {
                polygons: vec![p.clone()],
                bound: tile.fill.bound,
            };
            let b = p.bounds();
            pieces.push((ti, i, one.body(), b));
        }
    }
    let n = pieces.len();
    let mut root: Vec<usize> = (0..n).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    // Pieces of neighbouring tiles whose boxes meet, touching.
    let ats: Vec<Point> = t.tiles.iter().map(|x| x.at).collect();
    let side = t.side;
    let (ps, ats) = (&pieces, &ats);
    let joins: Vec<(usize, usize)> = (0..n)
        .into_par_iter()
        .flat_map_iter(|a| {
            (a + 1..n)
                .filter(move |&b| {
                    let (pa, pb) = (&ps[a], &ps[b]);
                    let (ta, tb) = (ats[pa.0], ats[pb.0]);
                    pa.0 != pb.0
                        && (ta.x - tb.x).abs() <= side
                        && (ta.y - tb.y).abs() <= side
                        && tree::meets(&tree::grown(&pa.3, 1), &pb.3)
                        && measure::touches(&pa.2, &pb.2) != Verdict::No
                })
                .map(move |b| (a, b))
        })
        .collect();
    for (a, b) in joins {
        let (ra, rb) = (find(&mut root, a), find(&mut root, b));
        if ra != rb {
            root[ra.max(rb)] = ra.min(rb);
        }
    }
    let contact: Vec<Verdict> = pieces
        .par_iter()
        .map(|p| own_contact(&ask.own, &p.2))
        .collect();
    let mut region: std::collections::BTreeMap<usize, Verdict> = std::collections::BTreeMap::new();
    for (k, c) in contact.iter().enumerate() {
        let r = find(&mut root, k);
        let v = region.entry(r).or_insert(Verdict::No);
        *v = match (*v, *c) {
            (Verdict::Yes, _) | (_, Verdict::Yes) => Verdict::Yes,
            (Verdict::Uncertain, _) | (_, Verdict::Uncertain) => Verdict::Uncertain,
            _ => Verdict::No,
        };
    }
    let mut removed = std::collections::BTreeSet::new();
    for k in 0..n {
        let r = find(&mut root, k);
        match region[&r] {
            Verdict::No if !ask.keep_islands => {
                removed.insert(r);
                t.tiles[pieces[k].0].dropped.push(pieces[k].1);
            }
            Verdict::Uncertain if contact[k] == Verdict::Uncertain => {
                t.uncertain.push((pieces[k].0, pieces[k].1));
            }
            _ => {}
        }
    }
    t.islands_removed = removed.len();
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
    fn thermal(at: Point) -> ThermalPad {
        ThermalPad {
            at,
            rotation_mdeg: 0,
            half: [MM / 2, MM / 2],
            gap: MM / 2,
            spoke: MM / 4,
        }
    }

    #[test]
    fn a_thermal_pad_gets_four_spokes_and_loses_one_per_blocked_side() {
        let pad = rect(9 * MM / 2, 9 * MM / 2, 11 * MM / 2, 11 * MM / 2);
        let mut a = ask(rect(0, 0, 10 * MM, 10 * MM));
        // The pad's own gap: the pad grown by 0.5 mm.
        a.obstacles.push(poly::grow(&pad, MM / 2, &opts()));
        a.thermals.push(thermal(Point::new(5 * MM, 5 * MM)));
        a.min_width = MM / 5;
        let f = fill(&a);
        assert_eq!(f.spokes, [4]);
        assert_eq!(f.polygons.polygons.len(), 1);
        assert!(
            f.polygons
                .contains(Point::new(5 * MM, 5 * MM + 3 * MM / 4))
                .is_yes(),
            "the up spoke crosses the gap"
        );
        // Another net's copper to the right, in the spoke's way.
        let other = rect(6 * MM + MM / 10, 4 * MM, 7 * MM, 6 * MM);
        a.obstacles.push(other.clone());
        a.spoke_obstacles.push(other);
        assert_eq!(fill(&a).spokes, [3]);
    }
    #[test]
    fn spokes_stop_at_the_area() {
        // A pad 1.2 mm from the area's left edge: its left spoke, 1.25 mm
        // long, would reach past it.
        let at = Point::new(6 * MM / 5, 5 * MM);
        let pad = rect(at.x - MM / 2, at.y - MM / 2, at.x + MM / 2, at.y + MM / 2);
        let mut a = ask(rect(0, 0, 10 * MM, 10 * MM));
        a.obstacles.push(poly::grow(&pad, MM / 2, &opts()));
        a.thermals.push(thermal(at));
        let f = fill(&a);
        assert_eq!(f.spokes, [4]);
        assert!(poly::difference(&f.polygons, &a.area).is_empty());
    }
    #[test]
    fn islands_without_own_copper_go_unless_kept() {
        let area = union_all(&[rect(0, 0, 4 * MM, 4 * MM), rect(6 * MM, 0, 10 * MM, 4 * MM)]);
        let mut a = ask(area);
        a.keep_islands = false;
        // A via of the pour's net on the left square only.
        a.own.push(Body::of_shape(
            &ryu_board::shape::Shape::regions(
                ryu_board::model::Polarity::Positive,
                [ryu_board::geom::Region::solid(
                    ryu_board::shape::circle(Point::new(2 * MM, 2 * MM), MM / 4).unwrap(),
                )],
            ),
            &crate::measure::BodyOptions::default(),
        ));
        let f = fill(&a);
        assert_eq!((f.polygons.polygons.len(), f.islands_removed), (1, 1));
        assert!(f.polygons.contains(Point::new(MM, MM)).is_yes());
        a.keep_islands = true;
        assert_eq!(fill(&a).polygons.polygons.len(), 2);
        a.own.clear();
        a.keep_islands = false;
        assert!(fill(&a).polygons.is_empty(), "no own copper: nothing kept");
    }
}
