//! The viewport compared with a picture of it, within a tolerance: GPU
//! rasterizers differ in the last bit, so this is not a golden hash. The
//! picture was drawn by lavapipe, Mesa's software Vulkan; other adapters
//! only check that the model was drawn. `UPDATE_SNAPSHOTS=1` draws it
//! again; a failing run leaves its picture beside it as `*.new.png`.

mod common;

use common::{Project, gpu, ryucad};
use ryucad::view::{self, StandardView};
use ryucad_app::core::compare::FaceMark;
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use ryucad_app::ui::scene::{self, DrawList, Scene, linear, theme};
use ryucad_app::ui::style;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const W: u32 = 400;
const H: u32 = 300;

/// A channel further off than this counts as a different pixel.
const CHANNEL: u8 = 24;
/// At most this share of pixels may differ.
const SHARE: f64 = 0.01;

fn write_png(path: &Path, px: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), W, H);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(px).unwrap();
}

fn read_png(path: &Path) -> Vec<u8> {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = d.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    buf
}

#[test]
fn the_viewport_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_part().with_top_sketch();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2
        && ws.model.sketches.len() == 1));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    // One face painted, as a selection is.
    let key = ("p".to_owned(), "boss".to_owned());
    let faces = [(key, 0, linear(theme::NOTE))];
    // The sketch on the box's top, under-defined.
    let overlay = ryucad_app::core::sketch::overlay(
        ws.model.sketches.values().map(|d| &**d),
        &pr,
        [f64::from(W), f64::from(H)],
        1.0,
    );
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: ryucad_app::ui::scene::theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &faces,
        overlay: &overlay,
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "viewport");
}

/// A compare with `HEAD`: the box made longer is painted where it
/// changed and grew, and the cylinder taken off it stands as a ghost.
#[test]
fn a_compare_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_part().committed();
    p.ok(&["feature", "edit", "p", "base", "--param", "x=30"]);
    p.ok(&["feature", "delete", "p", "boss"]);
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    p.ok(&["compare", "HEAD"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| {
        ws.idle()
            && ws.model.meshes.len() == 1
            && ws
                .compare
                .as_ref()
                .is_some_and(|c| c.loaded && c.ghosts.len() == 2)
    }));
    let c = ws.compare.as_ref().unwrap();
    let picker = Picker::new(&ws.model);
    let (lo, hi) = ([-0.004, -0.004, 0.0], [0.03, 0.01, 0.013]);
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.02);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    scene.sync_ghosts(&device, Some(c));
    // Painted as the view paints a compare.
    let mut faces = Vec::new();
    for (key, marks) in &c.faces {
        for (name, m) in marks {
            let r = format!("{}/{}/{name}", key.0, key.1);
            if let Some(ryucad_app::core::pick::Place::Face(k, i)) = picker.place(&r) {
                let colour = match m {
                    FaceMark::New => style::NEW,
                    FaceMark::Changed => style::CHANGED,
                };
                faces.push((k, i, linear(colour)));
            }
        }
    }
    assert!(!faces.is_empty());
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &faces,
        overlay: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "compare");
}

/// A plate with its vertical edges rounded and a row of four counterbored
/// holes, patterned from one: what phase 4's solids look like.
#[test]
fn a_drilled_part_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot");
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "plate", "--param", "x=30", "--param", "y=16",
        "--param", "z=4",
    ]);
    let body = p.ok(&["inspect", "p", "plate"])["bodies"][0].clone();
    let mut round = vec![
        "feature", "add", "p", "fillet", "--id", "round", "--radius", "3",
    ];
    let edges: Vec<String> = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["curve"]["type"] == "line" && e["curve"]["dir"][2].as_f64().unwrap().abs() == 1.0
        })
        .map(|e| format!("p/plate/{}", e["name"].as_str().unwrap()))
        .collect();
    assert_eq!(edges.len(), 4);
    for e in &edges {
        round.extend(["--edge", e.as_str()]);
    }
    p.ok(&round);
    p.ok(&[
        "feature", "add", "p", "sketch", "--id", "s", "--origin", "0,0,4",
    ]);
    let ops = r#"[{"op": "add", "type": "point", "at": [6, 8]},
                  {"op": "add", "type": "fix", "a": "p1"}]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "hole",
        "--id",
        "h",
        "--sketch",
        "s",
        "--form",
        "counterbore",
        "--diameter",
        "2.4",
        "--counterbore-diameter",
        "4.4",
        "--counterbore-depth",
        "1.5",
        "--depth",
        "through_all",
    ]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "pattern",
        "--id",
        "row",
        "--seed",
        "h",
        "--direction",
        "x",
        "--spacing",
        "6",
        "--count",
        "4",
    ]);
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 1
        && ws.model.sketches.len() == 1));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
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
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "solids");
}

/// Compares `px` with `tests/snapshots/<name>-lavapipe.png` on lavapipe,
/// or only checks something was drawn on other adapters.
fn looks_as_it_did(adapter: &eframe::wgpu::Adapter, px: &[u8], name: &str) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}-lavapipe.png"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        write_png(&path, px);
        return;
    }
    let info = adapter.get_info();
    let lavapipe = format!("{} {}", info.name, info.driver).contains("llvmpipe");
    if !lavapipe {
        let bg = theme::BACKGROUND.map(|x| (x * 255.0).round() as u8);
        let drawn = px
            .chunks(4)
            .filter(|c| c[..3].iter().zip(bg).any(|(a, b)| a.abs_diff(b) > 3))
            .count();
        assert!(drawn > 5000, "{} drew {drawn} pixels", info.name);
        eprintln!(
            "{}: drawn, not compared (the picture is lavapipe's)",
            info.name
        );
        return;
    }
    let want = read_png(&path);
    let differ = px
        .chunks(4)
        .zip(want.chunks(4))
        .filter(|(a, b)| {
            a.iter()
                .zip(b.iter())
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL)
        })
        .count();
    let limit = (f64::from(W * H) * SHARE) as usize;
    if differ > limit {
        write_png(&path.with_extension("new.png"), px);
    }
    assert!(
        differ <= limit,
        "{differ} pixels differ (at most {limit}); see {name}-lavapipe.new.png"
    );
}
