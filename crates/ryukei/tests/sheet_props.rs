//! Solids cut into sheets and sewn back (design 295, plan 12.C): seeded
//! sweeps, booleans and blends, as the fuzz targets make them from bytes,
//! each cut two ways: every face a sheet of its own (`sheet::extract`),
//! and the faces in two halves (`extract` and `remove`). Every sheet must
//! check clean and mesh, and sewing them back (`heal::sew`) must give a
//! solid per shell of the body (each lump, and each cavity turned to face
//! out), with the body's faces, edges and vertices between them, its
//! faces' names and its volume (certified, within both bounds: the outer
//! shells' less the cavities'). A body the operation made that doesn't
//! mesh is its fault, not sewing's, and is counted apart. Release only:
//! minutes in a debug build.

use ryukei::blend::blend;
use ryukei::boolean::boolean;
use ryukei::check::check;
use ryukei::heal::{SewOptions, sew};
use ryukei::measure::{Entity, MassOptions, Properties, Value, mass, properties};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Name, OpKey};
use ryukei::sheet::{extract, remove};
use ryukei::topo::{Body, BodyKind, ShellId};
use ryukei::{Error, SheetError};
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::blend_case;
use ryukei_testkit::boolean_bytes::boolean_case;
use ryukei_testkit::sweep_bytes::sweep;

fn seeded(seed: u64, n: usize) -> Vec<u8> {
    let mut g = Lcg::new(seed);
    (0..n).map(|_| (g.next_u64() >> 56) as u8).collect()
}

