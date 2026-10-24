//! The 3D view on the GPU: shaded faces and B-rep edges, drawn with wgpu
//! into a texture of the scene's own (with depth and multisampling), which
//! the window shows as an image and tests read back. Each body's mesh is
//! uploaded once per body hash; selections, highlights and hover paint
//! over faces from a small buffer built every frame, so moving the mouse
//! never uploads the model again.

use crate::core::compare::Compare;
use crate::core::model::{BodyKey, Model};
use crate::core::nav::Mat4;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use ryucad::mesh::{WireBody, WireMesh};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The colour the view is drawn in: `ryucad render`'s theme, so the window
/// and agents' renders look alike.
pub mod theme {
    pub use ryucad::render::{BACKGROUND, CHANGED, EDGE, NOTE, PENDING, SURFACE};
}

/// sRGB to linear: the scene's texture is sRGB, its arithmetic linear.
pub fn linear(c: [f32; 3]) -> [f32; 4] {
    let l = |x: f32| {
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    [l(c[0]), l(c[1]), l(c[2]), 1.0]
}

/// Towards the light for `camera`: over the viewer's left shoulder, as in
/// `ryucad render`.
pub fn light(camera: &ryucad::view::Camera) -> [f32; 3] {
    let (r, u, f) = camera.basis();
    let l = [0, 1, 2].map(|k| -0.4 * r[k] + 0.5 * u[k] - f[k]);
    let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
    l.map(|x| (x / n) as f32)
}

/// The texture format of the scene.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    matrix: Mat4,
    /// Towards the light, in model space; w unused.
    light: [f32; 4],
    /// x: the fraction of their distance lines are pulled towards the
    /// eye; y: 1 in perspective; z: the clip position of the eye's own z
    /// (perspective).
    flags: [f32; 4],
}

const SHADER: &str = r#"
struct U { matrix: mat4x4<f32>, light: vec4<f32>, flags: vec4<f32> }
@group(0) @binding(0) var<uniform> u: U;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
}
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
}

@vertex fn vs_face(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.matrix * vec4<f32>(v.pos, 1.0);
    o.normal = v.normal;
    o.color = v.color;
    return o;
}

@fragment fn fs_face(i: VOut) -> @location(0) vec4<f32> {
    let n = normalize(i.normal);
    // Ambient and diffuse as in `ryucad render`.
    let lit = 0.3 + 0.7 * max(dot(n, u.light.xyz), 0.0);
    return vec4<f32>(i.color.rgb * lit, i.color.a);
}

@vertex fn vs_line(v: VIn) -> VOut {
    var o: VOut;
    let c = u.matrix * vec4<f32>(v.pos, 1.0);
    // In front of the faces the edges bound, by a fraction of their
    // distance: a fixed step in depth would be meters far back in a
    // perspective view and pull hidden edges through solids. In
    // perspective the point moves along its own sight line, towards the
    // eye, so it stays on the same pixel.
    let e = u.flags.x;
    if u.flags.y > 0.5 {
        o.clip = vec4<f32>(c.xy * (1.0 - e), c.z * (1.0 - e) + e * u.flags.z, c.w * (1.0 - e));
    } else {
        o.clip = vec4<f32>(c.xy, max(c.z - e * c.w, 0.0), c.w);
    }
    o.normal = v.normal;
    o.color = v.color;
    return o;
}

@fragment fn fs_line(i: VOut) -> @location(0) vec4<f32> {
    return i.color;
}

@vertex fn vs_ghost(v: VIn) -> VOut {
    var o: VOut;
    let c = u.matrix * vec4<f32>(v.pos, 1.0);
    // Behind the model's faces by the fraction edges are in front of
    // them, along its sight line: where a ghost face and a face of the
    // model lie in one plane, triangulated differently, the model shows
    // rather than fighting it pixel by pixel.
    let e = -u.flags.x;
    if u.flags.y > 0.5 {
        o.clip = vec4<f32>(c.xy * (1.0 - e), c.z * (1.0 - e) + e * u.flags.z, c.w * (1.0 - e));
    } else {
        o.clip = vec4<f32>(c.xy, min(c.z - e * c.w, c.w), c.w);
    }
    o.normal = v.normal;
    o.color = v.color;
    return o;
}
"#;

