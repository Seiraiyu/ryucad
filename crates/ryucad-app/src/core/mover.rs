//! The Move tool's handles (design 374, phase 5): three arrows along the
//! assembly's axes (or the instance's own) and three rings about them, at
//! the middle of an instance's drawn boxes. A drag previews and leaves a
//! value to send; the value can be typed (see [`Mover::typed`]). The
//! requests are `assembly.solve` (the preview) and `assembly.move`, which
//! the workspace sends through its mode gate. Geometry is
//! [`crate::core::gizmo`]'s.

use crate::core::gizmo::{Arrow, Ring};
use crate::core::model::Pose;
use crate::core::nav::Projector;
use ryucad::proto::{InstanceMove, Request};
use ryucad::units::{Field, Quantity, Unit};
use serde_json::{Value, json};

/// A turn's snap: 15°.
pub const ANGLE_STEP: f64 = std::f64::consts::PI / 12.0;

/// Which handle a drag holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Arrow(usize),
    Ring(usize),
}

/// A handle held: which, and where the pointer took it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Held {
    pub grip: Grip,
    pub from: [f64; 2],
}

/// The Move tool on one instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Mover {
    pub part: String,
    pub instance: String,
    /// Where the handles stand: the middle of the instance's boxes.
    pub middle: [f64; 3],
    /// The instance's pose, for its own axes.
    pub pose: Pose,
    /// Along the instance's own axes, not the assembly's.
    pub own: bool,
    pub held: Option<Held>,
    /// The handle the value is for, after a drag or a click.
    pub grip: Option<Grip>,
    /// Meters along the arrow, or radians about the ring.
    pub value: f64,
    /// The value field's text while typed (empty: it shows the value).
    pub text: String,
    /// Why the typed text doesn't read, or the preview was refused.
    pub error: Option<String>,
}

impl Mover {
    pub fn new(part: &str, instance: &str, middle: [f64; 3], pose: Pose, own: bool) -> Mover {
        Mover {
            part: part.to_owned(),
            instance: instance.to_owned(),
            middle,
            pose,
            own,
            held: None,
            grip: None,
            value: 0.0,
            text: String::new(),
            error: None,
        }
    }

    /// The axes the handles run along.
    pub fn axes(&self) -> [[f64; 3]; 3] {
        if self.own {
            [self.pose.x, self.pose.y, self.pose.z]
        } else {
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        }
    }

    /// Its arrows and rings, `size` meters long (a tenth of the model's
    /// diagonal, as section handles are).
    pub fn handles(&self, size: f64) -> ([Arrow; 3], [Ring; 3]) {
        let axes = self.axes();
        (
            axes.map(|dir| Arrow {
                from: self.middle,
                dir,
                length: size,
            }),
            axes.map(|axis| Ring {
                centre: self.middle,
                axis,
                radius: size * 0.8,
            }),
        )
    }

    /// The handle under `at`, arrows before rings.
    pub fn under(&self, pr: &Projector, size: [f64; 2], at: [f64; 2], length: f64) -> Option<Grip> {
        let (arrows, rings) = self.handles(length);
        if let Some(k) = (0..3).find(|k| arrows[*k].near(pr, size, at)) {
            Some(Grip::Arrow(k))
        } else {
            (0..3)
                .find(|k| rings[*k].near(pr, size, at))
                .map(Grip::Ring)
        }
    }

    /// Takes hold of a handle under `at`, arrows before rings. A new grip
    /// starts its value at 0.
    pub fn grab(&mut self, pr: &Projector, size: [f64; 2], at: [f64; 2], length: f64) -> bool {
        let Some(grip) = self.under(pr, size, at, length) else {
            return false;
        };
        self.held = Some(Held { grip, from: at });
        self.grip = Some(grip);
        self.value = 0.0;
        self.text.clear();
        self.error = None;
        true
    }

