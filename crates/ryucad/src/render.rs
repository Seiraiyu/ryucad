//! Offscreen renders for agents: a small CPU rasterizer, so a render works
//! wherever the CLI does (static musl, containers, CI) with no GPU and no
//! drivers, and gives the same pixels on every platform.
//!
//! The image is drawn at twice its size and averaged down (4 samples a
//! pixel), in horizontal bands that are independent of each other, so
//! bands run in parallel and the result doesn't depend on the thread
//! count. Pixel arithmetic is f32 with no transcendental functions; the
//! view set-up is f64 through `ryukei::math`. Ghosts are blended after the
//! model and its silhouettes, nearest ghost first, so their order doesn't
//! matter.

use crate::error::{Code, Error, Result};
use crate::view::{Camera, Projection};
use rayon::prelude::*;

/// The largest render, in pixels on a side.
pub const MAX_SIZE: u32 = 4096;
/// Supersampling factor on each axis.
const SS: usize = 2;
/// Output rows per band.
const BAND: usize = 32;

pub type Rgb = [f32; 3];

pub const BACKGROUND: Rgb = [0.961, 0.965, 0.973];
pub const SURFACE: Rgb = [0.78, 0.80, 0.84];
pub const EDGE: Rgb = [0.12, 0.13, 0.15];
/// Highlight colours: about to change, just changed, pointed out.
pub const PENDING: Rgb = [1.0, 0.70, 0.22];
pub const CHANGED: Rgb = [0.33, 0.78, 0.42];
pub const NOTE: Rgb = [0.30, 0.58, 1.0];
/// A compare's colours (the app's too): faces only the rendered side has,
/// faces on both sides whose geometry differs, faces only the other side
/// has (in its ghost), and the rest of the ghost.
pub const COMPARE_NEW: Rgb = [0.30, 0.72, 0.38];
pub const COMPARE_CHANGED: Rgb = [0.96, 0.66, 0.18];
pub const COMPARE_REMOVED: Rgb = [0.90, 0.28, 0.28];
pub const COMPARE_GHOST: Rgb = [0.55, 0.60, 0.72];
/// How much of a ghost shows over what it covers.
pub const GHOST_ALPHA: f32 = 0.35;
/// How far a ghost is pushed back, as a fraction of its depth, so a face
/// it shares with the model shows the model.
const GHOST_PUSH: f32 = 1e-3;
/// Sketch colours by solve state: nothing can move, something can, some
/// constraint is redundant, some constraint can't hold.
pub const SKETCH_WELL: Rgb = [0.08, 0.09, 0.11];
pub const SKETCH_UNDER: Rgb = [0.13, 0.38, 0.82];
pub const SKETCH_OVER: Rgb = [0.86, 0.16, 0.16];
pub const SKETCH_FAILING: Rgb = [0.72, 0.12, 0.62];
const AMBIENT: f32 = 0.3;
const DIFFUSE: f32 = 0.7;

/// What to draw, in model space (meters).
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<Triangle>,
    /// Drawn over the surfaces they lie on; a line of one point is a dot.
    pub lines: Vec<Line>,
    pub labels: Vec<Label>,
    /// Drawn translucent over the rest, where nothing of the model is in
    /// front of them: the nearest ghost at each sample, so the order of
    /// ghosts doesn't matter.
    pub ghosts: Vec<Triangle>,
}

/// Text drawn at a model point, on top of everything.
#[derive(Clone, Debug)]
pub struct Label {
    pub at: [f64; 3],
    pub text: String,
}

/// What else to draw.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Silhouettes, where a surface meets the background or passes in
    /// front of another.
    pub silhouettes: bool,
    /// The axis triad in the bottom left corner.
    pub triad: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            silhouettes: true,
            triad: true,
        }
    }
}

/// A triangle of positions, counterclockwise from outside, with a normal
/// per corner and a colour.
#[derive(Clone, Debug)]
pub struct Triangle {
    pub p: [u32; 3],
    pub n: [[f32; 3]; 3],
    pub color: Rgb,
}

/// A polyline of positions drawn `width` output pixels wide, dashed if
/// `dash`.
#[derive(Clone, Debug)]
pub struct Line {
    pub points: Vec<u32>,
    pub color: Rgb,
    pub width: f32,
    pub dash: bool,
}

/// The origin's axes, x, y and z, as the triad colours them, and its
/// planes.
pub const AXIS_X: Rgb = [0.84, 0.19, 0.19];
pub const AXIS_Y: Rgb = [0.15, 0.68, 0.38];
pub const AXIS_Z: Rgb = [0.16, 0.38, 1.0];
pub const ORIGIN_PLANE: Rgb = [0.45, 0.50, 0.58];

/// How long the origin's axes are: a tenth of the diagonal of the box of
/// `positions`, or 5 mm when there are none.
pub fn origin_size(positions: &[[f64; 3]]) -> f64 {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let d = (0..3)
        .map(|k| (hi[k] - lo[k]).max(0.0))
        .map(|x| x * x)
        .sum::<f64>()
        .sqrt();
    if d.is_finite() && d > 0.0 {
        d / 10.0
    } else {
        0.005
    }
}

