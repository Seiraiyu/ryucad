//! Seeded sweeps, booleans and blends (corners among them), as the fuzz
//! targets make them from bytes, written to STEP and read back: the same
//! faces, edges and vertices on the same geometry (a revolution's frame
//! turned to its curve, a pipe as a NURBS), and from the first file
//! written from a body read back, the same bytes and the same body
//! however often they go round. A
//! body the operation made that doesn't mesh is its fault, not
//! writing's, and is counted apart. Release only: minutes in a debug
//! build.

use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::check;
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;
use ryustep::{LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};

fn write_one(b: &Body) -> Result<Vec<u8>, String> {
    write(
        &WriteModel {
            products: vec![Product {
                id: "part".into(),
                name: "part".into(),
                bodies: vec![b.clone()],
                children: vec![],
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            ..WriteOptions::default()
        },
    )
    .map_err(|e| format!("write: {e}"))
}

fn read_one(bytes: &[u8]) -> Result<Body, String> {
    let m = read(
        bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .map_err(|e| format!("read: {e}"))?;
    if m.bodies.len() != 1 {
        return Err(format!("{} bodies", m.bodies.len()));
    }
    let b = m.bodies[0]
        .result
        .clone()
        .map_err(|e| format!("heal: {e}"))?
        .0;
    let faults = check(&b);
    if !faults.is_empty() {
        return Err(format!("check: {faults:?}"));
    }
    Ok(b)
}

/// Each face's surface (with its sense), each edge's curve and each
/// vertex's point, as the dump gives their bits, sorted; revolutions and
/// pipes by kind only (their frames and forms change).
fn geometry(b: &Body) -> Vec<String> {
    let dump = b.dump();
    let lines: Vec<&str> = dump.lines().collect();
    let entry = |kind: &str, id: &str| {
        let head = format!("{kind} {id} ");
        let x = lines
            .iter()
            .find_map(|l| l.strip_prefix(&head))
            .unwrap_or_default();
        if x.starts_with("revolution ") || x.starts_with("pipe ") || x.starts_with("nurbs ") {
            x.split(' ').next().unwrap_or_default().to_string()
        } else {
            x.to_string()
        }
    };
    let field = |l: &str, key: &str| {
        l.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .unwrap_or_default()
            .to_string()
    };
    let mut out: Vec<String> = lines
        .iter()
        .filter_map(|l| {
            if l.starts_with("face ") {
                let s = entry("surface", &field(l, "surface="));
                // A pipe comes back a NURBS.
                let s = if s == "pipe" { "nurbs".to_string() } else { s };
                Some(format!("face {s} {}", field(l, "sense=")))
            } else if l.starts_with("edge ") {
                Some(format!("edge {}", entry("curve", &field(l, "curve="))))
            } else if l.starts_with("vertex ") {
                Some(format!("vertex {}", field(l, "point=")))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

fn round_trip(b: &Body) -> Result<(), String> {
    let w1 = write_one(b)?;
    let b1 = read_one(&w1)?;
    if b1.counts() != b.counts() {
        return Err(format!("counts {:?} != {:?}", b1.counts(), b.counts()));
    }
    if geometry(&b1) != geometry(b) {
        return Err("geometry differs".into());
    }
    // From the first file written from a body read back, files and
    // bodies go round unchanged.
    let w2 = write_one(&b1)?;
    let b2 = read_one(&w2)?;
    let w3 = write_one(&b2)?;
    if w3 != w2 {
        return Err("bytes not a fixed point".into());
    }
    if read_one(&w3)?.dump() != b2.dump() {
        return Err("not a fixed point".into());
    }
    Ok(())
}

fn meshes(b: &Body) -> bool {
    use ryukei::mesh::{MeshParams, check_mesh, tessellate};
    tessellate(b, &MeshParams::new(1e-2, 0.5))
        .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

fn seeded(seed: u64, n: usize) -> Vec<u8> {
    let mut g = Lcg::new(seed);
    (0..n).map(|_| (g.next_u64() >> 56) as u8).collect()
}

fn corpus(name: &str, seeds: u64, make: impl Fn(&[u8]) -> Option<Body>) {
    let mut tried = 0;
    let mut unsound = 0;
    let mut failed = Vec::new();
    for seed in 0..seeds {
        // An empty body (a boolean that left nothing) writes no solid.
        let Some(b) = make(&seeded(seed, 96)).filter(|b| b.faces().next().is_some()) else {
            continue;
        };
        tried += 1;
        if let Err(why) = round_trip(&b) {
            if meshes(&b) {
                failed.push(format!("{name} seed {seed}: {why}"));
            } else {
                unsound += 1;
            }
        }
    }
    assert!(tried > 0, "{name}: no bodies");
    assert!(
        failed.is_empty(),
        "{tried} tried, {unsound} unsound\n{}",
        failed.join("\n")
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn sweeps_round_trip() {
    corpus("sweep", 400, |d| sweep(d).map(|x| x.0));
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn booleans_round_trip() {
    corpus("boolean", 300, |d| {
        let case = boolean_case(d)?;
        boolean(OpKey(9), case.op, &case.target, &case.tools())
            .ok()
            .map(|x| x.0)
    });
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn blends_round_trip() {
    corpus("blend", 300, |d| {
        let case = blend_case(d)?;
        blend(OpKey(9), &case.body, &case.blends).ok().map(|x| x.0)
    });
}
