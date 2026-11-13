//! Cosmetic threads: a tapped hole's thread as data on its `wall` faces,
//! for the app's rings, renders and (28.D) drawings. Never kernel
//! geometry: derived from the holes the rebuild made, cached with them,
//! never on disk.

use crate::holes::{Holes, Row};
use ryukei::geom::{Point3, Vector3};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// The hole's reference.
    pub hole: String,
    pub spec: String,
    pub designation: String,
    pub standard: String,
    /// Metres; a pipe thread's (no built-in major) is its drill's.
    pub major: f64,
    /// The minor diameter's limits, when the standard gives them.
    pub minor: Option<[f64; 2]>,
    pub pitch: f64,
    pub lead: f64,
    pub left: bool,
    pub starts: u32,
    pub class: Option<String>,
    /// Internal (a tapped hole). External threads arrive with 28.B.
    pub internal: bool,
    /// The `wall` faces it lies on, as `<part>/<solid>/<name>`.
    pub faces: Vec<String>,
    pub axis: Vector3,
    /// Where it starts (the wall's top) and ends, on the axis.
    pub mouth: Point3,
    pub end: Point3,
}

impl Thread {
    pub fn to_json(&self) -> Value {
        let p = |x: Point3| json!([x.x, x.y, x.z]);
        json!({
            "hole": self.hole, "spec": self.spec, "designation": self.designation,
            "standard": self.standard, "major": self.major, "minor": self.minor,
            "pitch": self.pitch, "lead": self.lead,
            "hand": if self.left { "left" } else { "right" }, "starts": self.starts,
            "class": self.class, "internal": self.internal, "faces": self.faces,
            "axis": [self.axis.x, self.axis.y, self.axis.z],
            "mouth": p(self.mouth), "end": p(self.end),
        })
    }
}

/// Every tapped hole's thread, in the holes' order.
pub fn of(holes: &Holes) -> Vec<Thread> {
    holes.rows.iter().filter_map(of_row).collect()
}

/// A row's thread, if it is tapped.
pub fn of_row(r: &Row) -> Option<Thread> {
    let t = r.sizes.thread.as_ref()?;
    let start = r.sizes.thread_start();
    let length = r.depth.2.unwrap_or(r.depth.0);
    Some(Thread {
        hole: r.reference.clone(),
        spec: r.spec.clone(),
        designation: t.designation.clone(),
        standard: t.standard.clone(),
        major: t.major.as_ref().map_or(r.sizes.drill.si, |m| m.si),
        minor: t.minor.as_ref().map(|[a, b]| [a.si, b.si]),
        pitch: t.pitch.si,
        lead: t.lead.si,
        left: r.left,
        starts: t.starts,
        class: t.class.clone(),
        internal: true,
        faces: r
            .faces
            .get("wall")
            .into_iter()
            .flatten()
            .map(|n| format!("{}/{}/{n}", r.part, r.solid))
            .collect(),
        axis: r.axis,
        mouth: r.position + r.axis * start,
        end: r.position + r.axis * length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::id::Id;
    use crate::hole::tests::{drilled, put};
    use serde_json::json;

    #[test]
    fn a_tapped_holes_thread() {
        let (_dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M3", "depth": "2.5 mm"},
                   "depth": {"blind": "4 mm"}}),
        );
        let holes = crate::holes::list(&mut s, &Id::parse("p", "part").unwrap(), None).unwrap();
        let t = of(&holes);
        assert_eq!(t.len(), 1);
        let t = &t[0];
        assert_eq!(t.hole, "p/h/p1");
        assert_eq!(t.designation, "M3x0.5-6H");
        assert!(t.internal && !t.left);
        assert_eq!((t.major, t.pitch), (0.003, 0.0005));
        // From the mouth (the box's top, z = 5 mm) down 2.5 mm.
        assert!((t.mouth - Point3::new(0.010, 0.005, 0.005)).length() < 1e-12);
        assert!((t.end - Point3::new(0.010, 0.005, 0.0025)).length() < 1e-12);
        assert_eq!(t.faces.len(), 1);
        assert!(t.faces[0].starts_with("p/b/"), "{:?}", t.faces);
        let v = t.to_json();
        assert_eq!(v["designation"], "M3x0.5-6H");
        // ISO 965-1's formula (T_D1(6) at 0.5 mm, 132 µm): metric limits
        // are the formula's, never printed values (28.A's Task 6).
        assert_eq!(v["minor"], json!([0.002459, 0.002591]));
    }
}
