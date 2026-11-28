//! What changed since you looked (design 938, Pointing): an item that
//! `project.changed` names (`item/<id>`) glows in the `changed` colour
//! for [`GLOW`], fading, wherever its row is shown. `ui.flash_changes`
//! turns it off.

use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const GLOW: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct Flash {
    started: BTreeMap<String, Instant>,
}

impl Flash {
    /// `project.changed`'s `changed`: its items start glowing.
    pub fn changed(&mut self, changed: &Value, now: Instant) {
        for k in changed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if k.starts_with("item/") {
                self.started.insert(k.to_owned(), now);
            }
        }
    }

    /// 1 when it changed, falling to 0 over [`GLOW`].
    pub fn strength(&self, reference: &str, now: Instant) -> f32 {
        self.started.get(reference).map_or(0.0, |t| {
            let gone = now.saturating_duration_since(*t).as_secs_f32() / GLOW.as_secs_f32();
            (1.0 - gone).clamp(0.0, 1.0)
        })
    }

    pub fn glowing(&self) -> impl Iterator<Item = &str> {
        self.started.keys().map(String::as_str)
    }

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
    use serde_json::json;

    #[test]
    fn items_named_changed_glow_and_the_catalogue_doesnt() {
        let t0 = Instant::now();
        let mut f = Flash::default();
        f.changed(&json!(["catalogue", "item/a", "document/d"]), t0);
        assert_eq!(f.glowing().collect::<Vec<_>>(), ["item/a"]);
        assert_eq!(f.strength("item/a", t0), 1.0);
        assert_eq!(f.strength("item/b", t0), 0.0);
        f.clear();
        assert!(!f.due());
    }
}
