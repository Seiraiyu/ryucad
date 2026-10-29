//! The rasterizer and the SVG writer over a layer of 10,000 seeded
//! features, at the default 1600 pixels. Seeded features draw lines with
//! every kind of symbol, so four in ten are swept pens, which real boards
//! hardly have: the SVG writes each as polygons, seconds where a real
//! board takes a millisecond (`cargo bench -p ryudfm`).

use ryu_board::geom::{Point, Rect};
use ryu_board::model::Library;
use ryu_board::render::{Ink, Layer, Scene, View, raster, svg};
use ryu_board::shape::{Shape, feature_shape};
use ryu_board::testing::{Lcg, seeded_features};

fn main() {
    divan::main();
}

fn shapes() -> Vec<Shape> {
    let mut g = Lcg::new(3);
    let f = seeded_features(&mut g, 64, 10_000, 100_000_000);
    let lib = Library::default();
    (0..f.features().len())
        .filter_map(|i| feature_shape(&f, i, &lib).as_ref().ok().cloned())
        .collect()
}

fn scene(shapes: &[Shape]) -> Scene<'_> {
    Scene {
        view: View::fitting(
            Rect::new(
                Point::new(-60_000_000, -60_000_000),
                Point::new(60_000_000, 60_000_000),
            ),
            1600,
            false,
        ),
        background: [16, 20, 24],
        layers: vec![Layer {
            name: "copper".into(),
            ink: Ink::Paint {
                colour: [200, 130, 50],
                alpha: 230,
            },
            shapes: shapes.iter().collect(),
        }],
    }
}

#[divan::bench(args = [1, 0], sample_count = 10)]
fn raster_10000_features(bencher: divan::Bencher, threads: usize) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let shapes = shapes();
    let scene = scene(&shapes);
    bencher.bench(|| pool.install(|| raster(&scene).rgb.len()));
}

#[divan::bench]
fn png_of_1600_pixels(bencher: divan::Bencher) {
    let shapes = shapes();
    let img = raster(&scene(&shapes));
    bencher.bench(|| img.png().len());
}

#[divan::bench(sample_count = 5)]
fn svg_10000_features(bencher: divan::Bencher) {
    let shapes = shapes();
    let scene = scene(&shapes);
    bencher.bench(|| svg(&scene).len());
}
