//! The 3D view drawn on whatever GPU the machine has: software adapters
//! (lavapipe, WARP) count. See `common::gpu`.

mod common;

use common::{Project, gpu, ryucad};
use ryucad::view::{self, StandardView};
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use ryucad_app::ui::scene::{self, DrawList, Scene, linear, theme};
use std::sync::Arc;
use std::time::Duration;

/// Held by each test for its whole run: lavapipe crashes (SIGSEGV) when
/// several tests draw at once, as `window.rs` found (#230).
fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

fn srgb(c: [f32; 3]) -> [u8; 3] {
    c.map(|x| (x * 255.0).round() as u8)
}

fn close(a: &[u8], b: [u8; 3], by: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= by)
}

#[test]
fn the_model_is_drawn_with_its_edges_and_painted_faces() {
    let _one = one_at_a_time();
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::new().with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Top, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, 1.0, centre, 0.02);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let size = 160u32;
    let mut background = theme::BACKGROUND;
    let mut draw = |edges: bool, faces: &[scene::PaintedFace], background: [f32; 3]| {
        let list = DrawList {
            reference: centre,
            matrix: pr.matrix(centre),
            perspective: pr.line_pull(),
            light: scene::light(&camera),
            edges,
            faces,
            background,
            overlay: &[],
        };
        scene.render(&device, &queue, [size, size], &list);
        scene.read_pixels(&device, &queue).unwrap().2
    };
    let at = |px: &[u8], x: u32, y: u32| px[((y * size + x) * 4) as usize..][..3].to_vec();
    let plain = draw(true, &[], background);
    // The corner is background; the middle, the top of the box, is lit
    // surface.
    assert!(close(&at(&plain, 0, 0), srgb(theme::BACKGROUND), 2));
    let mid = at(&plain, size / 2, size / 2);
    assert!(!close(&mid, srgb(theme::BACKGROUND), 8), "{mid:?}");
    assert!(mid[0].abs_diff(mid[2]) < 20, "grey: {mid:?}");
    // Edges darken pixels; without them there are fewer dark ones.
    let dark = |px: &[u8]| px.chunks(4).filter(|c| c[0] < 80).count();
    assert!(dark(&plain) > dark(&draw(false, &[], background)));
    // Painting the face in the middle colours it.
    let hit = picker
        .pick(
            &pr,
            [f64::from(size), f64::from(size)],
            [f64::from(size) / 2.0; 2],
        )
        .unwrap();
    let key = (hit.part.clone(), hit.feature.clone());
    let mesh = ws.model.meshes[&key].mesh.as_ref().unwrap();
    let face = mesh
        .faces
        .iter()
        .position(|f| hit.entity.ends_with(&ryucad::mesh::name_hex(&f.name)))
        .unwrap();
    let painted = draw(true, &[(key, face, linear(theme::NOTE))], background);
    let blue = at(&painted, size / 2, size / 2);
    assert!(blue[2] > blue[0] + 40, "{blue:?}");
    // The same frame twice is the same pixels.
    assert_eq!(draw(true, &[], background), plain);
    // The dark theme's background.
    background = ryucad_app::ui::style::Look::default().viewport_background(true);
    let dark_frame = draw(true, &[], background);
    assert!(close(&at(&dark_frame, 0, 0), srgb(background), 2));
    assert!(at(&dark_frame, 0, 0)[0] < 90, "{:?}", at(&dark_frame, 0, 0));
}

