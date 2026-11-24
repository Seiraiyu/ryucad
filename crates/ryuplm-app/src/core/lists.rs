//! What each tile lists, read from the answers the daemon already gives
//! (design 938, The window): the open item's card (`item.show`, with
//! `settings.show`'s property types and `reps.list`'s folders), its BOM
//! (`bom.get`), where it is used (`bom.where_used`), the findings
//! (`check`), the changes (`diff`, `bom.diff`), releases (`release.list`),
//! imports (`import.list`) and activity (`activity.log`).

use ryu_doc::error::{Code, Error, Result};
use serde_json::Value;

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn texts(v: &Value) -> Vec<String> {
    ryu_app::people::strings(v)
}

/// A value as a person reads it: a string bare, anything else as JSON.
pub fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub name: String,
    pub value: String,
    /// Its declared type (`settings.show`'s `ryupdm.properties`).
    pub kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Aml {
    pub manufacturer: String,
    pub mpn: String,
    pub status: String,
    pub preferred: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub product: String,
    pub project: String,
    pub part: String,
    /// Where the project was found (`reps.list`), or `None`: not found.
    pub folder: Option<String>,
}

/// The open item, as the Item tile shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: String,
    pub number: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub class: Option<String>,
    pub unit: String,
    pub make_buy: Option<String>,
    pub state: String,
    pub label: Option<String>,
    pub properties: Vec<Prop>,
    /// The preferred entry first, then as written.
    pub aml: Vec<Aml>,
    pub links: Vec<Link>,
    pub previous: Vec<String>,
    /// `(import, key)`: which import made or updated it.
    pub provenance: Vec<(String, String)>,
    /// Its documents as written: an ID, or `<catalogue>:<ID>`.
    pub documents: Vec<String>,
    pub faults: Vec<Finding>,
    pub revision: Option<String>,
}

impl Card {
    /// `item.show`'s answer.
    pub fn read(v: &Value) -> Option<Card> {
        let i = &v["item"];
        let mut aml: Vec<Aml> = i["aml"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| Aml {
                manufacturer: text(&a["manufacturer"]).unwrap_or_default(),
                mpn: text(&a["mpn"]).unwrap_or_default(),
                status: text(&a["status"]).unwrap_or_else(|| "unknown".into()),
                preferred: a["preferred"] == true,
            })
            .collect();
        aml.sort_by_key(|a| !a.preferred);
        Some(Card {
            id: text(&i["id"])?,
            number: text(&i["number"])?,
            name: text(&i["name"]),
            description: text(&i["description"]),
            class: text(&i["class"]),
            unit: text(&i["unit"]).unwrap_or_else(|| "each".into()),
            make_buy: text(&i["make_buy"]),
            state: text(&v["state"]["state"]).unwrap_or_default(),
            label: text(&v["state"]["label"]),
            properties: i["properties"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, x)| Prop {
                    name: k.clone(),
                    value: shown(x),
                    kind: None,
                })
                .collect(),
            aml,
            links: i["links"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| Link {
                    product: text(&l["product"]).unwrap_or_default(),
                    project: text(&l["project"]).unwrap_or_default(),
                    part: text(&l["part"]).unwrap_or_default(),
                    folder: None,
                })
                .collect(),
            previous: texts(&i["previous_numbers"]),
            provenance: i["source"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, x)| {
                    (
                        k.clone(),
                        x["key"]
                            .as_str()
                            .map_or_else(|| shown(&x["key"]), str::to_owned),
                    )
                })
                .collect(),
            documents: texts(&i["documents"]),
            faults: v["faults"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Finding::read)
                .collect(),
            revision: text(&v["revision"]),
        })
    }

    /// Each property's declared type, from `settings.show`.
    pub fn types(&mut self, settings: &Value) {
        let declared = &settings["ryupdm"]["properties"];
        for p in &mut self.properties {
            p.kind = text(&declared[&p.name]["type"]);
        }
    }

    /// Where each link's project was found, from `reps.list`.
    pub fn reps(&mut self, reps: &Value) {
        let projects = reps["projects"].as_array().cloned().unwrap_or_default();
        for l in &mut self.links {
            l.folder = projects
                .iter()
                .find(|p| p["product"] == l.product.as_str() && p["project"] == l.project.as_str())
                .and_then(|p| text(&p["folder"]));
        }
    }

    pub fn reference(&self) -> String {
        format!("item/{}", self.id)
    }
}

