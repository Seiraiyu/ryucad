//! Benchmarks live in `benches/`. With the `occt` and `smlib` features
//! this crate also holds the comparison harnesses for OCCT ([`occt`]) and
//! SMLib, the kernel in NVIDIA's USD BRep ([`smlib`]); without them the
//! default build never needs a C++ toolchain.

#[cfg(feature = "occt")]
pub mod occt;
#[cfg(feature = "smlib")]
pub mod smlib;

use ryukei::geom::Point3;

/// The primitives every kernel builds, in the same local frames: a block
/// from the origin along +x, +y and +z; a cylinder and a cone on the
/// z axis with their base at the origin; a sphere and a torus centred on
/// the origin, the torus around the z axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    Block([f64; 3]),
    Cylinder {
        r: f64,
        h: f64,
    },
    /// From radius `r0` at the base to `r1` (0 for a point) at height `h`.
    Cone {
        r0: f64,
        r1: f64,
        h: f64,
    },
    Sphere(f64),
    Torus {
        major: f64,
        minor: f64,
    },
}

/// A triangle soup as the C++ kernels hand it back: positions (x, y, z
/// flattened) and triangles as position indices, counterclockwise seen
/// from outside.
#[derive(Clone, Copy, Debug)]
pub struct Soup<'a> {
    pub positions: &'a [f64],
    pub triangles: &'a [u32],
}

impl Soup<'_> {
    pub fn point(&self, i: u32) -> Point3 {
        let k = 3 * i as usize;
        Point3::new(
            self.positions[k],
            self.positions[k + 1],
            self.positions[k + 2],
        )
    }

    /// The enclosed volume and the area.
    pub fn volume_and_area(&self) -> (f64, f64) {
        let (mut vol, mut area) = (0.0, 0.0);
        for t in self.triangles.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.point(i).to_vector());
            vol += a.dot(b.cross(c)) / 6.0;
            area += (b - a).cross(c - a).length() / 2.0;
        }
        (vol, area)
    }

    /// The largest `distance` from sampled points of every `every`-th
    /// triangle: one way only, mesh to solid.
    pub fn deviation(&self, every: usize, distance: impl Fn(Point3) -> f64) -> f64 {
        let mut worst: f64 = 0.0;
        for t in self.triangles.chunks(3).step_by(every) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.point(i).to_vector());
            for (wa, wb) in [(1.0 / 3.0, 1.0 / 3.0), (0.5, 0.5), (0.5, 0.0), (0.0, 0.5)] {
                let wc = 1.0 - wa - wb;
                let p = Point3::ORIGIN + a * wa + b * wb + c * wc;
                worst = worst.max(distance(p));
            }
        }
        worst
    }
}