/// The origin's axes and planes in model space, `size` long: each axis a
/// line from the origin, each plane a dashed square about it, three
/// quarters of `size` from the origin each way.
pub fn origin_lines(size: f64) -> (Vec<[f64; 3]>, Vec<(Vec<u32>, Rgb, bool)>) {
    let mut positions = vec![[0.0; 3]];
    let mut lines = Vec::new();
    for (k, c) in [AXIS_X, AXIS_Y, AXIS_Z].into_iter().enumerate() {
        let mut tip = [0.0; 3];
        tip[k] = size;
        positions.push(tip);
        lines.push((vec![0, positions.len() as u32 - 1], c, false));
    }
    let h = 0.75 * size;
    for (a, b) in [(0, 1), (1, 2), (2, 0)] {
        let start = positions.len() as u32;
        for (sa, sb) in [(-h, -h), (h, -h), (h, h), (-h, h)] {
            let mut p = [0.0; 3];
            p[a] = sa;
            p[b] = sb;
            positions.push(p);
        }
        lines.push((
            vec![start, start + 1, start + 2, start + 3, start],
            ORIGIN_PLANE,
            true,
        ));
    }
    (positions, lines)
}

impl Geometry {
    /// Adds the origin's axes and planes, sized to what is drawn so far.
    pub fn add_origin(&mut self) {
        let size = origin_size(&self.positions);
        let (positions, lines) = origin_lines(size);
        let base = self.positions.len() as u32;
        self.positions.extend(positions);
        for (points, color, dash) in lines {
            self.lines.push(Line {
                points: points.into_iter().map(|i| base + i).collect(),
                color,
                width: if dash { 1.0 } else { 2.0 },
                dash,
            });
        }
    }
}

/// A dashed line's dashes and the gaps between them, in output pixels.
pub const DASH: [f32; 2] = [6.0, 4.0];

/// A sketch's colour for an entity in `state`.
pub fn sketch_color(state: crate::sketch_view::EntityState) -> Rgb {
    use crate::sketch_view::EntityState as S;
    match state {
        S::Well => SKETCH_WELL,
        S::Under => SKETCH_UNDER,
        S::Over => SKETCH_OVER,
        S::Failing => SKETCH_FAILING,
    }
}

/// An RGBA image, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The image as PNG bytes.
    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("writing to memory");
        w.write_image_data(&self.rgba).expect("sizes match");
        w.finish().expect("writing to memory");
        out
    }
}

/// Where each model position lands: supersampled pixel coordinates, view
/// depth (distance along the view direction), and `w`, what attributes
/// are divided by to interpolate them correctly (the depth in
/// perspective, 1 in orthographic).
#[derive(Clone, Copy, Debug)]
struct Screen {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

/// The view's basis and projection, in f64.
pub struct View {
    eye: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    forward: [f64; 3],
    /// Half the view's height at unit depth (perspective), or half its
    /// height (orthographic).
    half: f64,
    perspective: bool,
    width: f64,
    height: f64,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    a.map(|x| x / n)
}

/// The nearest depth drawn: anything closer to the eye is left out.
const NEAR: f64 = 1e-6;

impl View {
    /// The view of `camera` on an image `width`×`height` (output pixels).
    pub fn new(camera: &Camera, width: u32, height: u32) -> View {
        let forward = unit(sub(camera.target, camera.eye));
        let right = unit(cross(forward, camera.up));
        let up = cross(right, forward);
        let (half, perspective) = match camera.projection {
            Projection::Perspective { fov_deg } => (
                ryukei::math::tan(fov_deg * core::f64::consts::PI / 360.0),
                true,
            ),
            Projection::Orthographic { height } => (height / 2.0, false),
        };
        View {
            eye: camera.eye,
            right,
            up,
            forward,
            half,
            perspective,
            width: (width as usize * SS) as f64,
            height: (height as usize * SS) as f64,
        }
    }

    /// A model point in supersampled pixels, or `None` behind the eye.
    fn project(&self, p: [f64; 3]) -> Option<Screen> {
        let rel = sub(p, self.eye);
        let (x, y, z) = (
            dot(rel, self.right),
            dot(rel, self.up),
            dot(rel, self.forward),
        );
        let scale = if self.perspective {
            if z <= NEAR {
                return None;
            }
            self.half * z
        } else {
            self.half
        };
        // The view's height spans [-1, 1]; its width follows the aspect.
        let sy = y / scale;
        let sx = x / scale * self.height / self.width;
        Some(Screen {
            x: ((sx + 1.0) / 2.0 * self.width) as f32,
            y: ((1.0 - sy) / 2.0 * self.height) as f32,
            z: z as f32,
            w: if self.perspective { z as f32 } else { 1.0 },
        })
    }

