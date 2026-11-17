//! What a daemon spent answering, by request method (#849, design 698):
//! how long each method's requests waited for the session's lock, held
//! it, and spent telling subscribers. `daemon status --timings` reports
//! it. Tests and people read it; nothing that answers a request does, so
//! it never changes a result.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// One method's sums since the daemon started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spent {
    /// Requests answered.
    pub count: u64,
    /// Waiting for the session's lock.
    pub wait: Duration,
    /// Holding it, answering (a publish on the request's path included).
    pub held: Duration,
    /// Publishes made for it, and how long they took.
    pub publishes: u64,
    pub publish: Duration,
}

/// Every method's [`Spent`]. The daemon's watcher records its own
/// publishes as `daemon.publish` (a coalesced publish, design 698) and
/// `daemon.poll` (its periodic check for changes made behind its back).
#[derive(Clone, Debug, Default)]
pub struct Timings {
    by: BTreeMap<String, Spent>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Timings {
    /// One request of `method` answered.
    pub fn request(&mut self, method: &str, wait: Duration, held: Duration) {
        let s = self.by.entry(method.to_owned()).or_default();
        s.count += 1;
        s.wait += wait;
        s.held += held;
    }

    /// One publish made for `method`, taking `took`.
    pub fn publish(&mut self, method: &str, took: Duration) {
        let s = self.by.entry(method.to_owned()).or_default();
        s.publishes += 1;
        s.publish += took;
    }

    /// `method`'s sums (zero when it never ran).
    pub fn get(&self, method: &str) -> Spent {
        self.by.get(method).copied().unwrap_or_default()
    }

    /// `{"methods": {<method>: {count, wait_ms, held_ms, publishes,
    /// publish_ms}}}`, methods in name order.
    pub fn to_json(&self) -> Value {
        let methods: Map<String, Value> = self
            .by
            .iter()
            .map(|(m, s)| {
                let v = json!({
                    "count": s.count,
                    "wait_ms": ms(s.wait),
                    "held_ms": ms(s.held),
                    "publishes": s.publishes,
                    "publish_ms": ms(s.publish),
                });
                (m.clone(), v)
            })
            .collect();
        json!({ "methods": methods })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_are_kept_per_method() {
        let mut t = Timings::default();
        let d = Duration::from_millis;
        t.request("feature.add", d(1), d(4));
        t.request("feature.add", d(2), d(6));
        t.publish("feature.add", d(3));
        t.publish("daemon.publish", d(5));
        assert_eq!(
            t.get("feature.add"),
            Spent {
                count: 2,
                wait: d(3),
                held: d(10),
                publishes: 1,
                publish: d(3)
            }
        );
        assert_eq!(t.get("doctor"), Spent::default());
        assert_eq!(
            t.to_json(),
            json!({"methods": {
                "daemon.publish": {"count": 0, "wait_ms": 0.0, "held_ms": 0.0,
                                   "publishes": 1, "publish_ms": 5.0},
                "feature.add": {"count": 2, "wait_ms": 3.0, "held_ms": 10.0,
                                "publishes": 1, "publish_ms": 3.0},
            }})
        );
    }
}