/// A finding (`check`'s fault, or an item's own), with what it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub item: Option<String>,
    pub line: Option<String>,
    /// Not raised at the revision `check --against` compared with.
    pub new: bool,
}

impl Finding {
    pub fn read(v: &Value) -> Option<Finding> {
        Some(Finding {
            severity: text(&v["severity"])?,
            code: text(&v["code"])?,
            message: text(&v["message"]).unwrap_or_default(),
            item: text(&v["details"]["item"]),
            line: text(&v["details"]["line"]),
            new: v["new"] == true,
        })
    }

    /// The live reference its Show selects: the line, else the item.
    pub fn reference(&self) -> Option<String> {
        let item = self.item.as_ref()?;
        Some(match &self.line {
            Some(l) => format!("item/{item}/line/{l}"),
            None => format!("item/{item}"),
        })
    }
}

/// `check`'s faults, errors first, each kind in the order given. A
/// failed check (`check_failed`) is its report; any other refusal lists
/// nothing (the tile shows the error).
pub fn findings(a: &Result<Value>) -> Vec<Finding> {
    let report = match a {
        Ok(v) => v,
        Err(Error {
            code: Code::CheckFailed,
            details,
            ..
        }) => &details["report"],
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<Finding> = report["faults"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Finding::read)
        .collect();
    out.sort_by_key(|f| f.severity != "error");
    out
}

/// One line of the open item's joined BOM (`bom.get`).
#[derive(Clone, Debug, PartialEq)]
pub struct BomLine {
    /// `own`, `ryucad` or `ryueda`.
    pub from: String,
    /// An own line's ID.
    pub line: Option<String>,
    /// A representation line's key.
    pub key: Option<String>,
    /// The child as a live reference, when it is this catalogue's item.
    pub child: Option<String>,
    /// The child as written: `<catalogue>:<ID>` for another catalogue's.
    pub item: Option<String>,
    pub number: Option<String>,
    pub name: Option<String>,
    /// `product project part` of an unlinked representation child.
    pub representation: Option<String>,
    pub quantity: String,
    pub unit: Option<String>,
    pub find: Option<String>,
    pub refdes: Option<String>,
    /// `bom_unlinked` or `bom_line_twice`, on the row that has it.
    pub problem: Option<String>,
}

impl BomLine {
    /// The live reference of an own line of `parent`.
    pub fn reference(&self, parent: &str) -> Option<String> {
        self.line
            .as_ref()
            .map(|l| format!("item/{parent}/line/{l}"))
    }

    /// What the child column says.
    pub fn title(&self) -> String {
        match (&self.number, &self.name, &self.representation, &self.item) {
            (Some(n), Some(name), _, _) => format!("{n} {name}"),
            (Some(n), None, _, _) => n.clone(),
            (None, _, Some(r), _) => r.clone(),
            (None, _, None, Some(i)) => i.clone(),
            _ => String::new(),
        }
    }
}

/// `bom.get`'s lines, one level, with `bom_unlinked` on each
/// representation line whose child no item links and `bom_line_twice`
/// on each own line whose child a representation also places.
pub fn bom(v: &Value) -> Vec<BomLine> {
    let mut out: Vec<BomLine> = v["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let item = text(&l["item"]);
            let r = &l["representation"];
            BomLine {
                from: text(&l["from"]).unwrap_or_default(),
                line: text(&l["line"]),
                key: text(&l["key"]),
                child: item
                    .as_ref()
                    .filter(|i| !i.contains(':'))
                    .map(|i| format!("item/{i}")),
                item,
                number: text(&l["number"]),
                name: text(&l["name"]),
                representation: r.is_object().then(|| {
                    format!(
                        "{} {} {}",
                        shown(&r["product"]),
                        shown(&r["project"]),
                        shown(&r["part"])
                    )
                }),
                quantity: l["quantity"]
                    .as_str()
                    .map_or_else(|| shown(&l["quantity"]), str::to_owned),
                unit: text(&l["unit"]),
                find: text(&l["find"]),
                refdes: text(&l["refdes"]),
                problem: None,
            }
        })
        .collect();
    let placed: Vec<String> = out
        .iter()
        .filter(|l| l.from != "own")
        .filter_map(|l| l.item.clone())
        .collect();
    for l in &mut out {
        if l.from != "own" && l.item.is_none() {
            l.problem = Some("bom_unlinked".into());
        } else if l.from == "own" && l.item.as_ref().is_some_and(|i| placed.contains(i)) {
            l.problem = Some("bom_line_twice".into());
        }
    }
    out
}

