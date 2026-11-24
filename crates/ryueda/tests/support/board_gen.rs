//! A board for `generated::write`'s design, written as files: its chain
//! of resistors in a serpentine of rows, each joined to the next by
//! tracks (on top, or down a via, along the bottom and up again), and at
//! most one seeded fault (design 732's property tests and bars).

#![allow(dead_code)]

#[path = "generated.rs"]
pub mod generated;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub const UM: i64 = 1_000;
pub const MM: i64 = 1_000_000;
/// A part's pads' offset from its centre.
pub const PAD: i64 = 480 * UM;
/// The outline's margin round the parts' centres.
pub const MARGIN: i64 = 3 * MM;
const VIA_OFFSET: i64 = 1_200 * UM;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Connection `n`'s last point moved onto the next part's other pad.
    Short(usize),
    /// Part `n` moved up until its courtyard overlaps the row above's by 0.1 mm.
    Overlap(usize),
    /// Connection `n`'s first via left a nanometre short of its ring.
    ShrunkVia(usize),
    /// Connection `n`'s first via (by the bottom) ends at `in1`: a blind
    /// via, and the bottom leg cut off (ryueda 7.A).
    ViaToIn1(usize),
    /// Part `n` turned over to the bottom where it is, its routes left as
    /// they were (ryueda 7.A).
    Flip(usize),
    /// Connection `n`'s first via (by the bottom) made 400 µm across, and a
    /// stub of another net on `in1` 170 µm from its hole's wall and 120 µm
    /// from its ring (ryueda 7.A).
    HoleGap(usize),
    /// Connection `n` (on `in1`) with both its vias made blind, top to
    /// `in1`: their spans the only findings (ryueda 7.A).
    InnerBlind(usize),
    /// [`Fault::InnerBlind`] with the `in1` leg moved to `in2`, below the
    /// vias' spans: it meets neither, so its net opens (ryueda 7.A).
    InnerMove(usize),
    /// Connection `n`'s top-layer legs moved a nanometre towards the row
    /// above, still on their pads (ryueda 7.B: a waiver's limit exceeded).
    Nudge(usize),
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub channels: usize,
    pub parts: usize,
    pub cols: usize,
    pub pitch: (i64, i64),
    pub width: i64,
    /// Every n-th horizontal connection goes by the bottom; 0 for none.
    pub via_every: usize,
    pub via: (i64, i64),
    pub pieces: usize,
    pub fault: Option<Fault>,
    /// Copper layers: 2, or an even stackup of more, whose `in1` carries
    /// every third connection within a row (ryueda 7.A).
    pub layers: usize,
    /// Every n-th part on the bottom, its connections there too; 0 for
    /// none (ryueda 7.A).
    pub bottom_every: usize,
    /// The board's `rules.json`, when it has one (ryueda 7.B).
    pub rules: Option<Value>,
    /// Keepout files' bodies, written as `keepouts/k<i>.json` (ryueda 7.C).
    pub keepouts: Vec<Value>,
    /// Constraints added to part `n`'s placement (ryueda 7.C).
    pub constraints: Vec<(usize, Value)>,
    /// Drawing files' bodies, written as `drawings/d<i>.json` (ryueda 7.C).
    pub drawings: Vec<Value>,
    /// Pour files' bodies, written as `pours/p<i>.json` (ryueda 7.D).
    pub pours: Vec<Value>,
}

impl Spec {
    /// The bars' board: 1,000 parts (2,000 pads), 2,000 tracks.
    pub fn bar() -> Spec {
        Spec {
            channels: 10,
            parts: 100,
            cols: 25,
            pitch: (5 * MM, 3 * MM),
            width: 250 * UM,
            via_every: 0,
            via: (300 * UM, 600 * UM),
            pieces: 2,
            fault: None,
            layers: 2,
            bottom_every: 0,
            rules: None,
            keepouts: Vec::new(),
            pours: Vec::new(),
            constraints: Vec::new(),
            drawings: Vec::new(),
        }
    }

    /// Part `n`'s occurrence (`c<k>/r<i>`).
    pub fn occurrence(&self, n: usize) -> String {
        format!("c{}/r{}", n / self.parts, n % self.parts)
    }

