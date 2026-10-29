//! The requests the CLI and the daemon share, and what `version` says:
//! one definition, so they never diverge.

use crate::error::{Code, Error, Result};
use ryu_session::live::Kind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use ryu_session::rpc::Answer;

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

/// Every request a session answers, by JSON-RPC method name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    /// The project, its libraries and their revisions.
    #[serde(rename = "project.info")]
    ProjectInfo {},
    /// Names a library project by alias, at a path relative to this one.
    #[serde(rename = "lib.add")]
    LibAdd {
        alias: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// Every library with its revision, what it holds and its faults.
    #[serde(rename = "lib.list")]
    LibList {},
    /// A symbol called `name`, its ID from the name unless given, with
    /// `units` (1 by default) and a batch of ops applied.
    #[serde(rename = "symbol.create")]
    SymbolCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        units: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ops: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A batch of `add`, `remove`, `set` and `move` ops on a symbol's
    /// pins and graphics, and its name and unit count.
    #[serde(rename = "symbol.edit")]
    SymbolEdit {
        symbol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        units: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ops: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "symbol.show")]
    SymbolShow { symbol: String },
    #[serde(rename = "symbol.list")]
    SymbolList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    #[serde(rename = "symbol.delete")]
    SymbolDelete {
        symbol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "footprint.show")]
    FootprintShow { footprint: String },
    #[serde(rename = "footprint.list")]
    FootprintList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    /// A library component, its ID made from `name` unless given.
    #[serde(rename = "libcomp.create")]
    LibcompCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        fields: crate::libcmd::CompFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The fields given replace the component's; its parameters merge.
    #[serde(rename = "libcomp.edit")]
    LibcompEdit {
        libcomp: String,
        #[serde(default, flatten)]
        fields: crate::libcmd::CompFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "libcomp.show")]
    LibcompShow { libcomp: String },
    #[serde(rename = "libcomp.list")]
    LibcompList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        library: Option<String>,
    },
    #[serde(rename = "libcomp.delete")]
    LibcompDelete {
        libcomp: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A project-wide net, its ID random unless given.
    #[serde(rename = "rail.add")]
    RailAdd {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        intent: crate::topcmd::IntentArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A rail's name, class or intent; an empty text clears a field.
    #[serde(rename = "rail.edit")]
    RailEdit {
        rail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, flatten)]
        intent: crate::topcmd::IntentArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "rail.delete")]
    RailDelete {
        rail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "rail.list")]
    RailList {},
    #[serde(rename = "class.add")]
    ClassAdd {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "class.edit")]
    ClassEdit {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "class.delete")]
    ClassDelete {
        class: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "class.list")]
    ClassList {},
    /// A design: a name and its root block.
    #[serde(rename = "design.create")]
    DesignCreate {
        name: String,
        root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "design.edit")]
    DesignEdit {
        design: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "design.delete")]
    DesignDelete {
        design: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "design.list")]
    DesignList {},
    /// A functional block, its ID random unless given.
    #[serde(rename = "block.create")]
    BlockCreate {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A block's name and description, and ops on its ports and buses.
    #[serde(rename = "block.edit")]
    BlockEdit {
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ports: Vec<crate::topcmd::PortOp>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        buses: Vec<crate::topcmd::BusOp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A block with what it holds, its faults and revision.
    #[serde(rename = "block.show")]
    BlockShow { block: String },
    #[serde(rename = "block.list")]
    BlockList {},
    /// A block's folder, refused while instanced or a design's root.
    #[serde(rename = "block.delete")]
    BlockDelete {
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A library component placed in a block; its refdes the prefix and
    /// the project's next free number unless given.
    #[serde(rename = "component.add")]
    ComponentAdd {
        block: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, flatten)]
        fields: crate::partcmd::ComponentFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// The fields given replace the component's; its parameters merge.
    #[serde(rename = "component.edit")]
    ComponentEdit {
        component: String,
        #[serde(default, flatten)]
        fields: crate::partcmd::ComponentFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "component.delete")]
    ComponentDelete {
        component: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "component.show")]
    ComponentShow { component: String },
    /// A pin (`<block>/<component>/<pin>`) onto a net, `rail:<name>`,
    /// `port:<name>`, or with `new` a new net of the block by that name.
    #[serde(rename = "connect")]
    Connect {
        pin: String,
        net: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        new: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A pin off its net, unconnected.
    #[serde(rename = "disconnect")]
    Disconnect {
        pin: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    /// A pin marked not connected.
    #[serde(rename = "nc")]
    Nc {
        pin: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
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
        matches!(
            self,
            Request::LibAdd { .. }
                | Request::SymbolCreate { .. }
                | Request::SymbolEdit { .. }
                | Request::SymbolDelete { .. }
                | Request::LibcompCreate { .. }
                | Request::LibcompEdit { .. }
                | Request::LibcompDelete { .. }
                | Request::RailAdd { .. }
                | Request::RailEdit { .. }
                | Request::RailDelete { .. }
                | Request::ClassAdd { .. }
                | Request::ClassEdit { .. }
                | Request::ClassDelete { .. }
                | Request::DesignCreate { .. }
                | Request::DesignEdit { .. }
                | Request::DesignDelete { .. }
                | Request::BlockCreate { .. }
                | Request::BlockEdit { .. }
                | Request::BlockDelete { .. }
                | Request::ComponentAdd { .. }
                | Request::ComponentEdit { .. }
                | Request::ComponentDelete { .. }
                | Request::Connect { .. }
                | Request::Disconnect { .. }
                | Request::Nc { .. }
                | Request::Undo { .. }
                | Request::Redo { .. }
        )
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
            Request::LibAdd {
                alias: "passives".into(),
                path: "../passives".into(),
                expected_revision: Some("r".into()),
            },
            Request::Undo { entry: None },
            Request::Subscribe {
                topics: vec!["project".into()],
            },
        ];
        for r in reqs {
            assert_eq!(Request::from_rpc(&r.method(), Some(r.params())).unwrap(), r);
        }
        assert_eq!(Request::Undo { entry: None }.method(), "undo");
        assert_eq!(
            Request::from_rpc("nope", None).unwrap_err().code,
            Code::Usage
        );
        let e = Request::from_rpc("lib.add", Some(json!({"alias": 1}))).unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        // A library component's fields are flattened into the params, and
        // a misspelt one is still refused.
        let e = Request::from_rpc(
            "libcomp.create",
            Some(json!({"name": "x", "colour": "red"})),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        assert!(e.message.contains("colour"), "{}", e.message);
    }

    #[test]
    fn a_ryucad_version_is_never_ryuedas() {
        let me = VersionInfo::current();
        let ryucad = json!({"ryucad": me.ryueda, "ryukei": "0.0.0", "commit": me.commit, "target": me.target, "protocol": 2});
        assert!(!me.same_build(&ryucad));
        assert!(json!(me).get("ryucad").is_none());
    }
}