    /// Follows the pointer at `at`, the value snapped to `snap` (meters
    /// for an arrow, radians for a ring) when given.
    pub fn drag(
        &mut self,
        pr: &Projector,
        size: [f64; 2],
        at: [f64; 2],
        length: f64,
        snap: Option<f64>,
    ) {
        let Some(h) = self.held else { return };
        let (arrows, rings) = self.handles(length);
        let raw = match h.grip {
            Grip::Arrow(k) => arrows[k].slide(pr, size, h.from, at),
            Grip::Ring(k) => rings[k].turn(pr, size, h.from, at),
        };
        self.value = snap.map_or(raw, |s| snapped(raw, s));
        self.text.clear();
        self.error = None;
    }

    /// Lets go of the handle; the value stays to send.
    pub fn release(&mut self) {
        self.held = None;
    }

    /// Drops the value (the handles stay).
    pub fn clear(&mut self) {
        self.held = None;
        self.grip = None;
        self.value = 0.0;
        self.text.clear();
        self.error = None;
    }

    /// The move the value makes, in meters, if an arrow has it.
    pub fn translate(&self) -> Option<[f64; 3]> {
        let Some(Grip::Arrow(k)) = self.grip else {
            return None;
        };
        let d = self.axes()[k];
        Some(d.map(|x| x * self.value))
    }

    /// The turn the value makes (axis, radians), if a ring has it.
    pub fn turn(&self) -> Option<([f64; 3], f64)> {
        let Some(Grip::Ring(k)) = self.grip else {
            return None;
        };
        Some((self.axes()[k], self.value))
    }

    /// Whether the value is a turn.
    fn turning(&self) -> bool {
        matches!(self.grip, Some(Grip::Ring(_)))
    }

    /// Reads typed `text` as the value: a length (a bare number in
    /// `unit`) for an arrow, an angle (bare: degrees) for a ring. Text
    /// that doesn't read keeps the last value and says why.
    pub fn typed(&mut self, text: &str, unit: Unit) {
        self.text = text.to_owned();
        let field = if self.turning() {
            Field::Angle
        } else {
            Field::Length(unit)
        };
        match Quantity::from_json_as(&json!(text.trim()), field, &ryucad::expr::no_parameters) {
            Ok(q) => {
                self.value = if self.turning() {
                    q.radians()
                } else {
                    q.meters()
                };
                self.error = None;
            }
            Err(e) => self.error = Some(e.message),
        }
    }

    /// The value as the field shows it: in `unit`, or degrees.
    pub fn shown(&self, unit: Unit) -> String {
        if self.turning() {
            let d = (self.value.to_degrees() * 1e6).round() / 1e6;
            format!("{} deg", d + 0.0)
        } else {
            let x = (self.value / unit.factor() * 1e6).round() / 1e6;
            format!("{} {}", x + 0.0, unit.as_str())
        }
    }

    /// The field's text: what was typed, else the value.
    pub fn field(&self, unit: Unit) -> String {
        if self.text.is_empty() {
            self.shown(unit)
        } else {
            self.text.clone()
        }
    }

    /// Whether there is a value to send.
    pub fn ready(&self) -> bool {
        self.grip.is_some() && self.value != 0.0 && self.error.is_none()
    }

    fn how(&self) -> InstanceMove {
        let mut how = InstanceMove {
            instance: self.instance.clone(),
            ..Default::default()
        };
        if let Some(t) = self.translate() {
            how.translate = Some(t.map(m));
        } else if let Some((axis, angle)) = self.turn() {
            how.axis = Some(axis);
            let d = (angle.to_degrees() * 1e9).round() / 1e9;
            how.angle = Some(Value::from(format!("{} deg", d + 0.0)));
            how.about = Some(self.middle.map(m));
        }
        how
    }

    /// `assembly.move` of the value, against the assembly's revision.
    pub fn request(&self, expected_revision: Option<String>) -> Request {
        let how = self.how();
        Request::AssemblyMove {
            part: self.part.clone(),
            instance: self.instance.clone(),
            placement: None,
            translate: how.translate,
            axis: how.axis,
            angle: how.angle,
            about: how.about,
            expected_revision,
        }
    }

