//! The 3D view drawn on whatever GPU the machine has: software adapters
//! (lavapipe, WARP) count. Without one the tests pass with a note, unless
//! RYUCAD_REQUIRE_GPU is set, as CI sets it.

mod common;

use common::{Project, ryucad};
use ryucad::view::{self, StandardView};
use ryucad_app::core::nav::Projector;
use ryucad_app::core::pick::Picker;
use ryucad_app::core::workspace::Workspace;
use ryucad_app::ui::scene::{self, DrawList, Scene, linear, theme};
use std::sync::Arc;
use std::time::Duration;

fn gpu() -> Option<(
    eframe::wgpu::Adapter,
    eframe::wgpu::Device,
    eframe::wgpu::Queue,
)> {
    let g = scene::headless();
    if g.is_none() {
        assert!(
            std::env::var_os("RYUCAD_REQUIRE_GPU").is_none(),
            "no GPU adapter"
        );
        eprintln!("no GPU adapter: skipped");
    }
    g
}

fn srgb(c: [f32; 3]) -> [u8; 3] {
    c.map(|x| (x * 255.0).round() as u8)
}

fn close(a: &[u8], b: [u8; 3], by: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= by)
}

#[test]
fn the_model_is_drawn_with_its_edges_and_painted_faces() {
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
    let mut draw = |edges: bool, faces: &[scene::PaintedFace]| {
        let list = DrawList {
            reference: centre,
            matrix: pr.matrix(centre),
            light: scene::light(&camera),
            edges,
            faces,
        };
        scene.render(&device, &queue, [size, size], &list);
        scene.read_pixels(&device, &queue).unwrap().2
    };
    let at = |px: &[u8], x: u32, y: u32| px[((y * size + x) * 4) as usize..][..3].to_vec();
    let plain = draw(true, &[]);
    // The corner is background; the middle, the top of the box, is lit
    // surface.
    assert!(close(&at(&plain, 0, 0), srgb(theme::BACKGROUND), 2));
    let mid = at(&plain, size / 2, size / 2);
    assert!(!close(&mid, srgb(theme::BACKGROUND), 8), "{mid:?}");
    assert!(mid[0].abs_diff(mid[2]) < 20, "grey: {mid:?}");
    // Edges darken pixels; without them there are fewer dark ones.
    let dark = |px: &[u8]| px.chunks(4).filter(|c| c[0] < 80).count();
    assert!(dark(&plain) > dark(&draw(false, &[])));
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
    let painted = draw(true, &[(key, face, linear(theme::NOTE))]);
    let blue = at(&painted, size / 2, size / 2);
    assert!(blue[2] > blue[0] + 40, "{blue:?}");
    // The same frame twice is the same pixels.
    assert_eq!(draw(true, &[]), plain);
}
