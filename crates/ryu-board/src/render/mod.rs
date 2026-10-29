//! Pictures of shapes, for people and agents to look at: PNG from a CPU
//! scanline rasterizer and SVG from the same shapes as paths.
//!
//! A [`Scene`] is layers of shapes, each drawn in an [`Ink`], composited
//! in order over a background, seen through a [`View`]. Within a layer,
//! shapes are drawn in order: a negative part clears what the layer drew
//! before it, never another layer. A [`Ink::Cut`] layer (drills) shows
//! the background through everything below it.
//!
//! Both are the same bytes on every platform and thread count. The
//! rasterizer's arithmetic is `f64` with no transcendental function but
//! `ryu_math`'s; the SVG writes integer nanometres.

mod flatten;
mod raster;
mod svg;

pub use raster::{Image, raster};
pub use svg::svg;

use crate::geom::{Nm, Point, Rect};
use crate::shape::Shape;

/// The longest side a render may have, in pixels.
pub const MAX_SIDE: u32 = 16_384;

/// How a layer is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ink {
    /// In a colour, at an opacity out of 255.
    Paint { colour: [u8; 3], alpha: u8 },
    /// Through to the background: holes.
    Cut,
}

/// One layer of a scene: its shapes in drawing order.
#[derive(Clone, Debug)]
pub struct Layer<'a> {
    pub name: String,
    pub ink: Ink,
    pub shapes: Vec<&'a Shape>,
}

/// What a scene shows: a window of the board, in an image `width` by
/// `height` pixels. The window is scaled to fit and centred; with
/// `mirror` it is seen from below, x running right to left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct View {
    pub window: Rect,
    pub width: u32,
    pub height: u32,
    pub mirror: bool,
}

/// Layers over a background, seen through a view.
#[derive(Clone, Debug)]
pub struct Scene<'a> {
    pub view: View,
    pub background: [u8; 3],
    /// Bottom first: each is composited over those before it.
    pub layers: Vec<Layer<'a>>,
}

impl View {
    /// A view of `window` whose longer side is `long` pixels and whose
    /// shorter keeps the window's proportions (at least one pixel).
    pub fn fitting(window: Rect, long: u32, mirror: bool) -> View {
        let (w, h) = (
            span(window.min.x, window.max.x),
            span(window.min.y, window.max.y),
        );
        let short = |a: Nm, b: Nm| -> u32 {
            let s = (i128::from(long) * i128::from(a) + i128::from(b) / 2) / i128::from(b);
            s.clamp(1, i128::from(long)) as u32
        };
        let (width, height) = if w >= h {
            (long, short(h, w))
        } else {
            (short(w, h), long)
        };
        View {
            window,
            width,
            height,
            mirror,
        }
    }

    /// The part of the board the image shows: the window grown along one
    /// axis to the image's proportions, about its middle, rounded outward
    /// to whole nanometres. Shapes wholly outside it aren't drawn.
    pub fn visible(&self) -> Rect {
        let (min, max) = (self.window.min, self.window.max);
        let (w, h) = (span(min.x, max.x) as f64, span(min.y, max.y) as f64);
        let (iw, ih) = (f64::from(self.width), f64::from(self.height));
        let k = (iw / w).min(ih / h);
        let (vw, vh) = (iw / k / 2.0, ih / k / 2.0);
        let (cx, cy) = (
            (min.x as f64 + max.x as f64) / 2.0,
            (min.y as f64 + max.y as f64) / 2.0,
        );
        Rect {
            min: Point::new((cx - vw).floor() as Nm, (cy - vh).floor() as Nm),
            max: Point::new((cx + vw).ceil() as Nm, (cy + vh).ceil() as Nm),
        }
    }

    /// Whether any of `shape` may show: its box meets [`View::visible`].
    pub(crate) fn shows(visible: &Rect, shape: &Shape) -> bool {
        shape.bounds().is_some_and(|b| {
            b.max.x >= visible.min.x
                && b.min.x <= visible.max.x
                && b.max.y >= visible.min.y
                && b.min.y <= visible.max.y
        })
    }

