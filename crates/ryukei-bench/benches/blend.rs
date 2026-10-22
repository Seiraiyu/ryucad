//! Phase 6 benchmarks: a plate's 1, 100 and 1,000 hole rims filleted in
//! one call, on one thread and on all of them; every edge of a block
//! (12 cylinders, 8 balls); a boss filleted to a shaft (a pipe); a box
//! edge chamfered; a pipe met with a plane; and the self-intersection
//! check on a filleted block. `blend_kernels` times OCCT and SMLib on
//! the same cases.

use divan::black_box;
use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::check_intersections;
use ryukei::geom::{Box3, Curve, Point3, Rigid, Surface, Vector3};
use ryukei::intersect::pipe_surface;
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::FRAC_PI_2;

fn main() {
    divan::main();
}

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn fillets(b: &Body, edges: impl Iterator<Item = ryukei::topo::EdgeId>, r: f64) -> Vec<EdgeBlend> {
    edges
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: BlendKind::Fillet { radius: r },
        })
        .collect()
}

/// A plate `cols` by `rows` by 0.5 with a hole of radius 0.3 in each cell,
/// and its holes' top rims.
fn plate(cols: usize, rows: usize) -> (Body, Vec<EdgeBlend>) {
    let block = primitive::block(OpKey(1), cols as f64, rows as f64, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<Tool> = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| at(i as f64 + 0.5, j as f64 + 0.5, -0.25)))
        .enumerate()
        .map(|(i, place)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place,
        })
        .collect();
    let b = boolean(OpKey(3), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0;
    let rims = b.edges().filter(|&e| {
        let x = b.edge(e).unwrap();
        matches!(b.curve(x.curve), Some(Curve::Circle(c)) if c.frame().origin().z > 0.25)
    });
    let blends = fillets(&b, rims, 0.05);
    (b, blends)
}

const RIMS: [(usize, usize); 3] = [(1, 1), (10, 10), (40, 25)];

#[divan::bench(args = RIMS, sample_count = 10)]
fn plate_rims_one_thread(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let (b, blends) = plate(cols, rows);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    bencher.bench(|| pool.install(|| black_box(blend(OpKey(9), &b, &blends).unwrap())));
}

#[divan::bench(args = RIMS, sample_count = 10)]
fn plate_rims_all_threads(bencher: divan::Bencher, (cols, rows): (usize, usize)) {
    let (b, blends) = plate(cols, rows);
    bencher.bench(|| black_box(blend(OpKey(9), &b, &blends).unwrap()));
}

fn block() -> Body {
    primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0
}

#[divan::bench(sample_count = 10)]
fn every_edge_of_a_block(bencher: divan::Bencher) {
    let b = block();
    let blends = fillets(&b, b.edges(), 0.3);
    bencher.bench(|| black_box(blend(OpKey(9), &b, &blends).unwrap()));
}

#[divan::bench(sample_count = 10)]
fn a_block_edge_chamfered(bencher: divan::Bencher) {
    let b = block();
    let e = b.edges().next().unwrap();
    let blends = [EdgeBlend {
        edge: b.edge(e).unwrap().name,
        kind: BlendKind::Chamfer(Chamfer::Equal { distance: 0.3 }),
    }];
    bencher.bench(|| black_box(blend(OpKey(9), &b, &blends).unwrap()));
}

/// A shaft of radius 1 along `x` with a boss of radius 0.5 on it, and the
/// boss's foot.
fn shaft() -> (Body, Vec<EdgeBlend>) {
    let y = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let along_x = Rigid::rotation(y, FRAC_PI_2).then_after(&at(0.0, 0.0, -2.0));
    let shaft = primitive::cylinder(OpKey(1), 1.0, 4.0, &along_x).unwrap().0;
    let boss = primitive::cylinder(OpKey(2), 0.5, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &boss,
        place: Rigid::IDENTITY,
    };
    let b = boolean(OpKey(3), BooleanOp::Unite, &shaft, &[tool])
        .unwrap()
        .0;
    let foot = b
        .edges()
        .filter(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Nurbs(_))));
    let blends = fillets(&b, foot, 0.1);
    (b, blends)
}

#[divan::bench(sample_count = 3)]
fn a_boss_on_a_shaft(bencher: divan::Bencher) {
    let (b, blends) = shaft();
    bencher.bench(|| black_box(blend(OpKey(9), &b, &blends).unwrap()));
}

#[divan::bench(sample_count = 3)]
fn a_pipe_met_with_a_plane(bencher: divan::Bencher) {
    let (b, blends) = shaft();
    let r = blend(OpKey(9), &b, &blends).unwrap().0;
    let pipe = r
        .faces()
        .map(|f| r.surface(r.face(f).unwrap().surface).unwrap().clone())
        .find(|s| matches!(s, Surface::Pipe(_)))
        .unwrap();
    let plane = Surface::Plane(ryukei::geom::Plane::new(ryukei::geom::Frame::WORLD));
    let region = Box3::new(Point3::new(-2.0, -2.0, -2.0), Point3::new(2.0, 2.0, 2.0));
    bencher.bench(|| black_box(pipe_surface(&pipe, &plane, &region).unwrap()));
}

#[divan::bench(sample_count = 3)]
fn check_a_rounded_block(bencher: divan::Bencher) {
    let b = block();
    let blends = fillets(&b, b.edges(), 0.3);
    let r = blend(OpKey(9), &b, &blends).unwrap().0;
    bencher.bench(|| black_box(check_intersections(&r)));
}