    /// Whether part `n` sits at the left of its row.
    pub fn leftmost(&self, n: usize) -> bool {
        let (row, col) = (n / self.cols, n % self.cols);
        if row % 2 == 0 {
            col == 0
        } else {
            col == self.cols - 1
        }
    }

    pub fn count(&self) -> usize {
        self.channels * self.parts
    }

    fn rows(&self) -> usize {
        self.count().div_ceil(self.cols)
    }

    /// Part `n`'s place and rotation.
    fn place(&self, n: usize) -> ((i64, i64), i64) {
        let (row, col) = (n / self.cols, n % self.cols);
        let c = if row % 2 == 0 {
            col
        } else {
            self.cols - 1 - col
        };
        let mut at = (
            MARGIN + c as i64 * self.pitch.0,
            MARGIN + row as i64 * self.pitch.1,
        );
        if self.fault == Some(Fault::Overlap(n)) {
            at.1 += self.pitch.1 - MM + 100 * UM;
        }
        let rot = if row % 2 == 0 { 0 } else { 180_000 };
        (at, rot)
    }

    /// Part `n`'s placement file: a bottom part is mirrored, and turned a
    /// further half turn so its pads stay where the top part's were.
    fn placement(&self, n: usize) -> Value {
        let (at, rot) = self.place(n);
        let mut v = if self.bottom(n) || self.fault == Some(Fault::Flip(n)) {
            json!({"at": [at.0, at.1], "rotation_mdeg": (rot + 180_000) % 360_000, "side": "bottom"})
        } else {
            json!({"at": [at.0, at.1], "rotation_mdeg": rot})
        };
        let mine: Vec<&Value> = self
            .constraints
            .iter()
            .filter(|(k, _)| *k == n)
            .map(|(_, c)| c)
            .collect();
        if !mine.is_empty() {
            v["constraints"] = json!(mine);
        }
        v
    }

    /// Part `n`'s pad (1 or 2) centre.
    fn pad(&self, n: usize, pad: u8) -> (i64, i64) {
        let ((x, y), rot) = self.place(n);
        let sign = if (pad == 2) == (rot == 0) { 1 } else { -1 };
        (x + sign * PAD, y)
    }

    fn same_row(&self, n: usize) -> bool {
        n / self.cols == (n + 1) / self.cols
    }

    /// Whether connection `n` (part n to part n + 1) goes by the bottom.
    pub fn by_vias(&self, n: usize) -> bool {
        self.via_every > 0 && self.same_row(n) && n.is_multiple_of(self.via_every)
    }

    /// Whether part `n` sits on the bottom: every `bottom_every`-th, where
    /// both its connections run within a row on the outer layers.
    pub fn bottom(&self, n: usize) -> bool {
        self.bottom_every > 0
            && n % self.bottom_every == self.bottom_every - 1
            && n > 0
            && n + 1 < self.count()
            && [n - 1, n]
                .iter()
                .all(|&c| self.same_row(c) && !self.by_vias(c))
    }

    fn plain_inner(&self, n: usize) -> bool {
        self.layers > 2 && self.same_row(n) && !self.by_vias(n) && n % 3 == 1
    }

    /// Whether connection `n` runs on `in1`, through a via at each end.
    pub fn inner(&self, n: usize) -> bool {
        self.plain_inner(n) && !self.bottom(n) && !self.bottom(n + 1)
    }

    /// The board's size.
    fn size(&self) -> (i64, i64) {
        (
            2 * MARGIN + (self.cols as i64 - 1) * self.pitch.0,
            2 * MARGIN + (self.rows() as i64 - 1) * self.pitch.1,
        )
    }
}

fn put(dir: &Path, rel: &str, kind: &str, mut v: Value) {
    v["format"] = json!(ryueda::conn::format_of(kind, &v));
    v["kind"] = json!(kind);
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, ryueda::canonical::to_string(&v)).unwrap();
}

/// `a` to `b` in `k` collinear pieces.
fn split(a: (i64, i64), b: (i64, i64), k: usize) -> Vec<((i64, i64), (i64, i64))> {
    let k = k.max(1) as i64;
    let at = |j: i64| (a.0 + (b.0 - a.0) * j / k, a.1 + (b.1 - a.1) * j / k);
    (0..k).map(|j| (at(j), at(j + 1))).collect()
}

