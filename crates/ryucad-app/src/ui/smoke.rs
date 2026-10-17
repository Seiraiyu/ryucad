//! `ryucad-app --smoke DIR`: the release check that the app works on a
//! machine with no screen. It connects to the project's daemon (starting
//! it), loads the model, draws one frame offscreen and reports what it
//! drew, then exits.

use crate::core::link::find_ryucad;
use crate::core::nav::Projector;
use crate::core::pick::Picker;
use crate::core::workspace::Workspace;
use crate::ui::scene::{self, DrawList, Scene};
use ryucad::doc::project::find_root;
use ryucad::error::{Code, Error, Result};
use ryucad::view::{self, StandardView};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// How long loading may take.
const WAIT: Duration = Duration::from_secs(60);

/// The frame's size.
pub const SIZE: [u32; 2] = [320, 240];

/// Runs the smoke check on the project in or above `dir`.
pub fn run(dir: &Path) -> Result<Value> {
    let root = find_root(dir)?;
    let daemon = find_ryucad()?;
    let mut ws = Workspace::open(&root, &daemon, "smoke", Arc::new(|| {}))?;
    let loaded = ws.wait_until(WAIT, |ws| {
        ws.idle()
            && ws.model.parts.values().all(|p| {
                p.features.iter().all(|f| {
                    f.hash.is_none() || ws.model.meshes.contains_key(&(p.id.clone(), f.id.clone()))
                })
            })
    });
    if !loaded {
        return Err(Error::new(Code::Daemon, "the model didn't load in time"));
    }
    let (adapter, device, queue) = scene::headless()
        .ok_or_else(|| Error::new(Code::Internal, crate::ui::platform::NO_ADAPTER))?;
    let picker = Picker::new(&ws.model);
    let (lo, hi) = picker.bounds().unwrap_or(([-0.05; 3], [0.05; 3]));
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let d = [0, 1, 2].map(|k| hi[k] - lo[k]);
    let radius = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 2.0;
    let camera = view::frame(lo, hi, StandardView::Iso, view::DEFAULT_PROJECTION);
    let pr = Projector::new(
        &camera,
        f64::from(SIZE[0]) / f64::from(SIZE[1]),
        centre,
        radius,
    );
    let mut scene = Scene::new(&device, &adapter);
    scene.sync(&device, &ws.model, centre);
    let list = DrawList {
        reference: centre,
        matrix: pr.matrix(centre),
        light: scene::light(&camera),
        edges: true,
        faces: &[],
    };
    scene.render(&device, &queue, SIZE, &list);
    let (w, h, px) = scene
        .read_pixels(&device, &queue)
        .ok_or_else(|| Error::new(Code::Internal, "the frame couldn't be read back"))?;
    let bg = scene::theme::BACKGROUND.map(|x| (x * 255.0).round() as u8);
    let drawn = px
        .chunks(4)
        .filter(|c| c[..3].iter().zip(bg).any(|(a, b)| a.abs_diff(b) > 3))
        .count();
    let info = adapter.get_info();
    Ok(json!({
        "root": root,
        "parts": ws.model.parts.len(),
        "bodies": ws.model.meshes.len(),
        "frame": {"width": w, "height": h, "drawn_pixels": drawn},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend),
                    "software": info.device_type == eframe::wgpu::DeviceType::Cpu},
    }))
}
