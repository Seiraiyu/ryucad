//! The lists beside the canvas (design 519, The window): Properties,
//! Findings, Instructions and Activity, from `component.show`,
//! `libcomp.show`, `net.show`, `lint.get`, `erc`, `check`,
//! `layout.instructions` and `activity.log`. Read-only: the window
//! writes nothing but undo and redo.

use crate::core::activity::{can_redo, can_undo};
use crate::core::scene::Finding as SheetFinding;
use ryu_app::people::strings;
use serde_json::Value;
use std::collections::BTreeMap;

fn s(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// What Properties asks about for a reference of a kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// `component.show` (and `libcomp.show`): `<block>/<component>`.
    Component(String),
    /// `net.show`: `<block>/<net>`.
    Net(String),
    /// Nothing more than the reference and the instructions naming it.
    Other,
}

/// The subject of `reference`, an owner of `kind` (or `pin`).
pub fn subject(reference: &str, kind: &str) -> Subject {
    let (block, rest) = reference.split_once('/').unwrap_or((reference, ""));
    match kind {
        "pin" => Subject::Component(format!("{block}/{}", rest.split('/').next().unwrap_or(rest))),
        "component" => Subject::Component(reference.to_owned()),
        "net" | "bus" => Subject::Net(reference.to_owned()),
        "label" => rest
            .strip_prefix("label:")
            .and_then(|r| r.rsplit_once('@'))
            .map_or(Subject::Other, |(net, _)| Subject::Net(format!("{block}/{net}"))),
        _ => Subject::Other,
    }
}

/// The `libcomp.show` argument for a component's `of`: its ID in this
/// project's library, else `lib/<alias>/libcomp/<id>`.
pub fn libcomp_of(component: &Value) -> Option<String> {
    let of = &component["component"]["of"];
    let id = of["id"].as_str()?;
    Some(match of["lib"].as_str() {
        Some(lib) if lib != "self" => format!("lib/{lib}/libcomp/{id}"),
        _ => id.to_owned(),
    })
}

/// The Properties tile's contents for one reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Properties {
    pub reference: String,
    pub rows: Vec<(String, String)>,
    /// A component's pins and their nets, or a net's pins.
    pub pins: Vec<(String, String)>,
    /// The texts of the block's instructions that name it.
    pub naming: Vec<String>,
}

impl Properties {
    pub fn new(reference: &str) -> Properties {
        Properties { reference: reference.to_owned(), ..Default::default() }
    }

    fn row(&mut self, k: &str, v: &Value) {
        let t = s(v);
        if !t.is_empty() {
            self.rows.push((k.to_owned(), t));
        }
    }

    /// `component.show`.
    pub fn component(&mut self, v: &Value) {
        let c = &v["component"];
        self.row("Designator", &c["refdes"]);
        self.row("Value", &c["value"]);
        self.row("Library part", &c["of"]["id"]);
        self.row("Footprint", &c["footprint"]);
        for (k, x) in c["parameters"].as_object().into_iter().flatten() {
            self.row(k, x);
        }
        self.pins = v["connections"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(pin, net)| (pin.clone(), s(net)))
            .collect();
    }

    /// `libcomp.show` of the component's library part.
    pub fn libcomp(&mut self, v: &Value) {
        let l = &v["libcomp"];
        self.row("Manufacturer", &l["manufacturer"]);
        self.row("MPN", &l["mpn"]);
        let offers: Vec<String> = l["offers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| format!("{} {}", s(&o["supplier"]), s(&o["sku"])))
            .collect();
        if !offers.is_empty() {
            self.rows.push(("Offers".into(), offers.join(", ")));
        }
    }

    /// `net.show`.
    pub fn net(&mut self, v: &Value) {
        let n = &v["net"];
        self.row("Name", &n["name"]);
        self.row("Class", &n["class"]);
        for k in ["voltage", "current", "impedance"] {
            self.row(k, &n[k]);
        }
        self.pins = v["pins"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| (s(p), String::new()))
            .collect();
    }

    /// `layout.instructions` of its block: those naming it.
    pub fn instructions(&mut self, v: &Value) {
        let me = self.reference.clone();
        self.naming = v["instructions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| strings(&i["refs"]).iter().any(|r| *r == me || me.starts_with(&format!("{r}/"))))
            .map(|i| s(&i["text"]))
            .collect();
    }
}

/// One finding of any source, as the Findings tile lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Finding {
    /// `lint`, `erc` or `check`.
    pub source: &'static str,
    pub code: String,
    pub severity: String,
    pub message: String,
    pub refs: Vec<String>,
    pub what: String,
    pub why: String,
    pub fix: String,
}

#[derive(Debug, Default)]
pub struct Findings {
    rules: BTreeMap<String, (String, String, String)>,
    lint: Vec<SheetFinding>,
    erc: Vec<Finding>,
    check: Vec<Finding>,
}

fn of_json(source: &'static str, f: &Value) -> Finding {
    Finding {
        source,
        code: s(&f["code"]),
        severity: s(&f["severity"]),
        message: s(&f["message"]),
        refs: strings(&f["refs"]),
        ..Default::default()
    }
}

/// Every object holding a `code` and a `message` under `v`, depth first.
fn walk<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) if m.contains_key("code") && m.contains_key("message") => out.push(v),
        Value::Object(m) => m.values().for_each(|x| walk(x, out)),
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        _ => {}
    }
}

