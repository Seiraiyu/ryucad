//! The scanline rasterizer: each pixel 4 × 4 subsamples, sampled at their
//! middles on integer subsample rows; a layer's coverage is how many of a
//! pixel's sixteen its shapes leave set. Bands of rows render side by side
//! and are gathered in order, so the pixels are the same on any number of
//! threads.

use rayon::prelude::*;

use super::flatten::{self, Flat};
use super::{Ink, Scene, View};

/// Subsamples per pixel along each axis.
const SUB: usize = 4;
/// Pixel rows per band.
const BAND: usize = 16;
/// A quarter pixel, in subsamples: how far a chord may stray from its arc.
const TOL: f64 = 1.0;

/// An image: rows top first, each pixel red, green and blue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Image {
    /// The image as PNG bytes, at fixed settings (8-bit RGB, the Paeth
    /// filter on every row, balanced compression), so the same pixels give
    /// the same bytes.
    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        enc.set_filter(png::Filter::Paeth);
        let mut w = enc.write_header().expect("writing to memory");
        w.write_image_data(&self.rgb).expect("sizes match");
        w.finish().expect("writing to memory");
        out
    }
}

/// One fill's edges, each `[x0, y0, x1, y1]` in subsamples, horizontal
/// ones left out, and the rows it reaches.
struct Edges {
    top: f64,
    bottom: f64,
    edges: Vec<[f64; 4]>,
}

/// A part ready to draw.
struct Part {
    positive: bool,
    fills: Vec<Edges>,
}

fn edges(fill: &flatten::Fill) -> Edges {
    let mut out = Vec::new();
    let (mut top, mut bottom) = (f64::INFINITY, f64::NEG_INFINITY);
    for ring in fill {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            top = top.min(a.1);
            bottom = bottom.max(a.1);
            if a.1 != b.1 {
                out.push([a.0, a.1, b.0, b.1]);
            }
        }
    }
    Edges {
        top,
        bottom,
        edges: out,
    }
}

fn prepare(flat: Vec<Flat>) -> Vec<Part> {
    flat.into_iter()
        .map(|f| Part {
            positive: f.positive,
            fills: f.fills.iter().map(edges).collect(),
        })
        .collect()
}

/// Sets (or clears) in `mask` the subsamples of rows `y0..y1` (subsample
/// rows of the band, `y0` its first) that `fill` covers by the nonzero
/// rule. `mask` holds a `u16` per pixel of the band, bit `4 r + c` the
/// subsample at row `r` and column `c` within the pixel.
fn fill_rows(e: &Edges, y0: usize, y1: usize, width: usize, set: bool, mask: &mut [u16]) {
    let (lo, hi) = (y0 as f64, y1 as f64);
    if e.bottom < lo || e.top >= hi {
        return;
    }
    let near: Vec<&[f64; 4]> = e
        .edges
        .iter()
        .filter(|s| s[1].max(s[3]) >= lo && s[1].min(s[3]) < hi)
        .collect();
    let cols = (width * SUB) as f64;
    let mut cross: Vec<(f64, i32)> = Vec::new();
    for row in y0..y1 {
        let y = row as f64 + 0.5;
        cross.clear();
        for s in &near {
            let (ya, yb) = (s[1], s[3]);
            if (ya <= y && y < yb) || (yb <= y && y < ya) {
                let x = s[0] + (y - ya) * (s[2] - s[0]) / (yb - ya);
                cross.push((x, if yb > ya { 1 } else { -1 }));
            }
        }
        cross.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let r = row - y0;
        let (line, bit_row) = (r / SUB * width, (r % SUB) * SUB);
        let mut wind = 0;
        let mut start = 0.0;
        for &(x, d) in &cross {
            let before = wind;
            wind += d;
            if before == 0 && wind != 0 {
                start = x;
            } else if before != 0 && wind == 0 {
                // Columns whose middles, c + 0.5, lie in [start, x).
                let c0 = (start - 0.5).ceil().max(0.0);
                let c1 = (x - 0.5).ceil().min(cols);
                if c1 <= c0 {
                    continue;
                }
                for c in c0 as usize..c1 as usize {
                    let bit = 1u16 << (bit_row + c % SUB);
                    let px = &mut mask[line + c / SUB];
                    if set {
                        *px |= bit;
                    } else {
                        *px &= !bit;
                    }
                }
            }
        }
    }
}

