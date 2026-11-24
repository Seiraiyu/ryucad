//! ryuplm as the shell's product: the marker through which `ryu_app`'s
//! generic link, keymap and toasts take ryuplm's requests, commands and
//! words.

use crate::core::command::{COMMANDS, Here};
use ryu_app::command::Command;
use ryu_app::keys::Binding;
use ryuplm::proto::{Request, VersionInfo};
use serde_json::Value;
use std::collections::BTreeSet;

/// ryuplm, to the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plm;

impl ryu_app::link::Client for Plm {
    type Product = ryuplm::Plm;
    const PROGRAM: &'static str = "ryuplm";
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
        format!("ryuplm {} ({})", v.ryuplm, v.commit)
    }
}

impl ryu_app::keys::Registry for Plm {
    /// ryuplm has one keymap; no navigation presets.
    type Style = ();
    type Context = Here;
    fn commands() -> &'static [Command<Here>] {
        COMMANDS
    }
    fn preset(_: ()) -> &'static [Binding] {
        &[]
    }
}

/// What a file of the catalogue is, as a toast names it: an item or a
/// mapping by its ID or name (so a toast's Show names a live reference),
/// documents, or the catalogue's settings.
fn kind_of(path: &str) -> Option<(&'static str, Option<String>)> {
    if let Some(rest) = path.strip_prefix("items/") {
        let id = rest.split_once('/')?.1.strip_suffix(".json")?;
        return Some(("item", Some(id.to_owned())));
    }
    if let Some(name) = path
        .strip_prefix("imports/")
        .and_then(|n| n.strip_suffix(".json"))
    {
        return Some(("import", Some(name.to_owned())));
    }
    if path.starts_with("documents/") {
        return Some(("documents", None));
    }
    if path == "ryuplm.json" || path == "ryupdm.json" {
        return Some(("settings", None));
    }
    None
}

fn paths(entry: &Value) -> impl Iterator<Item = &str> {
    entry["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str())
}

impl ryu_app::toast::Words for Plm {
    const OBJECTS: &'static str = "records";
    const GROUPS: &'static str = "kinds";
    fn verb(method: &str, kind: &Value) -> String {
        match kind.as_str() {
            Some("undo") => return "undid".into(),
            Some("redo") => return "redid".into(),
            _ => {}
        }
        match method {
            "bom.add" => "added a line to",
            "bom.edit" => "edited a line of",
            "bom.remove" => "removed a line from",
            "item.link" => "linked",
            "item.unlink" => "unlinked",
            "import.run" => "imported",
            "import.guess" => "mapped",
            "document.attach" => "attached",
            "document.detach" => "detached",
            "document.replace" => "replaced",
            "release.propose" => "proposed",
            "release.obsolete" => "made obsolete",
            "settings.edit" => "set",
            m if m.ends_with(".create") || m.ends_with(".add") => "created",
            m if m.ends_with(".edit") => "edited",
            m if m.ends_with(".renumber") => "renumbered",
            m if m.ends_with(".delete") || m.ends_with(".remove") => "deleted",
            m => m,
        }
        .to_owned()
    }
    fn objects(entry: &Value) -> Vec<(String, String)> {
        paths(entry)
            .filter_map(kind_of)
            .filter_map(|(k, o)| Some((k.to_owned(), o?)))
            .collect()
    }
    fn groups(entry: &Value) -> BTreeSet<String> {
        paths(entry)
            .filter_map(kind_of)
            .map(|(k, _)| k.to_owned())
            .collect()
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
    fn an_entry_s_records_and_kinds_and_its_verb() {
        let e = json!({"method": "item.edit", "kind": "change", "undone_by": null, "files": [
            {"path": "items/k3/k3v9q2m8x4c1p.json"},
            {"path": "imports/erp.json"},
            {"path": "documents/ab/ab00000000001.json"},
            {"path": "ryupdm.json"},
        ]});
        assert_eq!(
            Plm::objects(&e),
            [
                ("item".to_owned(), "k3v9q2m8x4c1p".to_owned()),
                ("import".to_owned(), "erp".to_owned())
            ]
        );
        assert_eq!(
            Plm::groups(&e).into_iter().collect::<Vec<_>>(),
            ["documents", "import", "item", "settings"]
        );
        assert_eq!(Plm::verb("item.edit", &e["kind"]), "edited");
        assert_eq!(Plm::verb("bom.add", &e["kind"]), "added a line to");
        assert_eq!(Plm::verb("import.run", &e["kind"]), "imported");
        assert_eq!(Plm::verb("item.edit", &json!("undo")), "undid");
        assert!(Plm::undoable(&e));
    }
}
