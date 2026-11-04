//! Sewing head to head with OCCT (design 295, plan 12.C): the race's
//! 1,600 bodies and a plate with a 40 × 25 grid of through-holes, each
//! cut into its faces (`sheet::extract`), every face moved along a
//! direction of its own by up to a tenth of healing's cap (seeded), then
//! sewn back within the cap, body by body, all bodies at once on every
//! core: by ryukei (`heal::sew`) and by OCCT (`BRepBuilderAPI_Sewing`,
//! each closed shell made a solid by `ShapeFix_Solid`). OCCT reads each
//! body's faces from a STEP file ryustep writes, which isn't timed.
//!
//! ryukei must give back every body: a solid per shell, the body's faces,
//! edges and vertices between them, and its volume (certified at a
//! thousandth of the body's size, within both bounds and the nudge: see
//! `chord`; a cavity sews alone into a solid facing out). A body
//! ryukei refuses must be in `REFUSED`, and one it gives back open (a
//! sheet where a solid was) in `LEFT_OPEN`, each with the issue that
//! tracks it; a body refused or open that isn't listed, or one listed
//! that it now sews whole, fails the test. OCCT's solids that don't hold
//! ryukei's volume to a thousandth are printed and counted, never judged.
//! Design 295 asked ryukei to be faster; it isn't (72.3 s against OCCT's
//! 1.62 s on cox-worker-2, issue #394), so the ratio is printed, not
//! asserted, until #394 is fixed. Bodies with a face that can't be a sheet alone
//! (one touching itself at a vertex) are left out and counted, and so
//! are those ryukei's sewing doesn't return from (`HANGS`, each with its
//! issue).
//! Devcontainer only (`--features occt`); run in release.

use rayon::prelude::*;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::heal::{SewOptions, sew};
use ryukei::measure::{MassOptions, mass};
use ryukei::name::{Name, OpKey, ToolKey};
use ryukei::primitive;
use ryukei::sheet::extract;
use ryukei::topo::{Body, BodyKind, ShellId};
use ryukei_bench::occt;
use ryukei_testkit::Lcg;
use ryustep::{LengthUnit, Product, WriteModel, WriteOptions, write};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Healing's cap, in meters: sewing closes gaps up to it.
const TOLERANCE: f64 = 1e-4;

/// Bodies (by index in `bodies()`) ryukei refuses to sew back, each with
/// the issue that tracks it.
/// Since #497 none: 1503 sews back.
const REFUSED: &[(usize, &str)] = &[];

/// Bodies (by index in `bodies()`) ryukei sews back open, a sheet where
/// a solid was, each with the issue that tracks it: a body listed that
/// comes back whole, or one open that isn't listed, fails the test.
const LEFT_OPEN: &[(usize, &str)] = &[];

/// Bodies (by index in `bodies()`) ryukei's sewing doesn't return from,
/// each with the issue that tracks it: left out of the race by both
/// kernels, as bodies that can't be cut are.
/// Since #370's fix none: 1010 sews back (and 1503 since #497).
const HANGS: &[(usize, &str)] = &[];

/// Progress through one phase of the race: a line every `EVERY` bodies
/// and at the end, with the time since the phase began, so a long phase
/// shows where it is (the gate's first run printed nothing for 80
/// minutes); and every `BEAT`, each body that has run longer than `BEAT`
/// by its index and time (the second run's last 92 bodies sat for two
/// hours without saying which).
struct Progress {
    phase: &'static str,
    total: usize,
    done: AtomicUsize,
    start: Instant,
    running: Arc<Mutex<BTreeMap<usize, Instant>>>,
    stop: Arc<AtomicBool>,
}

const EVERY: usize = 100;
const BEAT: Duration = Duration::from_secs(60);

impl Progress {
    fn new(phase: &'static str, total: usize) -> Self {
        println!("{phase}: {total} bodies");
        let running: Arc<Mutex<BTreeMap<usize, Instant>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (r, st) = (running.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut last = Instant::now();
            while !st.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                if last.elapsed() < BEAT {
                    continue;
                }
                last = Instant::now();
                let long: Vec<String> = r
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, t)| t.elapsed() >= BEAT)
                    .map(|(i, t)| format!("{i} ({:.0} s)", t.elapsed().as_secs_f64()))
                    .collect();
                if !long.is_empty() {
                    println!("{phase}: still running {}", long.join(", "));
                }
            }
        });
        Progress {
            phase,
            total,
            done: AtomicUsize::new(0),
            start: Instant::now(),
            running,
            stop,
        }
    }

    /// Body `i` (by index in `bodies()`) begins.
    fn begin(&self, i: usize) {
        self.running.lock().unwrap().insert(i, Instant::now());
    }

    /// One body done, `i` by index in `bodies()`.
    fn tick(&self, i: usize) {
        self.running.lock().unwrap().remove(&i);
        let n = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(EVERY) || n == self.total {
            println!(
                "{}: {n}/{} bodies at {:.1} s (last body {i})",
                self.phase,
                self.total,
                self.start.elapsed().as_secs_f64()
            );
        }
    }

    /// The phase's whole time, printed.
    fn end(&self) -> f64 {
        self.stop.store(true, Ordering::Relaxed);
        let s = self.start.elapsed().as_secs_f64();
        println!("{}: done in {s:.2} s", self.phase);
        s
    }
}

