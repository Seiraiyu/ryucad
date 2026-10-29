//! The 3D view on the GPU: shaded faces and B-rep edges, drawn with wgpu
//! into a texture of the scene's own (with depth and multisampling), which
//! the window shows as an image and tests read back. Each body's mesh is
//! uploaded once per body hash, in its part's frame, and drawn by GPU
//! instancing: one draw per body with a buffer of the poses of every
//! occurrence showing it, relative to the scene's reference point, so a
//! thousand bolts are one mesh. Selections, highlights and hover paint
//! over faces from a small buffer built every frame, drawn at their
//! occurrences' poses, so moving the mouse never uploads the model again.
//! A body is uploaded once per look too ([`crate::core::appearance`]):
//! occurrences drawn alike share it, and a colour change uploads vertices
//! again but fetches no mesh. Translucent faces, and the edges they bound,
//! are drawn after everything else by weighted blended order-independent
//! transparency (McGuire and Bavoil): depth-tested against the opaque
//! scene without writing depth, accumulated into a float target and a
//! revealage target, then composited over the scene.

use crate::core::appearance::Look;
use crate::core::compare::Compare;
use crate::core::lod::Drawn;
use crate::core::model::{BodyKey, Model, Pose};
use crate::core::nav::Mat4;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use ryucad::mesh::{Quality, WireBody, WireMesh};
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
/// Translucent faces' accumulated colour, and how much they let through.
const ACCUM: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const REVEAL: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
}

/// Where one occurrence of a body is drawn: the matrix from its mesh's
/// coordinates (relative to the mesh's origin) to the scene's (relative to
/// its reference point), by columns.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub columns: [[f32; 4]; 4],
}

impl Instance {
    /// No turn, no move: what is already in the scene's coordinates.
    pub const IDENTITY: Instance = Instance {
        columns: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// A mesh whose positions are relative to `origin`, at `pose`, in a
    /// scene relative to `reference`. The translation is worked in f64
    /// and only the difference rounded to f32, so far from the world's
    /// origin positions stay precise.
    pub fn of(pose: &Pose, origin: [f64; 3], reference: [f64; 3]) -> Instance {
        let at = pose.apply(origin);
        let t = [0, 1, 2].map(|k| (at[k] - reference[k]) as f32);
        let c = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32, 0.0];
        Instance {
            columns: [c(pose.x), c(pose.y), c(pose.z), [t[0], t[1], t[2], 1.0]],
        }
    }
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
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
}

fn placed(v: VIn) -> vec4<f32> {
    return mat4x4<f32>(v.m0, v.m1, v.m2, v.m3) * vec4<f32>(v.pos, 1.0);
}

fn turned(v: VIn) -> vec3<f32> {
    return (mat4x4<f32>(v.m0, v.m1, v.m2, v.m3) * vec4<f32>(v.normal, 0.0)).xyz;
}
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
}

@vertex fn vs_face(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.matrix * placed(v);
    o.normal = turned(v);
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
    let c = u.matrix * placed(v);
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
    o.normal = turned(v);
    o.color = v.color;
    return o;
}

@fragment fn fs_line(i: VOut) -> @location(0) vec4<f32> {
    return i.color;
}

// Weighted blended order-independent transparency: each translucent
// fragment adds its premultiplied colour, weighted by its opacity and
// depth, to `accum`, and multiplies `reveal` by what it lets through.
struct Oit { @location(0) accum: vec4<f32>, @location(1) reveal: vec4<f32> }

fn oit(c: vec4<f32>, z: f32) -> Oit {
    let a = c.a;
    let w = clamp(a * max(1e-2, 3e3 * pow(1.0 - z, 3.0)), 1e-2, 3e3);
    var o: Oit;
    o.accum = vec4<f32>(c.rgb * a, a) * w;
    o.reveal = vec4<f32>(a, a, a, a);
    return o;
}

@fragment fn fs_glass(i: VOut) -> Oit {
    let n = normalize(i.normal);
    let lit = 0.3 + 0.7 * max(dot(n, u.light.xyz), 0.0);
    return oit(vec4<f32>(i.color.rgb * lit, i.color.a), i.clip.z);
}

@fragment fn fs_glass_line(i: VOut) -> Oit {
    return oit(i.color, i.clip.z);
}

@vertex fn vs_ghost(v: VIn) -> VOut {
    var o: VOut;
    let c = u.matrix * placed(v);
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
    o.normal = turned(v);
    o.color = v.color;
    return o;
}
"#;

/// Composites the translucent faces' targets over the scene.
const COMPOSITE: &str = r#"
@group(0) @binding(0) var accum_t: texture_2d<f32>;
@group(0) @binding(1) var reveal_t: texture_2d<f32>;