/// Draws `scene` into an image.
pub fn raster(scene: &Scene) -> Image {
    let view = scene.view;
    let map = view.map(SUB as f64);
    let visible = view.visible();
    let layers: Vec<(Ink, Vec<Part>)> = scene
        .layers
        .iter()
        .map(|l| {
            let flat: Vec<Vec<Flat>> = l
                .shapes
                .par_iter()
                .filter(|s| View::shows(&visible, s))
                .map(|s| flatten::shape(s, &map, TOL))
                .collect();
            (l.ink, prepare(flat.into_iter().flatten().collect()))
        })
        .collect();
    let (width, height) = (view.width as usize, view.height as usize);
    let bg = scene.background.map(f64::from);
    let bands: Vec<Vec<u8>> = (0..height.div_ceil(BAND))
        .into_par_iter()
        .map(|b| {
            let rows = BAND.min(height - b * BAND);
            let (y0, y1) = (b * BAND * SUB, (b * BAND + rows) * SUB);
            let mut canvas = vec![bg; rows * width];
            let mut mask = vec![0u16; rows * width];
            for (ink, parts) in &layers {
                mask.fill(0);
                for p in parts {
                    for f in &p.fills {
                        fill_rows(f, y0, y1, width, p.positive, &mut mask);
                    }
                }
                let (colour, alpha) = match ink {
                    Ink::Paint { colour, alpha } => {
                        (colour.map(f64::from), f64::from(*alpha) / 255.0)
                    }
                    Ink::Cut => (bg, 1.0),
                };
                for (c, m) in canvas.iter_mut().zip(&mask) {
                    if *m == 0 {
                        continue;
                    }
                    let a = alpha * f64::from(m.count_ones()) / 16.0;
                    for k in 0..3 {
                        c[k] += (colour[k] - c[k]) * a;
                    }
                }
            }
            canvas
                .iter()
                .flat_map(|c| c.map(|v| (v + 0.5).floor().clamp(0.0, 255.0) as u8))
                .collect()
        })
        .collect();
    Image {
        width: view.width,
        height: view.height,
        rgb: bands.concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point, Rect, Region};
    use crate::model::Polarity;
    use crate::render::{Layer, View};
    use crate::shape::{Shape, circle, rect};
    use crate::symbol::Corners;

    fn view(px: u32) -> View {
        View {
            window: Rect::new(Point::new(0, 0), Point::new(1000, 1000)),
            width: px,
            height: px,
            mirror: false,
        }
    }

    fn square(x0: i64, y0: i64, x1: i64, y1: i64, polarity: Polarity) -> Shape {
        let c = rect(x0, y0, x1, y1, &Corners::SHARP).unwrap();
        Shape::regions(polarity, [Region::solid(c)])
    }

    fn at(img: &Image, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * img.width + x) * 3) as usize;
        [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]]
    }

    const RED: Ink = Ink::Paint {
        colour: [255, 0, 0],
        alpha: 255,
    };

    #[test]
    fn a_square_covers_its_pixels_exactly() {
        let s = square(0, 0, 500, 500, Polarity::Positive);
        let scene = Scene {
            view: view(10),
            background: [0, 0, 0],
            layers: vec![Layer {
                name: "a".into(),
                ink: RED,
                shapes: vec![&s],
            }],
        };
        let img = raster(&scene);
        // y runs down: the square is the lower left quarter.
        assert_eq!(at(&img, 0, 9), [255, 0, 0]);
        assert_eq!(at(&img, 4, 5), [255, 0, 0]);
        assert_eq!(at(&img, 5, 5), [0, 0, 0]);
        assert_eq!(at(&img, 4, 4), [0, 0, 0]);
    }

    #[test]
    fn half_covered_pixels_are_half_painted() {
        let s = square(0, 0, 250, 1000, Polarity::Positive);
        let scene = Scene {
            view: view(4),
            background: [0, 0, 0],
            layers: vec![Layer {
                name: "a".into(),
                ink: RED,
                shapes: vec![&s],
            }],
        };
        let img = raster(&scene);
        assert_eq!(at(&img, 0, 0), [255, 0, 0]);
        let half = square(0, 0, 125, 1000, Polarity::Positive);
        let scene = Scene {
            layers: vec![Layer {
                name: "a".into(),
                ink: RED,
                shapes: vec![&half],
            }],
            ..scene
        };
        assert_eq!(at(&raster(&scene), 0, 0), [128, 0, 0]);
    }

    #[test]
    fn negatives_clear_their_own_layer_only() {
        let back = square(0, 0, 1000, 1000, Polarity::Positive);
        let hole = square(250, 250, 750, 750, Polarity::Negative);
        let blue = Ink::Paint {
            colour: [0, 0, 255],
            alpha: 255,
        };
        let scene = Scene {
            view: view(8),
            background: [0, 0, 0],
            layers: vec![
                Layer {
                    name: "under".into(),
                    ink: blue,
                    shapes: vec![&back],
                },
                Layer {
                    name: "over".into(),
                    ink: RED,
                    shapes: vec![&back, &hole],
                },
            ],
        };
        let img = raster(&scene);
        assert_eq!(at(&img, 0, 0), [255, 0, 0]);
        assert_eq!(at(&img, 4, 4), [0, 0, 255]);
    }

    #[test]
    fn cuts_show_the_background() {
        let back = square(0, 0, 1000, 1000, Polarity::Positive);
        let hole = Shape::regions(
            Polarity::Positive,
            [Region::solid(circle(Point::new(500, 500), 200).unwrap())],
        );
        let scene = Scene {
            view: view(10),
            background: [1, 2, 3],
            layers: vec![
                Layer {
                    name: "copper".into(),
                    ink: RED,
                    shapes: vec![&back],
                },
                Layer {
                    name: "drill".into(),
                    ink: Ink::Cut,
                    shapes: vec![&hole],
                },
            ],
        };
        let img = raster(&scene);
        assert_eq!(at(&img, 5, 5), [1, 2, 3]);
        assert_eq!(at(&img, 0, 0), [255, 0, 0]);
    }

    #[test]
    fn the_same_pixels_on_any_number_of_threads() {
        let shapes: Vec<Shape> = (0..50)
            .map(|i| {
                Shape::regions(
                    Polarity::Positive,
                    [Region::solid(
                        circle(Point::new(i * 19 % 1000, i * 37 % 1000), 30 + i).unwrap(),
                    )],
                )
            })
            .collect();
        let scene = Scene {
            view: view(333),
            background: [0, 0, 0],
            layers: vec![Layer {
                name: "a".into(),
                ink: RED,
                shapes: shapes.iter().collect(),
            }],
        };
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| raster(&scene));
        let many = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap()
            .install(|| raster(&scene));
        assert_eq!(one, many);
        let png = one.png();
        let dec = png::Decoder::new(std::io::Cursor::new(&png));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (333, 333));
        assert_eq!(&buf[..info.buffer_size()], &one.rgb[..]);
    }
}
