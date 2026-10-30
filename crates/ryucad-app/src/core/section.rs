//! The window's section planes, apart from egui: what its participant
//! cuts with (`participant.changed`), the plane the Section tile and the
//! viewport's gizmo work on, and a drag of the gizmo, which moves the
//! planes on screen at once and is written once, with `section.set`, when
//! let go.

use crate::core::gizmo::{Arrow, Ring};
use crate::core::nav::Projector;
use ryucad::proto::Request;
use ryucad::section::{Plane, PlaneArg};
use serde_json::json;

/// What the gizmo's drag holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grip {
    /// The arrow along the normal.
    Arrow,
    /// One of the rings about the plane's own axes ([`Plane::axes`]).
    Ring(usize),
}

/// A drag of the gizmo: what it holds, the planes when it began and
/// where the pointer was.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub grip: Grip,
    pub start: Vec<Plane>,
    pub from: [f64; 2],
}

/// The window's planes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sections {
    /// What the window cuts with: its participant's planes, or where a
    /// drag or the tile has them until the session says otherwise.
    pub planes: Vec<Plane>,
    /// The plane the gizmo and the tile's selection are on.
    pub active: usize,
    pub dragging: Option<Drag>,
    /// Whether the viewport shows the active plane's handles (the Section
    /// tile's choice).
    pub handles: bool,
}

impl Sections {
    /// Takes its participant's planes, unless a drag holds them. Returns
    /// whether anything drawn changed.
    pub fn follow(&mut self, planes: &[Plane]) -> bool {
        if self.dragging.is_some() || self.planes == planes {
            return false;
        }
        self.planes = planes.to_vec();
        if self.active >= self.planes.len() {
            self.active = self.planes.len().saturating_sub(1);
        }
        true
    }

    /// The active plane's handles, `size` meters long (a tenth of the
    /// model's diagonal): an arrow along its normal and a ring about each
    /// of its axes.
    pub fn gizmo(&self, size: f64) -> Option<(Arrow, [Ring; 2])> {
        let p = self.planes.get(self.active)?;
        let (u, v) = p.axes();
        let ring = |axis| Ring {
            centre: p.origin_m,
            axis,
            radius: size * 0.8,
        };
        Some((
            Arrow {
                from: p.origin_m,
                dir: p.normal,
                length: size,
            },
            [ring(u), ring(v)],
        ))
    }

    /// Takes hold of the gizmo if the pointer at `at` is on a handle.
    pub fn grab(&mut self, pr: &Projector, size: [f64; 2], at: [f64; 2], length: f64) -> bool {
        let Some((arrow, rings)) = self.gizmo(length) else {
            return false;
        };
        let grip = if arrow.near(pr, size, at) {
            Grip::Arrow
        } else if let Some(k) = (0..2).find(|k| rings[*k].near(pr, size, at)) {
            Grip::Ring(k)
        } else {
            return false;
        };
        self.dragging = Some(Drag {
            grip,
            start: self.planes.clone(),
            from: at,
        });
        true
    }

    /// Moves the held plane as the pointer is now at `at`.
    pub fn drag(&mut self, pr: &Projector, size: [f64; 2], at: [f64; 2], length: f64) {
        let Some(d) = &self.dragging else { return };
        let Some(was) = d.start.get(self.active).copied() else {
            return;
        };
        let held = Sections {
            planes: d.start.clone(),
            active: self.active,
            dragging: None,
            handles: true,
        };
        let Some((arrow, rings)) = held.gizmo(length) else {
            return;
        };
        let now = match d.grip {
            Grip::Arrow => was.moved(arrow.slide(pr, size, d.from, at)),
            Grip::Ring(k) => {
                let angle = rings[k].turn(pr, size, d.from, at);
                was.turned(rings[k].axis, angle)
            }
        };
        self.planes[self.active] = now;
    }

    /// Lets go: the planes to write, if the drag moved any.
    pub fn release(&mut self) -> Option<Vec<Plane>> {
        let d = self.dragging.take()?;
        (d.start != self.planes).then(|| self.planes.clone())
    }
}

