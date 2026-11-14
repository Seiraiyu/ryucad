//! ryueda as the shell's product: the marker through which `ryu_app`'s
//! generic link, keymap and toasts take ryueda's requests, commands and
//! words.

use crate::core::command::{COMMANDS, Here};
use ryu_app::command::Command;
use ryu_app::keys::Binding;
use ryueda::proto::{Request, VersionInfo};
use serde_json::Value;
use std::collections::BTreeSet;

/// ryueda, to the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Eda;

impl ryu_app::link::Client for Eda {
    type Product = ryueda::Eda;
    const PROGRAM: &'static str = "ryueda";
    fn hello(
        kind: ryu_session::live::Kind,
        name: Option<String>,
        participant: Option<String>,
    ) -> Request {
        Request::SessionHello {
            kind,
            name,
            participant,
        }
    }
    fn cancel(token: String) -> Request {
        Request::RequestCancel { token, name: None }
    }
    fn build(v: &VersionInfo) -> String {
        format!("ryueda {} ({})", v.ryueda, v.commit)
    }
}

impl ryu_app::keys::Registry for Eda {
    /// ryueda has one keymap; no navigation presets.
    type Style = ();
    type Context = Here;
    fn commands() -> &'static [Command<Here>] {
        COMMANDS
    }
    fn preset(_: ()) -> &'static [Binding] {
        &[]
    }
}

/// `blocks/<block>/<components|nets|instances>/<id>.json` as
/// `(block, id)`; the block's own files as its group alone.
fn object_of(path: &str) -> Option<(String, Option<String>)> {
    let rest = path.strip_prefix("blocks/")?;
    let (block, rest) = rest.split_once('/')?;
    let object = ["components/", "nets/", "instances/"]
        .iter()
        .find_map(|f| rest.strip_prefix(f))
        .and_then(|f| f.strip_suffix(".json"))
        .map(str::to_owned);
    Some((block.to_owned(), object))
}

fn paths(entry: &Value) -> impl Iterator<Item = &str> {
    entry["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str())
}

impl ryu_app::toast::Words for Eda {
    const OBJECTS: &'static str = "objects";
    const GROUPS: &'static str = "blocks";
    fn verb(method: &str, kind: &Value) -> String {
        match kind.as_str() {
            Some("undo") => return "undid".into(),
            Some("redo") => return "redid".into(),
            _ => {}
        }
        match method {
            m if m.ends_with(".add") || m.ends_with(".create") => "added",
            m if m.ends_with(".edit") || m == "connect" || m == "disconnect" || m == "nc" => {
                "edited"
            }
            m if m.ends_with(".delete") => "deleted",
            "net.rename" => "renamed",
            "net.join" => "joined",
            "layout" | "layout.place" => "laid out",
            "annotate" => "annotated",
            "style.set" | "style.unset" | "style.rule" => "styled",
            m => m,
        }
        .to_owned()
    }
    fn objects(entry: &Value) -> Vec<(String, String)> {
        paths(entry)
            .filter_map(object_of)
            .filter_map(|(b, o)| Some((b, o?)))
            .collect()
    }
    fn groups(entry: &Value) -> BTreeSet<String> {
        paths(entry).filter_map(object_of).map(|(b, _)| b).collect()
    }
    fn undoable(entry: &Value) -> bool {
        crate::core::activity::can_undo(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_app::toast::Words;
    use serde_json::json;

    #[test]
    fn an_entry_s_objects_and_blocks_and_its_verb() {
        let e = json!({"method": "component.edit", "kind": "change", "undone_by": null, "files": [
            {"path": "blocks/top/components/r1.json"},
            {"path": "blocks/top/layout.json"},
            {"path": "ryueda.json"},
        ]});
        assert_eq!(Eda::objects(&e), [("top".to_owned(), "r1".to_owned())]);
        assert_eq!(Eda::groups(&e).into_iter().collect::<Vec<_>>(), ["top"]);
        assert_eq!(Eda::verb("component.edit", &e["kind"]), "edited");
        assert_eq!(Eda::verb("component.edit", &json!("undo")), "undid");
        assert!(Eda::undoable(&e));
    }
}
