//! The SMLib spike (design 27, stage 0): a block minus a grid of 1, 100
//! and 1000 through-holes, built, cut and meshed by OCCT and by SMLib,
//! on one thread and on all of them. Times each step ("load" is building
//! the block and the cylinders), and hashes every result (the B-rep's
//! face count and vertex positions, and the mesh's positions and
//! triangles, bit for bit) to see whether it repeats within one process
//! and across processes. Prints a report (run with `--nocapture`) and
//! asserts the repeatability that held when the spike was run.
//!
//! The 1000-hole cases take SMLib about a minute and a half each, so they
//! only run with `RYUKEI_SPIKE_1000=1`, and then once per process rather
//! than ten times.
//!
//! The other process is this test binary run again with `CHILD` set: it
//! then prints one line of hashes per case and checks nothing.

use ryukei::geom::{Rigid, Vector3};
use ryukei_bench::Primitive;
use ryukei_bench::{occt, smlib};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::process::Command;
use std::time::{Duration, Instant};

const CHILD: &str = "RYUKEI_SPIKE_CHILD";
const LARGE: &str = "RYUKEI_SPIKE_1000";

/// The holes' spacing and radius, and the block's thickness: holes are
/// 0.4 apart and 0.2 from the block's sides, so nothing touches.
const PITCH: f64 = 1.0;
const RADIUS: f64 = 0.3;
const THICKNESS: f64 = 0.5;
/// How far the cylinders stick out of the block at each end, so no
/// faces coincide.
const OVERHANG: f64 = 0.25;
/// Coarser than the head-to-head's: SMLib's mesher slows down more than
/// linearly with the holes.
const CHORD: f64 = 1e-2;
const ANGLE: f64 = 0.5;

/// Hole grids, columns by rows: 1, 100 and (with `LARGE`) 1000 holes.
fn grids() -> Vec<(usize, usize)> {
    let mut g = vec![(1, 1), (10, 10)];
    if std::env::var_os(LARGE).is_some() {
        g.push((40, 25));
    }
    g
}

/// How many times each case runs in this process.
fn repeats(holes: usize) -> usize {
    if holes >= 1000 { 1 } else { 10 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kernel {
    Occt,
    Smlib,
}

impl Kernel {
    fn name(self) -> &'static str {
        match self {
            Kernel::Occt => "occt",
            Kernel::Smlib => "smlib",
        }
    }
}

/// The block and the cylinders' placements.
fn grid((cols, rows): (usize, usize)) -> (Primitive, Vec<Rigid>) {
    let block = Primitive::Block([cols as f64 * PITCH, rows as f64 * PITCH, THICKNESS]);
    let mut at = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        for i in 0..cols {
            at.push(Rigid::translation(Vector3::new(
                (i as f64 + 0.5) * PITCH,
                (j as f64 + 0.5) * PITCH,
                -OVERHANG,
            )));
        }
    }
    (block, at)
}

fn cylinder() -> Primitive {
    Primitive::Cylinder {
        r: RADIUS,
        h: THICKNESS + 2.0 * OVERHANG,
    }
}

/// The exact volume and area of the block with its holes.
fn exact((cols, rows): (usize, usize)) -> (f64, f64) {
    let (w, d, n) = (
        cols as f64 * PITCH,
        rows as f64 * PITCH,
        (cols * rows) as f64,
    );
    let face = w * d - n * PI * RADIUS * RADIUS;
    let area = 2.0 * face + 2.0 * (w + d) * THICKNESS + n * 2.0 * PI * RADIUS * THICKNESS;
    (face * THICKNESS, area)
}

/// FNV-1a over 64-bit words.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn u64(mut self, x: u64) -> Self {
        for byte in x.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    fn f64s(self, xs: &[f64]) -> Self {
        xs.iter().fold(self, |h, x| h.u64(x.to_bits()))
    }

    fn u32s(self, xs: &[u32]) -> Self {
        xs.iter().fold(self, |h, &x| h.u64(u64::from(x)))
    }

    /// A mesh: its triangle count, positions and triangles.
    fn mesh(positions: &[f64], triangles: &[u32]) -> u64 {
        Self::new()
            .u64(triangles.len() as u64)
            .f64s(positions)
            .u32s(triangles)
            .0
    }

    /// A B-rep: its face count and vertex positions.
    fn brep(faces: u32, vertices: &[f64]) -> u64 {
        Self::new().u64(faces.into()).f64s(vertices).0
    }
}

