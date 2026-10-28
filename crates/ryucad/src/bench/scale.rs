//! Design 192's benchmark project: thousands of distinct seeded parts in a
//! three-level tree of assemblies placing a hundred times as many
//! occurrences, written from a seed.
//!
//! Every subassembly places 50 parts of its own: a base plate (fixed),
//! 13 more plates, 10 brackets (fastened to the base), 10 shafts
//! (revolute), 2 or 3 imported race bodies (fixed) and pins (sliders on
//! the base), each pin repeated along x by an instance pattern so the
//! subassembly holds 500 occurrences. Modules place 10 subassemblies, the
//! root every module. [`Spec::full`] is the benchmark: 200 subassemblies,
//! so 10,000 parts and 100,000 occurrences.
//!
//! Parts are plates (a slab, a sketch of points, a hole and its grid,
//! then 1 to 36 pockets), brackets (a base and a wall joined, the inside
//! edge filleted and the outside one chamfered, then 1 to 12 holes),
//! shafts (a stepped profile revolved, its ends chamfered, then 2 to 8
//! cross holes), pins (a shank and a head joined, filleted, chamfered and
//! cross-drilled) and imports of `ryukei_testkit::race` bodies from one
//! STEP file. Fillets and chamfers name their edges, so a part's first
//! features are built in memory to find them: names follow from the part
//! and feature IDs, as in any project.
//!
//! Files are written plainly (not atomically and synced, as ryucad
//! writes): hundreds of thousands of syncs would take hours.

use crate::doc::canonical;
use crate::doc::id::Id;
use crate::doc::project::Project;
use rayon::prelude::*;
use ryukei_testkit::Lcg;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Parts per subassembly.
pub const PARTS_PER_SUB: usize = 50;
/// Occurrences per subassembly.
pub const OCCURRENCES_PER_SUB: usize = 500;
/// Where the race bodies are imported from.
pub const RACE_FILE: &str = "imports/race.step";

/// What to generate.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// Subassemblies, each [`PARTS_PER_SUB`] parts and
    /// [`OCCURRENCES_PER_SUB`] occurrences.
    pub subassemblies: usize,
    /// Subassemblies each module places (the last may place fewer).
    pub per_module: usize,
    pub seed: u64,
    /// Whether 2 or 3 parts of each subassembly import race bodies
    /// (else they are pins too): making the bodies takes minutes in a
    /// debug build.
    pub races: bool,
}

impl Spec {
    /// The benchmark: 10,000 parts, 221 assemblies, 100,000 occurrences.
    pub fn full() -> Spec {
        Spec {
            subassemblies: 200,
            per_module: 10,
            seed: 192,
            races: true,
        }
    }

    /// A smaller project of the same shape: `parts` rounded up to whole
    /// subassemblies.
    pub fn parts(parts: usize) -> Spec {
        Spec {
            subassemblies: parts.div_ceil(PARTS_PER_SUB).max(1),
            ..Spec::full()
        }
    }

    pub fn modules(&self) -> usize {
        self.subassemblies.div_ceil(self.per_module)
    }

    /// The race bodies the project imports: 3 in even subassemblies, 2 in
    /// odd ones.
    pub fn imports(&self) -> usize {
        (0..self.subassemblies).map(|s| self.imports_in(s)).sum()
    }

    fn imports_in(&self, sub: usize) -> usize {
        match (self.races, sub.is_multiple_of(2)) {
            (false, _) => 0,
            (true, true) => 3,
            (true, false) => 2,
        }
    }
}

/// What [`write`] made.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub parts: usize,
    pub assemblies: usize,
    pub occurrences: usize,
    /// Feature files, assemblies' included.
    pub features: usize,
    /// Every file written.
    pub files: usize,
    /// BLAKE3 of every file's path and bytes, in path order: the
    /// generator's golden.
    pub digest: String,
}