/// `section.set` making `participant`'s planes `planes` exactly.
pub fn request(planes: &[Plane], participant: &str) -> Request {
    let off = planes.is_empty();
    Request::SectionSet {
        planes: planes
            .iter()
            .map(|p| PlaneArg {
                origin: Some(p.origin_m.map(|x| json!(format!("{x} m")))),
                normal: Some(p.normal),
                ..PlaneArg::default()
            })
            .collect(),
        add: false,
        off,
        participant: Some(participant.to_owned()),
    }
}

/// `section.set` adding the plane `plane` names (`xy`, `yz`, `zx` or a
/// face's reference) to `participant`'s.
pub fn add(plane: &str, participant: &str) -> Request {
    Request::SectionSet {
        planes: vec![PlaneArg {
            plane: Some(plane.to_owned()),
            ..PlaneArg::default()
        }],
        add: true,
        off: false,
        participant: Some(participant.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::view::{Camera, Projection};

    fn front() -> (Projector, [f64; 2]) {
        let camera = Camera {
            eye: [0.0, -10.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Orthographic { height: 2.0 },
        };
        (Projector::new(&camera, 1.0, [0.0; 3], 1.0), [200.0, 200.0])
    }

    fn xy() -> Plane {
        Plane::new([0.0; 3], [0.0, 0.0, 1.0]).unwrap()
    }

    #[test]
    fn the_arrow_moves_a_plane_along_its_normal() {
        let (pr, size) = front();
        let mut s = Sections::default();
        assert!(s.follow(&[xy()]));
        assert!(!s.follow(&[xy()]));
        // The arrow runs up from the middle, 50 pixels for 0.5 m.
        assert!(!s.grab(&pr, size, [10.0, 10.0], 0.5));
        assert!(s.grab(&pr, size, [100.0, 80.0], 0.5));
        s.drag(&pr, size, [100.0, 60.0], 0.5);
        assert!((s.planes[0].origin_m[2] - 0.2).abs() < 1e-12);
        // While held, the session's planes don't move it.
        assert!(!s.follow(&[xy()]));
        s.drag(&pr, size, [100.0, 70.0], 0.5);
        let wrote = s.release().unwrap();
        assert!((wrote[0].origin_m[2] - 0.1).abs() < 1e-12);
        assert_eq!(s.dragging, None);
        // A grab let go where it began writes nothing.
        assert!(s.grab(&pr, size, [100.0, 80.0], 0.5));
        assert_eq!(s.release(), None);
    }

    #[test]
    fn a_ring_turns_a_plane_about_its_axis() {
        let (pr, size) = front();
        let mut s = Sections::default();
        let side = Plane::new([0.0; 3], [0.0, -1.0, 0.0]).unwrap();
        s.follow(&[xy(), side]);
        s.active = 1;
        // Facing the eye, its rings are about x and z: the one about z
        // shows as a line across the screen, at the middle's height.
        let (_, rings) = s.gizmo(0.5).unwrap();
        let k = (0..2).find(|k| rings[*k].axis[2].abs() > 0.5).unwrap();
        let ring = rings[k];
        assert!(s.grab(&pr, size, [140.0, 100.0], 0.5));
        assert_eq!(s.dragging.as_ref().unwrap().grip, Grip::Ring(k));
        s.drag(&pr, size, [100.0, 60.0], 0.5);
        let n = s.planes[1].normal;
        let turned = side.turned(
            ring.axis,
            ring.turn(&pr, size, [140.0, 100.0], [100.0, 60.0]),
        );
        assert_eq!(n, turned.normal);
        assert!(s.release().is_some());
    }

    #[test]
    fn requests_say_exactly_the_planes() {
        let p = Plane::new([0.0, 0.001, 0.5], [0.0, 0.0, 1.0]).unwrap();
        let v = request(&[p], "p-1").params();
        assert_eq!(
            v,
            json!({"planes": [{"origin": ["0 m", "0.001 m", "0.5 m"], "normal": [0.0, 0.0, 1.0]}],
                   "participant": "p-1"})
        );
        assert_eq!(
            request(&[], "p-1").params(),
            json!({"off": true, "participant": "p-1"})
        );
        assert_eq!(
            add("xy", "p-1").params(),
            json!({"planes": [{"plane": "xy"}], "add": true, "participant": "p-1"})
        );
    }
}
