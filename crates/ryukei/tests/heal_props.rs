//! Bodies written as a file must describe them (`heal::seamed`) and
//! healed again: seeded sweeps, booleans and blends, corners among them,
//! as the fuzz targets make them from bytes. Each must come back as the
//! same faces, edges and vertices on the same geometry (a pipe face as a
//! NURBS), pass the checker, and heal written again to the same body bit
//! for bit. A body the operation made that doesn't mesh is its fault, not
//! writing's, and is counted apart. Release only: minutes in a debug
//! build.

use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::check;
use ryukei::heal::{HealOptions, heal, seamed};
use ryukei::name::OpKey;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;

/// Each face's surface (with its sense), each edge's curve and each
/// vertex's point, as the body's dump gives their bits, sorted; and how
/// many faces lie on pipes.
fn geometry(b: &Body) -> (Vec<String>, usize) {
    let dump = b.dump();
    let mut surfaces = std::collections::BTreeMap::new();
    let mut curves = std::collections::BTreeMap::new();
    for line in dump.lines() {
        let mut w = line.splitn(3, ' ');
        match (w.next(), w.next(), w.next()) {
            (Some("surface"), Some(i), Some(r)) => {
                surfaces.insert(i.to_string(), r.to_string());
            }
            (Some("curve"), Some(i), Some(r)) => {
                curves.insert(i.to_string(), r.to_string());
            }
            _ => {}
        }
    }
    let field = |line: &str, key: &str| -> String {
        line.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .unwrap_or_default()
            .to_string()
    };
    let mut out = Vec::new();
    let mut pipes = 0;
    for line in dump.lines() {
        if line.starts_with("face ") {
            let s = &surfaces[&field(line, "surface=")];
            if s.starts_with("pipe ") {
                pipes += 1;
            }
            out.push(format!("face {s} {}", field(line, "sense=")));
        } else if line.starts_with("edge ") {
            out.push(format!("edge {}", curves[&field(line, "curve=")]));
        } else if line.starts_with("vertex ") {
            out.push(format!("vertex {}", field(line, "point=")));
        }
    }
    out.sort();
    (out, pipes)
}

/// Why a round trip failed, for the report.
fn round_trip(b: &Body) -> Result<(), String> {
    let s = seamed(b, 1e-7).map_err(|e| format!("seamed: {e}"))?;
    let (h, _, _) =
        heal(OpKey(9), &s, &HealOptions::default()).map_err(|e| format!("heal: {e}"))?;
    let faults = check(&h);
    if !faults.is_empty() {
        return Err(format!("check: {faults:?}"));
    }
    if h.counts() != b.counts() {
        return Err(format!("counts {:?} != {:?}", h.counts(), b.counts()));
    }
    let ((gb, pipes), (gh, _)) = (geometry(b), geometry(&h));
    if pipes == 0 && gb != gh {
        return Err("geometry differs".into());
    }
    let s2 = seamed(&h, 1e-7).map_err(|e| format!("seamed again: {e}"))?;
    let again = heal(OpKey(9), &s2, &HealOptions::default())
        .map_err(|e| format!("heal again: {e}"))?
        .0;
    if again.dump() != h.dump() {
        return Err("not a fixed point".into());
    }
    Ok(())
}

fn seeded(seed: u64, n: usize) -> Vec<u8> {
    let mut g = Lcg::new(seed);
    (0..n).map(|_| (g.next_u64() >> 56) as u8).collect()
}

/// Whether a body meshes without faults: the fuzz targets' bar for a
/// sound result. A body that doesn't is a fault of the operation that
/// made it, not of writing it.
fn meshes(b: &Body) -> bool {
    use ryukei::mesh::{MeshParams, check_mesh, tessellate};
    tessellate(b, &MeshParams::new(1e-2, 0.5))
        .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

/// Runs `make` over seeds, round trips what it gives, and fails listing
/// the seeds that didn't come back (unless the body made doesn't mesh).
fn corpus(name: &str, seeds: u64, make: impl Fn(&[u8]) -> Option<Body>) {
    let mut tried = 0;
    let mut unsound = 0;
    let mut failed = Vec::new();
    for seed in 0..seeds {
        let Some(b) = make(&seeded(seed, 96)) else {
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
    corpus("sweep", 600, |d| sweep(d).map(|x| x.0));
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn booleans_round_trip() {
    corpus("boolean", 400, |d| {
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
    corpus("blend", 400, |d| {
        let case = blend_case(d)?;
        blend(OpKey(9), &case.body, &case.blends).ok().map(|x| x.0)
    });
}