impl Findings {
    /// `lint.get`: every rule's what, why and fix.
    pub fn rules(&mut self, v: &Value) {
        for r in v["rules"].as_array().into_iter().flatten() {
            self.rules.insert(s(&r["code"]), (s(&r["what"]), s(&r["why"]), s(&r["fix"])));
        }
    }

    /// The shown sheet's lint, from its scene.
    pub fn sheet(&mut self, found: &[SheetFinding]) {
        self.lint = found.to_vec();
    }

    /// `erc`: every design's findings and the project's, waived ones left
    /// out.
    pub fn erc(&mut self, v: &Value) {
        let mut out = Vec::new();
        for d in v["designs"].as_array().into_iter().flatten() {
            for f in d["findings"].as_array().into_iter().flatten() {
                if f["waived"].is_null() {
                    out.push(of_json("erc", f));
                }
            }
        }
        for f in v["findings"].as_array().into_iter().flatten() {
            out.push(of_json("erc", f));
        }
        self.erc = out;
    }

    /// `check`: its files' faults and supply findings. ERC's are the ERC
    /// list's.
    pub fn check(&mut self, v: &Value) {
        let mut found = Vec::new();
        if let Some(m) = v.as_object() {
            for (k, x) in m {
                if k != "erc" && k != "designs" {
                    walk(x, &mut found);
                }
            }
        }
        self.check = found.into_iter().map(|f| of_json("check", f)).collect();
    }

    /// Lint, then ERC, then check, each with its rule's words where known.
    pub fn all(&self) -> Vec<Finding> {
        let lint = self.lint.iter().map(|f| Finding {
            source: "lint",
            code: f.code.clone(),
            severity: f.severity.clone(),
            message: f.message.clone(),
            refs: f.refs.clone(),
            ..Default::default()
        });
        lint.chain(self.erc.iter().cloned())
            .chain(self.check.iter().cloned())
            .map(|mut f| {
                if let Some((what, why, fix)) = self.rules.get(&f.code) {
                    (f.what, f.why, f.fix) = (what.clone(), why.clone(), fix.clone());
                }
                f
            })
            .collect()
    }
}

/// A lasting instruction as the Instructions tile lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub id: String,
    pub text: String,
    pub state: String,
    pub why: Option<String>,
    pub refs: Vec<String>,
}

impl Instruction {
    /// `held`, `pulled` or `not held`.
    pub fn state_text(&self) -> &str {
        match self.state.as_str() {
            "not_held" => "not held",
            s => s,
        }
    }
}

/// `layout.instructions`.
pub fn instructions(v: &Value) -> Vec<Instruction> {
    v["instructions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| Instruction {
            id: s(&i["id"]),
            text: s(&i["text"]),
            state: s(&i["state"]),
            why: i["why"].as_str().map(str::to_owned),
            refs: strings(&i["refs"]),
        })
        .collect()
}

/// One activity entry as the tile lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub seq: u64,
    /// `agent edited`.
    pub text: String,
    pub undo: bool,
    pub redo: bool,
}