    /// A model point in output pixels, if in front of the eye.
    pub fn pixel(&self, p: [f64; 3]) -> Option<(f32, f32)> {
        self.project(p).map(|s| (s.x / SS as f32, s.y / SS as f32))
    }

    /// A model direction (not a point) in output pixels, from the image's
    /// origin: for the axis triad.
    pub fn direction(&self, d: [f64; 3]) -> (f32, f32) {
        (dot(d, self.right) as f32, -dot(d, self.up) as f32)
    }

    /// The light: from over the viewer's left shoulder, in model space.
    fn light(&self) -> [f32; 3] {
        let l = unit([0, 1, 2].map(|k| -0.4 * self.right[k] + 0.5 * self.up[k] - self.forward[k]));
        l.map(|x| x as f32)
    }
}

/// Checks a requested size.
pub fn check_size(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
        return Err(Error::new(
            Code::RenderTooLarge,
            format!("a render is 1 to {MAX_SIZE} pixels on a side"),
        )
        .with("width", width)
        .with("height", height));
    }
    Ok(())
}

/// A band of supersampled rows: colour and depth per sample.
struct Band {
    /// The first supersampled row: one above the band's own rows, except
    /// at the top, so silhouettes can look across the band's edge.
    y0: usize,
    rows: usize,
    width: usize,
    color: Vec<Rgb>,
    depth: Vec<f32>,
    /// The nearest ghost's colour and depth at each sample; empty when
    /// there are no ghosts.
    ghost: Vec<Rgb>,
    ghost_depth: Vec<f32>,
}

impl Band {
    fn new(y0: usize, rows: usize, width: usize) -> Band {
        Band {
            y0,
            rows,
            width,
            color: vec![BACKGROUND; rows * width],
            depth: vec![f32::INFINITY; rows * width],
            ghost: Vec::new(),
            ghost_depth: Vec::new(),
        }
    }

    /// Calls `put(band, sample, depth, light)` for every sample a
    /// front-facing triangle covers.
    fn fill(
        &mut self,
        s: [Screen; 3],
        shade: [f32; 3],
        mut put: impl FnMut(&mut Band, usize, f32, f32),
    ) {
        // Counterclockwise from outside stays counterclockwise on screen,
        // which with y down has negative area: anything else faces away
        // or is edge-on, and isn't drawn. Swapping two corners makes the
        // area positive for the tests below.
        let area = (s[1].x - s[0].x) * (s[2].y - s[0].y) - (s[2].x - s[0].x) * (s[1].y - s[0].y);
        if area >= 0.0 {
            return;
        }
        let (s, shade, area) = ([s[0], s[2], s[1]], [shade[0], shade[2], shade[1]], -area);
        let min_y = s.iter().map(|v| v.y).fold(f32::INFINITY, f32::min);
        let max_y = s.iter().map(|v| v.y).fold(f32::NEG_INFINITY, f32::max);
        let min_x = s.iter().map(|v| v.x).fold(f32::INFINITY, f32::min);
        let max_x = s.iter().map(|v| v.x).fold(f32::NEG_INFINITY, f32::max);
        let y_lo = (min_y.floor().max(self.y0 as f32)) as usize;
        let y_hi = (max_y.ceil().min((self.y0 + self.rows) as f32)).max(0.0) as usize;
        let x_lo = min_x.floor().max(0.0) as usize;
        let x_hi = (max_x.ceil().min(self.width as f32)).max(0.0) as usize;
        let inv = [1.0 / s[0].w, 1.0 / s[1].w, 1.0 / s[2].w];
        for y in y_lo..y_hi {
            let py = y as f32 + 0.5;
            for x in x_lo..x_hi {
                let px = x as f32 + 0.5;
                let edge =
                    |a: Screen, b: Screen| (b.x - a.x) * (py - a.y) - (b.y - a.y) * (px - a.x);
                let w0 = edge(s[1], s[2]);
                let w1 = edge(s[2], s[0]);
                let w2 = edge(s[0], s[1]);
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let (b0, b1, b2) = (w0 / area, w1 / area, w2 / area);
                // Attributes over w interpolate linearly on screen.
                let iw = b0 * inv[0] + b1 * inv[1] + b2 * inv[2];
                let lerp = |a: [f32; 3]| {
                    (b0 * a[0] * inv[0] + b1 * a[1] * inv[1] + b2 * a[2] * inv[2]) / iw
                };
                let z = lerp([s[0].z, s[1].z, s[2].z]);
                let i = (y - self.y0) * self.width + x;
                put(self, i, z, lerp(shade));
            }
        }
    }

    fn triangle(&mut self, s: [Screen; 3], shade: [f32; 3], color: Rgb) {
        self.fill(s, shade, |b, i, z, light| {
            if z < b.depth[i] {
                b.depth[i] = z;
                b.color[i] = color.map(|c| c * light);
            }
        });
    }

