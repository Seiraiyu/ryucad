//! The request types shared by the CLI, the daemon and every later
//! adapter, and the JSON-RPC 2.0 framing they travel in. One definition,
//! so they never diverge.

use crate::error::{Code, Error, Result};
use crate::live::{Kind, SelectMode, Style};
use crate::units::Unit;
use crate::view::CameraArg;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 2;

/// What `ryucad version` reports. Future cache keys are built from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub ryucad: String,
    pub ryukei: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryucad: env!("CARGO_PKG_VERSION").to_owned(),
            ryukei: ryukei::VERSION.to_owned(),
            commit: env!("RYUCAD_COMMIT").to_owned(),
            target: env!("RYUCAD_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }
}

/// Where a feature goes in a part's list: after `after`, first, or (with
/// neither) at the end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub first: bool,
}

/// A placement as given by an agent: any part left out keeps its current
/// value (or the identity's, for a new feature).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementArg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<[Value; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<[f64; 3]>,
}

/// Every request a session answers, by JSON-RPC method name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "project.init")]
    ProjectInit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_unit: Option<Unit>,
    },
    #[serde(rename = "project.info")]
    ProjectInfo {},
    #[serde(rename = "part.create")]
    PartCreate { part: String },
    #[serde(rename = "part.delete")]
    PartDelete {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "part.list")]
    PartList {},
    #[serde(rename = "feature.add")]
    FeatureAdd {
        part: String,
        #[serde(rename = "type")]
        ty: String,
        #[serde(default)]
        params: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<PlacementArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        position: Position,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.edit")]
    FeatureEdit {
        part: String,
        id: String,
        /// Merged into the current parameters; `null` removes one.
        #[serde(default)]
        params: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<PlacementArg>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.delete")]
    FeatureDelete {
        part: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.move")]
    FeatureMove {
        part: String,
        id: String,
        #[serde(flatten)]
        position: Position,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "feature.list")]
    FeatureList { part: String },
    #[serde(rename = "feature.show")]
    FeatureShow { part: String, id: String },
    #[serde(rename = "rebuild")]
    Rebuild {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<String>,
    },
    #[serde(rename = "inspect")]
    Inspect {
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
    },
    #[serde(rename = "check_format")]
    CheckFormat {},
    #[serde(rename = "daemon.status")]
    DaemonStatus {},
    /// Registers the connection as a participant of the live session.
    #[serde(rename = "session.hello")]
    SessionHello {
        kind: Kind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Join this participant instead (an app's second connection).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    #[serde(rename = "participant.list")]
    ParticipantList {},
    /// Turns the connection into a subscription: after the answer, it
    /// receives notifications for `topics` (all when empty).
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        topics: Vec<String>,
    },
    /// A participant's selection: `participant`, or the one human.
    #[serde(rename = "selection.get")]
    SelectionGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Changes the caller's own selection.
    #[serde(rename = "selection.set")]
    SelectionSet {
        #[serde(default)]
        entities: Vec<String>,
        #[serde(default)]
        mode: SelectMode,
    },
    /// A participant's camera: `participant`, or the one human.
    #[serde(rename = "camera.get")]
    CameraGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        participant: Option<String>,
    },
    /// Sets the caller's own camera.
    #[serde(rename = "camera.set")]
    CameraSet { camera: CameraArg },
    /// Highlights entities for everyone to see, replacing the caller's
    /// highlight of the same style and label.
    #[serde(rename = "highlight.set")]
    HighlightSet {
        entities: Vec<String>,
        #[serde(default)]
        style: Style,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Removes the caller's highlights of `style`, or all of them.
    #[serde(rename = "highlight.clear")]
    HighlightClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<Style>,
    },
    /// Entries of the activity log older than `before`, oldest first.
    #[serde(rename = "activity.log")]
    ActivityLog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
    },
    /// Undoes `entry`, or the caller's latest change.
    #[serde(rename = "undo")]
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    /// Redoes `entry` (an undo), or the caller's latest undo.
    #[serde(rename = "redo")]
    Redo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<u64>,
    },
    /// One setting, or all of them (of `scope`, if given).
    #[serde(rename = "config.get")]
    ConfigGet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    /// Sets a project setting (in `ryucad.json`) or a user preference.
    #[serde(rename = "config.set")]
    ConfigSet {
        key: String,
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// For project settings: the project revision last seen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Says what the caller is doing; an empty status clears it.
    #[serde(rename = "activity.set")]
    ActivitySet {
        status: String,
        #[serde(default)]
        entities: Vec<String>,
    },
    #[serde(rename = "daemon.shutdown")]
    DaemonShutdown {},
}

impl Request {
    /// The request for JSON-RPC `method` with `params` (absent means `{}`).
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
        if let Request::ConfigSet { key, .. } = self {
            return crate::config::scope_of(key).is_ok_and(|s| s == crate::config::Scope::Project);
        }
        matches!(
            self,
            Request::ProjectInit { .. }
                | Request::PartCreate { .. }
                | Request::PartDelete { .. }
                | Request::FeatureAdd { .. }
                | Request::FeatureEdit { .. }
                | Request::FeatureDelete { .. }
                | Request::FeatureMove { .. }
                | Request::Undo { .. }
                | Request::Redo { .. }
        )
    }

    fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("requests serialize")
    }
}

/// The JSON-RPC error code that carries every ryucad error; the ryucad code
/// and details are in `data`.
pub const RPC_ERROR: i64 = -32000;

