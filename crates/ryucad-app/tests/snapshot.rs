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
use ryucad_app::core::pick::{Picker, Place};
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
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
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
        glass: &[],
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
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
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
        glass: &[],
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
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
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
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "solids");
}

/// An assembly: a plate fixed, a bolt on it and a circular pattern of
/// five more about the plate's middle, drawn as the window draws them:
/// one mesh per body, each occurrence at its level of detail.
#[test]
fn an_assembly_with_a_pattern_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot");
    p.ok(&["part", "create", "plate"]);
    p.ok(&[
        "feature",
        "add",
        "plate",
        "box",
        "--id",
        "slab",
        "--param",
        "x=40",
        "--param",
        "y=40",
        "--param",
        "z=4",
        "--origin",
        "-20,-20,0",
    ]);
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=1.5",
        "--param",
        "height=8",
    ]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "head",
        "--operation",
        "join",
        "--target",
        "shank",
        "--param",
        "radius=2.5",
        "--param",
        "height=1.5",
        "--origin",
        "0,0,8",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "base", "--of", "plate", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "seed", "--of", "bolt", "--origin", "12,0,4",
    ]);
    p.ok(&[
        "feature",
        "add",
        "frame",
        "instance_pattern",
        "--id",
        "ring",
        "--form",
        "circular",
        "--instance",
        "seed",
        "--axis",
        "z",
        "--count",
        "6",
        "--angle",
        "360",
    ]);
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.drawn().count() == 7));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    // The depth range holds the model's bounding sphere.
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) * (hi[k] - lo[k]))
        .sum::<f64>()
        .sqrt()
        / 2.0;
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, radius);
    // Each occurrence's level, fetched as the window fetches it.
    let mut drawn = Vec::new();
    for _ in 0..20 {
        let (d, wanted) = ryucad_app::core::lod::choose(&ws.model, &picker, &pr, f64::from(H));
        drawn = d;
        if wanted.is_empty() {
            break;
        }
        ws.fetch_levels(wanted);
        assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()));
    }
    assert_eq!(drawn.len(), 7);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync_drawn(&device, &drawn, centre);
    // Two bodies, each uploaded once per level its occurrences are drawn
    // at; seven occurrences.
    let (meshes, occurrences) = scene.counts();
    assert_eq!(occurrences, 7);
    assert!((2..=6).contains(&meshes), "{meshes} meshes on the GPU");
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        overlay: &[],
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "assembly");
}

/// A datum plane through the box, an axis, a point and a helix about z,
/// as the window draws them: the plane's square translucent and outlined,
/// the axis dashed, the point a cross, the helix a polyline.
#[test]
fn datums_paths_and_helices_look_as_they_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_part();
    for f in [
        &[
            "feature",
            "add",
            "p",
            "plane",
            "--id",
            "d1",
            "--by",
            "offset",
            "--from",
            "xy",
            "--distance",
            "3mm",
        ][..],
        &[
            "feature", "add", "p", "axis", "--id", "a1", "--by", "of", "--axis", "y",
        ],
        &[
            "feature",
            "add",
            "p",
            "point",
            "--id",
            "q1",
            "--by",
            "coordinates",
            "--coordinates",
            "15mm,5mm,8mm",
        ],
        &[
            "feature", "add", "p", "helix", "--id", "h", "--axis", "z", "--radius", "6mm",
            "--pitch", "3mm", "--turns", "4",
        ],
    ] {
        p.ok(f);
    }
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2
        && ws.model.paths.len() == 1));
    let picker = Picker::new(&ws.model).with_datums(&ws.model, true);
    assert_eq!(picker.marks().len(), 4);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let (overlay, glass) = ryucad_app::core::datums::overlay(
        picker.marks(),
        |_| None,
        &pr,
        [f64::from(W), f64::from(H)],
        1.0,
    );
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        overlay: &overlay,
        glass: &glass,
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "datums");
}

/// A loft chosen as the window draws it: its two square sections, each
/// with a square where its loop starts, over the loft and a twisted sweep
/// beside it.
#[test]
fn a_chosen_loft_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_loft();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(60), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    ws.choose(Some(("p".to_owned(), "l".to_owned())));
    assert!(ws.wait_until(Duration::from_secs(60), |ws| ws.swept.is_some()));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let overlay = ryucad_app::core::sweeps::overlay(
        ws.swept.as_ref().unwrap(),
        style::CHOSEN,
        &pr,
        [f64::from(W), f64::from(H)],
        1.0,
    );
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        overlay: &overlay,
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "lofts");
}

/// A split chosen as the window draws it: the box behind the yz plane in
/// one tint, the solid it founds in front in the other, and the plane as
/// a datum plane is drawn.
#[test]
fn a_chosen_split_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_shell_and_split();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(60), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    let b = ws.model.parts["p"]
        .feature("sp")
        .unwrap()
        .body
        .clone()
        .unwrap();
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let faces: Vec<scene::PaintedFace> = b
        .sides
        .iter()
        .flat_map(|s| {
            let key = ("p".to_owned(), s.solid.clone());
            let c = scene::linear(ryucad_app::core::bodies::tint(&s.side));
            (0..picker.faces(&key)).map(move |i| (key.clone(), i, c))
        })
        .collect();
    let mark = b.plane_mark("p", "sp", Some((lo, hi))).unwrap();
    let (overlay, glass) =
        ryucad_app::core::bodies::plane_overlay(&mark, &pr, [f64::from(W), f64::from(H)], 1.0);
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &faces,
        overlay: &overlay,
        glass: &glass,
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "split");
}