/// A parent of the open item (`bom.where_used`).
#[derive(Clone, Debug, PartialEq)]
pub struct Used {
    pub item: String,
    pub number: String,
    pub via: String,
    pub quantity: String,
    /// The catalogue's project ID and folder.
    pub catalogue: String,
    pub folder: Option<String>,
    /// The official release's label; `None` for the working files.
    pub release: Option<String>,
}

impl Used {
    pub fn reference(&self) -> String {
        format!("item/{}", self.item)
    }
}

/// `bom.where_used`'s working rows, then its released rows.
pub fn where_used(v: &Value) -> Vec<Used> {
    ["working", "released"]
        .iter()
        .flat_map(|k| v[*k].as_array().cloned().unwrap_or_default())
        .filter_map(|r| {
            Some(Used {
                item: text(&r["item"])?,
                number: text(&r["number"]).unwrap_or_default(),
                via: text(&r["via"]).unwrap_or_default(),
                quantity: shown(&r["quantity"]),
                catalogue: text(&r["catalogue"]).unwrap_or_default(),
                folder: text(&r["folder"]),
                release: text(&r["release"]),
            })
        })
        .collect()
}

/// An item added, changed or removed since `HEAD` (`diff`), with its
/// textconv lines and, from `bom.diff`, its BOM's counts.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub verb: &'static str,
    pub id: String,
    pub number: String,
    /// `- old line`, `+ new line`.
    pub lines: Vec<String>,
    pub bom: Option<String>,
}

impl Change {
    pub fn reference(&self) -> String {
        format!("item/{}", self.id)
    }
}

pub fn changes(diff: &Value, bom_diff: Option<&Value>) -> Vec<Change> {
    let mut out = Vec::new();
    for verb in ["added", "changed", "removed"] {
        for c in diff[verb].as_array().into_iter().flatten() {
            let Some(id) = text(&c["id"]) else { continue };
            let mut lines: Vec<String> = texts(&c["removed"])
                .into_iter()
                .map(|l| format!("- {l}"))
                .collect();
            lines.extend(texts(&c["added"]).into_iter().map(|l| format!("+ {l}")));
            out.push(Change {
                verb,
                number: text(&c["number"]).unwrap_or_default(),
                id,
                lines,
                bom: None,
            });
        }
    }
    for b in bom_diff
        .and_then(|b| b["items"].as_array())
        .into_iter()
        .flatten()
    {
        let Some(id) = b["item"].as_str() else {
            continue;
        };
        let n = |k: &str| b[k].as_array().map_or(0, Vec::len);
        let parts: Vec<String> = [
            ("added", n("added")),
            ("removed", n("removed")),
            ("changed", n("changed")),
        ]
        .into_iter()
        .filter(|(_, k)| *k > 0)
        .map(|(w, k)| format!("{k} {w}"))
        .collect();
        let said = format!("BOM: {}", parts.join(", "));
        match out.iter_mut().find(|c| c.id == id) {
            Some(c) => c.bom = Some(said),
            None => out.push(Change {
                verb: "changed",
                id: id.to_owned(),
                number: text(&b["number"]).unwrap_or_default(),
                lines: Vec::new(),
                bom: Some(said),
            }),
        }
    }
    out
}