/// Writes the design and its board `main` in `dir`; the occurrences in
/// chain order are `c<k>/r<i>`.
pub fn write(dir: &Path, s: &Spec) {
    generated::write(dir, s.channels, s.parts);
    let p = ryueda::project::Project::load(dir).unwrap();
    let design = ryueda::id::Id::parse("d", "design").unwrap();
    let (_, occs) = ryueda::board::occ::occurrences(&p, &design).unwrap();
    let nets: BTreeMap<String, BTreeMap<String, String>> = occs
        .iter()
        .map(|o| (o.reference.clone(), o.nets.clone()))
        .collect();
    let occ = |n: usize| format!("c{}/r{}", n / s.parts, n % s.parts);
    let (w, h) = s.size();
    let mut board = json!({
        "name": "Generated", "design": "d", "preset": "jlc-2l",
        "outline": {"start": [0, 0], "edges": [
            {"type": "line", "to": [w, 0]}, {"type": "line", "to": [w, h]},
            {"type": "line", "to": [0, h]}, {"type": "line", "to": [0, 0]}]},
    });
    if s.layers > 2 {
        let st = ryueda::board::stackup::even(&ryueda::board::preset::JLC_2L, s.layers).unwrap();
        board["stackup"] = serde_json::to_value(st).unwrap();
    }
    put(dir, "boards/main/board.json", "ryueda.board", board);
    for (i, k) in s.keepouts.iter().enumerate() {
        put(
            dir,
            &format!("boards/main/keepouts/k{i}.json"),
            "ryueda.keepout",
            k.clone(),
        );
    }
    for (i, p) in s.pours.iter().enumerate() {
        put(
            dir,
            &format!("boards/main/pours/p{i}.json"),
            "ryueda.pour",
            p.clone(),
        );
    }
    for (i, d) in s.drawings.iter().enumerate() {
        put(
            dir,
            &format!("boards/main/drawings/d{i}.json"),
            "ryueda.board-drawing",
            d.clone(),
        );
    }
    if let Some(r) = &s.rules {
        put(
            dir,
            "boards/main/rules.json",
            "ryueda.board-rules",
            r.clone(),
        );
    }
    for n in 0..s.count() {
        put(
            dir,
            &format!("boards/main/placements/{}.json", occ(n).replace('/', ".")),
            "ryueda.placement",
            s.placement(n),
        );
    }
    let preset = ryueda::board::preset::JLC_2L;
    let track = |id: String, net: &str, layer: &str, a: (i64, i64), b: (i64, i64)| {
        let nudged = matches!(s.fault, Some(Fault::Nudge(f)) if id.starts_with(&format!("t{f}_")))
            && layer == "top";
        let (a, b) = if nudged {
            ((a.0, a.1 - 1), (b.0, b.1 - 1))
        } else {
            (a, b)
        };
        put(
            dir,
            &format!("boards/main/tracks/{id}.json"),
            "ryueda.track",
            json!({"net": net, "layer": layer, "width_nm": s.width, "points": [[a.0, a.1], [b.0, b.1]]}),
        );
    };
    for n in 0..s.count() - 1 {
        let net = nets[&occ(n)]["2"].clone();
        let a = s.pad(n, 2);
        let b = if s.fault == Some(Fault::Short(n)) {
            s.pad(n + 1, 2)
        } else {
            s.pad(n + 1, 1)
        };
        let side = |p: usize| if s.bottom(p) { "bottom" } else { "top" };
        let (sa, sb) = (side(n), side(n + 1));
        let mut legs = vec![(sa, a, b)];
        if sa != sb {
            // A top part to a bottom one: down a via half way.
            let m = ((a.0 + b.0) / 2, a.1);
            legs = vec![(sa, a, m), (sb, m, b)];
            put(
                dir,
                &format!("boards/main/vias/v{n}_m.json"),
                "ryueda.via",
                json!({"net": net, "at": [m.0, m.1], "drill_nm": s.via.0, "diameter_nm": s.via.1}),
            );
        } else if s.by_vias(n) || s.inner(n) {
            let dir_x = if b.0 > a.0 { 1 } else { -1 };
            let va = (a.0 + dir_x * VIA_OFFSET, a.1);
            let vb = (s.pad(n + 1, 1).0 - dir_x * VIA_OFFSET, a.1);
            let blind =
                matches!(s.fault, Some(Fault::InnerBlind(f) | Fault::InnerMove(f)) if f == n);
            let middle = if s.fault == Some(Fault::InnerMove(n)) {
                "in2"
            } else if s.inner(n) {
                "in1"
            } else {
                "bottom"
            };
            legs = vec![("top", a, va), (middle, va, vb), ("top", vb, b)];
            for (k, at) in [("a", va), ("b", vb)] {
                let (mut drill, mut dia) = s.via;
                if k == "a" && s.fault == Some(Fault::ShrunkVia(n)) {
                    let ring = preset.via_annular_ring_min_nm;
                    drill = drill.max(preset.via_diameter_min_nm - 2 * ring + 2);
                    dia = drill + 2 * ring - 2;
                }
                let mut via =
                    json!({"net": net, "at": [at.0, at.1], "drill_nm": drill, "diameter_nm": dia});
                if (k == "a" && s.fault == Some(Fault::ViaToIn1(n))) || blind {
                    via["to"] = json!("in1");
                }
                if k == "a" && s.fault == Some(Fault::HoleGap(n)) {
                    via["diameter_nm"] = json!(400 * UM);
                    // Another net's stub on in1, its edge 320 µm from the
                    // via's centre: 120 µm from the 200 µm ring, 170 µm
                    // from the 150 µm wall.
                    let other = nets[&occ(n + 1)]["2"].clone();
                    let y = at.1 + 320 * UM + s.width / 2;
                    track(
                        format!("stub{n}"),
                        &other,
                        "in1",
                        (at.0 - 500 * UM, y),
                        (at.0 + 500 * UM, y),
                    );
                }
                put(
                    dir,
                    &format!("boards/main/vias/v{n}_{k}.json"),
                    "ryueda.via",
                    via,
                );
            }
        }
        let mut j = 0;
        for (layer, x, y) in legs {
            for (p, q) in split(x, y, s.pieces) {
                track(format!("t{n}_{j}"), &net, layer, p, q);
                j += 1;
            }
        }
    }
}

