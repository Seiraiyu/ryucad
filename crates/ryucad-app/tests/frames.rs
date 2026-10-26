//! Design 72's drawing bar: 1,000 pins of 19 sizes on a plate (1,001
//! occurrences of 20 parts) drawn at 60 frames a second on a hardware
//! adapter while the camera orbits and zooms in, so levels of detail
//! change, with no frame a stall. Each frame chooses levels and culls as
//! the window does (`core::lod::choose`), draws, and waits for the GPU by
//! reading the picture back, so a frame here costs at least what it costs
//! on screen. Ignored by default: it means something only on hardware,
//! and runs with `--ignored` where the bar is recorded; `RYUCAD_BARS=1`
//! asserts the bar there (and refuses a software adapter).

mod common;

use common::{Project, gpu, ryucad};
use ryucad::view::{self, StandardView};
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use ryucad_app::ui::scene::{self, DrawList, Scene, theme};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

const W: u32 = 1280;
const H: u32 = 800;
const SIZES: usize = 19;
const PINS: usize = 1000;
const FRAMES: usize = 240;

/// The project: the plate and the 19 pins through the CLI, the assembly's
/// 1,001 instances written as files (the CLI would take minutes).
fn project() -> Project {
    let p = Project::named("frames");
    p.ok(&["part", "create", "plate"]);
    p.ok(&[
        "feature", "add", "plate", "box", "--id", "slab", "--param", "x=820", "--param", "y=520",
        "--param", "z=10",
    ]);
    for k in 0..SIZES {
        let part = format!("pin{k}");
        let radius = format!("radius={}", 2.0 + 0.1 * k as f64);
        p.ok(&["part", "create", &part]);
        p.ok(&[
            "feature",
            "add",
            &part,
            "cylinder",
            "--id",
            "shank",
            "--param",
            &radius,
            "--param",
            "height=30",
        ]);
    }
    p.ok(&["part", "create", "asm", "--assembly"]);
    let write = |rel: &str, v: serde_json::Value| {
        let path = p.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, ryucad::doc::canonical::to_string(&v)).unwrap();
    };
    let mut list = vec![json!("base")];
    write(
        "parts/asm/features/base.json",
        json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "plate",
               "rev": "working", "fixed": true,
               "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
    );
    for i in 0..PINS {
        let (x, y) = (0.01 + 0.02 * (i % 40) as f64, 0.01 + 0.02 * (i / 40) as f64);
        write(
            &format!("parts/asm/features/p{i}.json"),
            json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                   "part": format!("pin{}", i % SIZES), "rev": "working",
                   "pose": {"origin_m": [x, y, 0.01], "z": [0, 0, 1], "x": [1, 0, 0]}}),
        );
        list.push(json!(format!("p{i}")));
    }
    write(
        "parts/asm/part.json",
        json!({"format": 2, "kind": "ryucad.part", "part_kind": "assembly", "features": list}),
    );
    p
}

#[test]
#[ignore = "a bar for hardware adapters: run with --ignored where it is recorded"]
fn a_thousand_occurrences_draw_at_60_frames_a_second() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    let strict = std::env::var_os("RYUCAD_BARS").is_some();
    if strict {
        assert_ne!(
            info.device_type,
            eframe::wgpu::DeviceType::Cpu,
            "the bar is for hardware: {} is software",
            info.name
        );
    }
    let p = project();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(300), |ws| ws.idle()
        && ws.model.drawn().count() == PINS + 1));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let start = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    // The depth range holds the model's bounding sphere.
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) * (hi[k] - lo[k]))
        .sum::<f64>()
        .sqrt()
        / 2.0;
    let mut scene = Scene::new(&device, &adapter);
    let mut times = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        // A turn about the middle, closing in to a tenth of the distance:
        // pins cross the level bands as they grow on screen.
        let t = f as f64 / FRAMES as f64;
        let turned = view::orbit(&start, centre, t * core::f64::consts::TAU, 0.0);
        let camera = view::zoom(&turned, 1.0 - 0.9 * t, Some(centre));
        let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, radius);
        let begin = Instant::now();
        ws.pump();
        let (drawn, wanted) = ryucad_app::core::lod::choose(&ws.model, &picker, &pr, f64::from(H));
        ws.fetch_levels(wanted);
        scene.sync_drawn(&device, &drawn, centre);
        let list = DrawList {
            reference: centre,
            matrix: pr.matrix(centre),
            perspective: pr.line_pull(),
            background: theme::BACKGROUND,
            light: scene::light(&camera),
            edges: true,
            faces: &[],
            overlay: &[],
        };
        scene.render(&device, &queue, [W, H], &list);
        scene.read_pixels(&device, &queue).unwrap();
        times.push(begin.elapsed().as_secs_f64() * 1e3);
    }
    // The first frames upload every body: not what a frame costs.
    let mut steady: Vec<f64> = times[10..].to_vec();
    steady.sort_by(f64::total_cmp);
    let median = steady[steady.len() / 2];
    let worst = steady[steady.len() - 1];
    let fps = 1000.0 / median;
    println!(
        "1,001 occurrences of 20 parts at {W}×{H} on {} ({:?}, {}), {} {}: median {median:.1} ms \
         ({fps:.0} fps), worst {worst:.1} ms",
        info.name,
        info.device_type,
        info.driver,
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    if strict {
        assert!(fps >= 60.0, "{fps:.0} frames a second");
        // A stall is a frame three budgets long: levels change without one.
        assert!(worst < 50.0, "worst frame {worst:.1} ms");
    }
}
