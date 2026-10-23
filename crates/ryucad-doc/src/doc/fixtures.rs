//! Files for tests, here and in the crates above.

use serde_json::{Value, json};

/// A 20 × 10 rectangle with a horizontal bottom, its width a
/// parameter, and a construction diagonal.
pub fn rectangle() -> Value {
    json!({
        "constraints": {
            "c1": {"a": "l1", "type": "horizontal"},
            "c2": {"a": "l1", "b": "l2", "type": "perpendicular"},
            "d1": {"a": "l1", "type": "length", "value": "width"},
            "d2": {"a": "p1", "b": "p3", "type": "distance", "value": 22.360679774997898}
        },
        "entities": {
            "l1": {"end": "p2", "start": "p1", "type": "line"},
            "l2": {"end": "p3", "start": "p2", "type": "line"},
            "l3": {"end": "p4", "start": "p3", "type": "line"},
            "l4": {"end": "p1", "start": "p4", "type": "line"},
            "l5": {"construction": true, "end": "p3", "start": "p1", "type": "line"},
            "p1": {"m": [0, 0], "type": "point"},
            "p2": {"m": [0.02, 0], "type": "point"},
            "p3": {"m": [0.02, 0.01], "type": "point"},
            "p4": {"m": [0, 0.01], "type": "point"}
        },
        "format": 2,
        "kind": "ryucad.feature",
        "plane": "xy",
        "type": "sketch"
    })
}