/// A release record (`release.list`).
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub part: String,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub by: Option<String>,
    pub tag: Option<String>,
}

impl Release {
    pub fn text(&self) -> String {
        let mut t = format!("{} {}", self.label, self.status);
        if let Some(b) = &self.by {
            t.push_str(&format!(" · by {b}"));
        }
        if let Some(s) = &self.tag {
            t.push_str(&format!(" · tag {s}"));
        }
        t
    }
}

pub fn releases(v: &Value) -> Vec<Release> {
    v["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Release {
                part: text(&r["part"])?,
                kind: text(&r["kind"]).unwrap_or_else(|| "item".into()),
                label: text(&r["label"])?,
                status: text(&r["status"]).unwrap_or_else(|| "proposed".into()),
                by: text(&r["released_by"]["name"]),
                tag: text(&r["tag"]["status"]),
            })
        })
        .collect()
}

/// A mapping (`import.list`).
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub name: String,
    pub reader: Option<String>,
    pub key: Option<String>,
    pub items: u64,
    pub error: Option<String>,
}

impl Import {
    pub fn reference(&self) -> String {
        format!("import/{}", self.name)
    }

    pub fn text(&self) -> String {
        match &self.error {
            Some(e) => format!("doesn't read: {e}"),
            None => format!(
                "{}, keyed by {} · {} items",
                self.reader.as_deref().unwrap_or("?"),
                self.key.as_deref().unwrap_or("?"),
                self.items
            ),
        }
    }
}

pub fn imports(v: &Value) -> Vec<Import> {
    v["imports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(Import {
                name: text(&m["name"])?,
                reader: text(&m["reader"]["type"]),
                key: m.get("key").filter(|k| !k.is_null()).map(shown),
                items: m["items"].as_u64().unwrap_or(0),
                error: text(&m["error"]["code"]),
            })
        })
        .collect()
}

/// An activity entry, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub seq: u64,
    pub method: String,
    /// `agent edited`.
    pub text: String,
    pub undo: bool,
    pub redo: bool,
}

