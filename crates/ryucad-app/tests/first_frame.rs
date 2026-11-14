//! Design 192's drawing bars, over its benchmark project (10,000 parts,
//! 100,000 occurrences; `ryucad::bench::scale`): the window's first full
//! frame over a warm disk cache within 20 s, then orbiting and zooming at
//! 60 frames a second with no frame over 50 ms. The project is generated,
//! a first window builds it and fetches every mesh (warming the cache),
//! its daemon stops, and a second window is timed from opening to the
//! first frame drawing every occurrence: the root's report, every body's
//! coarse mesh (`lod::COARSE_FIRST`), the picker, levels chosen, culled
//! and kept within `display.gpu_memory`, every body uploaded, drawn and
//! read back. Then frames orbit and close in as `tests/frames.rs` does,
//! and then stop where most of what is drawn is several pixels across
//! (#237): a module framed, a subassembly's own parts framed (every pin
//! drawn with its edges), and a low flight along a module's
//! subassemblies. Every frame counts towards the bar.
//!
//! Ignored: it means something only on hardware, and takes many minutes
//! (the first build of the project is a cold rebuild of 10,000 parts).
//! Without an adapter it times everything up to drawing and stops there;
//! `RYUCAD_BARS=1` asserts the bars (and refuses a software adapter).
//! `RYUCAD_SCALE_PARTS` makes a smaller project of the same shape.
//! `RYUCAD_SCALE_KEEP=<folder>` keeps the project and its disk cache
//! there for the next run, which then skips the cold rebuild (#237).
//! `RYUCAD_FRAME_PNG=<file>` writes the first full frame there.
//!
//! ```text
//! cargo test -p ryucad-app --release --test first_frame -- --ignored --nocapture
//! ```

mod common;

use common::{gpu, ryucad};
use ryucad::bench::scale::{Spec, race_file, write};
use ryucad::view::{self, StandardView};
use ryucad_app::core::lod;
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use ryucad_app::ui::scene::{self, DrawList, Scene, theme};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const W: u32 = 1280;
const H: u32 = 800;
const FRAMES: usize = 240;
const WAIT: Duration = Duration::from_secs(3600);

/// Frames turning about each of [`stops`].
const STOP_FRAMES: usize = 120;

/// What the stops frame, by the occurrence's path in the root: a module,
/// and a subassembly's own parts (its race bodies, metres off, left out),
/// whose pins are then tens of pixels across.
#[allow(clippy::type_complexity)]
fn stops(ws: &Workspace) -> Vec<(&'static str, bool, Box<dyn Fn(&str) -> bool>)> {
    let first = ws
        .part("root")
        .and_then(|r| r.solids.first().map(|s| s.id.clone()))
        .expect("the root places something");
    let mut steps = first.split('/');
    let (m, s) = (
        steps.next().unwrap().to_owned(),
        steps.next().unwrap().to_owned(),
    );
    let module = format!("{m}/");
    let own = format!("{m}/");
    let sub = format!("{m}/{s}/");
    // A subassembly's own parts: all but its race bodies, metres off.
    let own_part = |rest: &str| {
        rest.split_once('/')
            .is_some_and(|(_, inner)| !inner.starts_with('i'))
    };
    vec![
        (
            "a module",
            false,
            Box::new(move |k: &str| k.starts_with(&module)) as Box<dyn Fn(&str) -> bool>,
        ),
        (
            "a subassembly's own parts",
            false,
            Box::new(move |k: &str| {
                k.strip_prefix(&sub)
                    .is_some_and(|rest| !rest.starts_with('i'))
            }),
        ),
        (
            "a module's subassemblies, flying low along them",
            true,
            Box::new(move |k: &str| k.strip_prefix(&own).is_some_and(own_part)),
        ),
    ]
}

fn spec() -> Spec {
    match std::env::var("RYUCAD_SCALE_PARTS") {
        Ok(n) => Spec::parts(n.parse().expect("a number of parts")),
        Err(_) => Spec::full(),
    }
}

