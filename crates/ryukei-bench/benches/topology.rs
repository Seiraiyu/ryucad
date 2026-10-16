//! Phase 1 benchmarks: building each primitive, checking it, a long chain
//! of Euler operators, and cloning and compacting the body it leaves.

use divan::black_box;
use ryukei::check::check;
use ryukei::euler::{Corner, kev, mev};
use ryukei::geom::{Curve, Line, Point3, Rigid, UnitVector3, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive::{block, cone, cylinder, frustum, sphere, torus};
use ryukei::topo::{Body, EdgeId, VertexId};

fn main() {
    divan::main();
}

const KINDS: [&str; 6] = ["block", "cylinder", "cone", "frustum", "sphere", "torus"];

fn placement() -> Rigid {
    let axis = Vector3::new(1.0, 2.0, 3.0).normalize().unwrap();
    Rigid::translation(Vector3::new(1.0, 2.0, 3.0)).then_after(&Rigid::rotation(axis, 0.7))
}

fn build(kind: &str) -> Body {
    let (k, at) = (OpKey(1), placement());
    let r = match kind {
        "block" => block(k, 1.0, 2.0, 3.0, &at),
        "cylinder" => cylinder(k, 1.0, 2.0, &at),
        "cone" => cone(k, 1.0, 2.0, &at),
        "frustum" => frustum(k, 1.0, 0.5, 2.0, &at),
        "sphere" => sphere(k, 1.0, &at),
        _ => torus(k, 2.0, 0.5, &at),
    };
    r.unwrap().0
}

#[divan::bench(args = KINDS)]
fn build_primitive(kind: &str) -> Body {
    build(black_box(kind))
}

#[divan::bench(args = KINDS)]
fn check_primitive(bencher: divan::Bencher, kind: &str) {
    let body = build(kind);
    bencher.bench(|| check(black_box(&body)));
}

/// A box with a zigzag path of `n` dangling edges on its top face.
fn zigzag(n: usize) -> (Body, Vec<(EdgeId, VertexId)>) {
    let (mut b, _) = block(OpKey(1), 4.0, 4.0, 4.0, &Rigid::IDENTITY).unwrap();
    let top = b
        .vertices()
        .find(|&v| b.vertex(v).unwrap().point == Point3::new(0.0, 0.0, 4.0))
        .unwrap();
    // Start before the fin that leaves the corner along the top face.
    let mut at = b
        .fins()
        .find(|&f| {
            b.fin_start(f) == Some(top) && {
                let l = b.fin(f).unwrap().loop_;
                let face = b.loop_(l).unwrap().face;
                let s = b.surface(b.face(face).unwrap().surface).unwrap();
                s.normal(0.0, 0.0) == Some(UnitVector3::Z)
            }
        })
        .map(Corner::Before)
        .unwrap();
    let mut p = Point3::new(0.0, 0.0, 4.0);
    let mut made = Vec::with_capacity(n);
    for i in 0..n {
        let q = Point3::new(
            (i + 1) as f64 * 3.0 / n as f64,
            if i % 2 == 0 { 0.5 } else { 0.25 },
            4.0,
        );
        let c = b.add_curve(Curve::Line(Line::through(p, q).unwrap()));
        let key = OpKey(10 + i as u128);
        let (m, _) = mev(&mut b, key, at, q, c, [0.0, p.distance(q)]).unwrap();
        at = Corner::Before(m.fins[1]);
        made.push((m.edge, m.vertex));
        p = q;
    }
    (b, made)
}

#[divan::bench(args = [100, 1000])]
fn mev_then_kev(bencher: divan::Bencher, n: usize) {
    bencher.bench(|| {
        let (mut b, made) = zigzag(n);
        for &(e, v) in made.iter().rev() {
            kev(&mut b, e, v).unwrap();
        }
        b
    });
}

#[divan::bench]
fn clone_body(bencher: divan::Bencher) {
    let (b, _) = zigzag(1000);
    bencher.bench(|| black_box(&b).clone());
}

#[divan::bench]
fn compact_body(bencher: divan::Bencher) {
    let (mut b, made) = zigzag(1000);
    for &(e, v) in made.iter().rev().take(500) {
        kev(&mut b, e, v).unwrap();
    }
    bencher.with_inputs(|| b.clone()).bench_values(|mut b| {
        b.compact();
        b
    });
}