/// Whether a body meshes soundly.
fn meshes(b: &Body) -> bool {
    tessellate(b, &MeshParams::new(1e-2, 0.5))
        .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

/// A solid's certified volume.
fn volume(b: &Body) -> Result<Value, String> {
    match properties(b, Entity::Body) {
        Ok(Properties::Body { volume, .. }) => Ok(volume),
        other => Err(format!("volume: {other:?}")),
    }
}

/// The chord `properties` certifies `b`'s volume at: a hundred-thousandth
/// of its size. A piece sewn back is measured at its whole body's chord,
/// not its own: the mesher can stall on a face at one chord and not at
/// another (boolean seed 78's lens of two spheres stalls at 5e-5 m and
/// meshes at 4.9e-5 m and 6e-5 m, whole or sewn: issue #360), so
/// measuring a piece at another chord than its body's would judge the
/// mesher, not sewing.
fn chord(b: &Body) -> f64 {
    let x = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .expect("a body with faces");
    ((x.max() - x.min()).length() * 1e-5).max(10.0 * ryukei::tol::LINEAR_RESOLUTION)
}

/// A solid's certified volume at a given chord.
fn volume_at(b: &Body, chord: f64) -> Result<Value, String> {
    let options = MassOptions {
        chord: Some(chord),
        ..MassOptions::default()
    };
    mass(b, 1.0, &options)
        .map(|m| m.volume)
        .map_err(|e| format!("volume: {e:?}"))
}

/// The names of each shell's faces, sorted.
fn shell_names(b: &Body, sh: ShellId) -> Vec<Name> {
    let mut x: Vec<Name> = b.shell_faces(sh).map(|f| b.face(f).unwrap().name).collect();
    x.sort();
    x
}

/// Whether a shell is a cavity: not the one of its region with the
/// largest box, which holds the others.
fn cavity(b: &Body, sh: ShellId) -> bool {
    let size = |s: ShellId| {
        b.shell_faces(s)
            .map(|f| ryukei::intersect::face_box(b, f))
            .reduce(|x, y| x.union(&y))
            .map_or(0.0, |x| (x.max() - x.min()).length())
    };
    let region = b.shell(sh).unwrap().region;
    let outer = b
        .region_shells(region)
        .max_by(|&x, &y| size(x).total_cmp(&size(y)))
        .unwrap();
    outer != sh
}

/// Each shell's faces' names, sorted.
fn shells(b: &Body) -> Vec<Vec<Name>> {
    let mut out: Vec<Vec<Name>> = b.shells().map(|sh| shell_names(b, sh)).collect();
    out.sort();
    out
}

fn names(b: &Body) -> Vec<Name> {
    let mut x: Vec<Name> = b.faces().map(|f| b.face(f).unwrap().name).collect();
    x.sort();
    x
}

/// Why sewing `sheets` didn't give `b` back, for the report.
fn sewn_back(b: &Body, whole: Value, sheets: &[Body]) -> Result<(), String> {
    for s in sheets {
        if s.kind() != BodyKind::Sheet {
            return Err(format!("a cut is a {:?}", s.kind()));
        }
        let faults = check(s);
        if !faults.is_empty() {
            return Err(format!("a sheet: check {faults:?}"));
        }
        if !meshes(s) {
            return Err("a sheet doesn't mesh".into());
        }
    }
    let (out, _) =
        sew(OpKey(11), sheets, &SewOptions::default()).map_err(|e| format!("sew: {e}"))?;
    // A solid per shell, with that shell's faces.
    let want = shells(b);
    let mut got: Vec<Vec<Name>> = out.iter().map(names).collect();
    got.sort();
    if got != want {
        return Err(format!(
            "{} pieces for {} shells, or other faces",
            out.len(),
            want.len()
        ));
    }
    let (mut faces, mut edges, mut vertices) = (0, 0, 0);
    let (mut sum, mut bound) = (0.0, whole.bound());
    for h in &out {
        if h.kind() != BodyKind::Solid {
            return Err(format!("sewn into a {:?}", h.kind()));
        }
        let faults = check(h);
        if !faults.is_empty() {
            return Err(format!("check: {faults:?}"));
        }
        let c = h.counts();
        (faces, edges, vertices) = (faces + c.faces, edges + c.edges, vertices + c.vertices);
        let v = volume_at(h, chord(b))?;
        // A cavity sewn alone faces out: its volume is the body's less.
        let inside = b
            .shells()
            .any(|sh| shell_names(b, sh) == names(h) && cavity(b, sh));
        sum += if inside { -v.value() } else { v.value() };
        bound += v.bound();
    }
    let c = b.counts();
    if (faces, edges, vertices) != (c.faces, c.edges, c.vertices) {
        return Err(format!("counts {faces}, {edges}, {vertices} != {c:?}"));
    }
    // The bounds, and rounding: a block's bound is rounding alone.
    let bound = bound + 1e-12 * whole.value().abs();
    if (sum - whole.value()).abs() > bound {
        return Err(format!(
            "volume {sum} != {} (within {bound})",
            whole.value()
        ));
    }
    Ok(())
}

/// A cut, or `None` where faces that meet at a vertex in more than one
/// fan can't be a sheet (a face touching itself at a vertex): a typed
/// refusal, so that cut is passed over.
fn cut(made: Result<(Body, ryukei::name::Journal), Error>) -> Result<Option<Body>, String> {
    match made {
        Ok((s, _)) => Ok(Some(s)),
        Err(Error::InvalidSheet(SheetError::NonManifoldVertex { .. })) => Ok(None),
        Err(e) => Err(format!("cut: {e}")),
    }
}

/// The body cut both ways and sewn back.
fn round_trip(b: &Body, volume: Value) -> Result<(), String> {
    let mut each = Vec::new();
    for n in names(b) {
        match cut(extract(OpKey(10), b, &[n]))? {
            Some(s) => each.push(s),
            None => {
                each.clear();
                break;
            }
        }
    }
    if !each.is_empty() {
        sewn_back(b, volume, &each).map_err(|e| format!("each face: {e}"))?;
    }
    let all = names(b);
    if all.len() > 1 {
        let half = &all[..all.len() / 2];
        if let (Some(a), Some(r)) = (
            cut(extract(OpKey(10), b, half))?,
            cut(remove(OpKey(10), b, half))?,
        ) {
            sewn_back(b, volume, &[a, r]).map_err(|e| format!("halves: {e}"))?;
        }
    }
    Ok(())
}

/// Seeds of each kind, sized so the file takes about 6.5 minutes in
/// release on 18 threads (cox-worker-3: booleans 209 s, blends 186 s,
/// sweeps 378 s alone; 388 s side by side), a full.yml step of its own. At 200 each it took 33
/// minutes there and ran past 90 on cox-worker-4's Opteron: sweeps 100 to
/// 200 hold the slowest seeds (115 takes 4 minutes alone), blends 160 to
/// 200 a third of their time.
const SWEEPS: u64 = 100;
const BLENDS: u64 = 160;
const BOOLEANS: u64 = 200;

/// Runs `make` over seeds and fails listing those that didn't come back.
fn corpus(name: &str, seeds: u64, make: impl Fn(&[u8]) -> Option<Body>) {
    let mut tried = 0;
    let mut unsound: Vec<String> = Vec::new();
    let mut failed = Vec::new();
    for seed in 0..seeds {
        // An empty body (an intersection of things apart) has nothing to
        // cut.
        let Some(b) = make(&seeded(seed, 96)).filter(|b| b.faces().next().is_some()) else {
            continue;
        };
        if !meshes(&b) {
            unsound.push(format!("seed {seed}: doesn't mesh"));
            continue;
        }
        let volume = match volume(&b) {
            Ok(v) => v,
            Err(why) => {
                unsound.push(format!("seed {seed}: {why}"));
                continue;
            }
        };
        tried += 1;
        if let Err(why) = round_trip(&b, volume) {
            failed.push(format!("{name} seed {seed}: {why}"));
        }
    }
    println!(
        "{name}: {tried} tried, {} unsound {unsound:?}",
        unsound.len()
    );
    assert!(tried > 0, "{name}: no bodies");
    assert!(
        failed.is_empty(),
        "{tried} tried, {} unsound {unsound:?}\n{}",
        unsound.len(),
        failed.join("\n")
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn sweeps_sew_back() {
    corpus("sweep", SWEEPS, |d| sweep(d).map(|x| x.0));
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn booleans_sew_back() {
    corpus("boolean", BOOLEANS, |d| {
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
fn blends_sew_back() {
    corpus("blend", BLENDS, |d| {
        let case = blend_case(d)?;
        blend(OpKey(9), &case.body, &case.blends).ok().map(|x| x.0)
    });
}
