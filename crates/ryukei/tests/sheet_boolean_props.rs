//! Booleans on sheets against seeded solids (design 295, plan 12.D):
//! seeded primitives, sweeps and booleans' results, as the fuzz targets
//! make them from bytes ([`ryukei_testkit::blend_bytes::body`]), each met
//! by a plane sheet through its box at a seeded tilt and height, wide
//! enough to cross the whole body:
//!
//! - trimmed: `area(S ∩ A) + area(S − A) = area(S)`;
//! - divided: the pieces' volumes add up to the body's, and their areas
//!   to the body's plus twice the sheet's piece inside it (`S ∩ A`), each
//!   piece a sound solid.
//!
//! Each value is certified and judged within the sum of its bounds. A
//! typed refusal (`Uncertain`, `Unsupported`, `NonManifold`) is counted,
//! never a failure; a body that doesn't mesh, or whose volume can't be
//! certified, is its maker's fault and counted apart. Release only:
//! minutes in a debug build.

use ryukei::boolean::{BooleanOp, Divided, Tool, ToolKey, boolean, divide};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Frame, Plane, Rigid, Surface, Vector3};
use ryukei::measure::{Entity, Properties, Value, properties};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::sheet::from_surface;
use ryukei::topo::{Body, BodyKind};
use ryukei::{BooleanError, Error};
use ryukei_testkit::Lcg;
use ryukei_testkit::blend_bytes::body;

/// Seeds tried: about five minutes in release. At 69adbd3e on
/// cox-worker-2 (AMD Ryzen 7 9700X, 8 cores, shared, load 4 to 6) 90 were
/// judged, none refused and none unsound, in 325 s.
const SEEDS: u64 = 120;

fn seeded(seed: u64, n: usize) -> Vec<u8> {
    let mut g = Lcg::new(seed);
    (0..n).map(|_| (g.next_u64() >> 56) as u8).collect()
}

fn meshes(b: &Body) -> bool {
    tessellate(b, &MeshParams::new(1e-2, 0.5))
        .is_ok_and(|m| m.faults.is_empty() && check_mesh(b, &m).is_ok())
}

/// A body's certified volume (solid) and area.
fn measured(b: &Body) -> Result<(Option<Value>, Value), String> {
    match properties(b, Entity::Body) {
        Ok(Properties::Body { volume, area }) => Ok((Some(volume), area)),
        Ok(Properties::Sheet { area }) => Ok((None, area)),
        other => Err(format!("properties: {other:?}")),
    }
}

/// `values` add up to `expected` within their bounds and `slack`.
fn adds_up(values: &[Value], expected: Value, slack: f64) -> Result<(), String> {
    let sum: f64 = values.iter().map(|v| v.value()).sum();
    let bound: f64 = values.iter().map(|v| v.bound()).sum::<f64>() + expected.bound() + slack;
    if (sum - expected.value()).abs() <= bound {
        Ok(())
    } else {
        Err(format!(
            "{sum} against {} (bound {bound})",
            expected.value()
        ))
    }
}

/// The seeded plane sheet across `b`: through its box's middle moved by
/// up to a third of its size along a seeded normal, a square twice its
/// size each way.
fn across(b: &Body, g: &mut Lcg) -> Body {
    let x = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .expect("faces");
    let size = (x.max() - x.min()).length();
    let mut r = || (g.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
    let n = Vector3::new(r(), r(), r() + 1.5);
    let mid = x.min() + (x.max() - x.min()) * 0.5;
    let origin = mid + n.normalize().unwrap().get() * (r() * size / 3.0);
    let frame = Frame::new(origin, n, Vector3::new(1.0, 0.3, 0.1)).unwrap();
    let h = size;
    from_surface(
        OpKey(31),
        &Surface::Plane(Plane::new(frame)),
        [[-h, h], [-h, h]],
    )
    .unwrap()
    .0
}

fn tool(body: &Body) -> Tool<'_> {
    Tool {
        key: ToolKey(32),
        body,
        place: Rigid::IDENTITY,
    }
}

/// Whether a refusal is one this property counts rather than fails on.
fn counted(e: &Error) -> bool {
    matches!(
        e,
        Error::InvalidBoolean(
            BooleanError::Uncertain { .. }
                | BooleanError::Unsupported { .. }
                | BooleanError::NonManifold { .. }
        )
    )
}

fn sound(b: &Body) -> Result<(), String> {
    let faults = check(b);
    if !faults.is_empty() {
        return Err(format!("check: {faults:?}"));
    }
    let crossing = check_intersections(b);
    if !crossing.is_empty() {
        return Err(format!("check_intersections: {crossing:?}"));
    }
    if !meshes(b) {
        return Err("doesn't mesh".into());
    }
    Ok(())
}

