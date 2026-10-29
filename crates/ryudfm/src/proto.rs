//! The requests the CLI and the daemon share, and what `version` says:
//! one definition, so they never diverge.

use crate::error::{Code, Error, Result};
use ryu_session::live::Kind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use ryu_session::rpc::Answer;

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 1;

/// What `ryudfm version` reports. Its `ryudfm` key is how a client tells
/// a ryudfm daemon from another product's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub ryudfm: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryudfm: env!("CARGO_PKG_VERSION").to_owned(),
            commit: env!("RYUDFM_COMMIT").to_owned(),
            target: env!("RYUDFM_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }

    /// Whether `v` (a `version` object as JSON) is the same build: the same
    /// ryudfm, commit and protocol. The target may differ, as ryucad's may.
    pub fn same_build(&self, v: &Value) -> bool {
        let me = json!(self);
        ["ryudfm", "commit", "protocol"]
            .iter()
            .all(|k| v.get(k) == me.get(k))
    }
}

/// Every request a session answers, by JSON-RPC method name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    /// The project, its boards and their revisions.
    #[serde(rename = "project.info")]
    ProjectInfo {},
    /// An ODB++ job (a folder or a `.tgz`, `.tar` or `.zip`, by absolute
    /// path) read and committed as a board: a new one, its ID made from
    /// the file's name unless `board` names it, or with `update` the next
    /// revision of a board the project has.
    #[serde(rename = "import")]
    Import {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        board: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Every board with its input, step and revision.
    #[serde(rename = "board.list")]
    BoardList {},
    #[serde(rename = "board.show")]
    BoardShow { board: String },
    /// A board's name and step.
    #[serde(rename = "board.edit")]
    BoardEdit {
        board: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A board and its input.
    #[serde(rename = "board.delete")]
    BoardDelete {
        board: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A board summarised, or a page of a layer's features, its
    /// components or its nets, or one feature.
    #[serde(rename = "inspect")]
    Inspect {
        board: String,
        #[serde(default, flatten)]
        args: crate::inspect::Args,
    },
    /// Layers of a board drawn as PNG or SVG, the bytes after the
    /// response line.
    #[serde(rename = "render")]
    Render {
        board: String,
        #[serde(default, flatten)]
        args: crate::render::Args,
    },
    /// The project (or one board) loaded from nothing: every load fault
    /// as a finding; `check_failed` on any error.
    #[serde(rename = "check")]
    Check {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        board: Option<String>,
    },
    #[serde(rename = "activity.log")]
    ActivityLog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
    },
    #[serde(rename = "undo")]
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    #[serde(rename = "redo")]
    Redo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    #[serde(rename = "session.hello")]
    SessionHello {
        kind: Kind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    #[serde(rename = "participant.list")]
    ParticipantList {},
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(default)]
        topics: Vec<String>,
    },
    #[serde(rename = "daemon.status")]
    DaemonStatus {},
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown {},
}

impl Request {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("requests serialize")
    }

    /// Reads a request from its method and params.
    pub fn from_rpc(method: &str, params: Option<Value>) -> Result<Request> {
        let params = params.unwrap_or_else(|| json!({}));
        serde_json::from_value(json!({"method": method, "params": params})).map_err(|e| {
            let msg = e.to_string();
            let code = if msg.starts_with("unknown variant") {
                Code::Usage
            } else {
                Code::InvalidDocument
            };
            Error::new(code, format!("{method}: {msg}")).with("method", method)
        })
    }

    pub fn method(&self) -> String {
        self.to_value()["method"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    pub fn params(&self) -> Value {
        self.to_value()["params"].take()
    }

    /// Whether the request writes the project's files.
    pub fn mutates(&self) -> bool {
        match self {
            Request::Import { dry_run, .. } => !dry_run,
            Request::BoardEdit { .. }
            | Request::BoardDelete { .. }
            | Request::Undo { .. }
            | Request::Redo { .. } => true,
            _ => false,
        }
    }
}

impl ryu_session::client::Call for Request {
    fn method(&self) -> String {
        Request::method(self)
    }

    fn params(&self) -> Value {
        Request::params(self)
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
    fn requests_round_trip_through_rpc() {
        let reqs = [
            Request::ProjectInfo {},
            Request::Import {
                path: "/x/job.tgz".into(),
                board: None,
                step: Some("pcb".into()),
                update: Some("job".into()),
                dry_run: true,
                expected_revision: Some("r".into()),
            },
            Request::Inspect {
                board: "b".into(),
                args: crate::inspect::Args {
                    layer: Some("top".into()),
                    limit: Some(5),
                    ..Default::default()
                },
            },
            Request::Render {
                board: "b".into(),
                args: crate::render::Args {
                    layers: vec!["top".into(), "drill".into()],
                    side: Some(crate::render::Side::Bottom),
                    ..Default::default()
                },
            },
            Request::Check { board: None },
            Request::Undo { entry: None },
        ];
        for r in reqs {
            assert_eq!(Request::from_rpc(&r.method(), Some(r.params())).unwrap(), r);
        }
        assert_eq!(
            Request::from_rpc("nope", None).unwrap_err().code,
            Code::Usage
        );
        // Flattened arguments still refuse a misspelt field.
        let e =
            Request::from_rpc("inspect", Some(json!({"board": "b", "layr": "top"}))).unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        assert!(e.message.contains("layr"), "{}", e.message);
    }

    #[test]
    fn dry_runs_write_nothing() {
        let import = |dry_run| Request::Import {
            path: "/j".into(),
            board: None,
            step: None,
            update: None,
            dry_run,
            expected_revision: None,
        };
        assert!(import(false).mutates());
        assert!(!import(true).mutates());
        assert!(!Request::BoardList {}.mutates());
    }

    #[test]
    fn a_ryueda_version_is_never_ryudfms() {
        let me = VersionInfo::current();
        let ryueda =
            json!({"ryueda": me.ryudfm, "commit": me.commit, "target": me.target, "protocol": 1});
        assert!(!me.same_build(&ryueda));
    }
}