/// The race file, kept in the target folder after the first run.
fn race(n: usize) -> Vec<u8> {
    let kept = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("scale-race-{n}.step"));
    if let Ok(b) = std::fs::read(&kept) {
        return b;
    }
    let b = race_file(n);
    std::fs::write(&kept, &b).unwrap();
    b
}

/// Opens a window on `root` and waits until every occurrence of the root
/// has a mesh. Returns it, the seconds until the root's report and until
/// every mesh, and how many solids' occurrences it draws.
fn open(root: &Path) -> (Workspace, f64, f64, usize) {
    let t = Instant::now();
    let mut ws = common::open_editing(root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(WAIT, |ws| {
        ws.part("root").is_some_and(|r| !r.solids.is_empty())
    }));
    let report = t.elapsed().as_secs_f64();
    let all = |ws: &Workspace| {
        ws.part("root").is_some_and(|r| {
            !r.solids.is_empty()
                && r.solids.iter().all(|s| {
                    ws.model
                        .meshes
                        .contains_key(&("root".to_owned(), s.id.clone()))
                })
        })
    };
    assert!(ws.wait_until(WAIT, |ws| ws.idle() && all(ws)));
    let n = ws.part("root").unwrap().solids.len();
    (ws, report, t.elapsed().as_secs_f64(), n)
}