@vertex fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment fn fs_composite(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(p.xy);
    let r = textureLoad(reveal_t, at, 0).r;
    if r >= 1.0 {
        discard;
    }
    let a = textureLoad(accum_t, at, 0);
    return vec4<f32>(a.rgb / clamp(a.a, 1e-4, 5e4), 1.0 - r);
}
"#;

/// What a body is uploaded as: its hash, its level of detail and its
/// look's ID (0 for the plain look).
type Upload = (String, Quality, u64);

/// An occurrence placed for this frame: its key, its upload, where it
/// goes, and its mesh at the user's display quality.
type Placed = (BodyKey, Upload, Instance, Arc<WireBody>);

/// A body on the GPU: its mesh in its part's frame, relative to the
/// mesh's own origin.
struct GpuBody {
    mesh: Arc<WireBody>,
    faces: wgpu::Buffer,
    face_count: u32,
    edges: wgpu::Buffer,
    edge_count: u32,
    /// Its translucent faces, and the edges only they bound.
    glass: Option<Glass>,
}

/// A body's translucent faces and their edges, each vertex's alpha its
/// opacity.
struct Glass {
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
    /// Translucent faces' accumulated colour and revealage: multisampled
    /// views (when the scene is), what they resolve to, and the
    /// composite's bind group over those.
    accum: Option<wgpu::TextureView>,
    accum_resolved: wgpu::TextureView,
    reveal: Option<wgpu::TextureView>,
    reveal_resolved: wgpu::TextureView,
    composite: wgpu::BindGroup,
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
    /// Translucent faces and their edges, into the transparency targets.
    glass: wgpu::RenderPipeline,
    glass_lines: wgpu::RenderPipeline,
    /// The transparency targets over the scene.
    composite: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    samples: u32,
    reference: [f64; 3],
    /// Bodies by hash, level of detail and look, each uploaded once.
    bodies: BTreeMap<Upload, GpuBody>,
    /// How bodies look, by key: [`Scene::set_looks`].
    looks: Arc<BTreeMap<BodyKey, Arc<Look>>>,
    /// This frame's occurrences: each drawn key, its body and level,
    /// where it goes, and its mesh at the user's display quality.
    placed: Vec<Placed>,
    /// One identity instance, for what is drawn where it is.
    identity: wgpu::Buffer,
    /// A compare's ghosts, coloured for the compare generation they were
    /// uploaded at.
    ghosts: BTreeMap<BodyKey, GpuBody>,
    ghosts_at: u64,
    target: Option<Target>,
}

/// How opaque a ghost is.
pub const GHOST_ALPHA: f32 = ryucad::render::GHOST_ALPHA;