/// A part coloured draws in its colour; made translucent, what is behind
/// it shows through, and one frame twice is the same pixels.
#[test]
fn colours_and_translucent_faces_draw() {
    let _one = one_at_a_time();
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::new().with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Top, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, 1.0, centre, 0.02);
    let mut scene = Scene::new(&device, &adapter);
    let size = 160u32;
    let mut draw = |ws: &Workspace| {
        scene.sync(&device, &ws.model, centre);
        let list = DrawList {
            reference: centre,
            matrix: pr.matrix(centre),
            perspective: pr.line_pull(),
            light: scene::light(&camera),
            edges: true,
            faces: &[],
            background: theme::BACKGROUND,
            overlay: &[],
        };
        scene.render(&device, &queue, [size, size], &list);
        scene.read_pixels(&device, &queue).unwrap().2
    };
    let at = |px: &[u8], x: u32, y: u32| -> Vec<i32> {
        px[((y * size + x) * 4) as usize..][..3]
            .iter()
            .map(|c| i32::from(*c))
            .collect()
    };
    // Beside the boss, on the box's top.
    let (x, y) = (size / 2 - 35, size / 2);
    let plain = draw(&ws);
    p.ok(&["colour", "p", "--colour", "#ff0000"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.looks.len() == 2));
    let red = draw(&ws);
    let px = at(&red, x, y);
    assert!(px[0] > px[1] + 60 && px[0] > px[2] + 60, "{px:?}");
    assert_eq!(draw(&ws), red, "the same frame twice");
    p.ok(&["colour", "p/base", "--opacity", "0.3"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.looks.values().any(|l| l.translucent())));
    let glass = draw(&ws);
    let g = at(&glass, x, y);
    // The background shows through: paler than the red, still reddish.
    assert!(g[1] > px[1] + 40, "{g:?} over {px:?}");
    assert!(g[0] > g[2], "{g:?}");
    assert_eq!(draw(&ws), glass, "the same frame twice");
    // Cleared, it draws as it did.
    p.ok(&["colour", "p", "p/base", "--clear"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.looks.is_empty()));
    assert_eq!(draw(&ws), plain);
}

/// Edges hidden behind a face stay hidden, even zoomed in close: the eye
/// inside the model's bounding sphere, where the near plane is closest.
#[test]
fn hidden_edges_stay_hidden_close_up() {
    let _one = one_at_a_time();
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::new();
    p.ok(&["part", "create", "p"]);
    // A wide slab over a small box, 8 mm apart.
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "under",
        "--param",
        "x=10",
        "--param",
        "y=10",
        "--param",
        "z=2",
        "--origin=-5,-5,0",
    ]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "over",
        "--param",
        "x=40",
        "--param",
        "y=40",
        "--param",
        "z=2",
        "--origin=-20,-20,10",
    ]);
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let radius = ((0..3)
        .map(|k| (hi[k] - lo[k]) * (hi[k] - lo[k]))
        .sum::<f64>())
    .sqrt()
        / 2.0;
    // 6 mm above the slab, looking straight down: the small box's edges
    // are in view, behind the slab.
    let camera = view::Camera {
        eye: [0.0, 0.0, 0.018],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        projection: view::Projection::Perspective { fov_deg: 90.0 },
    };
    let pr = Projector::new(&camera, 1.0, centre, radius);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let size = 160u32;
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        overlay: &[],
        background: theme::BACKGROUND,
    };
    scene.render(&device, &queue, [size, size], &list);
    let px = scene.read_pixels(&device, &queue).unwrap().2;
    // All slab: lit surface everywhere, no edge lines.
    let edge = srgb(theme::EDGE);
    let lines = px.chunks(4).filter(|c| close(&c[..3], edge, 24)).count();
    assert_eq!(lines, 0, "hidden edges drawn over the slab");
}

