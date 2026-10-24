//! The window's geometry, kept in the `app.window` preference: its size in
//! points and whether it is maximized, so the next window opens as the
//! last one was left. Its position isn't kept: a monitor it was on may be
//! gone, and a window placed off every screen can't be found.
//!
//! A geometry is saved once it has stopped changing for [`SETTLE`], so a
//! drag along a window's edge writes the preference once, not every frame.

use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

/// How long a geometry must hold before it is saved.
pub const SETTLE: Duration = Duration::from_secs(1);

/// The sizes a saved window may have, in points: anything else in the
/// preference (edited by hand, from another screen) is ignored.
const SIZES: std::ops::RangeInclusive<f32> = 200.0..=16384.0;

/// A window's size in points and whether it is maximized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub size: [f32; 2],
    pub maximized: bool,
}

impl Geometry {
    /// The geometry the preference holds, if it holds a sensible one.
    pub fn from_value(v: &Value) -> Option<Geometry> {
        let s = v.get("size")?.as_array()?;
        let size = match s.as_slice() {
            [w, h] => [w.as_f64()? as f32, h.as_f64()? as f32],
            _ => return None,
        };
        if !size.iter().all(|x| SIZES.contains(x)) {
            return None;
        }
        Some(Geometry {
            size,
            maximized: v.get("maximized").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// `pref` (the preference as it is) with this geometry in it, keeping
    /// whatever else it holds (the panel layout, later).
    pub fn into_value(self, pref: &Value) -> Value {
        let mut o = pref.as_object().cloned().unwrap_or_else(Map::new);
        o.insert(
            "size".to_owned(),
            json!([self.size[0].round(), self.size[1].round()]),
        );
        o.insert("maximized".to_owned(), json!(self.maximized));
        Value::Object(o)
    }
}

/// Follows the window's geometry frame by frame and says when to save it.
#[derive(Debug, Default)]
pub struct Tracker {
    /// What the preference holds, or was last asked to.
    saved: Option<Geometry>,
    /// The geometry being watched settle, and since when.
    seen: Option<(Geometry, Instant)>,
    /// The size the window had when it was last not maximized: a maximized
    /// window is saved with it, so it comes back that size when restored.
    normal: Option<[f32; 2]>,
}

impl Tracker {
    /// A tracker for a window opened at `saved`.
    pub fn new(saved: Option<Geometry>) -> Tracker {
        Tracker {
            saved,
            seen: None,
            normal: saved.map(|g| g.size),
        }
    }

    /// The window as it is now, with `size` its inner size in points.
    /// Returns the geometry to save when it has settled and differs from
    /// the one saved; then it counts as saved.
    pub fn observe(&mut self, size: [f32; 2], maximized: bool, now: Instant) -> Option<Geometry> {
        // A window too small to be one (minimized, say) changes nothing.
        if !size.iter().all(|x| SIZES.contains(x)) {
            return None;
        }
        if !maximized {
            self.normal = Some(size);
        }
        let g = Geometry {
            size: self.normal.unwrap_or(size),
            maximized,
        };
        match self.seen {
            Some((was, since)) if was == g => {
                if now.duration_since(since) >= SETTLE && self.saved != Some(g) {
                    self.saved = Some(g);
                    return Some(g);
                }
            }
            _ => self.seen = Some((g, now)),
        }
        None
    }

    /// Whether a geometry is waiting to settle: the window must draw again
    /// after [`SETTLE`] to save it.
    pub fn pending(&self) -> bool {
        self.seen.is_some_and(|(g, _)| self.saved != Some(g))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preference_is_read_only_when_sensible() {
        let g = Geometry::from_value(&json!({"size": [1000, 700], "maximized": true})).unwrap();
        assert_eq!(
            g,
            Geometry {
                size: [1000.0, 700.0],
                maximized: true
            }
        );
        for v in [
            json!({}),
            json!({"size": [1000]}),
            json!({"size": [10, 700]}),
            json!({"size": ["1000", 700]}),
            json!([]),
        ] {
            assert_eq!(Geometry::from_value(&v), None, "{v}");
        }
        // Writing keeps what else the preference holds.
        let v = g.into_value(&json!({"panels": {"left": 300}}));
        assert_eq!(
            v,
            json!({"maximized": true, "panels": {"left": 300}, "size": [1000.0, 700.0]})
        );
        assert_eq!(Geometry::from_value(&v), Some(g));
    }

    #[test]
    fn a_geometry_is_saved_once_it_settles() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut t = Tracker::new(Some(Geometry {
            size: [1280.0, 800.0],
            maximized: false,
        }));
        // As it was saved: nothing to write.
        assert_eq!(t.observe([1280.0, 800.0], false, at(0)), None);
        assert_eq!(t.observe([1280.0, 800.0], false, at(2000)), None);
        assert!(!t.pending());
        // Dragged: written once it has held still for a second.
        assert_eq!(t.observe([1100.0, 800.0], false, at(2100)), None);
        assert_eq!(t.observe([1000.0, 750.0], false, at(2200)), None);
        assert!(t.pending());
        assert_eq!(t.observe([1000.0, 750.0], false, at(3000)), None);
        let g = t.observe([1000.0, 750.0], false, at(3200)).unwrap();
        assert_eq!(g.size, [1000.0, 750.0]);
        assert_eq!(t.observe([1000.0, 750.0], false, at(9000)), None);
        // Maximized: saved as maximized, with the size it had before.
        assert_eq!(t.observe([2560.0, 1400.0], true, at(10_000)), None);
        let g = t.observe([2560.0, 1400.0], true, at(11_000)).unwrap();
        assert_eq!(
            g,
            Geometry {
                size: [1000.0, 750.0],
                maximized: true
            }
        );
        // A window too small to be one (minimized) is never saved.
        assert_eq!(t.observe([0.0, 0.0], false, at(12_000)), None);
        assert_eq!(t.observe([0.0, 0.0], false, at(14_000)), None);
    }
}