/// splitmix64.
pub fn next(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A seeded clean board's spec: 2 to 6 columns of 3 to 40 parts, tracks
/// at the minimum width up to 250 µm, some connections by vias.
pub fn seeded(seed: u64) -> Spec {
    let mut s = seed;
    let preset = ryueda::board::preset::JLC_2L;
    let cols = 3 + (next(&mut s) % 4) as usize;
    let parts = 2 * cols + (next(&mut s) % 10) as usize;
    let width = preset.track_width_min_nm
        + (next(&mut s) as i64).rem_euclid(250 * UM - preset.track_width_min_nm + 1);
    Spec {
        channels: 1 + (next(&mut s) % 2) as usize,
        parts,
        cols,
        pitch: (
            [5 * MM, 6 * MM][(next(&mut s) % 2) as usize],
            [2_500 * UM, 3 * MM][(next(&mut s) % 2) as usize],
        ),
        width,
        via_every: [2, 3, 4][(next(&mut s) % 3) as usize],
        via: (preset.default_via_drill_nm, preset.default_via_diameter_nm),
        pieces: 1 + (next(&mut s) % 2) as usize,
        fault: None,
        layers: 2,
        bottom_every: 0,
        rules: None,
        keepouts: Vec::new(),
        pours: Vec::new(),
        constraints: Vec::new(),
        drawings: Vec::new(),
    }
}

/// [`seeded`] on 2, 4 or 6 copper layers, every third part on the bottom
/// or none (ryueda 7.A); its first draws are `seeded`'s.
pub fn seeded_multilayer(seed: u64) -> Spec {
    let mut s = seed ^ 0x6c61_7965_7273;
    Spec {
        layers: [2, 4, 6][(next(&mut s) % 3) as usize],
        bottom_every: [0, 3][(next(&mut s) % 2) as usize],
        ..seeded(seed)
    }
}
