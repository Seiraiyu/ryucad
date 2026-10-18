//! The viewport compared with a picture of it, within a tolerance: GPU
//! rasterizers differ in the last bit, so this is not a golden hash. The
//! picture was drawn by lavapipe, Mesa's software Vulkan; other adapters
//! only check that the model was drawn. `UPDATE_SNAPSHOTS=1` draws it
//! again; a failing run leaves its picture beside it as `*.new.png`.

mod common;

use common::{Project, gpu, ryucad};
use ryucad::view::{self, StandardView};
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use ryucad_app::ui::scene::{self, DrawList, Scene, linear, theme};
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
    let p = Project::named("snapshot").with_part();
    let mut ws = Workspace::open(&p.root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
    assert!(ws.wait_until(Duration::from_secs(30), |ws| ws.idle()
        && ws.model.meshes.len() == 2));
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
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        light: scene::light(&camera),
        edges: true,
        faces: &faces,
    };
    scene.render(&device, &queue, [W, H], &list);
    let (_, _, px) = scene.read_pixels(&device, &queue).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/viewport-lavapipe.png");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        write_png(&path, &px);
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
        write_png(&path.with_extension("new.png"), &px);
    }
    assert!(
        differ <= limit,
        "{differ} pixels differ (at most {limit}); see viewport-lavapipe.new.png"
    );
}
