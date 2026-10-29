//! Design 192's drawing bars, over its benchmark project (10,000 parts,
//! 100,000 occurrences; `ryucad::bench::scale`): the window's first full
//! frame over a warm disk cache within 20 s, then orbiting and zooming at
//! 60 frames a second with no frame over 50 ms. The project is generated,
//! a first window builds it and fetches every mesh (warming the cache),
//! its daemon stops, and a second window is timed from opening to the
//! first frame drawing every occurrence: the root's report, every body's
//! coarse mesh (`lod::COARSE_FIRST`), the picker, levels chosen, culled
//! and kept within `display.gpu_memory`, every body uploaded, drawn and
//! read back. Then frames orbit and close in as `tests/frames.rs` does.
//!
//! Ignored: it means something only on hardware, and takes many minutes
//! (the first build of the project is a cold rebuild of 10,000 parts).
//! Without an adapter it times everything up to drawing and stops there;
//! `RYUCAD_BARS=1` asserts the bars (and refuses a software adapter).
//! `RYUCAD_SCALE_PARTS` makes a smaller project of the same shape.
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
    let mut ws = Workspace::open(root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
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
    let root = dir.path().join("p");
    let spec = spec();
    let made = write(&root, &spec, &race(spec.imports()));
    println!(
        "{} parts, {} occurrences, {} {}",
        made.parts,
        made.occurrences,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
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
        ws.received as f64 / 1e6
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
    assert_eq!(drawn.len(), n, "every occurrence in the first frame");
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
    };
    scene.sync_drawn(&device, &drawn, centre);
    scene.render(&device, &queue, [W, H], &list(&pr, &start));
    scene.read_pixels(&device, &queue).unwrap();
    let first = t.elapsed().as_secs_f64();
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
    for f in 0..FRAMES {
        let t = f as f64 / FRAMES as f64;
        let turned = view::orbit(&start, centre, t * core::f64::consts::TAU, 0.0);
        let camera = view::zoom(&turned, 1.0 - 0.9 * t, Some(centre));
        let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, radius);
        let begin = Instant::now();
        ws.pump();
        if ws.model.generation != seen {
            (picker, seen) = (Picker::new(&ws.model), ws.model.generation);
        }
        let (drawn, wanted) = lod::choose(&ws.model, &picker, &pr, f64::from(H));
        let (drawn, wanted) = lod::within(&ws.model, &pr, drawn, wanted, budget);
        ws.fetch_levels(wanted);
        scene.sync_drawn(&device, &drawn, centre);
        scene.render(&device, &queue, [W, H], &list(&pr, &camera));
        scene.read_pixels(&device, &queue).unwrap();
        times.push(begin.elapsed().as_secs_f64() * 1e3);
    }
    let mut steady: Vec<f64> = times[10..].to_vec();
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