/// One run of one case.
struct Run {
    load: Duration,
    boolean: Duration,
    mesh: Duration,
    brep: u64,
    triangles: u64,
    faces: u32,
    triangle_count: usize,
    /// The mesh's enclosed volume.
    volume: f64,
}

fn run(kernel: Kernel, g: (usize, usize), parallel: bool) -> Run {
    let (block, holes) = grid(g);
    let start = Instant::now();
    match kernel {
        Kernel::Occt => {
            let solid = occt::Solid::new(block, &Rigid::IDENTITY);
            let tools: Vec<_> = holes
                .iter()
                .map(|at| occt::Solid::new(cylinder(), at))
                .collect();
            let load = start.elapsed();
            let start = Instant::now();
            let cut = solid.cut(&tools, parallel);
            let boolean = start.elapsed();
            let start = Instant::now();
            let m = cut.mesh(CHORD, ANGLE, parallel);
            let mesh = start.elapsed();
            assert_eq!(m.faces_without_mesh, 0, "OCCT left faces unmeshed");
            let faces = cut.faces();
            Run {
                load,
                boolean,
                mesh,
                brep: Fnv::brep(faces, &cut.vertices()),
                triangles: Fnv::mesh(&m.positions, &m.triangles),
                faces,
                triangle_count: m.triangles.len() / 3,
                volume: m.volume_and_area().0,
            }
        }
        Kernel::Smlib => {
            let mut solid = smlib::Solid::new(block, &Rigid::IDENTITY, false);
            for at in &holes {
                solid.add_tool(cylinder(), at);
            }
            let load = start.elapsed();
            let start = Instant::now();
            solid.subtract_tools(parallel);
            let boolean = start.elapsed();
            let start = Instant::now();
            let m = solid.mesh(CHORD, ANGLE, parallel);
            let mesh = start.elapsed();
            assert!(!m.failed_faces, "SMLib failed faces");
            assert_eq!(m.lamina_edges, 0, "SMLib mesh is open");
            let faces = solid.faces();
            Run {
                load,
                boolean,
                mesh,
                brep: Fnv::brep(faces, &solid.vertices()),
                triangles: Fnv::mesh(&m.positions, &m.triangles),
                faces,
                triangle_count: m.triangles.len() / 3,
                volume: m.soup().volume_and_area().0,
            }
        }
    }
}

/// Every case: kernel, grid, and whether on all threads.
fn cases() -> Vec<(Kernel, (usize, usize), bool)> {
    let mut out = Vec::new();
    for g in grids() {
        for parallel in [false, true] {
            for k in [Kernel::Occt, Kernel::Smlib] {
                out.push((k, g, parallel));
            }
        }
    }
    out
}

/// The hashes another process got, by kernel, holes and parallel: the
/// B-rep's, then the mesh's.
fn other_process() -> BTreeMap<(Kernel, usize, bool), (u64, u64)> {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["spike", "--exact", "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .output()
        .expect("the test binary runs again");
    assert!(out.status.success(), "the child process failed");
    let mut hashes = BTreeMap::new();
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        // The first line follows the test harness's "test spike ... ".
        let Some(at) = line.find("spike-hash ") else {
            continue;
        };
        let w: Vec<&str> = line[at..].split_whitespace().collect();
        let kernel = if w[1] == "occt" {
            Kernel::Occt
        } else {
            Kernel::Smlib
        };
        let key = (kernel, w[2].parse().unwrap(), w[3] == "all");
        hashes.insert(key, (w[4].parse().unwrap(), w[5].parse().unwrap()));
    }
    hashes
}

fn yes(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "NO",
        None => "-",
    }
}