/// One seed: `Ok(true)` judged, `Ok(false)` counted refusal.
fn judge(b: &Body, sheet: &Body) -> Result<bool, String> {
    let (Some(whole_v), whole_a) = measured(b)? else {
        return Err("not a solid".into());
    };
    let (_, sheet_a) = measured(sheet)?;
    let trim = |op| boolean(OpKey(33), op, sheet, &[tool(b)]);
    let (inside, outside) = match (trim(BooleanOp::Intersect), trim(BooleanOp::Subtract)) {
        (Ok((i, _)), Ok((o, _))) => (i, o),
        (Err(e), _) | (_, Err(e)) if counted(&e) => return Ok(false),
        (Err(e), _) | (_, Err(e)) => return Err(format!("trim: {e:?}")),
    };
    let mut areas = Vec::new();
    for s in [&inside, &outside] {
        if s.faces().next().is_some() {
            sound(s).map_err(|e| format!("trimmed: {e}"))?;
            areas.push(measured(s)?.1);
        }
    }
    adds_up(&areas, sheet_a, 1e-9).map_err(|e| format!("trimmed areas: {e}"))?;
    let inner = if inside.faces().next().is_some() {
        measured(&inside)?.1
    } else {
        Value::Exact(0.0)
    };
    let d: Divided = match divide(OpKey(34), b, &[tool(sheet)]) {
        Ok((d, _)) => d,
        Err(e) if counted(&e) => return Ok(false),
        Err(e) => return Err(format!("divide: {e:?}")),
    };
    let (mut vs, mut as_) = (Vec::new(), Vec::new());
    for piece in d.front.iter().chain(&d.back).chain(&d.apart) {
        if piece.kind() != BodyKind::Solid {
            return Err("a piece that isn't a solid".into());
        }
        sound(piece).map_err(|e| format!("piece: {e}"))?;
        let (v, a) = measured(piece)?;
        vs.push(v.expect("a solid"));
        as_.push(a);
    }
    adds_up(&vs, whole_v, 1e-9).map_err(|e| format!("volumes: {e}"))?;
    let twice = Value::Bounded {
        value: whole_a.value() + 2.0 * inner.value(),
        bound: whole_a.bound() + 2.0 * inner.bound(),
    };
    adds_up(&as_, twice, 1e-9).map_err(|e| format!("areas: {e}"))?;
    Ok(true)
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn sheets_trim_and_divide_seeded_solids() {
    let (mut judged, mut refused, mut unsound) = (0, 0, Vec::new());
    let mut failed = Vec::new();
    for seed in 0..SEEDS {
        let Some((b, _)) = body(&seeded(seed, 96)) else {
            continue;
        };
        if b.faces().next().is_none() || b.kind() != BodyKind::Solid {
            continue;
        }
        if !meshes(&b) || measured(&b).is_err() {
            unsound.push(seed);
            continue;
        }
        let sheet = across(&b, &mut Lcg::new(seed ^ 0x5d1e));
        match judge(&b, &sheet) {
            Ok(true) => judged += 1,
            Ok(false) => refused += 1,
            Err(why) => failed.push(format!("seed {seed}: {why}")),
        }
    }
    println!("{judged} judged, {refused} refused, unsound {unsound:?}");
    assert!(judged > SEEDS as usize / 2, "{judged} judged of {SEEDS}");
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Writes the seeds in `RYUKEI_DUMP_SEEDS` (comma-separated) as
/// `<seed>-body.bin` and `<seed>-sheet.bin` (`codec::encode_body`) into
/// `RYUKEI_DUMP_DIR`, for reproducing a failure elsewhere.
#[test]
#[ignore = "a tool for reproducing seeds: run by name"]
fn dump_seeds() {
    let dir = std::path::PathBuf::from(std::env::var("RYUKEI_DUMP_DIR").expect("RYUKEI_DUMP_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    for s in std::env::var("RYUKEI_DUMP_SEEDS")
        .expect("RYUKEI_DUMP_SEEDS")
        .split(',')
    {
        let seed: u64 = s.trim().parse().unwrap();
        let (b, _) = body(&seeded(seed, 96)).expect("a body");
        let sheet = across(&b, &mut Lcg::new(seed ^ 0x5d1e));
        for (what, x) in [("body", &b), ("sheet", &sheet)] {
            std::fs::write(
                dir.join(format!("{seed}-{what}.bin")),
                ryukei::codec::encode_body(x),
            )
            .unwrap();
        }
    }
}