/// A draft chosen (design 400, The app): the walls it drafted painted in
/// the chosen colour, its neutral plane drawn as a datum plane is, and its
/// pull as an arrow from the plane's middle.
#[test]
fn a_chosen_draft_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_tweaks();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(120), |ws| ws.idle()
        && ws.model.meshes.len() == 1));
    ws.choose(Some(("p".to_owned(), "d".to_owned())));
    assert!(ws.wait_until(Duration::from_secs(120), |ws| ws.idle()
        && ws.made.is_some()));
    let t = ws.model.parts["p"]
        .feature("d")
        .unwrap()
        .tweak
        .clone()
        .unwrap();
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let c = scene::linear(style::CHOSEN);
    let faces: Vec<scene::PaintedFace> = ws
        .made
        .as_ref()
        .unwrap()
        .faces
        .iter()
        .flat_map(|((part, solid), names)| {
            names
                .iter()
                .filter_map(|n| match picker.place(&format!("{part}/{solid}/{n}")) {
                    Some(Place::Face(k, i)) => Some((k, i, c)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(faces.len(), 2);
    let size = [f64::from(W), f64::from(H)];
    let mark = t.neutral_mark("p", "d", Some((lo, hi))).unwrap();
    let (mut overlay, glass) = ryucad_app::core::bodies::plane_overlay(&mark, &pr, size, 1.0);
    let d = [0, 1, 2].map(|k| hi[k] - lo[k]);
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 5.0;
    let arrow = ryucad_app::core::tweaks::arrow(t.neutral.unwrap().origin, t.pull.unwrap(), length);
    overlay.extend(ryucad_app::core::tweaks::lines_overlay(
        &arrow,
        ryucad::render::DATUM,
        &pr,
        size,
        1.0,
    ));
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &faces,
        overlay: &overlay,
        glass: &glass,
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "draft");
}

/// A sheet metal part in the flat view (design 346, The app): its plate
/// and flange laid flat, the flat body drawn in place of the folded one,
/// its bend line dashed in the colour of its direction, seen from above.
#[test]
fn a_flat_pattern_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_sheet_metal();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(120), |ws| ws.idle()
        && ws.model.meshes.len() == 1));
    ws.show_flat_view(Some(("p".to_owned(), String::new())));
    assert!(ws.wait_until(Duration::from_secs(120), |ws| {
        ws.idle()
            && ws.model.flat.is_some()
            && ws.flat_view.as_ref().is_some_and(|v| v.lines.is_some())
    }));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let lines = ws.flat_view.as_ref().unwrap().lines.as_ref().unwrap();
    // As the window turns to it: from the pattern's viewer side.
    let camera = lines.camera(&picker.points(None), f64::from(W) / f64::from(H));
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let overlay = ryucad_app::core::flat::overlay(lines, &pr, [f64::from(W), f64::from(H)], 1.0);
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        overlay: &overlay,
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "flat");
}

/// Compares `px` with `tests/snapshots/<name>-lavapipe.png` on lavapipe,
/// or only checks something was drawn on other adapters.
/// The part in steel blue, the base's top red, the boss translucent: the
/// base shows through it.
#[test]
fn a_coloured_translucent_part_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_part();
    let top = {
        let v = p.ok(&["inspect", "p", "base"]);
        let face = v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["surface"]["type"] == "plane"
                    && f["surface"]["frame"]["z"][2].as_f64().unwrap()
                        * if f["reversed"] == true { -1.0 } else { 1.0 }
                        == 1.0
            })
            .unwrap()
            .clone();
        format!("p/base/{}", face["name"].as_str().unwrap())
    };
    p.ok(&["colour", "p", "--colour", "#4a78c2"]);
    p.ok(&["colour", &top, "--colour", "#d03030"]);
    p.ok(&["colour", "p/boss", "--opacity", "0.4"]);
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2
        && ws.model.looks.len() == 2));
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
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "appearance");
}

/// The part cut by two planes an agent gives the window: the box's front
/// millimetre and half the boss, and the boss's top, taken away, the caps
/// hatched.
#[test]
fn a_cut_part_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_part();
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
    p.ok(&["section", "zx", "--offset", "1", "--flip"]);
    p.ok(&["section", "xy", "--offset", "10", "--add"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.sections.planes.len() == 2));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.015);
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    scene.cut(scene::Cut::of(&ws.sections.planes, &camera));
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        perspective: pr.line_pull(),
        background: theme::BACKGROUND,
        light: scene::light(&camera),
        edges: true,
        faces: &[],
        overlay: &[],
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "section");
}

/// The part placed twice, an explode an agent shows lifting one and
/// turning it.
#[test]
fn an_exploded_assembly_looks_as_it_did() {
    let Some((adapter, device, queue)) = gpu() else {
        return;
    };
    let p = Project::named("snapshot").with_part();
    p.ok(&["part", "create", "asm", "--assembly"]);
    p.ok(&[
        "feature", "add", "asm", "instance", "--id", "one", "--of", "p",
    ]);
    p.ok(&[
        "feature", "add", "asm", "instance", "--id", "two", "--of", "p", "--origin", "25,0,0",
    ]);
    p.ok(&[
        "explode",
        "save",
        "asm",
        "apart",
        "--steps",
        r#"[{"instances": ["two"], "translate": [0, 0, 15]},
            {"instances": ["two"], "rotate": {"axis": [1, 0, 0], "angle": "60 deg"}}]"#,
    ]);
    let mut ws = common::open_editing(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    let two = ("asm".to_owned(), "two/base".to_owned());
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.contains_key(&two)
        && ws.model.meshes.len() >= 4));
    p.ok(&["explode", "show", "apart"]);
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.exploded.is_some()
        && ws.explodes.list.len() == 1));
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap();
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(&camera, f64::from(W) / f64::from(H), centre, 0.03);
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
        glass: &[],
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    looks_as_it_did(&adapter, &px, "explode");
}

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