/// The STEP file of `n` race bodies, each its own product `r<i>`: bodies
/// that don't come back through STEP (design 55's limit) are left out,
/// and more seeds tried, so all `n` heal.
pub fn race_file(n: usize) -> Vec<u8> {
    let options = ryustep::WriteOptions {
        unit: ryustep::LengthUnit::Millimeter,
        time_stamp: "2026-10-28T00:00:00".into(),
        ..ryustep::WriteOptions::default()
    };
    let model = |bodies: &[ryukei::topo::Body]| ryustep::WriteModel {
        products: bodies
            .iter()
            .enumerate()
            .map(|(i, b)| ryustep::Product {
                id: format!("r{i}"),
                name: format!("r{i}"),
                bodies: vec![b.clone()],
                ..Default::default()
            })
            .collect(),
    };
    let mut bodies = ryukei_testkit::race::writable(n + n / 50 + 2);
    let first = ryustep::write(&model(&bodies), &options).expect("race bodies write");
    let read = ryustep::read(
        &first,
        &ryustep::ReadOptions {
            key: ryukei::name::OpKey(7),
            heal: Default::default(),
        },
    )
    .expect("and read");
    let refused: std::collections::BTreeSet<String> = read
        .bodies
        .iter()
        .filter(|b| b.result.is_err())
        .filter_map(|b| b.definition)
        .map(|d| read.products.definitions[d].product_id.to_string())
        .collect();
    let mut i = 0;
    bodies.retain(|_| {
        i += 1;
        !refused.contains(&format!("r{}", i - 1))
    });
    assert!(
        bodies.len() >= n,
        "{} race bodies heal, not {n}",
        bodies.len()
    );
    bodies.truncate(n);
    ryustep::write(&model(&bodies), &options).expect("race bodies write")
}

