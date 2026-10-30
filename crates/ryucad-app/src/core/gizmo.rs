//! Handles drawn over the viewport to move things in the model: an arrow
//! dragged along its axis and a ring turned about its axis. The window
//! draws them with egui's painter; this is the geometry, in viewport
//! pixels (y down) through the camera's [`Projector`]. Section planes and
//! explode steps share it.

use crate::core::nav::Projector;

/// An arrow from `from`, `length` meters along the unit `dir`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrow {
    pub from: [f64; 3],
    pub dir: [f64; 3],
    pub length: f64,
}

/// A ring of `radius` meters about the unit `axis` through `centre`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub centre: [f64; 3],
    pub axis: [f64; 3],
    pub radius: f64,
}

/// How near the pointer must be to a handle, in pixels, to grab it.
pub const REACH: f64 = 6.0;

/// A model point in viewport pixels, on a viewport `size` pixels big.
pub fn px(pr: &Projector, size: [f64; 2], p: [f64; 3]) -> Option<[f64; 2]> {
    let n = pr.ndc(p)?;
    Some([(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]])
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// How far `p` is from the segment `a`..`b`, in pixels.
fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = sub2(b, a);
    let len2 = dot2(ab, ab);
    let t = if len2 > 0.0 {
        (dot2(sub2(p, a), ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + t * ab[0] - p[0], a[1] + t * ab[1] - p[1]];
    dot2(q, q).sqrt()
}

impl Arrow {
    fn tip(&self) -> [f64; 3] {
        [0, 1, 2].map(|k| self.from[k] + self.dir[k] * self.length)
    }

    /// Its base and tip on screen.
    pub fn on_screen(&self, pr: &Projector, size: [f64; 2]) -> Option<([f64; 2], [f64; 2])> {
        Some((px(pr, size, self.from)?, px(pr, size, self.tip())?))
    }

    /// Whether the pointer at `at` is on it.
    pub fn near(&self, pr: &Projector, size: [f64; 2], at: [f64; 2]) -> bool {
        self.on_screen(pr, size)
            .is_some_and(|(a, b)| to_segment(at, a, b) <= REACH)
    }

    /// How far along its axis, in meters, a drag from `from` to `to`
    /// (pixels) takes it: the pointer's motion along the arrow as drawn.
    /// Nothing when the arrow points at the eye, and so has no length on
    /// screen.
    pub fn slide(&self, pr: &Projector, size: [f64; 2], from: [f64; 2], to: [f64; 2]) -> f64 {
        let Some((a, b)) = self.on_screen(pr, size) else {
            return 0.0;
        };
        let s = sub2(b, a);
        let len2 = dot2(s, s);
        if len2 < 1.0 {
            return 0.0;
        }
        dot2(sub2(to, from), s) / len2 * self.length
    }
}

impl Ring {
    /// Unit axes across the ring: its points are `centre + radius (cos t
    /// u + sin t v)`.
    fn across(&self) -> ([f64; 3], [f64; 3]) {
        let a = self.axis;
        let k = (0..3)
            .min_by(|x, y| a[*x].abs().total_cmp(&a[*y].abs()))
            .expect("three");
        let mut e = [0.0; 3];
        e[k] = 1.0;
        let cross = |p: [f64; 3], q: [f64; 3]| {
            [
                p[1] * q[2] - p[2] * q[1],
                p[2] * q[0] - p[0] * q[2],
                p[0] * q[1] - p[1] * q[0],
            ]
        };
        let u = cross(e, a);
        let l = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
        let u = u.map(|x| x / l);
        (u, cross(a, u))
    }

    /// The ring on screen, `n` points round it (the first again last).
    pub fn on_screen(&self, pr: &Projector, size: [f64; 2], n: usize) -> Vec<[f64; 2]> {
        let (u, v) = self.across();
        (0..=n)
            .filter_map(|i| {
                let t = std::f32::consts::TAU * i as f32 / n as f32;
                let (s, c) = (f64::from(t.sin()), f64::from(t.cos()));
                let p = [0, 1, 2].map(|k| self.centre[k] + self.radius * (c * u[k] + s * v[k]));
                px(pr, size, p)
            })
            .collect()
    }

    /// Whether the pointer at `at` is on it.
    pub fn near(&self, pr: &Projector, size: [f64; 2], at: [f64; 2]) -> bool {
        self.on_screen(pr, size, 48)
            .windows(2)
            .any(|w| to_segment(at, w[0], w[1]) <= REACH)
    }

    /// How far, in radians about its axis (right-handed), a drag from
    /// `from` to `to` (pixels) turns it: the pointer's turn about the
    /// ring's middle on screen, the way round the axis is seen from.
    pub fn turn(&self, pr: &Projector, size: [f64; 2], from: [f64; 2], to: [f64; 2]) -> f64 {
        let Some(c) = px(pr, size, self.centre) else {
            return 0.0;
        };
        let (a, b) = (sub2(from, c), sub2(to, c));
        if dot2(a, a) < 1.0 || dot2(b, b) < 1.0 {
            return 0.0;
        }
        // Counterclockwise on a screen whose y runs down is clockwise to
        // the eye: about the axis towards the eye, the turn is minus that.
        let cross = a[0] * b[1] - a[1] * b[0];
        let on_screen = -f64::from((cross as f32).atan2(dot2(a, b) as f32));
        let (_, _, forward) = pr.camera.basis();
        let towards_eye = -(0..3).map(|k| self.axis[k] * forward[k]).sum::<f64>();
        if towards_eye >= 0.0 {
            on_screen
        } else {
            -on_screen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::view::{Camera, Projection};

    /// Looking down -y at the origin, x right and z up, 2 m tall.
    fn front() -> (Projector, [f64; 2]) {
        let camera = Camera {
            eye: [0.0, -10.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Orthographic { height: 2.0 },
        };
        (Projector::new(&camera, 1.0, [0.0; 3], 1.0), [200.0, 200.0])
    }

    #[test]
    fn an_arrow_slides_as_the_pointer_runs_along_it() {
        let (pr, size) = front();
        let a = Arrow {
            from: [0.0; 3],
            dir: [1.0, 0.0, 0.0],
            length: 0.5,
        };
        let (base, tip) = a.on_screen(&pr, size).unwrap();
        assert_eq!(base, [100.0, 100.0]);
        assert_eq!(tip, [150.0, 100.0]);
        assert!(a.near(&pr, size, [120.0, 104.0]));
        assert!(!a.near(&pr, size, [120.0, 110.0]));
        // 25 pixels along it is a quarter of a meter; across it, nothing.
        assert_eq!(a.slide(&pr, size, [120.0, 90.0], [145.0, 90.0]), 0.25);
        assert_eq!(a.slide(&pr, size, [120.0, 90.0], [120.0, 40.0]), 0.0);
        // Pointing at the eye, it doesn't slide.
        let at_eye = Arrow {
            dir: [0.0, -1.0, 0.0],
            ..a
        };
        assert_eq!(at_eye.slide(&pr, size, [0.0, 0.0], [50.0, 50.0]), 0.0);
    }

    #[test]
    fn a_ring_turns_as_the_pointer_goes_round_it() {
        let (pr, size) = front();
        // About y, away from the eye: seen from -y.
        let r = Ring {
            centre: [0.0; 3],
            axis: [0.0, 1.0, 0.0],
            radius: 0.5,
        };
        assert!(r.near(&pr, size, [150.0, 100.0]));
        assert!(!r.near(&pr, size, [100.0, 100.0]));
        // From +x on screen (right) to +z (up): about +y that is -90°.
        let t = r.turn(&pr, size, [150.0, 100.0], [100.0, 50.0]);
        assert!((t + std::f64::consts::FRAC_PI_2).abs() < 1e-6, "{t}");
        let back = Ring {
            axis: [0.0, -1.0, 0.0],
            ..r
        };
        let t = back.turn(&pr, size, [150.0, 100.0], [100.0, 50.0]);
        assert!((t - std::f64::consts::FRAC_PI_2).abs() < 1e-6, "{t}");
        // At the middle, no turn.
        assert_eq!(r.turn(&pr, size, [100.0, 100.0], [150.0, 100.0]), 0.0);
    }
}
