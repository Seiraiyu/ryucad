//! Benchmarks live in `benches/`. With the `occt` and `smlib` features
//! this crate also holds the comparison harnesses for OCCT ([`occt`]) and
//! SMLib, the kernel in NVIDIA's USD BRep ([`smlib`]), and with `formats`
//! a reader of 3MF files through lib3mf ([`lib3mf`]); without them the
//! default build never needs a C++ toolchain.

#[cfg(feature = "formats")]
pub mod lib3mf;
#[cfg(feature = "occt")]
pub mod occt;
#[cfg(feature = "smlib")]
pub mod smlib;

use ryukei::Error;
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Vector2, Vector3};
use ryukei::math::{cos, sin};
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};

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

/// An edge to blend, picked by a point on it (each kernel finds its own
/// edge passing nearest, sampled at 17 points), and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BenchBlend {
    pub at: [f64; 3],
    pub kind: BenchKind,
}

/// A fillet, or a chamfer as setbacks from the edge along its faces; a
/// face is picked by a point nearer it than the edge's other face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BenchKind {
    Fillet(f64),
    Equal(f64),
    Distances {
        first: f64,
        second: f64,
        first_face: [f64; 3],
    },
    /// `angle` (radians) between the chamfer and the face.
    DistanceAngle {
        distance: f64,
        angle: f64,
        face: [f64; 3],
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

/// The vector `dist` along the direction of `dir`.
pub fn sweep_vector(dir: [f64; 3], dist: f64) -> [f64; 3] {
    let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    dir.map(|d| d / length * dist)
}

/// A planar profile for the sweep comparisons: loops of segments in the
/// plane through `origin` spanned by the unit vectors `x` and `y`. Loop
/// 0 is the outer loop and later loops are holes in it; either way
/// round is accepted ([`oriented`](Self::oriented) fixes the direction).
#[derive(Clone, Debug, PartialEq)]
pub struct BenchProfile {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub loops: Vec<Vec<BenchSegment>>,
}

/// A segment of a [`BenchProfile`], in the profile plane's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BenchSegment {
    Line {
        a: [f64; 2],
        b: [f64; 2],
    },
    /// From angle `a0` to `a1` (radians from the plane's x axis):
    /// counterclockwise when `a1 > a0`, clockwise otherwise.
    Arc {
        centre: [f64; 2],
        radius: f64,
        a0: f64,
        a1: f64,
    },
}

impl BenchSegment {
    /// The same segment the other way round.
    pub fn reversed(self) -> Self {
        match self {
            BenchSegment::Line { a, b } => BenchSegment::Line { a: b, b: a },
            BenchSegment::Arc {
                centre,
                radius,
                a0,
                a1,
            } => BenchSegment::Arc {
                centre,
                radius,
                a0: a1,
                a1: a0,
            },
        }
    }

    /// The integral of `(x dy - y dx) / 2` along the segment: summed
    /// round a loop, the loop's signed area.
    pub fn area_term(self) -> f64 {
        match self {
            BenchSegment::Line { a, b } => (a[0] * b[1] - b[0] * a[1]) / 2.0,
            BenchSegment::Arc {
                centre: [cx, cy],
                radius: r,
                a0,
                a1,
            } => {
                (r * r * (a1 - a0) + r * cx * (sin(a1) - sin(a0)) - r * cy * (cos(a1) - cos(a0)))
                    / 2.0
            }
        }
    }
}

impl BenchProfile {
    /// A loop's signed area: positive when counterclockwise.
    pub fn loop_area(segments: &[BenchSegment]) -> f64 {
        segments.iter().map(|s| s.area_term()).sum()
    }

    /// The profile's area: the outer loop's less the holes'.
    pub fn area(&self) -> f64 {
        let mut loops = self.loops.iter().map(|l| Self::loop_area(l).abs());
        let outer = loops.next().unwrap_or(0.0);
        outer - loops.sum::<f64>()
    }