    /// The preview: `assembly.solve` of the same move, held where it is
    /// put, only what the instance's mates reach, at `rev` (the drawn
    /// revision: a person's draft in Annotate).
    pub fn preview(&self, rev: Option<String>) -> Request {
        Request::AssemblySolve {
            part: self.part.clone(),
            moves: vec![self.how()],
            component: true,
            rev,
            mates: Vec::new(),
        }
    }
}

/// A move's snap in a project of `unit`: 1 mm in metric units, 0.1 in in
/// imperial ones; a tenth with Shift.
pub fn length_step(unit: Unit, shift: bool) -> f64 {
    let step = match unit {
        Unit::In | Unit::Ft => 0.00254,
        _ => 0.001,
    };
    if shift { step / 10.0 } else { step }
}

/// A turn's snap; a tenth with Shift.
pub fn angle_step(shift: bool) -> f64 {
    if shift { ANGLE_STEP / 10.0 } else { ANGLE_STEP }
}

/// `value` to the nearest multiple of `step`, written as the shortest
/// decimal near it (so 24 mm snapped is 0.024, not 0.024000000000000004).
pub fn snapped(value: f64, step: f64) -> f64 {
    if step > 0.0 {
        let n = (value / step).round() * step;
        (n * 1e12).round() / 1e12
    } else {
        value
    }
}