/// `activity.log`, newest first.
pub fn activity(v: &Value) -> Vec<Entry> {
    use ryu_app::toast::Words;
    let mut out: Vec<Entry> = v["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| Entry {
            seq: e["seq"].as_u64().unwrap_or(0),
            text: format!(
                "{} {}",
                e["participant"]["name"].as_str().unwrap_or("outside"),
                crate::core::product::Eda::verb(e["method"].as_str().unwrap_or(""), &e["kind"])
            ),
            undo: can_undo(e),
            redo: can_redo(e),
        })
        .collect();
    out.sort_by(|a, b| b.seq.cmp(&a.seq));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_properties_ask_and_show() {
        assert_eq!(subject("top/r1/2", "pin"), Subject::Component("top/r1".into()));
        assert_eq!(subject("top/r1", "component"), Subject::Component("top/r1".into()));
        assert_eq!(subject("top/label:sda@r1/1", "label"), Subject::Net("top/sda".into()));
        assert_eq!(subject("top/sda", "net"), Subject::Net("top/sda".into()));
        assert_eq!(subject("top/region:power", "region"), Subject::Other);
        let mut p = Properties::new("top/r1");
        p.component(&json!({"reference": "top/r1", "component": {"of": {"id": "r_100k"}, "refdes": "R1", "value": "100k",
            "parameters": {"tolerance": "1%"}}, "connections": {"1": "sda", "2": "rail:v3v3"}}));
        p.libcomp(&json!({"libcomp": {"manufacturer": "Yageo", "mpn": "RC0402FR-07100KL",
            "offers": [{"supplier": "LCSC", "sku": "C1"}]}}));
        p.instructions(&json!({"instructions": [
            {"id": "i1", "text": "beside R1 J1 right", "state": "held", "refs": ["top/j1", "top/r1"]},
            {"id": "i2", "text": "net SDA label", "state": "held", "refs": ["top/sda"]}]}));
        let rows: Vec<(&str, &str)> = p.rows.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert!(rows.contains(&("Designator", "R1")));
        assert!(rows.contains(&("Value", "100k")));
        assert!(rows.contains(&("tolerance", "1%")));
        assert!(rows.contains(&("MPN", "RC0402FR-07100KL")));
        assert!(rows.contains(&("Offers", "LCSC C1")));
        assert_eq!(p.pins, [("1".to_owned(), "sda".to_owned()), ("2".to_owned(), "rail:v3v3".to_owned())]);
        assert_eq!(p.naming, ["beside R1 J1 right"]);
        assert_eq!(libcomp_of(&json!({"component": {"of": {"id": "r_100k"}}})).as_deref(), Some("r_100k"));
        assert_eq!(libcomp_of(&json!({"component": {"of": {"lib": "parts", "id": "x"}}})).as_deref(), Some("lib/parts/libcomp/x"));
    }

    #[test]
    fn findings_from_lint_erc_and_check_with_their_rules() {
        let mut f = Findings::default();
        f.rules(&json!({"rules": [{"code": "sch_crossing", "what": "W", "why": "Y", "fix": "F"}]}));
        f.sheet(&[crate::core::scene::Finding {
            code: "sch_crossing".into(), severity: "warning".into(), message: "m".into(), refs: vec!["top/r1".into()] }]);
        f.erc(&json!({"designs": [{"design": "d", "findings": [
            {"code": "net_single_pin", "severity": "error", "message": "one pin", "refs": ["top/sda"], "waived": null},
            {"code": "x", "severity": "warning", "message": "waived", "refs": [], "waived": {"reason": "ok"}}]}],
            "findings": [{"code": "waiver_unused", "severity": "warning", "message": "w", "refs": ["waiver/w1"]}]}));
        f.check(&json!({"files": [{"faults": [{"code": "format_too_new", "message": "newer", "refs": ["blocks/top/block.json"]}]}],
            "erc": {"ignored": true}}));
        let all = f.all();
        assert_eq!(all.len(), 4, "{all:?}");
        assert_eq!(all[0].source, "lint");
        assert_eq!((all[0].what.as_str(), all[0].why.as_str(), all[0].fix.as_str()), ("W", "Y", "F"));
        assert_eq!(all[1].source, "erc");
        assert_eq!(all[1].refs, ["top/sda"]);
        assert_eq!(all[3].source, "check");
        assert_eq!(all[3].code, "format_too_new");
    }

    #[test]
    fn instructions_and_activity() {
        let i = instructions(&json!({"instructions": [
            {"id": "i1", "text": "near R1 U1", "holds": false, "state": "pulled", "why": "3.2 mm left; U2 in the way", "refs": ["top/r1"]}]}));
        assert_eq!(i[0].state_text(), "pulled");
        assert_eq!(i[0].why.as_deref(), Some("3.2 mm left; U2 in the way"));
        let a = activity(&json!({"entries": [
            {"seq": 1, "kind": "change", "method": "component.edit", "participant": {"name": "agent"}, "undone_by": null},
            {"seq": 2, "kind": "undo", "method": "component.edit", "participant": {"name": "Dana"}, "undone_by": null}]}));
        assert_eq!(a[0].seq, 2, "newest first");
        assert!(a[0].redo && !a[0].undo);
        assert!(a[1].undo && !a[1].redo);
        assert_eq!(a[1].text, "agent edited");
    }
}