#[test]
#[ignore = "design 192's drawing bars: run with --ignored on hardware"]
fn the_first_full_frame_of_100000_occurrences() {
    let strict = std::env::var_os("RYUCAD_BARS").is_some();
    let dir = tempfile::tempdir().unwrap();
    let keep = std::env::var_os("RYUCAD_SCALE_KEEP").map(std::path::PathBuf::from);
    // The shared folder is set up (and its cache emptied) first, so a kept
    // cache set after it stands.
    ryucad();
    let root = match &keep {
        Some(k) => {
            std::fs::create_dir_all(k).unwrap();
            // SAFETY: before any daemon starts; this test runs alone.
            unsafe { std::env::set_var("RYUCAD_CACHE_DIR", k.join("cache")) };
            k.join("p")
        }
        None => dir.path().join("p"),
    };
    let spec = spec();
    if root.join("ryucad.json").exists() {
        println!("the kept project at {}", root.display());
    } else {
        let made = write(&root, &spec, &race(spec.imports()));
        println!(
            "{} parts, {} occurrences, {} {}",
            made.parts,
            made.occurrences,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    // Warm the disk cache, then a new daemon over it.
    let (ws, report, cold, _) = open(&root);
    println!(
        "a window over an empty cache: the root's report in {report:.1} s, every mesh in \
         {cold:.1} s"
    );
    drop(ws);
    ryucad::client::stop(&root).unwrap();
    let t = Instant::now();
    let (mut ws, report, meshed, n) = open(&root);
    let base = ws.model.base;
    let coarse = ws
        .model
        .meshes
        .values()
        .filter(|m| lod::level_of(m, base) == ryucad::mesh::Quality::Coarse)
        .count();
    println!(
        "over the warm cache: the root's report in {report:.1} s, {n} occurrences' meshes in \
         {meshed:.1} s ({coarse} of {} coarse, {:.0} MB fetched)",
        ws.model.meshes.len(),
        ws.model.received as f64 / 1e6
    );
    // What the view does before it draws: its picker, then levels chosen
    // and kept within the GPU budget.
    let picked = Instant::now();
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    println!(
        "the picker over every occurrence: {:.2} s",
        picked.elapsed().as_secs_f64()
    );
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) * (hi[k] - lo[k]))
        .sum::<f64>()
        .sqrt()
        / 2.0;
    let start = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let budget = 2u64 << 30;
    let chose = Instant::now();
    let pr = Projector::new(&start, f64::from(W) / f64::from(H), centre, radius);
    let (drawn, wanted) = lod::choose(&ws.model, &picker, &pr, f64::from(H));
    let (drawn, wanted) = lod::within(&ws.model, &pr, drawn, wanted, budget);
    let held: std::collections::BTreeMap<(&str, _), u64> = drawn
        .iter()
        .map(|d| ((d.mesh.hash.as_str(), d.level), lod::gpu_bytes(&d.mesh)))
        .collect();
    println!(
        "levels chosen and kept within the budget: {:.0} ms, {} drawn, {} levels wanted, {:.0} MB \
         on the GPU",
        chose.elapsed().as_secs_f64() * 1e3,
        drawn.len(),
        wanted.len(),
        held.values().sum::<u64>() as f64 / 1e6
    );
    let Some((adapter, device, queue)) = gpu() else {
        println!(
            "no adapter: up to drawing in {:.1} s",
            t.elapsed().as_secs_f64()
        );
        // What a frame costs before the GPU: levels chosen and kept within
        // the budget while the camera orbits and closes in.
        let mut times: Vec<f64> = (0..40)
            .map(|f| {
                let t = f as f64 / 40.0;
                let turned = view::orbit(&start, centre, t * core::f64::consts::TAU, 0.0);
                let camera = view::zoom(&turned, 1.0 - 0.9 * t, Some(centre));
                let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, radius);
                let begin = Instant::now();
                let (drawn, wanted) = lod::choose(&ws.model, &picker, &pr, f64::from(H));
                let _ = lod::within(&ws.model, &pr, drawn, wanted, budget);
                begin.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        times.sort_by(f64::total_cmp);
        println!(
            "a frame's levels without drawing: median {:.1} ms, worst {:.1} ms",
            times[times.len() / 2],
            times[times.len() - 1]
        );
        return;
    };
    let info = adapter.get_info();
    if strict {
        assert_ne!(
            info.device_type,
            eframe::wgpu::DeviceType::Cpu,
            "the bar is for hardware: {} is software",
            info.name
        );
    }
    // Every occurrence in view is drawn but those under a pixel across.
    let cull = pr.cull();
    let tiny = picker
        .seen(|lo, hi| cull.sight(lo, hi))
        .iter()
        .filter(|b| pr.box_pixels(b.lo, b.hi, f64::from(H)) < lod::DRAWN_FROM)
        .count();
    println!(
        "  in the first frame: {} occurrences drawn, {tiny} under a pixel left out, {} without \
         edges",
        drawn.len(),
        drawn.iter().filter(|d| !d.edges).count()
    );
    assert_eq!(drawn.len() + tiny, n, "every occurrence in the first frame");
    let mut scene = Scene::new(&device, &adapter);
    let list = |pr: &Projector, camera: &ryucad::view::Camera| DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(camera),
        edges: true,
        faces: &[],
        overlay: &[],
        glass: &[],
    };
    scene.sync_drawn(&device, &drawn, centre);
    scene.render(&device, &queue, [W, H], &list(&pr, &start));
    let (w, h, pixels) = scene.read_pixels(&device, &queue).unwrap();
    let first = t.elapsed().as_secs_f64();
    // `RYUCAD_FRAME_PNG=<file>` keeps the first frame, to judge by eye.
    if let Some(out) = std::env::var_os("RYUCAD_FRAME_PNG") {
        image::RgbaImage::from_raw(w, h, pixels)
            .expect("the frame's size")
            .save(out)
            .unwrap();
    }
    let (bodies, placed) = scene.counts();
    println!(
        "the first full frame: {first:.1} s from opening ({bodies} bodies on the GPU, {placed} \
         occurrences)"
    );
    ws.fetch_levels(wanted);
    // Then a turn about the middle, closing in to a tenth of the distance.
    let mut times = Vec::with_capacity(FRAMES);
    // The view makes its picker again only when the model changes.
    let (mut picker, mut seen) = (picker, ws.model.generation);
    let mut pickers = 0usize;
    // Where each frame goes, step by step (#237): the window's pump and a
    // new picker, levels, fetches, the upload, the submit, and the wait for
    // the GPU to finish (`read_pixels`).
    const STEPS: [&str; 6] = ["pump", "levels", "fetch", "sync", "render", "gpu wait"];
    let mut steps: Vec<[f64; 6]> = Vec::with_capacity(FRAMES);
    let mut drawn_counts = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        let t = f as f64 / FRAMES as f64;
        let turned = view::orbit(&start, centre, t * core::f64::consts::TAU, 0.0);
        let camera = view::zoom(&turned, 1.0 - 0.9 * t, Some(centre));
        let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, radius);
        let begin = Instant::now();
        let mut at = begin;
        let mut lap = || {
            let now = Instant::now();
            let d = (now - at).as_secs_f64() * 1e3;
            at = now;
            d
        };
        ws.pump();
        if ws.model.generation != seen {
            (picker, seen) = (Picker::new(&ws.model), ws.model.generation);
            pickers += 1;
        } else {
            picker.refresh(&ws.model);
        }
        let pump = lap();
        let (drawn, wanted) = lod::choose(&ws.model, &picker, &pr, f64::from(H));
        let (drawn, wanted) = lod::within(&ws.model, &pr, drawn, wanted, budget);
        let levels = lap();
        drawn_counts.push(drawn.len());
        ws.fetch_levels(wanted);
        let fetch = lap();
        scene.sync_drawn(&device, &drawn, centre);
        let sync = lap();
        scene.render(&device, &queue, [W, H], &list(&pr, &camera));
        let render = lap();
        scene.read_pixels(&device, &queue).unwrap();
        let wait = lap();
        steps.push([pump, levels, fetch, sync, render, wait]);
        times.push(begin.elapsed().as_secs_f64() * 1e3);
    }
    for (k, name) in STEPS.iter().enumerate() {
        let mut v: Vec<f64> = steps[10..].iter().map(|s| s[k]).collect();
        v.sort_by(f64::total_cmp);
        println!(
            "  {name}: median {:.1} ms, worst {:.1} ms",
            v[v.len() / 2],
            v[v.len() - 1]
        );
    }
    println!(
        "  pickers made again while orbiting: {pickers} (meshes at the display quality taken in \
         place: {})",
        ws.model.reindexed.len()
    );
    let mut counts = drawn_counts[10..].to_vec();
    counts.sort_unstable();
    println!(
        "  occurrences drawn: median {}, fewest {}, most {}",
        counts[counts.len() / 2],
        counts[0],
        counts[counts.len() - 1]
    );
    // What the GPU's part is made of, at the whole model's view: the
    // triangles and edges drawn, and a frame with and without edges.
    {
        let (drawn, _) = lod::choose(&ws.model, &picker, &pr, f64::from(H));
        let (drawn, _) = lod::within(&ws.model, &pr, drawn, Vec::new(), budget);
        let (mut tris, mut lines) = (0usize, 0usize);
        for d in &drawn {
            if let Ok(m) = &d.mesh.mesh {
                tris += m.triangles.len();
                if !d.edges {
                    continue;
                }
                lines += m
                    .edges
                    .iter()
                    .map(|e| e.1.len().saturating_sub(1))
                    .sum::<usize>();
            }
        }
        println!(
            "  at the whole view: {} occurrences, {:.1}M triangles, {:.1}M edge segments",
            drawn.len(),
            tris as f64 / 1e6,
            lines as f64 / 1e6
        );
        scene.sync_drawn(&device, &drawn, centre);
        for edges in [true, false] {
            let mut l = list(&pr, &start);
            l.edges = edges;
            let mut v: Vec<f64> = (0..20)
                .map(|_| {
                    let b = Instant::now();
                    scene.render(&device, &queue, [W, H], &l);
                    scene.read_pixels(&device, &queue).unwrap();
                    b.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            v.sort_by(f64::total_cmp);
            println!(
                "  a whole-view frame, edges {edges}: median {:.1} ms",
                v[10]
            );
        }
    }
    // Views where most occurrences are several pixels across (#237): the
    // whole view draws the metre-wide race bodies and leaves the rest
    // under a pixel. Each stop frames some of the model and turns about
    // it; its frames count towards the bar.
    let mut steady: Vec<f64> = times[10..].to_vec();
    for (name, fly, keep) in stops(&ws) {
        let boxes: Vec<([f64; 3], [f64; 3])> = picker
            .seen(|_, _| ryucad_app::core::nav::Sight::In)
            .iter()
            .filter(|b| keep(&b.key.1))
            .map(|b| (b.lo, b.hi))
            .collect();
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for (l, h) in &boxes {
            for k in 0..3 {
                lo[k] = lo[k].min(l[k]);
                hi[k] = hi[k].max(h[k]);
            }
        }
        let at = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
        let pivot = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let mut v = Vec::new();
        let (mut most, mut edged, mut uploads) = (0usize, 0usize, 0.0f64);
        for f in 0..STOP_FRAMES {
            let t = f as f64 / STOP_FRAMES as f64;
            // Turning about what it frames, or flying along its length
            // at a tenth of its width above it, looking ahead.
            let camera = if fly {
                let x = lo[0] + 0.05 * (hi[0] - lo[0]);
                let y = lo[1] + t * (hi[1] - lo[1]);
                let z = hi[2] + 0.02 * (hi[0] - lo[0]);
                view::Camera {
                    eye: [x, y - 0.5, z],
                    target: [x + 0.2, y + 3.0, lo[2]],
                    up: [0.0, 0.0, 1.0],
                    projection: view::DEFAULT_PROJECTION,
                }
            } else {
                view::orbit(&at, pivot, t * core::f64::consts::TAU, 0.0)
            };
            let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, radius);
            let begin = Instant::now();
            ws.pump();
            if ws.model.generation != seen {
                (picker, seen) = (Picker::new(&ws.model), ws.model.generation);
                pickers += 1;
            } else {
                picker.refresh(&ws.model);
            }
            let (drawn, wanted) = lod::choose(&ws.model, &picker, &pr, f64::from(H));
            let (drawn, wanted) = lod::within(&ws.model, &pr, drawn, wanted, budget);
            most = most.max(drawn.len());
            edged = edged.max(drawn.iter().filter(|d| d.edges).count());
            ws.fetch_levels(wanted);
            let synced = Instant::now();
            scene.sync_drawn(&device, &drawn, centre);
            uploads = uploads.max(synced.elapsed().as_secs_f64() * 1e3);
            scene.render(&device, &queue, [W, H], &list(&pr, &camera));
            scene.read_pixels(&device, &queue).unwrap();
            v.push(begin.elapsed().as_secs_f64() * 1e3);
        }
        // The first frames fetch the stop's finer levels.
        let held = v[STOP_FRAMES / 4..].to_vec();
        let mut s = held.clone();
        s.sort_by(f64::total_cmp);
        let first = v.iter().take(STOP_FRAMES / 4).copied().fold(0.0, f64::max);
        println!(
            "  framing {name}: {} occurrences in it, up to {most} drawn ({edged} with edges); \
             median {:.1} ms, worst {:.1} ms (worst while its levels came {first:.1} ms, worst \
             sync {uploads:.1} ms)",
            boxes.len(),
            s[s.len() / 2],
            s[s.len() - 1],
        );
        steady.extend(v);
    }
    println!("  pickers made again by the end of the stops: {pickers}");
    steady.sort_by(f64::total_cmp);
    let median = steady[steady.len() / 2];
    let worst = steady[steady.len() - 1];
    let fps = 1000.0 / median;
    println!(
        "{n} occurrences at {W}×{H} on {} ({:?}, {}): median {median:.1} ms ({fps:.0} fps), \
         worst {worst:.1} ms",
        info.name, info.device_type, info.driver,
    );
    if strict {
        assert!(first <= 20.0, "the first full frame took {first:.1} s");
        assert!(fps >= 60.0, "{fps:.0} frames a second");
        assert!(worst < 50.0, "worst frame {worst:.1} ms");
    }
}