/// A body on the GPU.
struct GpuBody {
    hash: String,
    mesh: Arc<WireBody>,
    faces: wgpu::Buffer,
    face_count: u32,
    edges: wgpu::Buffer,
    edge_count: u32,
}

/// The scene's render target.
struct Target {
    size: [u32; 2],
    color: Option<wgpu::TextureView>,
    resolved: wgpu::Texture,
    view: wgpu::TextureView,
    /// The same pixels for egui to show: a plain (not sRGB) view, since
    /// egui treats what it samples as already encoded. Sampled through
    /// an sRGB view they would be decoded once more and show too dark.
    shown: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// A face painted over the model: its body, its index in the body's mesh,
/// and a linear colour.
pub type PaintedFace = (BodyKey, usize, [f32; 4]);

/// What to draw this frame.
pub struct DrawList<'a> {
    /// The point positions are relative to: see [`crate::core::nav`].
    pub reference: [f64; 3],
    pub matrix: Mat4,
    /// Towards the light, in model space.
    pub light: [f32; 3],
    pub edges: bool,
    /// 1 in perspective, and the clip z of the eye: see
    /// [`crate::core::nav::Projector::line_pull`].
    pub perspective: [f32; 2],
    /// Faces painted over, last on top.
    pub faces: &'a [PaintedFace],
    /// The background, sRGB: see [`crate::ui::style::Look::viewport_background`].
    pub background: [f32; 3],
    /// Sketches, as [`crate::core::sketch::overlay`] makes them.
    pub overlay: &'a [crate::core::sketch::Corner],
}

pub struct Scene {
    faces: wgpu::RenderPipeline,
    paint: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    /// Flat triangles pulled towards the eye as lines are: sketches.
    marks: wgpu::RenderPipeline,
    /// Translucent faces behind nothing nearer: a compare's other side.
    ghost: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    samples: u32,
    reference: [f64; 3],
    bodies: BTreeMap<BodyKey, GpuBody>,
    /// A compare's ghosts, coloured for the compare generation they were
    /// uploaded at.
    ghosts: BTreeMap<BodyKey, GpuBody>,
    ghosts_at: u64,
    target: Option<Target>,
}

/// How opaque a ghost is.
pub const GHOST_ALPHA: f32 = ryucad::render::GHOST_ALPHA;

/// Vertices of a body's triangles, relative to `reference`.
fn face_vertices(
    m: &WireMesh,
    reference: [f64; 3],
    tris: &[[u32; 3]],
    color: [f32; 4],
) -> Vec<Vertex> {
    let offset = [0, 1, 2].map(|k| (m.origin[k] - reference[k]) as f32);
    let mut out = Vec::with_capacity(tris.len() * 3);
    for t in tris {
        for c in t {
            let (p, n) = m.corners[*c as usize];
            let q = m.positions[p as usize];
            out.push(Vertex {
                pos: [q[0] + offset[0], q[1] + offset[1], q[2] + offset[2]],
                normal: n,
                color,
            });
        }
    }
    out
}

fn edge_vertices(m: &WireMesh, reference: [f64; 3]) -> Vec<Vertex> {
    let offset = [0, 1, 2].map(|k| (m.origin[k] - reference[k]) as f32);
    let color = linear(theme::EDGE);
    let mut out = Vec::new();
    for (_, ps) in &m.edges {
        for w in ps.windows(2) {
            for i in w {
                let q = m.positions[*i as usize];
                out.push(Vertex {
                    pos: [q[0] + offset[0], q[1] + offset[1], q[2] + offset[2]],
                    normal: [0.0; 3],
                    color,
                });
            }
        }
    }
    out
}