/// Writes the project `spec` describes into `root` (which must be empty
/// or missing), importing from `race` (made by [`race_file`] with at
/// least [`Spec::imports`] bodies; empty without races).
pub fn write(root: &Path, spec: &Spec, race: &[u8]) -> Summary {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let text = |v: &Value| canonical::to_string(v).into_bytes();
    files.insert(
        crate::doc::project::MANIFEST.into(),
        text(&json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"})),
    );
    files.insert(
        ".gitattributes".into(),
        crate::drivers::with_lines_for("", spec.races).into_bytes(),
    );
    if spec.races {
        files.insert(RACE_FILE.into(), race.to_vec());
    }
    let race_hash = blake3::hash(race).to_hex().to_string();
    // Every part, in parallel: some build their first features to name
    // edges.
    let parts: Vec<(usize, usize)> = (0..spec.subassemblies)
        .flat_map(|s| (0..PARTS_PER_SUB).map(move |j| (s, j)))
        .collect();
    let made: Vec<BTreeMap<String, Vec<u8>>> = parts
        .par_iter()
        .map(|&(s, j)| part_files(spec, s, j, &race_hash))
        .collect();
    let mut features = 0;
    for m in made {
        features += m.len() - 1;
        files.extend(m);
    }
    for s in 0..spec.subassemblies {
        let sub = subassembly(spec, s);
        features += sub.len() - 1;
        files.extend(sub);
    }
    for m in 0..spec.modules() {
        let module = module(spec, m);
        features += module.len() - 1;
        files.extend(module);
    }
    let root_files = root_assembly(spec);
    features += root_files.len() - 1;
    files.extend(root_files);
    let mut h = blake3::Hasher::new();
    for (path, bytes) in &files {
        h.update(&(path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    let summary = Summary {
        parts: spec.subassemblies * PARTS_PER_SUB,
        assemblies: spec.subassemblies + spec.modules() + 1,
        occurrences: spec.subassemblies * OCCURRENCES_PER_SUB,
        features,
        files: files.len(),
        digest: h.finalize().to_hex().to_string(),
    };
    let dirs: std::collections::BTreeSet<std::path::PathBuf> = files
        .keys()
        .filter_map(|p| root.join(p).parent().map(Path::to_path_buf))
        .collect();
    for d in dirs {
        std::fs::create_dir_all(&d).expect("make the project's folders");
    }
    files.par_iter().for_each(|(path, bytes)| {
        std::fs::write(root.join(path), bytes).expect("write the project's files");
    });
    summary
}

/// Part `j` of subassembly `s`'s ID: its family and its place among all
/// parts.
pub fn part_id(spec: &Spec, s: usize, j: usize) -> String {
    let n = s * PARTS_PER_SUB + j;
    let family = match Family::of(spec, s, j) {
        Family::Plate => "plate",
        Family::Bracket => "bracket",
        Family::Shaft => "shaft",
        Family::Pin => "pin",
        Family::Race => "race",
    };
    format!("{family}-{n:05}")
}

pub fn sub_id(s: usize) -> String {
    format!("sub-{s:03}")
}

pub fn module_id(m: usize) -> String {
    format!("mod-{m:02}")
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Family {
    Plate,
    Bracket,
    Shaft,
    Pin,
    Race,
}

impl Family {
    /// Places 0 to 13 plates (0 the base), 14 to 23 brackets, 24 to 33
    /// shafts, then pins, and the last 2 or 3 race bodies.
    fn of(spec: &Spec, s: usize, j: usize) -> Family {
        match j {
            0..14 => Family::Plate,
            14..24 => Family::Bracket,
            24..34 => Family::Shaft,
            _ if j >= PARTS_PER_SUB - spec.imports_in(s) => Family::Race,
            _ => Family::Pin,
        }
    }
}

/// A quantity in millimetres, as a file writes one.
fn mm(x: f64) -> Value {
    json!(format!("{x} mm"))
}

fn place(origin: [f64; 3]) -> Value {
    json!({"origin": origin.map(mm), "x": [1, 0, 0], "z": [0, 0, 1]})
}

fn place_along(origin: [f64; 3], z: [i32; 3], x: [i32; 3]) -> Value {
    json!({"origin": origin.map(mm), "x": x, "z": z})
}

fn feature(ty: &str, format: u64, mut fields: Value) -> Value {
    fields["type"] = json!(ty);
    fields["format"] = json!(format);
    fields["kind"] = json!("ryucad.feature");
    fields
}

fn shape(ty: &str, params: Value, placement: Value) -> Value {
    feature(ty, 1, json!({"params": params, "placement": placement}))
}

fn cut(ty: &str, params: Value, placement: Value, target: &str) -> Value {
    feature(
        ty,
        3,
        json!({"params": params, "placement": placement, "operation": "cut",
               "targets": [target]}),
    )
}

fn join(ty: &str, params: Value, placement: Value, target: &str) -> Value {
    feature(
        ty,
        3,
        json!({"params": params, "placement": placement, "operation": "join",
               "targets": [target]}),
    )
}

fn blend(ty: &str, group: Value) -> Value {
    feature(ty, 3, json!({"groups": [group]}))
}

/// A part's files as they are built up: its features in order.
struct PartFiles {
    id: String,
    order: Vec<(String, Value)>,
}

impl PartFiles {
    fn new(id: String) -> PartFiles {
        PartFiles {
            id,
            order: Vec::new(),
        }
    }

    fn add(&mut self, id: impl Into<String>, f: Value) {
        self.order.push((id.into(), f));
    }

    fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let list: Vec<&str> = self.order.iter().map(|(id, _)| id.as_str()).collect();
        out.insert(
            format!("parts/{}/part.json", self.id),
            canonical::to_string(&json!({"features": list, "format": 1, "kind": "ryucad.part"}))
                .into_bytes(),
        );
        for (id, f) in &self.order {
            out.insert(
                format!("parts/{}/features/{id}.json", self.id),
                canonical::to_string(f).into_bytes(),
            );
        }
        out
    }

    /// The names of the edges of `solid`, as the part so far leaves it,
    /// whose middles `pick` accepts, in the body's edge order. Built in
    /// memory, as a rebuild would build it.
    fn edges(&self, solid: &str, pick: impl Fn(ryukei::geom::Point3) -> bool) -> Vec<String> {
        let mut files: BTreeMap<String, Arc<[u8]>> = self
            .files()
            .into_iter()
            .map(|(p, b)| (p, Arc::from(b)))
            .collect();
        files.insert(
            crate::doc::project::MANIFEST.into(),
            Arc::from(
                canonical::to_string(
                    &json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
                )
                .into_bytes(),
            ),
        );
        let project = Project::load_from(Path::new("/"), &files).expect("the part loads");
        let part = &project.parts[&Id::parse(&self.id, "part").expect("an ID")];
        let r = crate::rebuild::rebuild_part(part);
        let (_, last) = r.features.last().expect("features");
        let solid = Id::parse(solid, "feature").expect("an ID");
        let out = last
            .as_ref()
            .unwrap_or_else(|e| panic!("{}: {e}", self.id))
            .solid(&solid)
            .expect("the solid");
        let body = &out.body;
        body.edges()
            .filter_map(|e| {
                let edge = body.edge(e)?;
                let curve = body.curve(edge.curve)?;
                let mid = curve
                    .eval(0.5 * (edge.interval[0] + edge.interval[1]))
                    .point;
                pick(mid).then(|| edge.name.to_string())
            })
            .collect()
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-7
}

fn edge_ref(solid: &str, name: &str) -> Value {
    json!({"feature": solid, "name": name})
}

/// Part `j` of subassembly `s`'s files.
fn part_files(spec: &Spec, s: usize, j: usize, race_hash: &str) -> BTreeMap<String, Vec<u8>> {
    let id = part_id(spec, s, j);
    let mut g = Lcg::new(spec.seed ^ ((s * PARTS_PER_SUB + j) as u64).wrapping_mul(0x9e37_79b9));
    g.next_u64();
    let mut p = PartFiles::new(id);
    match Family::of(spec, s, j) {
        Family::Plate if j == 0 => plate(
            &mut p,
            &mut g,
            [340.0, 160.0, 10.0],
            [0.0, 132.0, 340.0, 160.0],
        ),
        Family::Plate => {
            let size = [
                60.0 + 10.0 * (g.next_u64() % 7) as f64,
                40.0 + 10.0 * (g.next_u64() % 5) as f64,
                4.0 + (g.next_u64() % 7) as f64,
            ];
            plate(&mut p, &mut g, size, [0.0, 0.0, size[0], size[1]])
        }
        Family::Bracket => bracket(&mut p, &mut g),
        Family::Shaft => shaft(&mut p, &mut g),
        Family::Pin => pin(&mut p, &mut g),
        Family::Race => {
            let k = (0..s).map(|t| spec.imports_in(t)).sum::<usize>() + j
                - (PARTS_PER_SUB - spec.imports_in(s));
            p.add(
                "body",
                feature(
                    "import",
                    5,
                    json!({"body": 0, "file": {"blake3": race_hash, "path": RACE_FILE},
                           "product": {"id": format!("r{k}"), "name": format!("r{k}")}}),
                ),
            );
        }
    }
    p.files()
}

/// A slab `size` (mm) with its top at z = 0, a hole at up to three points
/// repeated along x, and pockets, all inside `rect` (x0, y0, x1, y1): the
/// holes in its left part, the pockets to their right.
fn plate(p: &mut PartFiles, g: &mut Lcg, size: [f64; 3], rect: [f64; 4]) {
    let [x, y, t] = size;
    let [x0, y0, x1, y1] = rect;
    p.add(
        "slab",
        shape(
            "box",
            json!({"x": mm(x), "y": mm(y), "z": mm(t)}),
            place([0.0, 0.0, -t]),
        ),
    );
    let rows = (((y1 - y0 - 12.0) / 8.0) as usize + 1).min(1 + (g.next_u64() % 3) as usize);
    let mut entities = serde_json::Map::new();
    for i in 0..rows {
        let at = [(x0 + 6.0) * 1e-3, (y0 + 6.0 + 8.0 * i as f64) * 1e-3];
        entities.insert(format!("p{}", i + 1), json!({"m": at, "type": "point"}));
    }
    p.add(
        "pts",
        feature(
            "sketch",
            2,
            json!({"constraints": {}, "entities": entities, "plane": "xy"}),
        ),
    );
    let d = 3.0 + (g.next_u64() % 3) as f64;
    p.add(
        "h1",
        feature(
            "hole",
            3,
            json!({"depth": "through_all", "diameter": mm(d), "form": "simple",
                   "sketch": "pts", "targets": ["slab"]}),
        ),
    );
    p.add(
        "grid",
        feature(
            "pattern",
            3,
            json!({"count": format!("{}", 2 + g.next_u64() % 2), "direction": "x",
                   "features": ["h1"], "form": "linear", "spacing": mm(6.0),
                   "targets": ["slab"]}),
        ),
    );
    let pockets = if g.next_u64().is_multiple_of(10) {
        10 + (g.next_u64() % 27) as usize
    } else {
        1 + (g.next_u64() % 8) as usize
    };
    let (cols, rows) = (
        ((x1 - x0 - 34.0) / 4.0) as usize + 1,
        ((y1 - y0 - 8.0) / 4.0) as usize + 1,
    );
    for k in 0..pockets.min(cols * rows) {
        let at = [
            x0 + 30.0 + 4.0 * (k % cols) as f64,
            y0 + 4.0 + 4.0 * (k / cols) as f64,
            -t / 2.0,
        ];
        p.add(
            format!("c{}", k + 1),
            cut(
                "cylinder",
                json!({"height": mm(t), "radius": mm(1.2)}),
                place(at),
                "slab",
            ),
        );
    }
}

/// A base and a wall joined, the inside edge filleted and the outside top
/// edge chamfered, then holes through the base.
fn bracket(p: &mut PartFiles, g: &mut Lcg) {
    let bx = 40.0 + 5.0 * (g.next_u64() % 5) as f64;
    let by = 30.0 + 5.0 * (g.next_u64() % 3) as f64;
    let h = 20.0 + 5.0 * (g.next_u64() % 5) as f64;
    p.add(
        "base",
        shape(
            "box",
            json!({"x": mm(bx), "y": mm(by), "z": mm(5.0)}),
            place([0.0; 3]),
        ),
    );
    p.add(
        "wall",
        join(
            "box",
            json!({"x": mm(5.0), "y": mm(by), "z": mm(h)}),
            place([0.0; 3]),
            "base",
        ),
    );
    let along = |x: f64| {
        move |m: ryukei::geom::Point3| {
            near(m.x, x * 1e-3) && near(m.z, 5e-3) && near(m.y, by * 5e-4)
        }
    };
    let inside = p.edges("base", along(5.0));
    let outside = p.edges("base", along(bx));
    assert_eq!((inside.len(), outside.len()), (1, 1), "{}", p.id);
    p.add(
        "fillet",
        blend(
            "fillet",
            json!({"edges": [edge_ref("base", &inside[0])], "radius": mm(2.0)}),
        ),
    );
    p.add(
        "chamfer",
        blend(
            "chamfer",
            json!({"d": mm(1.0), "edges": [edge_ref("base", &outside[0])], "form": "equal"}),
        ),
    );
    let r = 1.5 + 0.5 * (g.next_u64() % 3) as f64;
    let spots: Vec<[f64; 2]> = (0..)
        .map(|i| 12.0 + 7.0 * i as f64)
        .take_while(|x| *x <= bx - 6.0)
        .flat_map(|x| {
            (0..)
                .map(|k| 6.0 + 7.0 * k as f64)
                .take_while(|y| *y <= by - 6.0)
                .map(move |y| [x, y])
        })
        .collect();
    let n = 1 + (g.next_u64() as usize % spots.len().min(12));
    for (k, [x, y]) in spots.into_iter().take(n).enumerate() {
        p.add(
            format!("h{}", k + 1),
            cut(
                "cylinder",
                json!({"height": mm(7.0), "radius": mm(r)}),
                place([x, y, -1.0]),
                "base",
            ),
        );
    }
}

/// A stepped profile revolved about x, both ends chamfered, then holes
/// across it.
fn shaft(p: &mut PartFiles, g: &mut Lcg) {
    let steps = 2 + (g.next_u64() % 3) as usize;
    let mut r = 6.0 + (g.next_u64() % 4) as f64;
    let mut radii = Vec::new();
    let mut ends = vec![0.0];
    for _ in 0..steps {
        radii.push(r);
        r -= 1.0;
        ends.push(ends.last().unwrap() + 10.0 + 5.0 * (g.next_u64() % 3) as f64);
    }
    let len = ends[steps];
    // Along the axis, out at the far end, then back step by step.
    let mut points = vec![[0.0, 0.0], [len, 0.0], [len, radii[steps - 1]]];
    for i in (1..steps).rev() {
        points.push([ends[i], radii[i]]);
        points.push([ends[i], radii[i - 1]]);
    }
    points.push([0.0, radii[0]]);
    let mut entities = serde_json::Map::new();
    let n = points.len();
    for (i, at) in points.iter().enumerate() {
        entities.insert(
            format!("p{}", i + 1),
            json!({"m": [at[0] * 1e-3, at[1] * 1e-3], "type": "point"}),
        );
        entities.insert(
            format!("l{}", i + 1),
            json!({"end": format!("p{}", (i + 1) % n + 1), "start": format!("p{}", i + 1),
                   "type": "line"}),
        );
    }
    p.add(
        "prof",
        feature(
            "sketch",
            2,
            json!({"constraints": {}, "entities": entities, "plane": "xy"}),
        ),
    );
    p.add(
        "body",
        feature(
            "revolve",
            2,
            json!({"angle": "full", "axis": "l1", "sketch": "prof"}),
        ),
    );
    let at_end = |x: f64| move |m: ryukei::geom::Point3| near(m.x, x * 1e-3);
    let mut edges = p.edges("body", at_end(0.0));
    edges.extend(p.edges("body", at_end(len)));
    assert_eq!(edges.len(), 2, "{}", p.id);
    let edges: Vec<Value> = edges.iter().map(|e| edge_ref("body", e)).collect();
    p.add(
        "chamfer",
        blend(
            "chamfer",
            json!({"d": mm(0.5), "edges": edges, "form": "equal"}),
        ),
    );
    let per = 1 + (g.next_u64() % 2) as usize;
    let mut k = 0;
    for i in 0..steps {
        for q in 0..per {
            k += 1;
            let x = ends[i] + (ends[i + 1] - ends[i]) * (q + 1) as f64 / (per + 1) as f64;
            p.add(
                format!("x{k}"),
                cut(
                    "cylinder",
                    json!({"height": mm(2.0 * radii[0] + 2.0), "radius": mm(0.6)}),
                    place_along([x, -radii[0] - 1.0, 0.0], [0, 1, 0], [1, 0, 0]),
                    "body",
                ),
            );
        }
    }
}

/// A shank standing on z = 0 and a head on it, the edge under the head
/// filleted, the foot chamfered, and a hole across the shank.
fn pin(p: &mut PartFiles, g: &mut Lcg) {
    let r = 1.0 + 0.25 * (g.next_u64() % 7) as f64;
    let h = 8.0 + 2.0 * (g.next_u64() % 9) as f64;
    p.add(
        "shank",
        shape(
            "cylinder",
            json!({"height": mm(h), "radius": mm(r)}),
            place([0.0; 3]),
        ),
    );
    p.add(
        "head",
        join(
            "cylinder",
            json!({"height": mm(2.0), "radius": mm(1.5 * r)}),
            place([0.0, 0.0, h]),
            "shank",
        ),
    );
    let circle = |z: f64| {
        move |m: ryukei::geom::Point3| {
            near(m.z, z * 1e-3) && near(ryukei::math::hypot(m.x, m.y), r * 1e-3)
        }
    };
    let (under, foot) = (p.edges("shank", circle(h)), p.edges("shank", circle(0.0)));
    assert_eq!((under.len(), foot.len()), (1, 1), "{}", p.id);
    p.add(
        "fillet",
        blend(
            "fillet",
            json!({"edges": [edge_ref("shank", &under[0])], "radius": mm(0.3)}),
        ),
    );
    p.add(
        "chamfer",
        blend(
            "chamfer",
            json!({"d": mm(0.2), "edges": [edge_ref("shank", &foot[0])], "form": "equal"}),
        ),
    );
    p.add(
        "cross",
        cut(
            "cylinder",
            json!({"height": mm(4.0 * r), "radius": mm(r / 3.0)}),
            place_along([-2.0 * r, 0.0, 0.6 * h], [1, 0, 0], [0, 1, 0]),
            "shank",
        ),
    );
}

fn pose(origin_mm: [f64; 3]) -> Value {
    json!({"origin_m": origin_mm.map(|x| x * 1e-3), "x": [1, 0, 0], "z": [0, 0, 1]})
}

fn instance(part: &str, origin_mm: [f64; 3], fixed: bool) -> Value {
    let mut v = feature(
        "instance",
        4,
        json!({"part": part, "pose": pose(origin_mm), "rev": "working"}),
    );
    if fixed {
        v["fixed"] = json!(true);
    }
    v
}

/// A mate of `form` from the base's origin moved by `offset_mm` to
/// `inst`'s origin.
fn mate(form: &str, inst: &str, offset_mm: [f64; 3]) -> Value {
    feature(
        "mate",
        4,
        json!({"a": {"at": "origin", "instance": ["base"], "offset": {"origin": offset_mm.map(mm)}},
               "b": {"at": "origin", "instance": [inst]}, "form": form}),
    )
}

/// An assembly's files: its features in order, each part numbered in the
/// order it is first placed.
fn assembly_files(id: &str, order: Vec<(String, Value)>) -> BTreeMap<String, Vec<u8>> {
    let mut items = serde_json::Map::new();
    for (_, f) in &order {
        if let Some(part) = f["part"].as_str()
            && !items.contains_key(part)
        {
            items.insert(part.to_owned(), json!(items.len() + 1));
        }
    }
    let list: Vec<&str> = order.iter().map(|(i, _)| i.as_str()).collect();
    let mut out = BTreeMap::new();
    out.insert(
        format!("parts/{id}/part.json"),
        canonical::to_string(&json!({"features": list, "format": 3, "items": items,
                                     "kind": "ryucad.part", "part_kind": "assembly"}))
        .into_bytes(),
    );
    for (f, v) in order {
        out.insert(
            format!("parts/{id}/features/{f}.json"),
            canonical::to_string(&v).into_bytes(),
        );
    }
    out
}

/// Subassembly `s`: the base plate fixed at its origin, pins standing on
/// it in rows (each repeated along x), and beside it the plates, brackets
/// (fastened), shafts (revolute) and, metres off, the race bodies.
fn subassembly(spec: &Spec, s: usize) -> BTreeMap<String, Vec<u8>> {
    let mut order: Vec<(String, Value)> = Vec::new();
    let mut mates = Vec::new();
    let mut patterns = Vec::new();
    let pins: Vec<usize> = (0..PARTS_PER_SUB)
        .filter(|j| Family::of(spec, s, *j) == Family::Pin)
        .collect();
    let singles = PARTS_PER_SUB - pins.len();
    // Each pin and its copies: the occurrences the singles leave.
    let copies = OCCURRENCES_PER_SUB - singles;
    let (each, extra) = (copies / pins.len(), copies % pins.len());
    for j in 0..PARTS_PER_SUB {
        let part = part_id(spec, s, j);
        match Family::of(spec, s, j) {
            Family::Plate if j == 0 => order.push(("base".into(), instance(&part, [0.0; 3], true))),
            Family::Plate => {
                let k = (j - 1) as f64;
                let at = [
                    400.0 + 130.0 * (k % 7.0),
                    350.0 + 100.0 * (k / 7.0).floor(),
                    0.0,
                ];
                order.push((format!("pl{j}"), instance(&part, at, true)));
            }
            Family::Bracket => {
                let at = [400.0 + 70.0 * (j - 14) as f64, 0.0, 0.0];
                order.push((format!("b{j}"), instance(&part, at, false)));
                mates.push((format!("m{j}"), mate("fastened", &format!("b{j}"), at)));
            }
            Family::Shaft => {
                let at = [400.0 + 110.0 * (j - 24) as f64, 200.0, 0.0];
                order.push((format!("s{j}"), instance(&part, at, false)));
                mates.push((format!("m{j}"), mate("revolute", &format!("s{j}"), at)));
            }
            Family::Pin => {
                let row = pins.iter().position(|p| *p == j).expect("a pin");
                let at = [10.0, 10.0 + 9.0 * row as f64, 0.0];
                let inst = format!("n{j}");
                order.push((inst.clone(), instance(&part, at, false)));
                mates.push((format!("m{j}"), mate("slider", &inst, at)));
                let count = each + usize::from(row < extra);
                patterns.push((
                    format!("r{j}"),
                    feature(
                        "instance_pattern",
                        4,
                        json!({"count": format!("{count}"), "direction": "x",
                               "form": "linear", "instances": [inst], "spacing": mm(9.0)}),
                    ),
                ));
            }
            Family::Race => {
                let k = (j - (PARTS_PER_SUB - spec.imports_in(s))) as f64;
                let at = [2000.0 + 10000.0 * k, -8000.0, 0.0];
                order.push((format!("i{j}"), instance(&part, at, true)));
            }
        }
    }
    order.extend(mates);
    order.extend(patterns);
    assembly_files(&sub_id(s), order)
}

/// Module `m`: its subassemblies in two columns of five, fixed.
fn module(spec: &Spec, m: usize) -> BTreeMap<String, Vec<u8>> {
    let first = m * spec.per_module;
    let last = (first + spec.per_module).min(spec.subassemblies);
    let order = (first..last)
        .map(|s| {
            let k = (s - first) as f64;
            let at = [30_000.0 * (k % 2.0), 15_000.0 * (k / 2.0).floor(), 0.0];
            (format!("s{s}"), instance(&sub_id(s), at, true))
        })
        .collect();
    assembly_files(&module_id(m), order)
}

/// The root: every module, five to a row, fixed.
fn root_assembly(spec: &Spec) -> BTreeMap<String, Vec<u8>> {
    let order = (0..spec.modules())
        .map(|m| {
            let at = [
                -150_000.0 + 65_000.0 * (m % 5) as f64,
                -160_000.0 + 85_000.0 * (m / 5) as f64,
                0.0,
            ];
            (format!("m{m}"), instance(&module_id(m), at, true))
        })
        .collect();
    assembly_files("root", order)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two subassemblies' files hash as `tests/golden/scale.txt` says (the
    /// generator's golden: `UPDATE_GOLDEN=1` after a deliberate change),
    /// build without a failure, and their mates hold as written.
    #[test]
    fn a_small_project_is_as_pinned_builds_and_holds_its_mates() {
        let dir = tempfile::tempdir().unwrap();
        let spec = Spec {
            subassemblies: 2,
            per_module: 1,
            seed: 192,
            races: true,
        };
        let summary = write(dir.path(), &spec, &race_file(spec.imports()));
        assert_eq!(
            (summary.parts, summary.assemblies, summary.occurrences),
            (100, 5, 1000)
        );
        let mut golden = ryukei_testkit::Golden::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/golden/scale.txt"
        ));
        golden.section("scale").str(&summary.digest);
        golden.finish();
        let mut s = crate::session::Session::open(dir.path()).unwrap();
        let root = Id::parse("root", "part").unwrap();
        let r = s.rebuilt(&root).unwrap().clone();
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        for sub in 0..2 {
            let id = Id::parse(&sub_id(sub), "part").unwrap();
            let r = s.rebuilt(&id).unwrap().clone();
            assert_eq!(
                r.failed(),
                0,
                "{:?}",
                r.features.iter().find(|f| f.1.is_err())
            );
            assert!(r.warnings(&id).is_empty(), "{:?}", r.warnings(&id));
        }
        for p in s.project().parts.keys().cloned().collect::<Vec<_>>() {
            let r = s.rebuilt(&p).unwrap();
            assert_eq!(
                r.failed(),
                0,
                "{p}: {:?}",
                r.features.iter().find(|f| f.1.is_err())
            );
        }
        let model = s.model(&root, &crate::doc::assembly::Rev::Working).unwrap();
        assert_eq!(model.occurrences.len(), 1000);
    }
}