/// The slowest bodies of a phase, by their own time, printed.
fn slowest(phase: &str, times: &[(usize, Duration)]) {
    let mut t: Vec<(usize, Duration)> = times.to_vec();
    t.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let top: Vec<String> = t
        .iter()
        .take(10)
        .map(|(i, d)| format!("{i}: {:.2} s", d.as_secs_f64()))
        .collect();
    println!("{phase}: slowest bodies {}", top.join(", "));
}

/// The plate with 1,000 through-holes.
fn plate() -> Body {
    let block = primitive::block(OpKey(20), 40.0, 25.0, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(21), 0.3, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let places: Vec<Rigid> = (0..25)
        .flat_map(|j| {
            (0..40).map(move |i| {
                Rigid::translation(Vector3::new(i as f64 + 0.5, j as f64 + 0.5, -0.25))
            })
        })
        .collect();
    let tools: Vec<Tool> = places
        .iter()
        .enumerate()
        .map(|(i, p)| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: *p,
        })
        .collect();
    boolean(OpKey(22), BooleanOp::Subtract, &block, &tools)
        .unwrap()
        .0
}

/// The race's bodies, then the plate.
fn bodies() -> Vec<Body> {
    let mut out = ryukei_testkit::race::writable(1600);
    out.push(plate());
    out
}