fn buffer(device: &wgpu::Device, label: &str, v: &[Vertex]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(if v.is_empty() {
            &[Vertex {
                pos: [0.0; 3],
                normal: [0.0; 3],
                color: [0.0; 4],
            }]
        } else {
            v
        }),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

impl Scene {
    /// A scene for `device`, multisampled if `adapter` can.
    pub fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Scene {
        let can = |f: wgpu::TextureFormat| {
            adapter
                .get_texture_format_features(f)
                .flags
                .sample_count_supported(4)
        };
        let samples = if can(FORMAT) && can(DEPTH) { 4 } else { 1 };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: (&str, &str),
                        topology,
                        compare,
                        cull,
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry.0),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry.0),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    // Translucent faces hide nothing.
                    depth_write_enabled: Some(blend.is_none()),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry.1),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let back = Some(wgpu::Face::Back);
        Scene {
            faces: pipeline(
                ("vs_face", "fs_face"),
                tri,
                wgpu::CompareFunction::Less,
                back,
                None,
            ),
            // The same triangles again, so the same depths: equal passes.
            paint: pipeline(
                ("vs_face", "fs_face"),
                tri,
                wgpu::CompareFunction::LessEqual,
                back,
                None,
            ),
            lines: pipeline(
                ("vs_line", "fs_line"),
                wgpu::PrimitiveTopology::LineList,
                wgpu::CompareFunction::LessEqual,
                None,
                None,
            ),
            marks: pipeline(
                ("vs_line", "fs_line"),
                tri,
                wgpu::CompareFunction::LessEqual,
                None,
                None,
            ),
            // Pushed back a little (see `vs_ghost`): where the ghost and the
            // model are the same face, the model shows.
            ghost: pipeline(
                ("vs_ghost", "fs_face"),
                tri,
                wgpu::CompareFunction::Less,
                back,
                Some(wgpu::BlendState::ALPHA_BLENDING),
            ),
            uniforms,
            bind,
            samples,
            reference: [0.0; 3],
            bodies: BTreeMap::new(),
            ghosts: BTreeMap::new(),
            ghosts_at: u64::MAX,
            target: None,
        }
    }

    /// Uploads bodies whose mesh changed and drops those that are gone.
    /// A new `reference` uploads everything.
    pub fn sync(&mut self, device: &wgpu::Device, model: &Model, reference: [f64; 3]) {
        if reference != self.reference {
            self.bodies.clear();
            self.ghosts.clear();
            self.ghosts_at = u64::MAX;
            self.reference = reference;
        }
        self.bodies
            .retain(|k, b| model.meshes.get(k).is_some_and(|m| m.hash == b.hash));
        let surface = linear(theme::SURFACE);
        for (key, body) in &model.meshes {
            if self.bodies.contains_key(key) {
                continue;
            }
            let Ok(m) = &body.mesh else { continue };
            let faces = face_vertices(m, reference, &m.triangles, surface);
            let edges = edge_vertices(m, reference);
            self.bodies.insert(
                key.clone(),
                GpuBody {
                    hash: body.hash.clone(),
                    mesh: Arc::clone(body),
                    faces: buffer(device, "faces", &faces),
                    face_count: faces.len() as u32,
                    edges: buffer(device, "edges", &edges),
                    edge_count: edges.len() as u32,
                },
            );
        }
    }

    /// Uploads a compare's ghosts, each face translucent in the ghost
    /// colour or, gone from the working files, the removed one: again
    /// whenever the compare changes. Call it after [`Scene::sync`]; with
    /// no compare there are none.
    pub fn sync_ghosts(&mut self, device: &wgpu::Device, compare: Option<&Compare>) {
        let Some(c) = compare else {
            self.ghosts.clear();
            self.ghosts_at = u64::MAX;
            return;
        };
        if self.ghosts_at == c.generation {
            return;
        }
        self.ghosts.clear();
        self.ghosts_at = c.generation;
        let tint = |rgb: [f32; 3]| {
            let mut l = linear(rgb);
            l[3] = GHOST_ALPHA;
            l
        };
        let (ghost, gone) = (
            tint(crate::ui::style::GHOST),
            tint(crate::ui::style::REMOVED),
        );
        for (key, body) in &c.ghosts {
            let Ok(m) = &body.mesh else { continue };
            let mut faces = Vec::new();
            for f in &m.faces {
                let tris = &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize];
                let name = ryucad::mesh::name_hex(&f.name);
                let color = if c.is_gone(key, &name) { gone } else { ghost };
                faces.extend(face_vertices(m, self.reference, tris, color));
            }
            self.ghosts.insert(
                key.clone(),
                GpuBody {
                    hash: body.hash.clone(),
                    mesh: Arc::clone(body),
                    faces: buffer(device, "ghost", &faces),
                    face_count: faces.len() as u32,
                    edges: buffer(device, "ghost edges", &[]),
                    edge_count: 0,
                },
            );
        }
    }

    fn target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> &Target {
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let tex = |label, format, samples, usage, view_formats| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats,
                })
            };
            let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
            let plain = wgpu::TextureFormat::Rgba8Unorm;
            let plain_too = [plain];
            let resolved = tex(
                "scene",
                FORMAT,
                1,
                attach | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                &plain_too,
            );
            let color = (self.samples > 1).then(|| {
                tex("scene msaa", FORMAT, self.samples, attach, &[])
                    .create_view(&Default::default())
            });
            let depth = tex("scene depth", DEPTH, self.samples, attach, &[])
                .create_view(&Default::default());
            self.target = Some(Target {
                size,
                color,
                view: resolved.create_view(&Default::default()),
                shown: resolved.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(plain),
                    ..Default::default()
                }),
                resolved,
                depth,
            });
        }
        self.target.as_ref().expect("just made")
    }

    /// Draws `list` into the scene's texture, `size` pixels big, and
    /// returns a view of it to show in egui.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
        list: &DrawList,
    ) -> &wgpu::TextureView {
        let size = [size[0].max(1), size[1].max(1)];
        let u = Uniforms {
            matrix: list.matrix,
            light: [list.light[0], list.light[1], list.light[2], 0.0],
            flags: [1e-3, list.perspective[0], list.perspective[1], 0.0],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        // The painted faces, all in one buffer.
        let mut painted = Vec::new();
        for (key, face, color) in list.faces {
            let Some(b) = self.bodies.get(key) else {
                continue;
            };
            let Ok(m) = &b.mesh.mesh else { continue };
            let Some(f) = m.faces.get(*face) else {
                continue;
            };
            let tris = &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize];
            painted.extend(face_vertices(m, self.reference, tris, *color));
        }
        let paint = (!painted.is_empty()).then(|| buffer(device, "painted", &painted));
        let marks: Vec<Vertex> = list
            .overlay
            .iter()
            .map(|(p, c)| Vertex {
                pos: [0, 1, 2].map(|k| (p[k] - self.reference[k]) as f32),
                normal: [0.0; 3],
                color: linear(*c),
            })
            .collect();
        let marked = (!marks.is_empty()).then(|| buffer(device, "marks", &marks));
        self.target(device, size);
        let t = self.target.as_ref().expect("made above");
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let bg = linear(list.background);
            let (view, resolve) = match &t.color {
                Some(msaa) => (msaa, Some(&t.view)),
                None => (&t.view, None),
            };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(bg[0]),
                            g: f64::from(bg[1]),
                            b: f64::from(bg[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.faces);
            for b in self.bodies.values() {
                pass.set_vertex_buffer(0, b.faces.slice(..));
                pass.draw(0..b.face_count, 0..1);
            }
            if let Some(p) = &paint {
                pass.set_pipeline(&self.paint);
                pass.set_vertex_buffer(0, p.slice(..));
                pass.draw(0..painted.len() as u32, 0..1);
            }
            if list.edges {
                pass.set_pipeline(&self.lines);
                for b in self.bodies.values() {
                    pass.set_vertex_buffer(0, b.edges.slice(..));
                    pass.draw(0..b.edge_count, 0..1);
                }
            }
            if !self.ghosts.is_empty() {
                pass.set_pipeline(&self.ghost);
                for b in self.ghosts.values() {
                    pass.set_vertex_buffer(0, b.faces.slice(..));
                    pass.draw(0..b.face_count, 0..1);
                }
            }
            if let Some(m) = &marked {
                pass.set_pipeline(&self.marks);
                pass.set_vertex_buffer(0, m.slice(..));
                pass.draw(0..marks.len() as u32, 0..1);
            }
        }
        queue.submit([enc.finish()]);
        &self.target.as_ref().expect("made above").shown
    }

    /// The last frame's pixels, RGBA in rows top to bottom: for tests and
    /// the smoke run.
    pub fn read_pixels(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<(u32, u32, Vec<u8>)> {
        let t = self.target.as_ref()?;
        let [w, h] = t.size;
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            t.resolved.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            out.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        Some((w, h, out))
    }
}

/// A device for drawing without a window: tests and the smoke run. Any
/// adapter will do, software ones (lavapipe, WARP) included; `None` when
/// the machine has none.
pub fn headless() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ryucad-app headless"),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .ok()?;
    Some((adapter, device, queue))
}
