//! The requests the CLI and the daemon share, and what `version` says.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 1;

/// What `ryueda version` reports. Its `ryueda` key is how a client tells
/// a ryueda daemon from another product's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub ryueda: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryueda: env!("CARGO_PKG_VERSION").to_owned(),
            commit: env!("RYUEDA_COMMIT").to_owned(),
            target: env!("RYUEDA_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }

    /// Whether `v` (a `version` object as JSON) is the same build: the same
    /// ryueda, commit and protocol. The target may differ, as ryucad's may.
    pub fn same_build(&self, v: &Value) -> bool {
        let me = json!(self);
        ["ryueda", "commit", "protocol"]
            .iter()
            .all(|k| v.get(k) == me.get(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_build_may_have_another_target() {
        let me = VersionInfo::current();
        let mut v = json!(me);
        assert!(me.same_build(&v));
        v["target"] = json!("x86_64-unknown-linux-musl");
        assert!(me.same_build(&v));
        v["commit"] = json!("0123456789ab");
        assert!(!me.same_build(&v));
    }

    #[test]
    fn a_ryucad_version_is_never_ryuedas() {
        let me = VersionInfo::current();
        let ryucad = json!({"ryucad": me.ryueda, "ryukei": "0.0.0", "commit": me.commit, "target": me.target, "protocol": 2});
        assert!(!me.same_build(&ryucad));
        assert!(json!(me).get("ryucad").is_none());
    }
}