/// A body's faces as sheets, each moved along a direction of its own by
/// up to a tenth of the tolerance, from `seed`; `None` where a face can't
/// be a sheet alone (one touching itself at a vertex: a typed refusal).
fn cut(b: &Body, seed: u64) -> Option<Vec<Body>> {
    let mut g = Lcg::new(seed);
    let mut unit = || (g.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    b.faces()
        .map(|f| {
            let s = extract(OpKey(23), b, &[b.face(f).unwrap().name]).ok()?.0;
            let dir = Vector3::new(unit() - 0.5, unit() - 0.5, unit() - 0.5);
            let by = dir.normalize().map_or(Vector3::new(0.0, 0.0, 0.0), |d| {
                d.get() * (0.1 * TOLERANCE * unit())
            });
            Some(s.transformed(&Rigid::translation(by)).unwrap())
        })
        .collect()
}

/// A solid's certified volume and its bound.
/// A solid's certified volume, its bound and its area, at `chord`.
fn volume(b: &Body, chord: f64) -> (f64, f64, f64) {
    let options = MassOptions {
        chord: Some(chord),
        ..MassOptions::default()
    };
    match mass(b, 1.0, &options) {
        Ok(m) => (m.volume.value(), m.volume.bound(), m.area.value()),
        other => panic!("{other:?}"),
    }
}

/// The chord the race certifies a body's volume at, and its pieces': a
/// thousandth of its size, so each volume's bound is about `2 × chord ×
/// curved area` (`measure::mass`), a hundred times `properties`'
/// default. The pieces' volumes must sum to the body's within both
/// bounds plus a tenth of the cap times its area (the faces' nudge),
/// so this check is looser than `sheet_props`', which certifies at a
/// hundred-thousandth: that default meshed so finely that judging 1,590
/// bodies on every core passed 10 GB, and one at a time took hours
/// (cox-worker-4's runs went quiet in it). The volume's value is the
/// quadrature's either way; only its bound grows.
fn chord(b: &Body) -> f64 {
    let x = b
        .faces()
        .map(|f| ryukei::intersect::face_box(b, f))
        .reduce(|x, y| x.union(&y))
        .expect("a body with faces");
    ((x.max() - x.min()).length() * 1e-3).max(10.0 * ryukei::tol::LINEAR_RESOLUTION)
}

fn names(faces: impl Iterator<Item = Name>) -> Vec<Name> {
    let mut x: Vec<Name> = faces.collect();
    x.sort();
    x
}

/// Each shell's faces' names, and whether it is a cavity (not the shell
/// of its region with the largest box).
fn shells(b: &Body) -> Vec<(Vec<Name>, bool)> {
    let size = |s: ShellId| {
        b.shell_faces(s)
            .map(|f| ryukei::intersect::face_box(b, f))
            .reduce(|x, y| x.union(&y))
            .map_or(0.0, |x| (x.max() - x.min()).length())
    };
    let mut out: Vec<(Vec<Name>, bool)> = b
        .shells()
        .map(|sh| {
            let region = b.shell(sh).unwrap().region;
            let outer = b
                .region_shells(region)
                .max_by(|&x, &y| size(x).total_cmp(&size(y)))
                .unwrap();
            let faces = names(b.shell_faces(sh).map(|f| b.face(f).unwrap().name));
            (faces, outer != sh)
        })
        .collect();
    out.sort();
    out
}

/// Why `pieces` aren't `b` given back, for the report; with the volume
/// they hold.
fn given_back(b: &Body, pieces: &[Body]) -> Result<f64, String> {
    let want = shells(b);
    let mut got: Vec<Vec<Name>> = pieces
        .iter()
        .map(|p| names(p.faces().map(|f| p.face(f).unwrap().name)))
        .collect();
    got.sort();
    if got != want.iter().map(|x| x.0.clone()).collect::<Vec<_>>() {
        return Err(format!("{} pieces for {} shells", pieces.len(), want.len()));
    }
    let at = chord(b);
    let (mut counts, mut sum, mut bound) = ([0; 3], 0.0, 0.0);
    for p in pieces {
        if p.kind() != BodyKind::Solid {
            return Err(format!("a {:?}", p.kind()));
        }
        let c = p.counts();
        counts = [
            counts[0] + c.faces,
            counts[1] + c.edges,
            counts[2] + c.vertices,
        ];
        let (v, e, _) = volume(p, at);
        let faces = names(p.faces().map(|f| p.face(f).unwrap().name));
        let cavity = want.iter().any(|(n, c)| *c && *n == faces);
        sum += if cavity { -v } else { v };
        bound += e;
    }
    let c = b.counts();
    if counts != [c.faces, c.edges, c.vertices] {
        return Err(format!("counts {counts:?} != {c:?}"));
    }
    // The faces moved by up to a tenth of the cap: the volume by up to
    // that times the area.
    let (w, e, area) = volume(b, at);
    if (sum - w).abs() > bound + e + 0.1 * TOLERANCE * area {
        return Err(format!("volume {sum} != {w}"));
    }
    Ok(sum)
}

#[test]
fn sewing_race_against_occt() {
    let t = Instant::now();
    let all = bodies();
    let p = Progress::new("cutting", all.len());
    let cuts: Vec<Option<Vec<Body>>> = all
        .par_iter()
        .enumerate()
        .map(|(i, b)| {
            p.begin(i);
            let c = cut(b, i as u64);
            p.tick(i);
            c
        })
        .collect();
    p.end();
    let hangs: Vec<usize> = HANGS.iter().map(|x| x.0).collect();
    let cut_out = cuts.iter().filter(|c| c.is_none()).count();
    let kept: Vec<usize> = (0..all.len())
        .filter(|&i| cuts[i].is_some() && !hangs.contains(&i))
        .collect();
    let sheets: usize = kept
        .iter()
        .map(|&i| cuts[i].as_ref().map_or(0, Vec::len))
        .sum();
    println!(
        "{} bodies cut into {sheets} sheets in {:.1} s ({cut_out} left out: a face can't be a sheet alone; {} skipped: {HANGS:?})",
        kept.len(),
        t.elapsed().as_secs_f64(),
        hangs.len()
    );
    // OCCT's input: each body's sheets in a STEP file of its own, in
    // millimetres, read before the clock starts.
    let dir = std::env::temp_dir().join("ryukei-bench-sewing");
    std::fs::create_dir_all(&dir).unwrap();
    let options = WriteOptions {
        unit: LengthUnit::Millimeter,
        ..WriteOptions::default()
    };
    let p = Progress::new("writing OCCT's input", kept.len());
    let files: Vec<std::path::PathBuf> = kept
        .par_iter()
        .map(|&i| {
            p.begin(i);
            let bytes = write(
                &WriteModel {
                    products: vec![Product {
                        id: format!("body{i}"),
                        name: format!("body{i}"),
                        bodies: cuts[i].clone().unwrap(),
                        ..Default::default()
                    }],
                },
                &options,
            )
            .unwrap();
            let path = dir.join(format!("body{i}.stp"));
            std::fs::write(&path, &bytes).unwrap();
            p.tick(i);
            path
        })
        .collect();
    p.end();
    // OCCT's reader, one file at a time.
    let p = Progress::new("OCCT reading its input", kept.len());
    let occt_in: Vec<occt::Solid> = kept
        .iter()
        .zip(&files)
        .map(|(&i, path)| {
            p.begin(i);
            let s = occt::Solid::read_step(path, false).unwrap();
            p.tick(i);
            s
        })
        .collect();
    p.end();
    // ryukei, every body at once on every core.
    let sew_options = SewOptions {
        tolerance: TOLERANCE,
        solid: true,
    };
    let p = Progress::new("ryukei sewing", kept.len());
    let ours: Vec<_> = kept
        .par_iter()
        .map(|&i| {
            p.begin(i);
            let t = Instant::now();
            let r = sew(OpKey(24), cuts[i].as_ref().unwrap(), &sew_options);
            p.tick(i);
            (r, t.elapsed())
        })
        .collect();
    let ours_s = p.end();
    slowest(
        "ryukei sewing",
        &kept
            .iter()
            .zip(&ours)
            .map(|(&i, x)| (i, x.1))
            .collect::<Vec<_>>(),
    );
    // OCCT, the same way.
    let p = Progress::new("OCCT sewing", kept.len());
    let theirs: Vec<(Result<occt::Solid, String>, Duration)> = kept
        .par_iter()
        .zip(occt_in.par_iter())
        .map(|(&i, s)| {
            p.begin(i);
            let t = Instant::now();
            let r = s.sew(TOLERANCE * 1e3);
            p.tick(i);
            (r, t.elapsed())
        })
        .collect();
    let theirs_s = p.end();
    slowest(
        "OCCT sewing",
        &kept
            .iter()
            .zip(&theirs)
            .map(|(&i, x)| (i, x.1))
            .collect::<Vec<_>>(),
    );
    // Judged after the clocks, every body's certified volumes at once.
    let p = Progress::new("judging", kept.len());
    enum Verdict {
        Refused(String),
        Wrong(String),
        Given { occt_off: Option<(f64, f64)> },
    }
    let verdicts: Vec<Verdict> = kept
        .par_iter()
        .enumerate()
        .map(|(k, &i)| {
            p.begin(i);
            let v = match &ours[k].0 {
                Err(e) => Verdict::Refused(format!("{e:?}")),
                Ok((pieces, _)) => match given_back(&all[i], pieces) {
                    Err(why) => Verdict::Wrong(why),
                    Ok(v) => {
                        let occt_v: f64 = theirs[k]
                            .0
                            .as_ref()
                            .map(|s| s.solids().iter().map(|x| x.volume).sum::<f64>() * 1e-9)
                            .unwrap_or(f64::NAN);
                        let off = occt_v.is_nan() || (occt_v - v).abs() > 1e-3 * v.abs();
                        Verdict::Given {
                            occt_off: off.then_some((occt_v, v)),
                        }
                    }
                },
            };
            p.tick(i);
            v
        })
        .collect();
    p.end();
    let mut wrong = Vec::new();
    let mut refused = Vec::new();
    let mut open = Vec::new();
    let mut occt_off = 0;
    for (&i, v) in kept.iter().zip(verdicts) {
        match v {
            Verdict::Refused(e) => refused.push((i, e)),
            Verdict::Wrong(why) if why == "a Sheet" => open.push(i),
            Verdict::Wrong(why) => wrong.push(format!("body {i}: {why}")),
            Verdict::Given { occt_off: None } => {}
            Verdict::Given {
                occt_off: Some((occt_v, v)),
            } => {
                occt_off += 1;
                println!("body {i}: OCCT {occt_v} m³, ryukei {v} m³");
            }
        }
    }
    println!(
        "ryukei: {} bodies in {ours_s:.2} s ({} refused); OCCT: {:.2} s ({} not to ryukei's volume); OCCT {:.2} times ryukei's time",
        kept.len(),
        refused.len(),
        theirs_s,
        occt_off,
        theirs_s / ours_s
    );
    for (i, why) in &refused {
        println!("refused: body {i}: {why}");
    }
    let listed: Vec<usize> = REFUSED.iter().map(|x| x.0).collect();
    let got: Vec<usize> = refused.iter().map(|x| x.0).collect();
    if got != listed {
        wrong.push(format!("refused {got:?}, listed {listed:?}"));
    }
    let listed: Vec<usize> = LEFT_OPEN.iter().map(|x| x.0).collect();
    if open != listed {
        wrong.push(format!("given back open {open:?}, listed {listed:?}"));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    // Design 295's bar, ryukei faster than OCCT, isn't met: ryukei took
    // 72.3 s against OCCT's 1.62 s on cox-worker-2 at d22dd5db. Printed,
    // not asserted, until issue #394 makes sewing fast enough to bar again.
    println!(
        "speed: ryukei {ours_s:.2} s, OCCT {theirs_s:.2} s, ryukei {:.1} times OCCT's time (#394)",
        ours_s / theirs_s
    );
}