    /// Nanometres per pixel and where the window's corner lands: the
    /// window scaled to fit the image and centred in it.
    pub(crate) fn map(&self, sub: f64) -> Map {
        let (w, h) = (
            span(self.window.min.x, self.window.max.x) as f64,
            span(self.window.min.y, self.window.max.y) as f64,
        );
        let (iw, ih) = (f64::from(self.width) * sub, f64::from(self.height) * sub);
        let k = (iw / w).min(ih / h);
        Map {
            x0: if self.mirror {
                self.window.max.x
            } else {
                self.window.min.x
            },
            y0: self.window.max.y,
            k,
            dx: (iw - w * k) / 2.0,
            dy: (ih - h * k) / 2.0,
            mirror: self.mirror,
        }
    }
}

/// A window's extent along one axis: at least one nanometre.
fn span(lo: Nm, hi: Nm) -> Nm {
    (hi - lo).max(1)
}

/// Board nanometres to image units (pixels, or subsamples of them): y
/// down, x mirrored when seen from below.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Map {
    x0: Nm,
    y0: Nm,
    k: f64,
    dx: f64,
    dy: f64,
    mirror: bool,
}

impl Map {
    pub(crate) fn point(&self, p: Point) -> (f64, f64) {
        let x = if self.mirror {
            self.x0 - p.x
        } else {
            p.x - self.x0
        };
        (
            x as f64 * self.k + self.dx,
            (self.y0 - p.y) as f64 * self.k + self.dy,
        )
    }

    /// A vector in board nanometres (a pen's own coordinates), as the
    /// image sees it.
    pub(crate) fn vector(&self, x: f64, y: f64) -> (f64, f64) {
        let x = if self.mirror { -x } else { x };
        (x * self.k, -y * self.k)
    }

    pub(crate) fn scale(&self) -> f64 {
        self.k
    }
}

/// `Rect::union` over every part of `shapes`, or `None` for none.
pub fn bounds<'a>(shapes: impl IntoIterator<Item = &'a Shape>) -> Option<Rect> {
    shapes
        .into_iter()
        .filter_map(Shape::bounds)
        .reduce(Rect::union)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_keep_the_windows_proportions() {
        let w = Rect::new(Point::new(0, 0), Point::new(2_000, 1_000));
        let v = View::fitting(w, 1600, false);
        assert_eq!((v.width, v.height), (1600, 800));
        let tall = Rect::new(Point::new(0, 0), Point::new(1, 3_000_000));
        let v = View::fitting(tall, 100, true);
        assert_eq!((v.width, v.height), (1, 100));
        let m = View::fitting(w, 1600, false).map(1.0);
        assert_eq!(m.point(Point::new(0, 1_000)), (0.0, 0.0));
        assert_eq!(m.point(Point::new(2_000, 0)), (1600.0, 800.0));
        let mirrored = View::fitting(w, 1600, true).map(1.0);
        assert_eq!(mirrored.point(Point::new(2_000, 1_000)), (0.0, 0.0));
        assert_eq!(mirrored.vector(10.0, 10.0), (-8.0, -8.0));
    }

    #[test]
    fn a_window_is_centred_in_an_image_of_another_shape() {
        let w = Rect::new(Point::new(0, 0), Point::new(1_000, 1_000));
        let v = View {
            window: w,
            width: 200,
            height: 100,
            mirror: false,
        };
        let m = v.map(1.0);
        assert_eq!(m.point(Point::new(0, 1_000)), (50.0, 0.0));
        assert_eq!(m.point(Point::new(1_000, 0)), (150.0, 100.0));
        assert_eq!(
            v.visible(),
            Rect::new(Point::new(-500, 0), Point::new(1_500, 1_000))
        );
        let tiny = Shape::regions(
            crate::model::Polarity::Positive,
            [crate::geom::Region::solid(
                crate::shape::circle(Point::new(1_600, 500), 50).unwrap(),
            )],
        );
        assert!(!View::shows(&v.visible(), &tiny));
        assert!(!View::shows(&v.visible(), &Shape::default()));
    }
}
