//! `ryueda-app --smoke [DIR]`: the project opened, its first scene
//! awaited, and one frame of the real window drawn offscreen through
//! egui's wgpu renderer, read back and counted: a release check that the
//! app draws on this machine.

use crate::core::link::find_ryueda;
use crate::core::window::Window;
use crate::ui::app::App;
use crate::ui::platform::NO_ADAPTER;
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use ryu_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

pub const SIZE: [u32; 2] = [640, 400];
const WAIT: Duration = Duration::from_secs(60);

/// A headless adapter, its device and queue, if any draws here.
pub fn headless() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ryueda-app headless"),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .ok()?;
    Some((adapter, device, queue))
}

pub fn run(dir: &Path) -> Result<Value> {
    let daemon = find_ryueda()?;
    let ctx = egui::Context::default();
    let mut app = App::open(&ctx, dir, &daemon);
    let w: &mut Window = app.opened_mut().ok_or_else(|| Error::new(Code::NotFound, "no ryueda project here"))?;
    if !w.wait_until(WAIT, |w| w.scene.is_some()) {
        return Err(Error::new(Code::Daemon, "the first sheet wasn't drawn in time"));
    }
    let sheet = w.scene.as_ref().map(|s| s.name.clone());
    let (adapter, device, queue) = headless().ok_or_else(|| Error::new(Code::Internal, NO_ADAPTER))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE[0] as f32, SIZE[1] as f32))),
        ..Default::default()
    };
    // Two frames: the second has the tiles' sizes and the canvas's fit.
    let _ = ctx.run_ui(input.clone(), |ui| app.frame(ui));
    let out = ctx.run_ui(input, |ui| app.frame(ui));
    let jobs = ctx.tessellate(out.shapes, 1.0);
    let mut renderer = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
    for (id, delta) in &out.textures_delta.set {
        renderer.update_texture(&device, &queue, *id, delta);
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("smoke"),
        size: wgpu::Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: SIZE, pixels_per_point: 1.0 };
    let mut enc = device.create_command_encoder(&Default::default());
    renderer.update_buffers(&device, &queue, &mut enc, &jobs, &screen);
    {
        let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("smoke"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    let row = (SIZE[0] * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke read"),
        size: u64::from(row * SIZE[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(SIZE[1]) },
        },
        wgpu::Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::Wait);
    rx.recv()
        .ok()
        .and_then(|r| r.ok())
        .ok_or_else(|| Error::new(Code::Internal, "the frame couldn't be read back"))?;
    let px = slice.get_mapped_range();
    let mut drawn = 0;
    for y in 0..SIZE[1] as usize {
        let line = &px[y * row as usize..][..SIZE[0] as usize * 4];
        drawn += line.chunks(4).filter(|c| c[..3].iter().any(|x| *x > 3)).count();
    }
    let info = adapter.get_info();
    Ok(json!({
        "root": app.opened().map(|w| w.root.clone()),
        "sheet": sheet,
        "frame": {"width": SIZE[0], "height": SIZE[1], "drawn_pixels": drawn},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend),
                    "software": info.device_type == wgpu::DeviceType::Cpu},
    }))
}