/// One JSON-RPC request line.
pub fn rpc_request(id: u64, req: &Request) -> String {
    let mut v = json!({"jsonrpc": "2.0", "id": id, "method": req.method()});
    let params = req.params();
    if !params.is_null() {
        v["params"] = params;
    }
    v.to_string()
}

/// Parses one JSON-RPC request line into its id and request.
pub fn parse_rpc_request(line: &str) -> (Value, Result<Request>) {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return (
                Value::Null,
                Err(Error::new(Code::InvalidJson, format!("bad request: {e}"))),
            );
        }
    };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let method = match v.get("method").and_then(Value::as_str) {
        Some(m) if v.get("jsonrpc") == Some(&json!("2.0")) => m,
        _ => {
            let e = Error::new(Code::Usage, "not a JSON-RPC 2.0 request");
            return (id, Err(e));
        }
    };
    (id, Request::from_rpc(method, v.get("params").cloned()))
}

/// One JSON-RPC response line.
pub fn rpc_response(id: Value, result: &Result<Value>) -> String {
    match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": RPC_ERROR, "message": e.message, "data": e},
        }),
    }
    .to_string()
}

/// Parses one JSON-RPC response line back into a result.
pub fn parse_rpc_response(line: &str) -> Result<Value> {
    let bad = |why: &str| Error::new(Code::Daemon, format!("bad response from the daemon: {why}"));
    let mut v: Value = serde_json::from_str(line).map_err(|e| bad(&e.to_string()))?;
    if let Some(r) = v.get_mut("result") {
        return Ok(r.take());
    }
    let data = v
        .pointer_mut("/error/data")
        .map(Value::take)
        .ok_or_else(|| bad("no result or error"))?;
    Err(serde_json::from_value(data).map_err(|e| bad(&e.to_string()))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_through_rpc() {
        let reqs = [
            Request::ProjectInfo {},
            Request::ProjectInit {
                default_unit: Some(Unit::In),
            },
            Request::FeatureAdd {
                part: "p".into(),
                ty: "box".into(),
                params: [("x".to_owned(), json!("1 mm"))].into_iter().collect(),
                placement: Some(PlacementArg {
                    z: Some([0.0, 1.0, 0.0]),
                    ..Default::default()
                }),
                id: Some("b".into()),
                position: Position {
                    after: Some("a".into()),
                    first: false,
                },
                expected_revision: None,
            },
            Request::FeatureMove {
                part: "p".into(),
                id: "b".into(),
                position: Position {
                    after: None,
                    first: true,
                },
                expected_revision: Some("r".into()),
            },
            Request::Rebuild { part: None },
            Request::SessionHello {
                kind: Kind::Human,
                name: Some("Dana".into()),
                participant: None,
            },
            Request::ParticipantList {},
        ];
        for (i, r) in reqs.into_iter().enumerate() {
            let line = rpc_request(i as u64, &r);
            assert!(!line.contains('\n'));
            let (id, back) = parse_rpc_request(&line);
            assert_eq!(id, json!(i));
            assert_eq!(back.unwrap(), r);
        }
    }

    #[test]
    fn missing_params_mean_empty() {
        let (_, r) = parse_rpc_request(r#"{"jsonrpc":"2.0","id":1,"method":"part.list"}"#);
        assert_eq!(r.unwrap(), Request::PartList {});
    }

    #[test]
    fn bad_requests() {
        let (_, r) = parse_rpc_request(r#"{"jsonrpc":"2.0","id":1,"method":"part.explode"}"#);
        assert_eq!(r.unwrap_err().code, Code::Usage);
        let (_, r) = parse_rpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"part.create","params":{"prt":"x"}}"#,
        );
        assert_eq!(r.unwrap_err().code, Code::InvalidDocument);
        // A misspelt optional field must not be silently ignored.
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"part.create","params":{"part":"x","bogus":1}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"feature.delete","params":{"part":"p","id":"a","expected_revison":"r"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"feature.add","params":{"part":"p","type":"box","expected_revison":"r"}}"#,
        ] {
            let (_, r) = parse_rpc_request(line);
            assert_eq!(r.unwrap_err().code, Code::InvalidDocument, "{line}");
        }
        let (id, r) = parse_rpc_request("{nope");
        assert_eq!((id, r.unwrap_err().code), (Value::Null, Code::InvalidJson));
        let (_, r) = parse_rpc_request(r#"{"id":1,"method":"part.list"}"#);
        assert_eq!(r.unwrap_err().code, Code::Usage);
    }

    #[test]
    fn responses_round_trip() {
        let ok = rpc_response(json!(3), &Ok(json!({"a": 1})));
        assert_eq!(parse_rpc_response(&ok).unwrap(), json!({"a": 1}));
        let e = Error::new(Code::Conflict, "changed").with("expected", "x");
        let line = rpc_response(json!(3), &Err(e.clone()));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error"]["code"], RPC_ERROR);
        assert_eq!(parse_rpc_response(&line).unwrap_err(), e);
        assert_eq!(
            parse_rpc_response("garbage").unwrap_err().code,
            Code::Daemon
        );
    }

    #[test]
    fn only_writers_mutate() {
        assert!(Request::PartCreate { part: "p".into() }.mutates());
        assert!(!Request::Rebuild { part: None }.mutates());
        assert_eq!(Request::Rebuild { part: None }.method(), "rebuild");
    }
}