/// A length in meters as a request takes it.
fn m(x: f64) -> Value {
    Value::from(format!("{} m", x + 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::view::{Camera, Projection};

    /// Looking down +y at the origin, x right and z up, 2 m tall, on a
    /// 200-pixel viewport: 100 pixels a meter, the origin at (100, 100).
    fn front() -> (Projector, [f64; 2]) {
        let camera = Camera {
            eye: [0.0, -10.0, 0.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
            projection: Projection::Orthographic { height: 2.0 },
        };
        (Projector::new(&camera, 1.0, [0.0; 3], 1.0), [200.0, 200.0])
    }

    fn mover(own: bool) -> Mover {
        let turned = Pose::of([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
        Mover::new("frame", "b2", [0.0; 3], turned, own)
    }

    #[test]
    fn handles_run_along_the_assemblys_axes_or_the_instances() {
        let (arrows, rings) = mover(false).handles(0.5);
        assert_eq!(
            arrows.map(|a| a.dir),
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert_eq!(rings[2].axis, [0.0, 0.0, 1.0]);
        let (arrows, _) = mover(true).handles(0.5);
        assert_eq!(arrows[0].dir, [0.0, 1.0, 0.0], "the instance's own x");
    }

    #[test]
    fn a_drag_along_an_arrow_moves_by_its_slide() {
        let (pr, size) = front();
        let mut m = mover(false);
        // x's arrow runs 50 px right of the middle (z's ring lies on the
        // same line, edge-on, but arrows are grabbed first).
        assert!(m.grab(&pr, size, [130.0, 100.0], 0.5));
        m.drag(&pr, size, [155.0, 100.0], 0.5, None);
        assert_eq!(m.held.unwrap().grip, Grip::Arrow(0));
        assert!((m.value - 0.25).abs() < 1e-12, "{}", m.value);
        let t = m.translate().unwrap();
        assert!((t[0] - 0.25).abs() < 1e-12 && t[1] == 0.0 && t[2] == 0.0);
        assert_eq!(m.turn(), None);
    }

    #[test]
    fn a_drag_round_a_ring_turns_about_the_middle() {
        let (pr, size) = front();
        let mut m = mover(false);
        // y's ring faces the eye, 40 px round the middle: grab it below and
        // to the right (off the x and z rings, edge-on along the axes,
        // and off the arrows), and go round to the top.
        let r = 40.0 / 2f64.sqrt();
        assert!(m.grab(&pr, size, [100.0 + r, 100.0 + r], 0.5));
        assert_eq!(m.held.unwrap().grip, Grip::Ring(1));
        m.drag(&pr, size, [100.0, 60.0], 0.5, None);
        let (axis, angle) = m.turn().unwrap();
        assert_eq!(axis, [0.0, 1.0, 0.0]);
        let quarter_and_half = 3.0 * std::f64::consts::FRAC_PI_4;
        // Right-handed about +y, from below right to the top, is minus.
        assert!((angle + quarter_and_half).abs() < 1e-5, "{angle}");
    }

    #[test]
    fn nothing_is_grabbed_off_the_handles() {
        let (pr, size) = front();
        let mut m = mover(false);
        assert!(!m.grab(&pr, size, [10.0, 10.0], 0.5));
        assert!(m.held.is_none());
    }

    #[test]
    fn steps_follow_the_default_unit_and_shift_takes_a_tenth() {
        assert_eq!(length_step(Unit::Mm, false), 0.001);
        assert_eq!(length_step(Unit::M, false), 0.001);
        assert!((length_step(Unit::In, false) - 0.00254).abs() < 1e-15);
        assert!((length_step(Unit::Mm, true) - 0.0001).abs() < 1e-15);
        assert!((ANGLE_STEP - 15f64.to_radians()).abs() < 1e-15);
        assert_eq!(snapped(0.0237, 0.001), 0.024);
    }

    #[test]
    fn typed_values_read_as_quantities_and_show_in_the_unit() {
        let mut m = Mover::new("frame", "b2", [0.0; 3], Pose::IDENTITY, false);
        m.grip = Some(Grip::Arrow(0));
        m.typed("20", Unit::Mm);
        assert!((m.value - 0.02).abs() < 1e-15 && m.error.is_none());
        m.typed("1 in", Unit::Mm);
        assert!((m.value - 0.0254).abs() < 1e-15);
        m.typed("twenty", Unit::Mm);
        assert!(m.error.is_some(), "kept the last value and said why");
        assert!((m.value - 0.0254).abs() < 1e-15);
        m.grip = Some(Grip::Ring(2));
        m.typed("15", Unit::Mm);
        assert!(
            (m.value - 15f64.to_radians()).abs() < 1e-12,
            "a bare turn is degrees"
        );
        m.value = 0.02;
        m.grip = Some(Grip::Arrow(0));
        assert_eq!(m.shown(Unit::Mm), "20 mm");
        m.grip = Some(Grip::Ring(0));
        m.value = std::f64::consts::FRAC_PI_4;
        assert_eq!(m.shown(Unit::Mm), "45 deg");
    }

    #[test]
    fn a_turn_is_sent_about_the_middle_and_a_move_as_meters() {
        let mut m = Mover::new("frame", "b2", [0.01, 0.0, 0.0], Pose::IDENTITY, false);
        m.grip = Some(Grip::Arrow(2));
        m.value = 0.02;
        let r = m.request(Some("r1".into()));
        assert_eq!(r.method(), "assembly.move");
        let v = r.params();
        assert_eq!(v["translate"], json!(["0 m", "0 m", "0.02 m"]));
        assert_eq!(v["expected_revision"], "r1");
        m.grip = Some(Grip::Ring(2));
        m.value = std::f64::consts::FRAC_PI_2;
        let v = m.request(None).params();
        assert_eq!(v["axis"], json!([0.0, 0.0, 1.0]));
        assert_eq!(v["about"], json!(["0.01 m", "0 m", "0 m"]));
        assert_eq!(v["angle"], json!("90 deg"));
        // The preview solves the same move, held, only what it reaches.
        let p = m.preview(Some("draft:dave".into())).params();
        assert_eq!(p["component"], true);
        assert_eq!(p["rev"], "draft:dave");
        assert_eq!(p["moves"][0]["about"], v["about"]);
    }
}