/// Sketches draw in their solve state's colour on the face they lie on,
/// and hide behind solids as edges do.
#[test]
fn sketches_show_on_faces_and_hide_behind_them() {
    let _one = one_at_a_time();
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::new().with_part().with_top_sketch();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    let key = ("p".to_owned(), "top".to_owned());
    let drawn = |ws: &Workspace, z: f64| {
        ws.model
            .sketches
            .get(&key)
            .is_some_and(|d| (d.dots[0].at[2] - z).abs() < 1e-9)
    };
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2
        && drawn(ws, 0.005)));
    let size = 160u32;
    let mut scene = Scene::new(&device, &adapter);
    let mut blue = |ws: &Workspace| {
        let picker = Picker::new(&ws.model);
        let (lo, hi) = picker.bounds().unwrap();
        let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
        let camera = view::frame(lo, hi, StandardView::Top, view::DEFAULT_PROJECTION);
        let pr = Projector::new(&camera, 1.0, centre, 0.02);
        scene.sync(&device, &ws.model, centre);
        let s = f64::from(size);
        let overlay = ryucad_app::core::sketch::overlay(
            ws.model.sketches.values().map(|d| &**d),
            &pr,
            [s, s],
            1.0,
        );
        let list = DrawList {
            reference: centre,
            matrix: pr.matrix(centre),
            perspective: pr.line_pull(),
            light: scene::light(&camera),
            edges: true,
            faces: &[],
            background: theme::BACKGROUND,
            overlay: &overlay,
        };
        scene.render(&device, &queue, [size, size], &list);
        let px = scene.read_pixels(&device, &queue).unwrap().2;
        let under = srgb(ryucad::render::SKETCH_UNDER);
        px.chunks(4).filter(|c| close(&c[..3], under, 24)).count()
    };
    assert!(blue(&ws) > 60, "the sketch on the top face shows");
    // Under the box, seen from above, it is hidden.
    p.ok(&["feature", "edit", "p", "top", "--origin", "0,0,-1"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle() && drawn(ws, -0.001)));
    assert_eq!(blue(&ws), 0, "the sketch under the box is hidden");
}

/// A compare ghosts what the working files no longer have: a body that
/// is gone shows, translucent, in the removed colour.
#[test]
fn a_compare_ghosts_what_is_gone() {
    let _one = one_at_a_time();
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::new().with_part().committed();
    p.ok(&["feature", "delete", "p", "boss"]);
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 1));
    p.ok(&["compare", "HEAD"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.compare.as_ref().is_some_and(|c| c.ghosts.len() == 1)));
    // From the top, over the box and the cylinder that stood on it.
    let (lo, hi) = ([-0.004, -0.004, 0.0], [0.021, 0.011, 0.013]);
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Top, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, 1.0, centre, 0.02);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let size = 160u32;
    let draw = |scene: &mut Scene| {
        let list = DrawList {
            reference: centre,
            matrix: pr.matrix(centre),
            perspective: pr.line_pull(),
            light: scene::light(&camera),
            edges: false,
            faces: &[],
            background: theme::BACKGROUND,
            overlay: &[],
        };
        scene.render(&device, &queue, [size, size], &list);
        scene.read_pixels(&device, &queue).unwrap().2
    };
    // The cylinder's axis, where it rose through the box's top.
    let n = pr.ndc([0.0005, 0.0005, 0.013]).unwrap();
    let (x, y) = (
        ((n[0] + 1.0) / 2.0 * f64::from(size)) as u32,
        ((1.0 - n[1]) / 2.0 * f64::from(size)) as u32,
    );
    let at = |px: &[u8]| px[((y * size + x) * 4) as usize..][..3].to_vec();
    let plain = at(&draw(&mut scene));
    scene.sync_ghosts(&device, ws.compare.as_ref());
    let ghosted = at(&draw(&mut scene));
    let red = |c: &[u8]| i32::from(c[0]) - i32::from(c[1]);
    assert!(red(&ghosted) > red(&plain) + 30, "{plain:?} {ghosted:?}");
    // Out of the compare, the ghost goes.
    scene.sync_ghosts(&device, None);
    assert_eq!(at(&draw(&mut scene)), plain);
}

#[test]
fn an_adapter_that_draws_passes_the_probe() {
    let _one = one_at_a_time();
    // #85: the window passes over adapters whose devices can't draw.
    let Some((adapter, _, _)) = gpu() else {
        return;
    };
    assert!(ryucad_app::ui::platform::draws(&adapter, None));
}

