//! What changed since you looked (design 519, Pointing and highlighting):
//! owners whose hash moved between two scenes of one sheet glow in the
//! `changed` colour for [`GLOW`], fading. The scene's owner hashes make
//! it free.

use crate::core::scene::Scene;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long a change glows.
pub const GLOW: Duration = Duration::from_secs(2);

/// Owners glowing, by reference, with when they started.
#[derive(Debug, Default)]
pub struct Flash {
    started: BTreeMap<String, Instant>,
}

impl Flash {
    /// A scene arrived; `old` is the one it replaces. Another sheet (or
    /// block, or layer) clears what glowed.
    pub fn arrived(&mut self, old: Option<&Scene>, new: &Scene, now: Instant) {
        let Some(old) =
            old.filter(|o| o.block == new.block && o.sheet == new.sheet && o.layer == new.layer)
        else {
            self.started.clear();
            return;
        };
        self.started.retain(|r, _| new.owner(r).is_some());
        for o in &new.owners {
            let same = old
                .owner(&o.reference)
                .is_some_and(|i| old.owners[i].hash == o.hash);
            if !same {
                self.started.insert(o.reference.clone(), now);
            }
        }
    }

    /// How strongly `reference` glows: 1 when it changed, falling to 0
    /// over [`GLOW`].
    pub fn strength(&self, reference: &str, now: Instant) -> f32 {
        self.started.get(reference).map_or(0.0, |t| {
            let gone = now.saturating_duration_since(*t).as_secs_f32() / GLOW.as_secs_f32();
            (1.0 - gone).clamp(0.0, 1.0)
        })
    }

    pub fn glowing(&self) -> impl Iterator<Item = &str> {
        self.started.keys().map(String::as_str)
    }

    /// Drops what has stopped glowing.
    pub fn expire(&mut self, now: Instant) {
        self.started
            .retain(|_, t| now.saturating_duration_since(*t) < GLOW);
    }

    /// Whether the window must keep drawing to fade.
    pub fn due(&self) -> bool {
        !self.started.is_empty()
    }

    pub fn clear(&mut self) {
        self.started.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::{Scene, tests::small};
    use serde_json::json;

    #[test]
    fn changed_and_new_owners_glow_for_two_seconds_and_fade() {
        let t0 = Instant::now();
        let old = Scene::from_json(&small()).unwrap();
        let mut v = small();
        v["owners"][0]["hash"] = json!("h-r1-moved");
        v["owners"].as_array_mut().unwrap().push(json!(
            {"ref": "top/c9", "kind": "component", "net": null, "box": null, "hash": "new"}));
        v["owners"].as_array_mut().unwrap().remove(1);
        let new = Scene::from_json(&v).unwrap();
        let mut f = Flash::default();
        f.arrived(None, &old, t0);
        assert!(
            f.glowing().next().is_none(),
            "the first scene glows nothing"
        );
        f.arrived(Some(&old), &new, t0);
        let mut on: Vec<&str> = f.glowing().collect();
        on.sort();
        assert_eq!(on, ["top/c9", "top/r1"]);
        assert_eq!(f.strength("top/r1", t0), 1.0);
        let half = f.strength("top/r1", t0 + Duration::from_secs(1));
        assert!((half - 0.5).abs() < 1e-3, "{half}");
        assert_eq!(f.strength("top/label:sda@1", t0), 0.0);
        assert!(f.due());
        f.expire(t0 + GLOW);
        assert!(!f.due() && f.glowing().next().is_none());
    }

    #[test]
    fn another_sheet_starts_afresh() {
        let t0 = Instant::now();
        let a = Scene::from_json(&small()).unwrap();
        let mut v = small();
        v["sheet"] = json!("s2");
        v["owners"][0]["hash"] = json!("other");
        let b = Scene::from_json(&v).unwrap();
        let mut f = Flash::default();
        f.arrived(Some(&a), &b, t0);
        assert!(f.glowing().next().is_none());
    }
}
