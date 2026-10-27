//! Phase 7 benchmarks (design 55, plan 7.D): STEP parsed, decoded, read
//! (mapped and healed) and written, separately, for a NIST test model
//! and for an assembly of 24 seeded parts written by ryustep, on one
//! thread and on all of them.

use divan::black_box;
use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;
use ryustep::schema::Entities;
use ryustep::{Product, ReadOptions, WriteModel, WriteOptions, part21, read, write};

fn main() {
    divan::main();
}

const FILES: [&str; 2] = ["nist", "assembly"];
const THREADS: [usize; 2] = [1, 0];

/// Seeded sweeps, booleans and blends.
fn parts(seeds: u64) -> Vec<Body> {
    let bytes = |seed: u64| {
        let mut g = Lcg::new(seed);
        (0..96)
            .map(|_| (g.next_u64() >> 56) as u8)
            .collect::<Vec<u8>>()
    };
    let mut out = Vec::new();
    for seed in 0..seeds {
        if let Some((b, _)) = sweep(&bytes(seed)) {
            out.push(b);
        }
        if let Some(case) = boolean_case(&bytes(seed))
            && let Ok((b, _)) = boolean(OpKey(9), case.op, &case.target, &case.tools())
        {
            out.push(b);
        }
        if let Some(case) = blend_case(&bytes(seed))
            && let Ok((b, _)) = blend(OpKey(9), &case.body, &case.blends)
        {
            out.push(b);
        }
    }
    out.retain(|b| b.faces().next().is_some() && ryukei::heal::seamed(b, 1e-7).is_ok());
    out.truncate(24);
    out
}

fn model(bodies: Vec<Body>) -> WriteModel {
    WriteModel {
        products: bodies
            .into_iter()
            .enumerate()
            .map(|(i, b)| Product {
                id: format!("p{i}"),
                name: format!("p{i}"),
                bodies: vec![b],
                children: vec![],
                ..Default::default()
            })
            .collect(),
    }
}

fn file(name: &str) -> Vec<u8> {
    match name {
        "nist" => std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ryustep/tests/corpus/nist_ctc_01_asme1_rd.stp"
        ))
        .unwrap(),
        _ => write(&model(parts(24)), &WriteOptions::default()).unwrap(),
    }
}

fn opts() -> ReadOptions {
    ReadOptions {
        key: OpKey(7),
        heal: Default::default(),
    }
}

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
}

#[divan::bench(args = FILES)]
fn parse(bencher: divan::Bencher, name: &str) {
    let bytes = file(name);
    bencher.bench(|| black_box(part21::parse(&bytes).unwrap()));
}

#[divan::bench(args = FILES)]
fn decode(bencher: divan::Bencher, name: &str) {
    let parsed = part21::parse(&file(name)).unwrap();
    bencher.bench(|| black_box(Entities::decode(&parsed)));
}

/// Parsed, decoded, mapped and healed, on one thread or on all.
#[divan::bench(args = FILES, consts = THREADS, sample_count = 10)]
fn read_heal<const T: usize>(bencher: divan::Bencher, name: &str) {
    let bytes = file(name);
    let pool = pool(T);
    bencher.bench(|| pool.install(|| black_box(read(&bytes, &opts()).unwrap())));
}

/// The bodies read written again, on one thread or on all.
#[divan::bench(args = FILES, consts = THREADS)]
fn write_back<const T: usize>(bencher: divan::Bencher, name: &str) {
    let m = read(&file(name), &opts()).unwrap();
    let bodies: Vec<Body> = m
        .bodies
        .into_iter()
        .filter_map(|b| b.result.ok().map(|r| r.0))
        .collect();
    let model = model(bodies);
    let pool = pool(T);
    bencher.bench(|| pool.install(|| black_box(write(&model, &WriteOptions::default()).unwrap())));
}