/// A cube of side `s` with its corner at the origin, as a display mesh:
/// six faces of two triangles, each corner with its face's normal.
fn cube(s: f32) -> ryucad::mesh::WireMesh {
    use ryucad::mesh::{WireFace, WireMesh};
    let p = |i: u32| {
        let b = |k: u32| if i >> k & 1 == 1 { s } else { 0.0 };
        [b(0), b(1), b(2)]
    };
    // Each face: its normal and its corners counterclockwise from outside.
    let faces: [([f32; 3], [u32; 4]); 6] = [
        ([-1.0, 0.0, 0.0], [0, 4, 6, 2]),
        ([1.0, 0.0, 0.0], [1, 3, 7, 5]),
        ([0.0, -1.0, 0.0], [0, 1, 5, 4]),
        ([0.0, 1.0, 0.0], [2, 6, 7, 3]),
        ([0.0, 0.0, -1.0], [0, 2, 3, 1]),
        ([0.0, 0.0, 1.0], [4, 5, 7, 6]),
    ];
    let mut m = WireMesh {
        origin: [0.0; 3],
        chord: 0.0,
        angle: 0.0,
        positions: (0..8).map(p).collect(),
        corners: Vec::new(),
        triangles: Vec::new(),
        faces: Vec::new(),
        edges: Vec::new(),
        vertices: Vec::new(),
        faults: Vec::new(),
    };
    for (i, (n, c)) in faces.iter().enumerate() {
        let base = m.corners.len() as u32;
        let t = m.triangles.len() as u32;
        m.corners.extend(c.iter().map(|&k| (k, *n)));
        m.triangles.push([base, base + 1, base + 2]);
        m.triangles.push([base, base + 2, base + 3]);
        m.faces.push(WireFace {
            name: [i as u8; 16],
            corners: (base, base + 4),
            triangles: (t, t + 2),
        });
    }
    m
}

#[test]
fn a_thousand_occurrences_are_one_mesh() {
    let _one = one_at_a_time();
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    use ryucad_app::core::model::Model;
    use serde_json::json;
    // A 40 × 25 grid of 1 cm cubes, 2 cm apart.
    let occurrences: Vec<serde_json::Value> = (0..1000)
        .map(|i| {
            let (x, y) = (f64::from(i % 40) * 0.02, f64::from(i / 40) * 0.02);
            json!({"path": format!("c{i}"), "part": "cube",
                   "solids": [{"solid": "s", "hash": "h"}],
                   "pose": {"origin_m": [x, y, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
        })
        .collect();
    let mut model = Model::default();
    let want = model.apply_part(&json!({"part": "grid", "revision": "r", "kind": "assembly",
        "features": [], "solids": [], "occurrences": occurrences}));
    assert_eq!(want.len(), 1000);
    assert_eq!(model.ask(want).len(), 1, "one body, asked for once");
    let body = ryucad::mesh::WireBody {
        feature: "c0/s".into(),
        hash: "h".into(),
        mesh: Ok(cube(0.01)),
    };
    assert!(model.take_mesh("grid", body));
    assert_eq!(model.drawn().count(), 1000);
    let picker = Picker::new(&model);
    let (lo, hi) = picker.bounds().unwrap();
    assert_eq!(hi.map(|x| (x * 1e6).round() / 1e6), [0.79, 0.49, 0.01]);
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Top, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, 1.0, centre, 1.0);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &model, centre);
    assert_eq!(
        scene.counts(),
        (1, 1000),
        "one mesh on the GPU, drawn 1000 times"
    );
    let size = 400u32;
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        background: theme::BACKGROUND,
        overlay: &[],
    };
    scene.render(&device, &queue, [size, size], &list);
    let px = scene.read_pixels(&device, &queue).unwrap().2;
    // Seen from above, the cubes cover about a quarter of the grid's box.
    let lit = px
        .chunks(4)
        .filter(|c| !close(&c[..3], srgb(theme::BACKGROUND), 8))
        .count();
    let grid = (size * size) as f64 * (0.49 / 0.79);
    let share = lit as f64 / grid;
    assert!((0.15..0.4).contains(&share), "{share}");
    // Picking finds the cube under a pixel, by its occurrence: the top
    // of the 42nd (second row, second column).
    let n = pr.ndc([0.025, 0.025, 0.01]).unwrap();
    let at = [(n[0] + 1.0) / 2.0, (1.0 - n[1]) / 2.0].map(|x| x * f64::from(size));
    let hit = picker.pick(&pr, [f64::from(size); 2], at).unwrap();
    assert_eq!((hit.part.as_str(), hit.feature.as_str()), ("grid", "c41/s"));
    assert!((hit.point[2] - 0.01).abs() < 1e-9, "{:?}", hit.point);
}
