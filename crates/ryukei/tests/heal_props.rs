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
/// many faces lie on surfaces a file holds only as NURBS forms (pipes,
/// and bent walls in a refolded bend).
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
            if s.starts_with("pipe ") || s.starts_with("bent ") {
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

/// Why a round trip failed, for the report. Healing the body written
/// again must give the same bits when `fixed`.
fn round_trip(b: &Body, fixed: Fixed) -> Result<(), String> {
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
    if fixed == Fixed::No {
        return Ok(());
    }
    let s2 = seamed(&h, 1e-7).map_err(|e| format!("seamed again: {e}"))?;
    let again = heal(OpKey(9), &s2, &HealOptions::default())
        .map_err(|e| format!("heal again: {e}"))?
        .0;
    if again.dump() == h.dump() {
        return Ok(());
    }
    if fixed == Fixed::Bits {
        return Err("not a fixed point".into());
    }
    within_first(&h, &again)
}

/// How a round trip's second healing must match its first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fixed {
    /// Not judged.
    No,
    /// The same bits.
    Bits,
    /// The same topology, names and geometry, every edge's and vertex's
    /// tolerance within the first healing's (issue #418): the first
    /// healing keeps the certified bound a bent wall's NURBS form was
    /// written with, which the second, on exact NURBS faces, measures
    /// down towards the real gap. The first is the conservative one, so
    /// the second may only tighten it.
    WithinFirst,
}