    /// The same profile with the outer loop counterclockwise and the
    /// holes clockwise, as the kernels want them.
    pub fn oriented(&self) -> Self {
        let loops = self
            .loops
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if (Self::loop_area(l) > 0.0) == (i == 0) {
                    l.clone()
                } else {
                    l.iter().rev().map(|s| s.reversed()).collect()
                }
            })
            .collect();
        Self {
            loops,
            ..self.clone()
        }
    }

    /// The profile as ryukei's, validated: segment keys counted from 1 in
    /// order, loop keys from 1.
    pub fn to_ryukei(&self) -> Result<ValidProfile, Error> {
        let frame = Frame::new(
            Point3::new(self.origin[0], self.origin[1], self.origin[2]),
            Vector3::new(self.x[0], self.x[1], self.x[2])
                .cross(Vector3::new(self.y[0], self.y[1], self.y[2])),
            Vector3::new(self.x[0], self.x[1], self.x[2]),
        )?;
        let mut key = 0;
        let mut loops = Vec::new();
        for (i, l) in self.loops.iter().enumerate() {
            let mut segments = Vec::new();
            for s in l {
                key += 1;
                let (curve, t0, t1) = match *s {
                    BenchSegment::Line { a, b } => {
                        let (a, b) = (Point2::new(a[0], a[1]), Point2::new(b[0], b[1]));
                        (Curve2::Line2(Line2::through(a, b)?), 0.0, a.distance(b))
                    }
                    BenchSegment::Arc {
                        centre,
                        radius,
                        a0,
                        a1,
                    } => {
                        let c = Point2::new(centre[0], centre[1]);
                        let f = Frame2::new(c, Vector2::new(1.0, 0.0), true)?;
                        (Curve2::Circle2(Circle2::new(f, radius)?), a0, a1)
                    }
                };
                segments.push(Segment {
                    key: SegmentKey(key),
                    curve,
                    t0,
                    t1,
                });
            }
            loops.push(Loop {
                key: LoopKey(i as u128 + 1),
                segments,
            });
        }
        ValidProfile::new(Profile { frame, loops })
    }

    /// A regular `n`-gon of circumradius 1 centred at `(2, 0)` in the
    /// world's xy plane, for timing: off the y axis, so it revolves.
    pub fn polygon(n: usize) -> Self {
        let at = |i: usize| {
            let a = 2.0 * core::f64::consts::PI * i as f64 / n as f64;
            [2.0 + cos(a), sin(a)]
        };
        Self {
            origin: [0.0; 3],
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
            loops: vec![
                (0..n)
                    .map(|i| BenchSegment::Line {
                        a: at(i),
                        b: at((i + 1) % n),
                    })
                    .collect(),
            ],
        }
    }

    /// The oriented profile flattened for the C++ shims: the frame
    /// (origin, x, y), each loop's segment count, and six numbers per
    /// segment: 0 and a line's ends, or 1 and an arc's centre, radius and
    /// angles.
    pub fn flatten(&self) -> ([f64; 9], Vec<u32>, Vec<f64>) {
        let p = self.oriented();
        let mut frame = [0.0; 9];
        frame[..3].copy_from_slice(&p.origin);
        frame[3..6].copy_from_slice(&p.x);
        frame[6..].copy_from_slice(&p.y);
        let sizes = p.loops.iter().map(|l| l.len() as u32).collect();
        let mut segments = Vec::new();
        for s in p.loops.iter().flatten() {
            segments.extend(match *s {
                BenchSegment::Line { a, b } => [0.0, a[0], a[1], b[0], b[1], 0.0],
                BenchSegment::Arc {
                    centre,
                    radius,
                    a0,
                    a1,
                } => [1.0, centre[0], centre[1], radius, a0, a1],
            });
        }
        (frame, sizes, segments)
    }
}

/// Curves as the comparison shims read them: each its exact NURBS form,
/// clamped, as `[degree, distinct knots, control points, rational]` in
/// `spec` and its knots, multiplicities, control points (x, y, z) and
/// weights (rational ones only) in `data`, one after another.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BenchCurves {
    pub spec: Vec<u32>,
    pub data: Vec<f64>,
}

impl BenchCurves {
    /// Adds `curve` over `range`.
    pub fn push(&mut self, curve: &ryukei::geom::Curve, range: [f64; 2]) -> Result<(), Error> {
        let (n, _) = curve.to_nurbs(range)?;
        let n = n.to_clamped();
        let k = n.knots();
        let rational = n.weights().iter().any(|&w| w != 1.0);
        let points: Vec<Point3> = n.control_points().collect();
        self.spec.extend([
            k.degree() as u32,
            k.knots().len() as u32,
            points.len() as u32,
            u32::from(rational),
        ]);
        self.data.extend_from_slice(k.knots());
        self.data.extend(k.mults().iter().map(|&m| m as f64));
        self.data
            .extend(points.iter().flat_map(|p| [p.x, p.y, p.z]));
        if rational {
            self.data.extend_from_slice(n.weights());
        }
        Ok(())
    }
}