pub fn activity(v: &Value) -> Vec<Entry> {
    use crate::core::activity::{can_redo, can_undo};
    use ryu_app::toast::Words;
    let mut out: Vec<Entry> = v["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| Entry {
            seq: e["seq"].as_u64().unwrap_or(0),
            method: text(&e["method"]).unwrap_or_default(),
            text: format!(
                "{} {}",
                e["participant"]["name"].as_str().unwrap_or("outside"),
                crate::core::product::Plm::verb(e["method"].as_str().unwrap_or(""), &e["kind"])
            ),
            undo: can_undo(e),
            redo: can_redo(e),
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.seq));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_doc::error::{Code, Error};
    use serde_json::json;

    fn shown() -> Value {
        json!({"schema": 1, "path": "items/k3/k3v9q2m8x4c1p.json", "revision": "r1",
            "state": {"state": "released", "text": "released at A", "label": "A"},
            "item": {"id": "k3v9q2m8x4c1p", "number": "100-0042", "previous_numbers": ["OLD-7"],
                "name": "Bracket kit", "class": "mech", "unit": "each", "make_buy": "make",
                "properties": {"mass": 0.25, "finish": "zinc"},
                "aml": [{"manufacturer": "Bolt Co", "mpn": "BR-42", "status": "nrnd"},
                        {"manufacturer": "Acme", "mpn": "BR-42", "status": "active", "preferred": true}],
                "links": [{"product": "ryucad", "project": "p1", "part": "bracket"}],
                "documents": ["d000000000001", "c0ffee:d000000000002"],
                "source": {"erp": {"key": "42", "digest": "ab"}}},
            "faults": [{"code": "item_link_unresolved", "severity": "warning", "message": "not found",
                        "details": {"item": "k3v9q2m8x4c1p"}}]})
    }

    #[test]
    fn an_item_s_card() {
        let mut c = Card::read(&shown()).unwrap();
        assert_eq!(
            (c.number.as_str(), c.state.as_str()),
            ("100-0042", "released")
        );
        assert_eq!(c.label.as_deref(), Some("A"));
        assert_eq!(c.aml[0].manufacturer, "Acme", "the preferred entry first");
        assert!(c.aml[0].preferred && !c.aml[1].preferred);
        assert_eq!(c.previous, ["OLD-7"]);
        assert_eq!(c.provenance, [("erp".to_owned(), "42".to_owned())]);
        assert_eq!(c.documents, ["d000000000001", "c0ffee:d000000000002"]);
        assert_eq!(c.faults[0].code, "item_link_unresolved");
        assert_eq!(
            c.properties[0],
            Prop {
                name: "finish".into(),
                value: "zinc".into(),
                kind: None
            }
        );
        c.types(&json!({"ryupdm": {"properties": {"mass": {"type": "number"}, "finish": {"type": "string"}}}}));
        assert_eq!(c.properties[1].kind.as_deref(), Some("number"));
        c.reps(&json!({"projects": [{"product": "ryucad", "project": "p1", "items": 3, "folder": "/w/mech"}]}));
        assert_eq!(c.links[0].folder.as_deref(), Some("/w/mech"));
        c.reps(&json!({"projects": [{"product": "ryucad", "project": "p1", "items": 3, "folder": null}]}));
        assert_eq!(c.links[0].folder, None);
    }

    #[test]
    fn bom_lines_say_their_source_and_their_problems() {
        let v = json!({"schema": 1, "item": "k3v9q2m8x4c1p", "number": "100-0042", "rev": "working",
            "lines": [
                {"from": "own", "line": "l000000000001", "item": "a000000000001", "number": "SCREW-1",
                 "name": "Screw", "quantity": "4", "find": "10"},
                {"from": "ryucad", "key": "screw", "rev": "working", "quantity": "2",
                 "representation": {"product": "ryucad", "project": "p1", "part": "screw"}},
                {"from": "ryucad", "key": "nut", "rev": "working", "quantity": "1", "item": "a000000000001",
                 "number": "SCREW-1", "representation": {"product": "ryucad", "project": "p1", "part": "nut"}},
                {"from": "own", "line": "l000000000002", "item": "c0ffee:b000000000001", "quantity": "1"},
            ],
            "findings": []});
        let b = bom(&v);
        assert_eq!(b.len(), 4);
        assert_eq!(
            b[0].reference("k3v9q2m8x4c1p").as_deref(),
            Some("item/k3v9q2m8x4c1p/line/l000000000001")
        );
        assert_eq!(b[0].child.as_deref(), Some("item/a000000000001"));
        assert_eq!(
            b[0].problem.as_deref(),
            Some("bom_line_twice"),
            "SCREW-1 is placed by ryucad too"
        );
        assert_eq!(b[1].problem.as_deref(), Some("bom_unlinked"));
        assert_eq!(b[1].title(), "ryucad p1 screw");
        assert_eq!(
            b[1].reference("k3v9q2m8x4c1p"),
            None,
            "a representation's line is no target"
        );
        assert_eq!(
            b[3].child, None,
            "another catalogue's item opens nothing here"
        );
        assert_eq!(b[3].title(), "c0ffee:b000000000001");
    }

    #[test]
    fn where_used_rows_working_then_released() {
        let v = json!({"schema": 1, "item": "a000000000001", "number": "SCREW-1",
            "working": [{"item": "k3v9q2m8x4c1p", "number": "100-0042", "via": "own", "line": "l1",
                         "quantity": "4", "catalogue": "p0", "folder": "."}],
            "released": [{"item": "k3v9q2m8x4c1p", "number": "100-0042", "via": "own", "line": "l1",
                          "quantity": "4", "catalogue": "p0", "folder": ".", "release": "A"}],
            "problems": [], "searched": []});
        let u = where_used(&v);
        assert_eq!(u.len(), 2);
        assert_eq!(
            (u[0].release.as_deref(), u[1].release.as_deref()),
            (None, Some("A"))
        );
        assert_eq!(u[0].reference(), "item/k3v9q2m8x4c1p");
    }

    #[test]
    fn findings_errors_first_from_a_pass_or_a_failure() {
        let report = json!({"schema": 1, "errors": 1, "warnings": 1, "faults": [
            {"code": "part_number_unschemed", "severity": "warning", "message": "w",
             "details": {"item": "a000000000001"}},
            {"code": "bom_cycle", "severity": "error", "message": "e",
             "details": {"item": "k3v9q2m8x4c1p", "line": "l000000000001"}, "new": true}]});
        let passed = findings(&Ok(report.clone()));
        assert_eq!(passed[0].code, "bom_cycle");
        assert_eq!(
            passed[0].reference().as_deref(),
            Some("item/k3v9q2m8x4c1p/line/l000000000001")
        );
        assert!(passed[0].new);
        let mut e = Error::new(Code::CheckFailed, "check found 1 error(s)");
        e.details = json!({"report": report});
        assert_eq!(findings(&Err(e)), passed, "a failed check is its report");
        assert!(findings(&Err(Error::new(Code::Internal, "x"))).is_empty());
    }

    #[test]
    fn changes_from_diff_and_bom_diff() {
        let d = json!({"schema": 1, "from": "HEAD", "to": "working tree",
            "added": [{"id": "n000000000001", "number": "NEW-1"}], "removed": [],
            "changed": [{"id": "k3v9q2m8x4c1p", "number": "100-0042",
                         "removed": ["name = Bracket"], "added": ["name = Bracket kit"]}],
            "documents": {"added": [], "removed": [], "changed": []}});
        let bd = json!({"schema": 1, "summary": {"added": 1, "removed": 0, "changed": 2},
            "items": [{"item": "k3v9q2m8x4c1p", "number": "100-0042",
                       "added": [{}], "removed": [], "changed": [{}, {}]}]});
        let c = changes(&d, Some(&bd));
        assert_eq!(c[0].verb, "added");
        assert_eq!(c[1].lines, ["- name = Bracket", "+ name = Bracket kit"]);
        assert_eq!(c[1].bom.as_deref(), Some("BOM: 1 added, 2 changed"));
        assert_eq!(c[1].reference(), "item/k3v9q2m8x4c1p");
    }

    #[test]
    fn releases_imports_and_activity() {
        let r = releases(&json!({"schema": 1, "repository": true, "records": [
            {"part": "k3v9q2m8x4c1p", "label": "A", "kind": "item", "status": "official",
             "released_by": {"name": "Dana"}, "tag": {"name": "ryupdm/k3v9q2m8x4c1p/A", "status": "signed"}}],
            "items": []}));
        assert_eq!(r[0].text(), "A official · by Dana · tag signed");
        let i = imports(&json!({"imports": [
            {"name": "erp", "path": "imports/erp.json", "reader": {"type": "csv"}, "key": "Part", "bom": false, "items": 12},
            {"name": "bad", "path": "imports/bad.json", "error": {"code": "import_mapping_invalid", "message": "x"}, "items": 0}]}));
        assert_eq!(i[0].text(), "csv, keyed by Part · 12 items");
        assert_eq!(i[1].text(), "doesn't read: import_mapping_invalid");
        let a = activity(&json!({"entries": [
            {"seq": 1, "method": "item.create", "kind": "change", "undone_by": null, "participant": {"name": "agent"}},
            {"seq": 2, "method": "item.create", "kind": "undo", "undone_by": null, "participant": {"name": "Dana"}}]}));
        assert_eq!(
            (a[0].seq, a[0].text.as_str(), a[0].redo),
            (2, "Dana undid", true)
        );
        assert!(a[1].undo);
    }
}