/// [`Fixed::WithinFirst`]'s judgement of `again` against `h`.
fn within_first(h: &Body, again: &Body) -> Result<(), String> {
    use std::collections::BTreeMap;
    if again.counts() != h.counts() || geometry(again).0 != geometry(h).0 {
        return Err("healed again: topology or geometry differs".into());
    }
    let tols = |b: &Body| -> BTreeMap<String, f64> {
        let mut m = BTreeMap::new();
        for e in b.edges() {
            let e = b.edge(e).unwrap();
            m.insert(format!("edge {:?}", e.name), e.tolerance.get());
        }
        for v in b.vertices() {
            let v = b.vertex(v).unwrap();
            m.insert(format!("vertex {:?}", v.name), v.tolerance.get());
        }
        m
    };
    let (first, second) = (tols(h), tols(again));
    if first.keys().ne(second.keys()) {
        return Err("healed again: names differ".into());
    }
    for (k, t) in &second {
        if *t > first[k] {
            return Err(format!("healed again: {k} grew from {} to {t}", first[k]));
        }
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

/// Seeds that mesh but don't heal back, each with the issue that says
/// why. One that comes back fails, so its line goes. Blend seed 267
/// (#371) came off when the mesh estimate stopped refusing its ruled
/// chamfer for shell facing.
const KNOWN: &[(&str, u64, &str)] = &[];

/// Runs `make` over seeds, round trips what it gives, and fails listing
/// the seeds that didn't come back (unless the body made doesn't mesh).
fn corpus(name: &str, seeds: u64, make: impl Fn(&[u8]) -> Option<Body>) {
    corpus_to(name, seeds, Fixed::Bits, make);
}

/// [`corpus`], with [`round_trip`]'s `fixed`.
fn corpus_to(name: &str, seeds: u64, fixed: Fixed, make: impl Fn(&[u8]) -> Option<Body>) {
    let mut tried = 0;
    let mut unsound = 0;
    let mut failed = Vec::new();
    for seed in 0..seeds {
        let Some(b) = make(&seeded(seed, 96)) else {
            continue;
        };
        tried += 1;
        let known = KNOWN.iter().find(|k| k.0 == name && k.1 == seed);
        let r = round_trip(&b, fixed);
        if let (Some((_, _, issue)), Ok(())) = (known, &r) {
            failed.push(format!(
                "{name} seed {seed}: comes back now; take it off KNOWN ({issue})"
            ));
        }
        if let Err(why) = r {
            if known.is_some() {
                continue;
            }
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

/// Seeded sheet-metal parts ([`ryukei_testkit::sheet_metal_bytes`]),
/// flanged and drilled, and refolded from flats cut across their bend
/// lines (their walls in a bend bent surfaces, written as NURBS forms).
/// A refolded part heals to its faces, edges and vertices, and healing
/// it written again gives the same topology and geometry with every
/// tolerance within the first's: the first healing keeps the bounds its
/// fitted edges and bent walls' NURBS forms were written with, the
/// second measures them down towards resolution (issue #418, judged
/// with [`Fixed::WithinFirst`]).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn sheet_metal_round_trip() {
    use ryukei_testkit::sheet_metal_bytes::{case, trip};
    corpus("sheet metal", 48, |d| case(d)?.ok().map(|c| c.folded));
    corpus_to("refolded", 48, Fixed::WithinFirst, |d| {
        let c = case(d)?.ok()?;
        trip(&c).ok().map(|r| r.refolded)
    });
}

/// Shapes with a file's faults ([`ryukei_testkit::heal_bytes`]): those
/// healing must undo come back as the body's faces, edges and vertices
/// and pass the checker; the rest are refused by name or heal to a body
/// that checks.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn faults_heal_or_are_refused() {
    use ryukei::Error;
    use ryukei_testkit::heal_bytes::heal_case;
    let mut tried = 0;
    let mut mild = 0;
    let mut failed = Vec::new();
    for seed in 0..400 {
        let Some(case) = heal_case(&seeded(seed, 128)) else {
            continue;
        };
        if !meshes(&case.body) {
            continue;
        }
        tried += 1;
        mild += usize::from(case.mild);
        match heal(OpKey(9), &case.shape, &HealOptions::default()) {
            Ok((h, _, _)) => {
                let faults = check(&h);
                if !faults.is_empty() {
                    failed.push(format!("seed {seed}: check {faults:?}"));
                } else if case.mild && h.counts() != case.body.counts() {
                    failed.push(format!(
                        "seed {seed}: counts {:?} != {:?}",
                        h.counts(),
                        case.body.counts()
                    ));
                }
            }
            Err(Error::InvalidHeal(e)) if case.mild => {
                failed.push(format!(
                    "seed {seed}: mild refused {e:?} after {:?}",
                    case.faults
                ));
            }
            Err(Error::InvalidHeal(_)) => {}
            Err(e) => failed.push(format!("seed {seed}: {e:?}")),
        }
    }
    assert!(tried > 100 && mild > 50, "{tried} tried, {mild} mild");
    assert!(
        failed.is_empty(),
        "{tried} tried, {mild} mild, {} failed\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// A plate's bore and faces in NURBS form, written with its rim moved
/// 2 µm off both: healing puts the rim back on the spline faces' meeting
/// (a closed branch the general intersector traces, started at the rim's
/// vertex), with the branch's own pcurves, so its tolerance falls back to
/// the branch's.
#[test]
fn spline_edges_are_tightened() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey};
    use ryukei::geom::{Rigid, Vector3};
    use ryukei::primitive;
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &rod,
        place: Rigid::translation(Vector3::new(2.0, 2.0, -1.0)),
    };
    let mut holed = boolean(OpKey(3), BooleanOp::Subtract, &plate, &[tool])
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut holed).unwrap();
    let mut s = seamed(&holed, 1e-6).unwrap();
    let mut moved = 0;
    for e in 0..s.edges.len() {
        let c = &s.curves[s.edges[e].curve];
        let [t0, t1] = s.edges[e].interval;
        let p = c.point((t0 + t1) / 2.0);
        let r = ryukei::math::hypot(p.x - 2.0, p.y - 2.0);
        if (r - 0.5).abs() < 1e-6 && (p.z - 1.0).abs() < 1e-6 {
            let shifted = c.transformed(&Rigid::translation(Vector3::new(2e-6, 1e-6, 0.0)));
            s.curves.push(shifted);
            s.edges[e].curve = s.curves.len() - 1;
            moved += 1;
        }
    }
    assert_eq!(moved, 1);
    let (h, _, report) = heal(OpKey(7), &s, &HealOptions::default()).unwrap();
    assert_eq!(report.tightened.len(), 1);
    assert_eq!(check(&h), []);
    let worst = h
        .edges()
        .map(|e| h.edge(e).unwrap().tolerance.get())
        .fold(0.0, f64::max);
    assert!(worst <= 1e-7, "{worst}");
}

/// Whether edge `a` of shape `s` runs along edge `b`: its interior
/// samples on `b`, inside `b`'s interval.
fn along(s: &ryukei::heal::Shape, a: usize, b: usize, size: f64) -> bool {
    let (ea, eb) = (&s.edges[a], &s.edges[b]);
    let (ca, cb) = (&s.curves[ea.curve], &s.curves[eb.curve]);
    let [t0, t1] = ea.interval;
    let [u0, u1] = eb.interval;
    let on = (1..8)
        .filter(|&i| {
            let p = ca.point(t0 + (t1 - t0) * f64::from(i) / 8.0);
            let q = cb.project(p);
            let mut t = q.param;
            if let Some(per) = cb.period() {
                t += ((u0 - t) / per).ceil() * per;
            }
            let slack = 1e-9 * (u1 - u0).abs().max(1.0);
            q.distance <= 1e-7 * size && t > u0 + slack && t < u1 - slack
        })
        .count();
    on >= 2
}

/// No two edges of one face in a written shape run along each other: a
/// seam laid on a parameter line an edge of a winding loop already runs
/// on (a cone cut through its axis by a block's face) overlapped that
/// edge, and SMLib, building the face's trim from its edges, took more
/// than the face (race body 16, design 193's plan 8.B).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn seams_never_run_along_an_edge() {
    use ryukei::heal::Loop;
    let mut wrong = Vec::new();
    for (i, b) in ryukei_testkit::race::bodies(24).iter().enumerate() {
        let Ok(s) = seamed(b, 1e-7) else { continue };
        let size = s
            .vertices
            .iter()
            .map(|v| v.point.distance(s.vertices[0].point))
            .fold(1.0, f64::max);
        for (k, f) in s.faces.iter().enumerate() {
            let mut edges: Vec<usize> = f
                .loops
                .iter()
                .flat_map(|l| match l {
                    Loop::Edges(u) => u.iter().map(|x| x.edge).collect(),
                    Loop::Vertex(_) => Vec::new(),
                })
                .collect();
            edges.sort_unstable();
            edges.dedup();
            for &a in &edges {
                for &b in &edges {
                    if a != b && along(&s, a, b, size) {
                        wrong.push(format!("body {i}, face {k}: edge {a} along edge {b}"));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A block with three holes, written for a file and healed under 4,000
/// keys (issue #234: healing a part depended on its import key). The
/// key orders the build, and some orders left a face's four loops two on
/// one face and two on another, which KFMRH, taking a face of one loop,
/// couldn't join.
#[test]
fn every_key_heals_a_face_of_many_loops() {
    use rayon::prelude::*;
    use ryukei::boolean::{BooleanOp, Tool, ToolKey};
    use ryukei::geom::{Rigid, Vector3};
    let block = ryukei::primitive::block(OpKey(1), 4.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = ryukei::primitive::cylinder(OpKey(2), 0.2, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<Tool> = (0..3)
        .map(|i| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: Rigid::translation(Vector3::new(0.5 + i as f64, 0.5, -0.5)),
        })
        .collect();
    let (b, _) = boolean(OpKey(3), BooleanOp::Subtract, &block, &tools).unwrap();
    let shape = seamed(&b, 1e-7).unwrap();
    let failed: Vec<u128> = (0..4000u128)
        .into_par_iter()
        .filter(|&k| match heal(OpKey(k), &shape, &HealOptions::default()) {
            Ok((h, _, _)) => h.counts() != b.counts(),
            Err(_) => true,
        })
        .collect();
    assert!(
        failed.is_empty(),
        "{} keys failed: {:?}",
        failed.len(),
        &failed[..failed.len().min(10)]
    );
}

/// Every edge of a written shape starts and ends at its vertices, within
/// their tolerances, and no edge of a face runs along another: race bodies
/// (issue #469) whose seams `seamed` cut from the wrong place. A loop of
/// one closed edge had its crossing bracketed from its last sample to its
/// own start, so the bisection found the far side's jump of a period (an
/// oblique extrusion's seam ended half a turn round); a crossing whose
/// next vertex lay on the line's other side by its own curve but not by
/// the edge before was never bisected (a cone's seam ended 0.11 rad off);
/// and a hole whose edge ran along a candidate line wasn't kept clear of
/// it (a torus's seam lay along a hole's edge, which then fell outside the
/// band).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn seams_start_and_end_at_their_vertices() {
    use ryukei::heal::Loop;
    use ryukei_testkit::race::{blend_body, boolean_body};
    // Race bodies 133, 196, 220, 365, 392, 479, 498, 587, 819, 1054, 1314
    // and 1434 (blends) and 529, 1003, 1372 and 488 (booleans).
    let blends = [75, 110, 122, 199, 215, 266, 276, 325, 455, 584, 728, 796];
    let booleans = [292, 559, 763, 270];
    let bodies: Vec<(String, Body)> = blends
        .iter()
        .map(|&k| (format!("blend {k}"), blend_body(k).expect("it builds")))
        .chain(
            booleans
                .iter()
                .map(|&k| (format!("boolean {k}"), boolean_body(k).expect("it builds"))),
        )
        .collect();
    let mut wrong = Vec::new();
    for (what, b) in &bodies {
        let s = seamed(b, 1e-7).unwrap_or_else(|e| panic!("{what}: {e}"));
        let size = s
            .vertices
            .iter()
            .map(|v| v.point.distance(s.vertices[0].point))
            .fold(1.0, f64::max);
        for (k, e) in s.edges.iter().enumerate() {
            let c = &s.curves[e.curve];
            for (t, v) in [(e.interval[0], e.start), (e.interval[1], e.end)] {
                let off = c.point(t).distance(s.vertices[v].point);
                let reach = e.tolerance + s.vertices[v].tolerance + 1e-8;
                if off > reach {
                    wrong.push(format!("{what}: edge {k} {off} off vertex {v}"));
                }
            }
        }
        for (k, f) in s.faces.iter().enumerate() {
            let mut edges: Vec<usize> = f
                .loops
                .iter()
                .flat_map(|l| match l {
                    Loop::Edges(u) => u.iter().map(|x| x.edge).collect(),
                    Loop::Vertex(_) => Vec::new(),
                })
                .collect();
            edges.sort_unstable();
            edges.dedup();
            for &a in &edges {
                for &b in &edges {
                    if a != b && along(&s, a, b, size) {
                        wrong.push(format!("{what}, face {k}: edge {a} along edge {b}"));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