#[test]
fn spike() {
    if std::env::var_os(CHILD).is_some() {
        for (k, g, parallel) in cases() {
            let r = run(k, g, parallel);
            let threads = if parallel { "all" } else { "one" };
            println!(
                "spike-hash {} {} {threads} {} {}",
                k.name(),
                g.0 * g.1,
                r.brep,
                r.triangles
            );
        }
        return;
    }

    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let child = other_process();
    println!("on one thread or all {threads}; times in ms, of the first run");
    println!(
        "{:<6} {:>5} {:>7} | {:>9} {:>9} {:>9} | {:>6} {:>8} | {:>4} {:>11} {:>11}",
        "kernel",
        "holes",
        "threads",
        "load",
        "boolean",
        "mesh",
        "faces",
        "tris",
        "runs",
        "in process",
        "across"
    );
    // Whether each property held, by kernel and threads, over every case
    // that tested it: B-rep and mesh hashes in this process, then across
    // processes, then the same B-rep and mesh at one thread and at all.
    let mut held: BTreeMap<(Kernel, bool), [Option<bool>; 4]> = BTreeMap::new();
    let mut one_thread: BTreeMap<(Kernel, usize), (u64, u64)> = BTreeMap::new();
    let mut same_threads: BTreeMap<Kernel, (bool, bool)> = BTreeMap::new();
    for (k, g, parallel) in cases() {
        let holes = g.0 * g.1;
        let runs: Vec<Run> = (0..repeats(holes)).map(|_| run(k, g, parallel)).collect();
        let first = &runs[0];
        let (volume, area) = exact(g);
        assert!(
            (first.volume - volume).abs() <= CHORD * area,
            "{}, {holes} holes: volume {} vs {volume}",
            k.name(),
            first.volume
        );
        assert_eq!(first.faces as usize, 6 + holes, "{}: faces", k.name());
        let here = (runs.len() > 1).then(|| {
            (
                runs.iter().all(|r| r.brep == first.brep),
                runs.iter().all(|r| r.triangles == first.triangles),
            )
        });
        let there = child[&(k, holes, parallel)];
        let now = [
            here.map(|h| h.0),
            here.map(|h| h.1),
            Some(there.0 == first.brep),
            Some(there.1 == first.triangles),
        ];
        let h = held.entry((k, parallel)).or_insert([None; 4]);
        for (h, now) in h.iter_mut().zip(now) {
            if let Some(now) = now {
                *h = Some(h.unwrap_or(true) && now);
            }
        }
        if parallel {
            let one = one_thread[&(k, holes)];
            let s = same_threads.entry(k).or_insert((true, true));
            s.0 &= one.0 == first.brep;
            s.1 &= one.1 == first.triangles;
        } else {
            one_thread.insert((k, holes), (first.brep, first.triangles));
        }
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        println!(
            "{:<6} {holes:>5} {:>7} | {:>9.1} {:>9.1} {:>9.1} | {:>6} {:>8} | {:>4} {:>5} {:>5} {:>5} {:>5}",
            k.name(),
            if parallel { "all" } else { "one" },
            ms(first.load),
            ms(first.boolean),
            ms(first.mesh),
            first.faces,
            first.triangle_count,
            runs.len(),
            yes(now[0]),
            yes(now[1]),
            yes(now[2]),
            yes(now[3]),
        );
    }
    println!("(in process and across: the B-rep's hash, then the mesh's)");
    for ((k, parallel), h) in &held {
        println!(
            "{:<5} on {} thread(s): B-rep repeats in process {}, across processes {}; mesh in process {}, across {}",
            k.name(),
            if *parallel { "all" } else { "one" },
            yes(h[0]),
            yes(h[2]),
            yes(h[1]),
            yes(h[3]),
        );
    }
    for (k, (brep, mesh)) in &same_threads {
        println!(
            "{:<5}: one thread and all give the same B-rep {}, the same mesh {}",
            k.name(),
            yes(Some(*brep)),
            yes(Some(*mesh)),
        );
    }
    // What held when the spike was run: both kernels repeat bit for bit,
    // B-rep and mesh, in one process and across processes, at one thread
    // and at all, and give the same bits at one thread as at all.
    for ((k, parallel), h) in &held {
        assert_eq!(
            h,
            &[Some(true); 4],
            "{} on {} threads no longer repeats",
            k.name(),
            if *parallel { "all" } else { "one" }
        );
    }
    for (k, same) in &same_threads {
        assert_eq!(
            same,
            &(true, true),
            "{} differs between one thread and all",
            k.name()
        );
    }
}
