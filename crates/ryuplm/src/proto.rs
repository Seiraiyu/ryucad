//! The requests the CLI and the daemon share, and what `version` says:
//! one definition, so they never diverge.

use crate::error::{Code, Error, Result};
use ryu_session::live::Kind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use ryu_session::rpc::Answer;

/// The protocol version. A CLI and a daemon talk only when theirs match.
pub const PROTOCOL_VERSION: u32 = 1;

/// What `ryuplm version` reports. Its `ryuplm` key is how a client tells
/// a ryuplm daemon from another product's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub ryuplm: String,
    pub commit: String,
    pub target: String,
    pub protocol: u32,
}

impl VersionInfo {
    /// This binary's version information.
    pub fn current() -> Self {
        Self {
            ryuplm: env!("CARGO_PKG_VERSION").to_owned(),
            commit: env!("RYUPLM_COMMIT").to_owned(),
            target: env!("RYUPLM_TARGET").to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }

    /// Whether `v` (a `version` object as JSON) is the same build: the same
    /// ryuplm, commit and protocol. The target may differ, as ryucad's may.
    pub fn same_build(&self, v: &Value) -> bool {
        let me = json!(self);
        ["ryuplm", "commit", "protocol"]
            .iter()
            .all(|k| v.get(k) == me.get(k))
    }
}

/// Every request a session answers, by JSON-RPC method name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "project.info")]
    ProjectInfo {},
    #[serde(rename = "item.create")]
    ItemCreate {
        #[serde(default, flatten)]
        fields: crate::items::Fields,
        /// The number, or the scheme whose next number to take ("" for
        /// the one that applies).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "item.show")]
    ItemShow { item: String },
    #[serde(rename = "item.list")]
    ItemList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    #[serde(rename = "item.edit")]
    ItemEdit {
        item: String,
        #[serde(default, flatten)]
        fields: crate::items::Fields,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unset: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "item.renumber")]
    ItemRenumber {
        item: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "item.delete")]
    ItemDelete {
        item: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "item.link")]
    ItemLink {
        item: String,
        product: String,
        /// A project ID, or the project's folder (absolute, or relative
        /// to the catalogue).
        project: String,
        part: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "item.unlink")]
    ItemUnlink {
        item: String,
        product: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "bom.add")]
    BomAdd {
        item: String,
        #[serde(default, flatten)]
        fields: crate::bom::LineFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "bom.edit")]
    BomEdit {
        item: String,
        line: String,
        #[serde(default, flatten)]
        fields: crate::bom::LineFields,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unset: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "bom.remove")]
    BomRemove {
        item: String,
        line: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "bom.get")]
    BomGet {
        item: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<usize>,
        /// Folders searched for representations besides the work tree
        /// and `pdm.dependents`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        within: Vec<String>,
    },
    #[serde(rename = "bom.diff")]
    BomDiff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        within: Vec<String>,
    },
    #[serde(rename = "bom.where_used")]
    BomWhereUsed {
        item: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        within: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_dependents: bool,
        /// Working files only, no releases.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        working: bool,
    },
    #[serde(rename = "import.guess")]
    ImportGuess {
        /// An absolute path: the CLI makes it one.
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delimiter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encoding: Option<String>,
        /// JSON and XML: where the records are, instead of the proposal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<String>,
        /// Save the proposal as `imports/<save>.json`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save: Option<String>,
    },
    #[serde(rename = "import.check")]
    ImportCheck {
        name: String,
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    #[serde(rename = "import.run")]
    ImportRun {
        name: String,
        file: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        /// `source` or `catalogue`: how a conflict is settled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefer: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "import.list")]
    ImportList {},
    /// What the CLI reads before it asks the ledger (ryuplm 2.B): see
    /// `numbers::local`. Reads only.
    #[serde(rename = "number.local")]
    NumberLocal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    #[serde(rename = "settings.show")]
    SettingsShow {},
    #[serde(rename = "settings.edit")]
    SettingsEdit {
        #[serde(flatten)]
        change: crate::settings::Change,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "check")]
    Check {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        within: Vec<String>,
    },
    #[serde(rename = "release.propose")]
    ReleasePropose {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        items: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "release.obsolete")]
    ReleaseObsolete {
        item: String,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<String>,
    },
    #[serde(rename = "release.list")]
    ReleaseList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    #[serde(rename = "release.check")]
    ReleaseCheck {},
    #[serde(rename = "release.sync")]
    ReleaseSync {},
    #[serde(rename = "diff")]
    Diff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
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

    /// Whether the request may write files (the activity log records it).
    pub fn mutates(&self) -> bool {
        matches!(
            self,
            Request::ItemCreate { .. }
                | Request::ItemEdit { .. }
                | Request::ItemRenumber { .. }
                | Request::ItemDelete { .. }
                | Request::ItemLink { .. }
                | Request::ItemUnlink { .. }
                | Request::BomAdd { .. }
                | Request::BomEdit { .. }
                | Request::BomRemove { .. }
                | Request::SettingsEdit { .. }
                | Request::ImportGuess { .. }
                | Request::ImportRun { .. }
                | Request::ReleasePropose { .. }
                | Request::ReleaseObsolete { .. }
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
            Request::ItemCreate {
                fields: crate::items::Fields {
                    name: Some("Bracket".into()),
                    ..Default::default()
                },
                number: None,
                next: Some("mech".into()),
                expected_revision: Some("r".into()),
            },
            Request::ItemEdit {
                item: "k3v9q2m8x4c1p".into(),
                fields: Default::default(),
                unset: vec!["class".into()],
                expected_revision: None,
            },
            Request::SettingsEdit {
                change: crate::settings::Change {
                    class: Some("fastener".into()),
                    ..Default::default()
                },
                expected_revision: None,
            },
            Request::Check { within: vec![] },
            Request::ReleasePropose {
                items: vec!["A-1".into()],
                all: false,
                dry_run: true,
                note: None,
                expected_revision: None,
            },
            Request::ItemLink {
                item: "A-1".into(),
                product: "ryucad".into(),
                project: "p".into(),
                part: "x".into(),
                expected_revision: None,
            },
            Request::BomEdit {
                item: "A-1".into(),
                line: "0000000000001".into(),
                fields: Default::default(),
                unset: vec!["find".into()],
                expected_revision: None,
            },
            Request::Undo { entry: None },
            Request::ImportGuess {
                file: "/x/items.csv".into(),
                sheet: None,
                delimiter: Some(";".into()),
                encoding: None,
                records: None,
                save: Some("erp".into()),
            },
            Request::ImportCheck {
                name: "erp".into(),
                file: "/x/items.csv".into(),
                limit: Some(5),
            },
            Request::ImportRun {
                name: "erp".into(),
                file: "/x/items.csv".into(),
                dry_run: true,
                prefer: Some("source".into()),
                limit: None,
                expected_revision: None,
            },
            Request::ImportList {},
            Request::NumberLocal {
                next: Some(String::new()),
                class: Some("machined".into()),
                item: None,
            },
        ];
        for r in reqs {
            assert_eq!(Request::from_rpc(&r.method(), Some(r.params())).unwrap(), r);
        }
        assert_eq!(
            Request::from_rpc("nope", None).unwrap_err().code,
            Code::Usage
        );
        // Flattened fields still refuse a misspelt one.
        let e = Request::from_rpc("item.create", Some(json!({"nmae": "x"}))).unwrap_err();
        assert_eq!(e.code, Code::InvalidDocument);
        assert!(e.message.contains("nmae"), "{}", e.message);
    }

    #[test]
    fn a_ryudfm_version_is_never_ryuplms() {
        let me = VersionInfo::current();
        let ryudfm =
            json!({"ryudfm": me.ryuplm, "commit": me.commit, "target": me.target, "protocol": 1});
        assert!(!me.same_build(&ryudfm));
    }
}