    /// A ghost triangle: kept where it is the nearest ghost and nothing
    /// of the model is in front of it, pushed back a little.
    fn ghost_triangle(&mut self, s: [Screen; 3], shade: [f32; 3], color: Rgb) {
        self.fill(s, shade, |b, i, z, light| {
            let z = z + GHOST_PUSH * z.abs();
            if z < b.depth[i] && z < b.ghost_depth[i] {
                b.ghost_depth[i] = z;
                b.ghost[i] = color.map(|c| c * light);
            }
        });
    }

    /// Mixes each sample's nearest ghost into it.
    fn blend_ghosts(&mut self) {
        for (i, gz) in self.ghost_depth.iter().enumerate() {
            if gz.is_finite() {
                let (c, g) = (self.color[i], self.ghost[i]);
                self.color[i] = [0, 1, 2].map(|k| c[k] + GHOST_ALPHA * (g[k] - c[k]));
            }
        }
    }
}

impl Band {
    /// A line segment `width` samples wide, drawn where it isn't behind a
    /// surface (with a little slack, so lines on a surface show).
    fn segment(&mut self, a: Screen, b: Screen, half: f32, color: Rgb) {
        let lo_y = (a.y.min(b.y) - half).floor().max(self.y0 as f32) as usize;
        let hi_y = ((a.y.max(b.y) + half)
            .ceil()
            .min((self.y0 + self.rows) as f32))
        .max(0.0) as usize;
        let lo_x = (a.x.min(b.x) - half).floor().max(0.0) as usize;
        let hi_x = ((a.x.max(b.x) + half).ceil().min(self.width as f32)).max(0.0) as usize;
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        for y in lo_y..hi_y {
            let py = y as f32 + 0.5;
            for x in lo_x..hi_x {
                let px = x as f32 + 0.5;
                let t = if len2 > 0.0 {
                    (((px - a.x) * dx + (py - a.y) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (cx, cy) = (a.x + t * dx - px, a.y + t * dy - py);
                if cx * cx + cy * cy > half * half {
                    continue;
                }
                // Depth along the segment, interpolated over w.
                let iw = (1.0 - t) / a.w + t / b.w;
                let z = ((1.0 - t) * a.z / a.w + t * b.z / b.w) / iw;
                let i = (y - self.y0) * self.width + x;
                if z > self.depth[i] + 4e-3 * z.abs() + 1e-9 {
                    continue;
                }
                self.color[i] = color;
            }
        }
    }
}

/// The point a fraction `t` along `a`..`b` on screen, its depth
/// interpolated over w as [`Band::segment`] does.
fn between(a: Screen, b: Screen, t: f32) -> Screen {
    let iw = (1.0 - t) / a.w + t / b.w;
    Screen {
        x: a.x + t * (b.x - a.x),
        y: a.y + t * (b.y - a.y),
        z: ((1.0 - t) * a.z / a.w + t * b.z / b.w) / iw,
        w: 1.0 / iw,
    }
}

/// Where `t` along `a`..`b` is on screen, worked out in f64: the ends of
/// a segment cut down to the screen, which may lie far outside it.
fn between_far(a: Screen, b: Screen, t: f64) -> Screen {
    let lerp = |p: f32, q: f32| f64::from(p) + t * (f64::from(q) - f64::from(p));
    let (aw, bw) = (f64::from(a.w), f64::from(b.w));
    let iw = (1.0 - t) / aw + t / bw;
    let z = ((1.0 - t) * f64::from(a.z) / aw + t * f64::from(b.z) / bw) / iw;
    Screen {
        x: lerp(a.x, b.x) as f32,
        y: lerp(a.y, b.y) as f32,
        z: z as f32,
        w: (1.0 / iw) as f32,
    }
}

/// The dashes of a polyline on screen: `on` samples drawn, `off` left
/// out, the pattern carried on along the whole polyline. Only what lies
/// within `margin` of a screen `size` big is dashed.
fn dashes(
    points: &[Screen],
    on: f32,
    off: f32,
    size: [f32; 2],
    margin: f32,
) -> Vec<(Screen, Screen)> {
    let mut out = Vec::new();
    // Where in the pattern the next point is.
    let mut phase = 0.0f32;
    let period = on + off;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let full = (dx * dx + dy * dy).sqrt();
        // The pattern moves on over what isn't shown, before and after.
        let skip = |phase: f32, t: f64| {
            ((f64::from(phase) + t * f64::from(full)) % f64::from(period)) as f32
        };
        let shown = crate::sketch_view::on_screen(
            [a.x, a.y].map(f64::from),
            [b.x, b.y].map(f64::from),
            size.map(f64::from),
            f64::from(margin),
        );
        let Some((t0, t1)) = shown else {
            phase = skip(phase, 1.0);
            continue;
        };
        let (a, b, len) = if (t0, t1) == (0.0, 1.0) {
            (a, b, full)
        } else {
            phase = skip(phase, t0);
            let (a, b) = (between_far(a, b, t0), between_far(a, b, t1));
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            (a, b, (dx * dx + dy * dy).sqrt())
        };
        let mut s = 0.0f32;
        while s < len {
            let (drawn, left) = if phase < on {
                (true, on - phase)
            } else {
                (false, period - phase)
            };
            let step = left.min(len - s);
            if drawn {
                out.push((between(a, b, s / len), between(a, b, (s + step) / len)));
            }
            s += step;
            phase = (phase + step) % period;
        }
        if t1 < 1.0 {
            phase = skip(phase, 1.0 - t1);
        }
    }
    out
}

impl Band {
    /// Darkens covered samples next to the background or to a surface much
    /// farther away.
    fn silhouettes(&mut self) {
        let (w, h) = (self.width, self.rows);
        let edge: Vec<bool> = (0..w * h)
            .map(|i| {
                let z = self.depth[i];
                if !z.is_finite() {
                    return false;
                }
                let (x, y) = (i % w, i / w);
                let mut near = Vec::with_capacity(4);
                if x > 0 {
                    near.push(i - 1);
                }
                if x + 1 < w {
                    near.push(i + 1);
                }
                if y > 0 {
                    near.push(i - w);
                }
                if y + 1 < h {
                    near.push(i + w);
                }
                near.iter()
                    .any(|&j| self.depth[j] > z + 0.02 * z.abs() + 1e-6)
            })
            .collect();
        for (i, e) in edge.into_iter().enumerate() {
            if e {
                self.color[i] = EDGE;
            }
        }
    }
}

/// Lighting at a corner: ambient plus diffuse.
fn shade(n: [f32; 3], light: [f32; 3]) -> f32 {
    let d = n[0] * light[0] + n[1] * light[1] + n[2] * light[2];
    AMBIENT + DIFFUSE * d.max(0.0)
}

fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Renders `geometry` seen by `camera` into a `width`×`height` image.
pub fn render(
    geometry: &Geometry,
    camera: &Camera,
    width: u32,
    height: u32,
    options: Options,
) -> Result<Image> {
    check_size(width, height)?;
    camera.check()?;
    let view = View::new(camera, width, height);
    let screen: Vec<Option<Screen>> = geometry
        .positions
        .iter()
        .map(|p| view.project(*p))
        .collect();
    let light = view.light();
    let (w, h) = (width as usize, height as usize);
    let sw = w * SS;
    let mut rgba = vec![0u8; w * h * 4];
    rgba.par_chunks_mut(BAND * w * 4)
        .enumerate()
        .for_each(|(band, out)| {
            let rows_out = out.len() / (w * 4);
            let own = band * BAND * SS;
            let top = usize::from(own > 0);
            let bottom = usize::from(own + rows_out * SS < h * SS);
            let mut b = Band::new(own - top, rows_out * SS + top + bottom, sw);
            for t in &geometry.triangles {
                let (Some(a), Some(c), Some(d)) = (
                    screen[t.p[0] as usize],
                    screen[t.p[1] as usize],
                    screen[t.p[2] as usize],
                ) else {
                    continue;
                };
                let s = [
                    shade(t.n[0], light),
                    shade(t.n[1], light),
                    shade(t.n[2], light),
                ];
                b.triangle([a, c, d], s, t.color);
            }
            if options.silhouettes {
                b.silhouettes();
            }
            if !geometry.ghosts.is_empty() {
                let n = b.color.len();
                b.ghost = vec![BACKGROUND; n];
                b.ghost_depth = vec![f32::INFINITY; n];
                for t in &geometry.ghosts {
                    let (Some(a), Some(c), Some(d)) = (
                        screen[t.p[0] as usize],
                        screen[t.p[1] as usize],
                        screen[t.p[2] as usize],
                    ) else {
                        continue;
                    };
                    let s = [
                        shade(t.n[0], light),
                        shade(t.n[1], light),
                        shade(t.n[2], light),
                    ];
                    b.ghost_triangle([a, c, d], s, t.color);
                }
                b.blend_ghosts();
            }
            for l in &geometry.lines {
                let half = l.width * SS as f32 / 2.0;
                let pts: Vec<Option<Screen>> =
                    l.points.iter().map(|p| screen[*p as usize]).collect();
                if let [Some(p)] = pts[..] {
                    b.segment(p, p, half, l.color);
                }
                if l.dash {
                    // Each run of points in front of the eye is dashed on
                    // its own.
                    let [on, off] = DASH.map(|d| d * SS as f32);
                    for run in pts.split(Option::is_none) {
                        let run: Vec<Screen> = run.iter().flatten().copied().collect();
                        let size = [sw as f32, (h * SS) as f32];
                        for (p, q) in dashes(&run, on, off, size, half + 1.0) {
                            b.segment(p, q, half, l.color);
                        }
                    }
                    continue;
                }
                for pair in pts.windows(2) {
                    if let [Some(p), Some(q)] = pair {
                        b.segment(*p, *q, half, l.color);
                    }
                }
            }
            for (i, px) in out.chunks_mut(4).enumerate() {
                let (x, y) = (i % w, i / w);
                let mut sum = [0.0f32; 3];
                for dy in 0..SS {
                    for dx in 0..SS {
                        let c = b.color[(y * SS + dy + top) * sw + x * SS + dx];
                        for k in 0..3 {
                            sum[k] += c[k];
                        }
                    }
                }
                let n = (SS * SS) as f32;
                px.copy_from_slice(&[to_u8(sum[0] / n), to_u8(sum[1] / n), to_u8(sum[2] / n), 255]);
            }
        });
    let mut img = Image {
        width,
        height,
        rgba,
    };
    for l in &geometry.labels {
        if let Some((x, y)) = view.pixel(l.at) {
            img.text(x as i64 + 4, y as i64 - 12, &l.text);
        }
    }
    if options.triad {
        img.triad(&view);
    }
    Ok(img)
}

impl Image {
    fn put(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.rgba[i..i + 3].copy_from_slice(&c);
    }

    /// ASCII text, dark on a white halo, its top left at `(x, y)`.
    fn text(&mut self, x: i64, y: i64, s: &str) {
        let glyph = |c: char| {
            let i = if c.is_ascii() {
                c as usize
            } else {
                b'?' as usize
            };
            font8x8::legacy::BASIC_LEGACY[i]
        };
        let on = |c: char, gx: i64, gy: i64| {
            (0..8).contains(&gx) && (0..8).contains(&gy) && glyph(c)[gy as usize] >> gx & 1 == 1
        };
        let white = [255, 255, 255];
        let dark = EDGE.map(to_u8);
        for (k, c) in s.chars().enumerate() {
            let ox = x + 8 * k as i64;
            for gy in -1..9 {
                for gx in -1..9 {
                    let near = (-1..=1).any(|dy| (-1..=1).any(|dx| on(c, gx + dx, gy + dy)));
                    if on(c, gx, gy) {
                        self.put(ox + gx, y + gy, dark);
                    } else if near {
                        self.put(ox + gx, y + gy, white);
                    }
                }
            }
        }
    }

    /// A line two pixels wide.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3]) {
        let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as i64).max(1);
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let (x, y) = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                self.put(x as i64 + dx, y as i64 + dy, c);
            }
        }
    }

    /// The world axes as seen by the view, bottom left.
    fn triad(&mut self, view: &View) {
        let o = (28.0, self.height as f32 - 28.0);
        let axes = [
            ([1.0, 0.0, 0.0], [214, 48, 49], "X"),
            ([0.0, 1.0, 0.0], [39, 174, 96], "Y"),
            ([0.0, 0.0, 1.0], [41, 98, 255], "Z"),
        ];
        for (d, c, name) in axes {
            let (dx, dy) = view.direction(d);
            let tip = (o.0 + 20.0 * dx, o.1 + 20.0 * dy);
            self.line(o, tip, c);
            self.text(
                tip.0 as i64 + 2 * dx.signum() as i64 - 4,
                tip.1 as i64 - 4 + 4 * dy.signum() as i64,
                name,
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unit cube's six faces as twelve outward triangles.
    pub(crate) fn cube() -> Geometry {
        let positions: Vec<[f64; 3]> = (0..8)
            .map(|i| [(i & 1) as f64, (i >> 1 & 1) as f64, (i >> 2 & 1) as f64])
            .collect();
        // Each face: four corners counterclockwise from outside, and its
        // normal.
        let faces: [([u32; 4], [f32; 3]); 6] = [
            ([0, 2, 3, 1], [0.0, 0.0, -1.0]),
            ([4, 5, 7, 6], [0.0, 0.0, 1.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
            ([2, 6, 7, 3], [0.0, 1.0, 0.0]),
            ([0, 4, 6, 2], [-1.0, 0.0, 0.0]),
            ([1, 3, 7, 5], [1.0, 0.0, 0.0]),
        ];
        let mut triangles = Vec::new();
        for (q, n) in faces {
            for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                triangles.push(Triangle {
                    p: t,
                    n: [n; 3],
                    color: SURFACE,
                });
            }
        }
        Geometry {
            positions,
            triangles,
            lines: Vec::new(),
            labels: Vec::new(),
            ghosts: Vec::new(),
        }
    }

    /// A unit cube moved along x, as ghosts.
    fn moved(g: &mut Geometry, dx: f64, color: Rgb) {
        let plain = cube();
        let base = g.positions.len() as u32;
        g.positions
            .extend(plain.positions.iter().map(|p| [p[0] + dx, p[1], p[2]]));
        g.ghosts.extend(plain.triangles.iter().map(|t| Triangle {
            p: t.p.map(|i| base + i),
            n: t.n,
            color,
        }));
    }

    #[test]
    fn ghosts_tint_what_they_cover_and_hide_behind_the_model() {
        let c = camera([0.5, -5.0, 0.5]);
        let plain = render(&cube(), &c, 128, 64, bare()).unwrap();
        let mut g = cube();
        // One ghost exactly where the model is (pushed back, so hidden),
        // and one beside it, over the background.
        moved(&mut g, 0.0, COMPARE_GHOST);
        moved(&mut g, 1.5, COMPARE_REMOVED);
        let img = render(&g, &c, 128, 64, bare()).unwrap();
        assert_eq!(at(&img, 64, 32), at(&plain, 64, 32));
        let bg = BACKGROUND.map(to_u8);
        assert_eq!(&at(&plain, 97, 32)[..3], &bg);
        let px = at(&img, 97, 32);
        assert_ne!(&px[..3], &bg);
        // Removed red over the background's grey: more red than green.
        assert!(px[0] > px[1], "{px:?}");
    }

    #[test]
    fn ghosts_are_the_same_on_any_number_of_threads() {
        let mut g = cube();
        moved(&mut g, 0.6, COMPARE_GHOST);
        moved(&mut g, -0.7, COMPARE_REMOVED);
        let c = camera([2.0, -3.0, 1.5]);
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let many = rayon::ThreadPoolBuilder::new()
            .num_threads(7)
            .build()
            .unwrap();
        let a = one.install(|| render(&g, &c, 200, 150, Options::default()).unwrap());
        let b = many.install(|| render(&g, &c, 200, 150, Options::default()).unwrap());
        assert_eq!(a, b);
    }

    /// Surfaces only.
    fn bare() -> Options {
        Options {
            silhouettes: false,
            triad: false,
        }
    }

    fn camera(eye: [f64; 3]) -> Camera {
        Camera {
            eye,
            target: [0.5, 0.5, 0.5],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Perspective { fov_deg: 30.0 },
        }
    }

    fn at(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_cube_covers_the_middle_and_not_the_corners() {
        let img = render(&cube(), &camera([3.0, -4.0, 3.0]), 64, 48, bare()).unwrap();
        let bg = BACKGROUND.map(to_u8);
        assert_eq!(&at(&img, 0, 0)[..3], &bg);
        assert_ne!(&at(&img, 32, 24)[..3], &bg);
        assert_eq!(img.rgba.len(), 64 * 48 * 4);
    }

    #[test]
    fn faces_towards_the_light_are_brighter() {
        // Looking straight at the -Y face, then from behind at the +Y face:
        // both are lit the same, being square to the view.
        let front = render(&cube(), &camera([0.5, -5.0, 0.5]), 32, 32, bare()).unwrap();
        let back = render(&cube(), &camera([0.5, 6.0, 0.5]), 32, 32, bare()).unwrap();
        assert_eq!(at(&front, 16, 16), at(&back, 16, 16));
        // From above at an angle, the top (facing the light more) is
        // brighter than a side.
        let img = render(&cube(), &camera([0.5, -3.0, 4.0]), 64, 64, bare()).unwrap();
        let top = at(&img, 32, 22)[0];
        let side = at(&img, 32, 40)[0];
        assert!(top > side, "{top} > {side}");
    }

    #[test]
    fn the_same_pixels_on_any_number_of_threads() {
        let mut g = cube();
        g.lines.push(Line {
            points: vec![0, 1, 3, 2, 0],
            color: EDGE,
            width: 1.0,
            dash: false,
        });
        g.labels.push(Label {
            at: [1.0, 1.0, 1.0],
            text: "corner".into(),
        });
        let c = camera([2.0, -3.0, 1.5]);
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let many = rayon::ThreadPoolBuilder::new()
            .num_threads(7)
            .build()
            .unwrap();
        let a = one.install(|| render(&g, &c, 200, 150, Options::default()).unwrap());
        let b = many.install(|| render(&g, &c, 200, 150, Options::default()).unwrap());
        assert_eq!(a, b);
    }

    #[test]
    fn behind_the_eye_is_not_drawn() {
        let mut c = camera([0.5, 0.5, 0.5]);
        c.target = [0.5, 10.0, 0.5];
        // The eye is inside the cube: every face is behind or edge-on, or
        // seen from inside (culled).
        let img = render(&cube(), &c, 16, 16, bare()).unwrap();
        let bg = BACKGROUND.map(to_u8);
        assert!(img.rgba.chunks(4).all(|p| p[..3] == bg));
    }

    #[test]
    fn lines_show_on_surfaces_but_not_through_them() {
        let c = camera([0.5, -5.0, 0.5]);
        // An edge along the near face's middle, and one on the far face.
        let mut g = cube();
        g.positions.extend([
            [0.0, 0.0, 0.5],
            [1.0, 0.0, 0.5],
            [0.0, 1.0, 0.75],
            [1.0, 1.0, 0.75],
        ]);
        let red = [1.0, 0.0, 0.0];
        g.lines.push(Line {
            points: vec![8, 9],
            color: red,
            width: 2.0,
            dash: false,
        });
        g.lines.push(Line {
            points: vec![10, 11],
            color: red,
            width: 2.0,
            dash: false,
        });
        let img = render(&g, &c, 64, 64, bare()).unwrap();
        let reds = |y: u32| {
            (0..64)
                .filter(|x| at(&img, *x, y)[..3] == [255, 0, 0])
                .count()
        };
        assert!(reds(32) > 10, "the near edge shows");
        let far_y = (0..64).find(|y| reds(*y) > 0 && (*y as i32 - 32).abs() > 3);
        assert_eq!(far_y, None, "the far edge is hidden");
    }

    #[test]
    fn dashed_lines_have_gaps() {
        let c = camera([0.5, -5.0, 0.5]);
        let mut g = cube();
        g.positions.extend([[0.0, 0.0, 0.5], [1.0, 0.0, 0.5]]);
        let red = [1.0, 0.0, 0.0];
        let line = |dash| Line {
            points: vec![8, 9],
            color: red,
            width: 2.0,
            dash,
        };
        let reds = |g: &Geometry| {
            let img = render(g, &c, 64, 64, bare()).unwrap();
            (0..64)
                .filter(|x| at(&img, *x, 32)[..3] == [255, 0, 0])
                .count()
        };
        g.lines.push(line(false));
        let solid = reds(&g);
        g.lines[0] = line(true);
        let dashed = reds(&g);
        // Six on, four off: about 60% of it.
        assert!(
            dashed > solid / 2 && dashed < solid * 3 / 4,
            "{dashed} of {solid}"
        );
        // The pattern runs on across a polyline's corners.
        let pts: Vec<Screen> = [[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]]
            .map(|[x, y]| Screen {
                x,
                y,
                z: 1.0,
                w: 1.0,
            })
            .to_vec();
        let d = dashes(&pts, 6.0, 4.0, [100.0, 100.0], 1.0);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].1.x, d[0].1.y), (5.0, 0.0));
        assert_eq!((d[1].0.x, d[1].0.y, d[1].1.y), (5.0, 0.0, 1.0));
    }

    #[test]
    fn dashes_are_laid_only_where_they_show() {
        // A construction line seen from very close runs far off screen
        // both ways; past 2^28 pixels an f32 walk along it stops moving.
        for far in [2e7f32, 1e10] {
            let pts: Vec<Screen> = [[-far, 50.0], [far, 50.0]]
                .map(|[x, y]| Screen {
                    x,
                    y,
                    z: 1.0,
                    w: 1.0,
                })
                .to_vec();
            let d = dashes(&pts, 6.0, 4.0, [100.0, 100.0], 1.0);
            assert!((10..=12).contains(&d.len()), "{far}: {}", d.len());
            assert!(d.iter().all(|(p, q)| p.x >= -2.0 && q.x <= 102.0));
        }
    }

    #[test]
    fn silhouettes_outline_the_shape() {
        let c = camera([3.0, -4.0, 3.0]);
        let plain = render(&cube(), &c, 64, 64, bare()).unwrap();
        let outlined = render(
            &cube(),
            &c,
            64,
            64,
            Options {
                silhouettes: true,
                triad: false,
            },
        )
        .unwrap();
        let dark = |img: &Image| img.rgba.chunks(4).filter(|p| p[0] < 80).count();
        assert!(dark(&outlined) > dark(&plain));
    }

    #[test]
    fn labels_and_the_triad_draw_on_top() {
        let c = camera([3.0, -4.0, 3.0]);
        let mut g = cube();
        g.labels.push(Label {
            at: [0.5, 0.5, 0.5],
            text: "hi".into(),
        });
        let plain = render(&g.clone(), &c, 64, 64, bare());
        g.labels.clear();
        let without = render(&g, &c, 64, 64, bare()).unwrap();
        assert_ne!(plain.unwrap(), without);
        let triad = render(
            &g,
            &c,
            64,
            64,
            Options {
                silhouettes: false,
                triad: true,
            },
        )
        .unwrap();
        // The X axis is red near the bottom left corner.
        let red = triad.rgba.chunks(4).enumerate().any(|(i, p)| {
            let (x, y) = (i % 64, i / 64);
            x < 40 && y > 24 && p[..3] == [214, 48, 49]
        });
        assert!(red);
    }

    #[test]
    fn sizes_and_png() {
        let c = camera([3.0, -4.0, 3.0]);
        assert_eq!(
            render(&cube(), &c, 0, 10, bare()).unwrap_err().code,
            Code::RenderTooLarge
        );
        assert_eq!(
            render(&cube(), &c, MAX_SIZE + 1, 10, bare())
                .unwrap_err()
                .code,
            Code::RenderTooLarge
        );
        let img = render(&cube(), &c, 20, 10, bare()).unwrap();
        let png = img.png();
        let dec = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (20, 10));
        assert_eq!(&buf[..info.buffer_size()], &img.rgba[..]);
    }
}