/// Vertices of a body's triangles, moved by `offset`: none for a mesh
/// drawn at its instances, or from its origin to the scene's reference.
fn face_vertices(
    m: &WireMesh,
    offset: [f32; 3],
    tris: &[[u32; 3]],
    color: [f32; 4],
) -> Vec<Vertex> {
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

/// Vertices made relative to `reference` in their part's frame, moved to
/// `pose`: worked in f64, so a ghost far from the reference stays
/// precise.
fn place(vs: &mut [Vertex], pose: &Pose, reference: [f64; 3]) {
    for v in vs {
        let local = [0, 1, 2].map(|k| f64::from(v.pos[k]) + reference[k]);
        let w = pose.apply(local);
        v.pos = [0, 1, 2].map(|k| (w[k] - reference[k]) as f32);
        let n = pose.turn(v.normal.map(f64::from));
        v.normal = n.map(|x| x as f32);
    }
}

fn edge_vertices(m: &WireMesh, offset: [f32; 3]) -> Vec<Vertex> {
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

/// A body's vertices as `look` draws it: its opaque faces and edges,
/// then its translucent faces and the edges only translucent faces bound,
/// each edge as opaque as the most opaque face it bounds.
fn look_vertices(m: &WireMesh, look: Option<&Look>) -> [Vec<Vertex>; 4] {
    let plain = crate::core::appearance::Face::default();
    let face_of = |i: usize| match look {
        Some(l) => l.face(&ryucad::mesh::name_hex(&m.faces[i].name)),
        None => plain,
    };
    let (mut faces, mut glass) = (Vec::new(), Vec::new());
    for (i, f) in m.faces.iter().enumerate() {
        let look = face_of(i);
        let mut color = linear(look.colour.unwrap_or(theme::SURFACE));
        color[3] = look.opacity;
        let tris = &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize];
        let out = if look.translucent() {
            &mut glass
        } else {
            &mut faces
        };
        out.extend(face_vertices(m, [0.0; 3], tris, color));
    }
    let edges = edge_vertices(m, [0.0; 3]);
    if glass.is_empty() {
        return [faces, edges, glass, Vec::new()];
    }
    // The faces at each position: an edge's faces are those at all of its
    // samples.
    let mut at: Vec<Vec<u32>> = vec![Vec::new(); m.positions.len()];
    for (i, f) in m.faces.iter().enumerate() {
        for c in &m.corners[f.corners.0 as usize..f.corners.1 as usize] {
            let list = &mut at[c.0 as usize];
            if list.last() != Some(&(i as u32)) && !list.contains(&(i as u32)) {
                list.push(i as u32);
            }
        }
    }
    let (mut solid, mut faint) = (Vec::new(), Vec::new());
    let mut k = 0;
    for (_, ps) in &m.edges {
        let mut shared: Vec<u32> = ps
            .first()
            .map(|p| at[*p as usize].clone())
            .unwrap_or_default();
        for p in ps {
            shared.retain(|f| at[*p as usize].contains(f));
        }
        let alpha = shared
            .iter()
            .map(|f| face_of(*f as usize).opacity)
            .fold(0.0f32, f32::max);
        let n = 2 * ps.len().saturating_sub(1);
        let mut vs = edges[k..k + n].to_vec();
        k += n;
        if alpha > 0.0 && alpha < 1.0 {
            for v in &mut vs {
                v.color[3] = alpha;
            }
            faint.extend(vs);
        } else {
            solid.extend(vs);
        }
    }
    [faces, solid, glass, faint]
}

fn instances(device: &wgpu::Device, label: &str, v: &[Instance]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(if v.is_empty() {
            &[Instance::IDENTITY]
        } else {
            v
        }),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

/// From a mesh's origin to the scene's reference point.
fn offset(m: &WireMesh, reference: [f64; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| (m.origin[k] - reference[k]) as f32)
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
        let resolves = |f: wgpu::TextureFormat| {
            adapter
                .get_texture_format_features(f)
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE)
        };
        let samples = if can(FORMAT)
            && can(DEPTH)
            && can(ACCUM)
            && can(REVEAL)
            && resolves(ACCUM)
            && resolves(REVEAL)
        {
            4
        } else {
            1
        };
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
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: size_of::<Instance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4],
                        }),
                    ],
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
        // Translucent faces and edges: depth-tested, writing no depth, into
        // the accumulation (added) and revealage (multiplied) targets.
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let through = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        };
        let oit = |entry: (&str, &str), topology, compare, cull| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry.1),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry.0),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: size_of::<Instance>() as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4],
                        }),
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(false),
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
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: ACCUM,
                            blend: Some(wgpu::BlendState {
                                color: add,
                                alpha: add,
                            }),
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: REVEAL,
                            blend: Some(wgpu::BlendState {
                                color: through,
                                alpha: through,
                            }),
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE.into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[texture(0), texture(1)],
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("composite"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Scene {
            glass: oit(
                ("vs_face", "fs_glass"),
                tri,
                wgpu::CompareFunction::Less,
                back,
            ),
            glass_lines: oit(
                ("vs_line", "fs_glass_line"),
                wgpu::PrimitiveTopology::LineList,
                wgpu::CompareFunction::LessEqual,
                None,
            ),
            composite,
            composite_layout,
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
            looks: Arc::new(BTreeMap::new()),
            placed: Vec::new(),
            identity: instances(device, "identity", &[Instance::IDENTITY]),
            ghosts: BTreeMap::new(),
            ghosts_at: u64::MAX,
            target: None,
        }
    }

    /// Draws everything the model draws, at the user's display quality,
    /// culling nothing: [`Scene::sync_drawn`] of all of it.
    pub fn sync(&mut self, device: &wgpu::Device, model: &Model, reference: [f64; 3]) {
        let all: Vec<Drawn> = model
            .drawn()
            .map(|(key, mesh, pose)| Drawn {
                key: key.clone(),
                mesh: Arc::clone(mesh),
                level: model.base,
                base: Arc::clone(mesh),
                pose,
            })
            .collect();
        self.set_looks(&model.looks);
        self.sync_drawn(device, &all, reference);
    }

    /// Takes how bodies look, by key: bodies drawn differently from now
    /// are uploaded again at the next [`Scene::sync_drawn`].
    pub fn set_looks(&mut self, looks: &Arc<BTreeMap<BodyKey, Arc<Look>>>) {
        if !Arc::ptr_eq(&self.looks, looks) {
            self.looks = Arc::clone(looks);
        }
    }

    /// Uploads the bodies of `drawn` (at their levels and looks) that
    /// aren't on the GPU yet, drops those no longer drawn, and places
    /// every occurrence for this frame. A new `reference` uploads the
    /// ghosts again.
    pub fn sync_drawn(&mut self, device: &wgpu::Device, drawn: &[Drawn], reference: [f64; 3]) {
        if reference != self.reference {
            self.ghosts.clear();
            self.ghosts_at = u64::MAX;
            self.reference = reference;
        }
        self.placed.clear();
        for d in drawn {
            let (key, body) = (&d.key, &d.mesh);
            let Ok(m) = &body.mesh else { continue };
            let look = self.looks.get(key);
            let at = (body.hash.clone(), d.level, look.map_or(0, |l| l.id));
            let inst = Instance::of(&d.pose, m.origin, reference);
            self.placed
                .push((key.clone(), at.clone(), inst, Arc::clone(&d.base)));
            if self.bodies.contains_key(&at) {
                continue;
            }
            let [faces, edges, glass, faint] = look_vertices(m, look.map(|l| &**l));
            let glass = (!glass.is_empty()).then(|| Glass {
                faces: buffer(device, "glass", &glass),
                face_count: glass.len() as u32,
                edges: buffer(device, "glass edges", &faint),
                edge_count: faint.len() as u32,
            });
            self.bodies.insert(
                at,
                GpuBody {
                    mesh: Arc::clone(body),
                    faces: buffer(device, "faces", &faces),
                    face_count: faces.len() as u32,
                    edges: buffer(device, "edges", &edges),
                    edge_count: edges.len() as u32,
                    glass,
                },
            );
        }
        let kept: std::collections::BTreeSet<&Upload> =
            self.placed.iter().map(|(_, at, ..)| at).collect();
        self.bodies.retain(|at, _| kept.contains(at));
    }

    /// How many bodies are on the GPU, and how many occurrences of them
    /// this frame draws.
    pub fn counts(&self) -> (usize, usize) {
        (self.bodies.len(), self.placed.len())
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
            let pose = c.ghost_pose(key);
            let mut faces = Vec::new();
            for f in &m.faces {
                let tris = &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize];
                let name = ryucad::mesh::name_hex(&f.name);
                let color = if c.is_gone(key, &name) { gone } else { ghost };
                faces.extend(face_vertices(m, offset(m, self.reference), tris, color));
            }
            // An occurrence's ghost where it was (issue #109).
            if pose != Pose::IDENTITY {
                place(&mut faces, &pose, self.reference);
            }
            self.ghosts.insert(
                key.clone(),
                GpuBody {
                    mesh: Arc::clone(body),
                    faces: buffer(device, "ghost", &faces),
                    face_count: faces.len() as u32,
                    edges: buffer(device, "ghost edges", &[]),
                    edge_count: 0,
                    glass: None,
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
            // The transparency targets: multisampled like the scene, so the
            // translucent faces are depth-tested against its depth, and
            // resolved for the composite to read.
            let read = attach | wgpu::TextureUsages::TEXTURE_BINDING;
            let accum_resolved = tex("accum", ACCUM, 1, read, &[]).create_view(&Default::default());
            let reveal_resolved =
                tex("reveal", REVEAL, 1, read, &[]).create_view(&Default::default());
            let multi = |label, format| {
                (self.samples > 1).then(|| {
                    tex(label, format, self.samples, attach, &[]).create_view(&Default::default())
                })
            };
            let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&accum_resolved),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&reveal_resolved),
                    },
                ],
            });
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
                accum: multi("accum msaa", ACCUM),
                accum_resolved,
                reveal: multi("reveal msaa", REVEAL),
                reveal_resolved,
                composite,
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
        self.target(device, size);
        // Every occurrence of a body together, in one instance buffer.
        let mut order: Vec<usize> = (0..self.placed.len()).collect();
        order.sort_by(|a, b| self.placed[*a].1.cmp(&self.placed[*b].1));
        let mut runs: Vec<(&Upload, &GpuBody, core::ops::Range<u32>)> = Vec::new();
        let mut all = Vec::with_capacity(order.len());
        for i in order {
            let (_, at, inst, _) = &self.placed[i];
            let Some(b) = self.bodies.get(at) else {
                continue;
            };
            let n = all.len() as u32;
            match runs.last_mut() {
                Some((last, _, r)) if *last == at => r.end = n + 1,
                _ => runs.push((at, b, n..n + 1)),
            }
            all.push(*inst);
        }
        let glassy = runs.iter().any(|(_, b, _)| b.glass.is_some());
        let placed = instances(device, "occurrences", &all);
        // The painted faces, all in one buffer, each drawn at its
        // occurrence: the same positions and matrix as the face itself, so
        // the same depths.
        let mut painted = Vec::new();
        let mut paint_runs: Vec<(core::ops::Range<u32>, u32)> = Vec::new();
        let mut paint_at = Vec::new();
        for (key, face, color) in list.faces {
            let Some((_, at, inst, base)) = self.placed.iter().find(|(k, ..)| k == key) else {
                continue;
            };
            let Some(b) = self.bodies.get(at) else {
                continue;
            };
            let Ok(m) = &b.mesh.mesh else { continue };
            // Faces are counted in the base mesh; the level drawn may
            // leave out another faulted face, so find it by name.
            let f = match &base.mesh {
                Ok(bm) if !Arc::ptr_eq(base, &b.mesh) => bm
                    .faces
                    .get(*face)
                    .and_then(|bf| m.faces.iter().find(|f| f.name == bf.name)),
                _ => m.faces.get(*face),
            };
            let Some(f) = f else {
                continue;
            };
            let tris = &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize];
            let start = painted.len() as u32;
            painted.extend(face_vertices(m, [0.0; 3], tris, *color));
            paint_runs.push((start..painted.len() as u32, paint_at.len() as u32));
            paint_at.push(*inst);
        }
        let paint = (!painted.is_empty()).then(|| {
            (
                buffer(device, "painted", &painted),
                instances(device, "painted at", &paint_at),
            )
        });
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
                        // Translucent faces are tested against it after.
                        store: if glassy {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.faces);
            pass.set_vertex_buffer(1, placed.slice(..));
            for (_, b, r) in &runs {
                pass.set_vertex_buffer(0, b.faces.slice(..));
                pass.draw(0..b.face_count, r.clone());
            }
            if let Some((p, at)) = &paint {
                pass.set_pipeline(&self.paint);
                pass.set_vertex_buffer(0, p.slice(..));
                pass.set_vertex_buffer(1, at.slice(..));
                for (r, i) in &paint_runs {
                    pass.draw(r.clone(), *i..*i + 1);
                }
            }
            if list.edges {
                pass.set_pipeline(&self.lines);
                pass.set_vertex_buffer(1, placed.slice(..));
                for (_, b, r) in &runs {
                    pass.set_vertex_buffer(0, b.edges.slice(..));
                    pass.draw(0..b.edge_count, r.clone());
                }
            }
            pass.set_vertex_buffer(1, self.identity.slice(..));
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
        if glassy {
            // The translucent faces and their edges, accumulated...
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("translucent"),
                color_attachments: &[
                    attach(&t.accum, &t.accum_resolved, wgpu::Color::TRANSPARENT),
                    attach(&t.reveal, &t.reveal_resolved, wgpu::Color::WHITE),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(1, placed.slice(..));
            pass.set_pipeline(&self.glass);
            for (_, b, r) in &runs {
                if let Some(g) = &b.glass {
                    pass.set_vertex_buffer(0, g.faces.slice(..));
                    pass.draw(0..g.face_count, r.clone());
                }
            }
            if list.edges {
                pass.set_pipeline(&self.glass_lines);
                for (_, b, r) in &runs {
                    if let Some(g) = b.glass.as_ref().filter(|g| g.edge_count > 0) {
                        pass.set_vertex_buffer(0, g.edges.slice(..));
                        pass.draw(0..g.edge_count, r.clone());
                    }
                }
            }
            drop(pass);
            // ...then laid over the scene.
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.composite);
            pass.set_bind_group(0, &t.composite, &[]);
            pass.draw(0..3, 0..1);
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

/// A transparency target cleared to `clear`: drawn multisampled and
/// resolved when `ms` is there, else drawn as it is.
fn attach<'a>(
    ms: &'a Option<wgpu::TextureView>,
    resolved: &'a wgpu::TextureView,
    clear: wgpu::Color,
) -> Option<wgpu::RenderPassColorAttachment<'a>> {
    let (view, resolve_target) = match ms {
        Some(m) => (m, Some(resolved)),
        None => (resolved, None),
    };
    Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(clear),
            store: wgpu::StoreOp::Store,
        },
    })
}

/// A device for drawing without a window: tests and the smoke run. The
/// window's choice ([`crate::ui::platform::choose`]): an adapter the scene
/// can use, hardware first, software ones (lavapipe, WARP) included;
/// `None` when the machine has none.
pub fn headless() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let adapter = crate::ui::platform::choose(&adapters, None).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ryucad-app headless"),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .ok()?;
    Some((adapter, device, queue))
}
