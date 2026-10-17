//! Offscreen renders for agents: a small CPU rasterizer, so a render works
//! wherever the CLI does (static musl, containers, CI) with no GPU and no
//! drivers, and gives the same pixels on every platform.
//!
//! The image is drawn at twice its size and averaged down (4 samples a
//! pixel), in horizontal bands that are independent of each other, so
//! bands run in parallel and the result doesn't depend on the thread
//! count. Pixel arithmetic is f32 with no transcendental functions; the
//! view set-up is f64 through `ryukei::math`.

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
const AMBIENT: f32 = 0.3;
const DIFFUSE: f32 = 0.7;

/// What to draw, in model space (meters).
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<Triangle>,
    pub lines: Vec<Line>,
}

/// A triangle of positions, counterclockwise from outside, with a normal
/// per corner and a colour.
#[derive(Clone, Debug)]
pub struct Triangle {
    pub p: [u32; 3],
    pub n: [[f32; 3]; 3],
    pub color: Rgb,
}

/// A polyline of positions drawn `width` output pixels wide.
#[derive(Clone, Debug)]
pub struct Line {
    pub points: Vec<u32>,
    pub color: Rgb,
    pub width: f32,
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
    /// The first supersampled row, which may be one above the band's own
    /// rows (a margin for edge detection).
    y0: usize,
    rows: usize,
    width: usize,
    color: Vec<Rgb>,
    depth: Vec<f32>,
}

impl Band {
    fn new(y0: usize, rows: usize, width: usize) -> Band {
        Band {
            y0,
            rows,
            width,
            color: vec![BACKGROUND; rows * width],
            depth: vec![f32::INFINITY; rows * width],
        }
    }

    fn triangle(&mut self, s: [Screen; 3], shade: [f32; 3], color: Rgb) {
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
                if z >= self.depth[i] {
                    continue;
                }
                let light = lerp(shade);
                self.depth[i] = z;
                self.color[i] = color.map(|c| c * light);
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
pub fn render(geometry: &Geometry, camera: &Camera, width: u32, height: u32) -> Result<Image> {
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
            let y0 = band * BAND * SS;
            let mut b = Band::new(y0, rows_out * SS, sw);
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
            for (i, px) in out.chunks_mut(4).enumerate() {
                let (x, y) = (i % w, i / w);
                let mut sum = [0.0f32; 3];
                for dy in 0..SS {
                    for dx in 0..SS {
                        let c = b.color[(y * SS + dy) * sw + x * SS + dx];
                        for k in 0..3 {
                            sum[k] += c[k];
                        }
                    }
                }
                let n = (SS * SS) as f32;
                px.copy_from_slice(&[to_u8(sum[0] / n), to_u8(sum[1] / n), to_u8(sum[2] / n), 255]);
            }
        });
    Ok(Image {
        width,
        height,
        rgba,
    })
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
        let img = render(&cube(), &camera([3.0, -4.0, 3.0]), 64, 48).unwrap();
        let bg = BACKGROUND.map(to_u8);
        assert_eq!(&at(&img, 0, 0)[..3], &bg);
        assert_ne!(&at(&img, 32, 24)[..3], &bg);
        assert_eq!(img.rgba.len(), 64 * 48 * 4);
    }

    #[test]
    fn faces_towards_the_light_are_brighter() {
        // Looking straight at the -Y face, then from behind at the +Y face:
        // both are lit the same, being square to the view.
        let front = render(&cube(), &camera([0.5, -5.0, 0.5]), 32, 32).unwrap();
        let back = render(&cube(), &camera([0.5, 6.0, 0.5]), 32, 32).unwrap();
        assert_eq!(at(&front, 16, 16), at(&back, 16, 16));
        // From above at an angle, the top (facing the light more) is
        // brighter than a side.
        let img = render(&cube(), &camera([0.5, -3.0, 4.0]), 64, 64).unwrap();
        let top = at(&img, 32, 22)[0];
        let side = at(&img, 32, 40)[0];
        assert!(top > side, "{top} > {side}");
    }

    #[test]
    fn the_same_pixels_on_any_number_of_threads() {
        let g = cube();
        let c = camera([2.0, -3.0, 1.5]);
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let many = rayon::ThreadPoolBuilder::new()
            .num_threads(7)
            .build()
            .unwrap();
        let a = one.install(|| render(&g, &c, 200, 150).unwrap());
        let b = many.install(|| render(&g, &c, 200, 150).unwrap());
        assert_eq!(a, b);
    }

    #[test]
    fn behind_the_eye_is_not_drawn() {
        let mut c = camera([0.5, 0.5, 0.5]);
        c.target = [0.5, 10.0, 0.5];
        // The eye is inside the cube: every face is behind or edge-on, or
        // seen from inside (culled).
        let img = render(&cube(), &c, 16, 16).unwrap();
        let bg = BACKGROUND.map(to_u8);
        assert!(img.rgba.chunks(4).all(|p| p[..3] == bg));
    }

    #[test]
    fn sizes_and_png() {
        let c = camera([3.0, -4.0, 3.0]);
        assert_eq!(
            render(&cube(), &c, 0, 10).unwrap_err().code,
            Code::RenderTooLarge
        );
        assert_eq!(
            render(&cube(), &c, MAX_SIZE + 1, 10).unwrap_err().code,
            Code::RenderTooLarge
        );
        let img = render(&cube(), &c, 20, 10).unwrap();
        let png = img.png();
        let dec = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (20, 10));
        assert_eq!(&buf[..info.buffer_size()], &img.rgba[..]);
    }
}
