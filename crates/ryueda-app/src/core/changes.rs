//! The Changes tab (design 688, The compare view): `diff --layout` from
//! the compared revision to the working files, by block, and `diff
//! --netlist` as its Electrical section. Read from the daemon's answers,
//! keyed by the same reference paths as the scene.

use serde_json::Value;

/// One object's change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub verb: String,
    pub reference: String,
    pub name: Option<String>,
    pub lines: Vec<String>,
    /// The sheet its layout entry draws on.
    pub sheet: Option<String>,
    pub block: String,
}

/// An electrical change, with what Show frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Electrical {
    pub text: String,
    pub reference: Option<String>,
}

/// Where a row's Show goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Show {
    Working(String),
    Removed {
        block: String,
        sheet: String,
        reference: String,
    },
    Nowhere,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub rev: String,
    pub blocks: Vec<(String, Vec<Row>)>,
    pub electrical: Vec<Electrical>,
    got_layout: bool,
    got_netlist: bool,
}

/// A JSON value as a change line shows it: a string bare, else its JSON;
/// nothing as `none`.
fn shown(v: &Value) -> String {
    match v {
        Value::Null => "none".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_owned))
        .collect()
}

impl Changes {
    pub fn new(rev: &str) -> Changes {
        Changes {
            rev: rev.to_owned(),
            ..Changes::default()
        }
    }

    pub fn loaded(&self) -> bool {
        self.got_layout && self.got_netlist
    }

    /// `diff --layout`'s answer.
    pub fn layout(&mut self, v: &Value) {
        self.blocks.clear();
        for b in v["blocks"].as_array().into_iter().flatten() {
            let block = b["block"].as_str().unwrap_or_default().to_owned();
            let mut rows: Vec<Row> = Vec::new();
            for key in ["components", "nets", "instances", "ports", "buses"] {
                for o in b[key].as_array().into_iter().flatten() {
                    let Some(reference) = o["ref"].as_str() else {
                        continue;
                    };
                    let mut lines = strings(&o["pins"]);
                    lines.extend(strings(&o["ports"]));
                    for c in o["changes"].as_array().into_iter().flatten() {
                        lines.push(format!(
                            "{}: {} → {}",
                            c["at"].as_str().unwrap_or_default().trim_start_matches('/'),
                            shown(&c["from"]),
                            shown(&c["to"])
                        ));
                    }
                    rows.push(Row {
                        verb: o["status"].as_str().unwrap_or("changed").to_owned(),
                        reference: reference.to_owned(),
                        name: o["name"].as_str().map(str::to_owned),
                        lines,
                        sheet: None,
                        block: block.clone(),
                    });
                }
            }
            for c in b["layout"]["changes"].as_array().into_iter().flatten() {
                let Some(reference) = c["ref"].as_str() else {
                    continue;
                };
                let change = c["change"].as_str().unwrap_or_default();
                let sheet = c["sheet"].as_str().map(str::to_owned);
                let line = format!("{change} on {}", sheet.as_deref().unwrap_or("?"));
                match rows.iter_mut().find(|r| r.reference == reference) {
                    Some(r) => {
                        if !r.lines.contains(&line) && change != r.verb {
                            r.lines.push(line);
                        }
                        r.sheet = r.sheet.take().or(sheet);
                    }
                    None => rows.push(Row {
                        verb: change.to_owned(),
                        reference: reference.to_owned(),
                        name: None,
                        lines: vec![line],
                        sheet,
                        block: block.clone(),
                    }),
                }
            }
            if !rows.is_empty() {
                self.blocks.push((block, rows));
            }
        }
        self.got_layout = true;
    }

    /// `diff --netlist`'s answer.
    pub fn netlist(&mut self, v: &Value) {
        self.electrical.clear();
        for d in v["netlist"].as_array().into_iter().flatten() {
            for o in d["occurrences"].as_array().into_iter().flatten() {
                let mut text = format!(
                    "{} {} {}",
                    o["status"].as_str().unwrap_or_default(),
                    o["refdes"].as_str().unwrap_or_default(),
                    o["value"].as_str().unwrap_or_default()
                )
                .trim_end()
                .to_owned();
                if let Some(c) = o["changes"].as_object() {
                    let parts: Vec<String> = c
                        .iter()
                        .map(|(k, x)| format!("{k}: {} → {}", shown(&x["from"]), shown(&x["to"])))
                        .collect();
                    text = format!("{text}: {}", parts.join(", "));
                }
                self.electrical.push(Electrical {
                    text,
                    reference: o["occurrence"].as_str().map(str::to_owned),
                });
            }
            for n in d["nets"].as_array().into_iter().flatten() {
                let mut parts = Vec::new();
                if let Some(r) = n["renamed"].as_object() {
                    parts.push(format!(
                        "renamed {} → {}",
                        shown(&r["from"]),
                        shown(&r["to"])
                    ));
                }
                for (k, word) in [("joined", "joined"), ("left", "left")] {
                    let list = strings(&n[k]);
                    if !list.is_empty() {
                        parts.push(format!("{word} {}", list.join(", ")));
                    }
                }
                let head = format!(
                    "{} net {}",
                    n["status"].as_str().unwrap_or_default(),
                    n["name"].as_str().unwrap_or_default()
                );
                self.electrical.push(Electrical {
                    text: if parts.is_empty() {
                        head
                    } else {
                        format!("{head}: {}", parts.join("; "))
                    },
                    reference: n["net"].as_str().map(str::to_owned),
                });
            }
            for (k, word) in [("appeared", "appeared"), ("gone", "gone")] {
                for f in d["erc"][k].as_array().into_iter().flatten() {
                    self.electrical.push(Electrical {
                        text: format!(
                            "ERC {word}: {} {}",
                            f["code"].as_str().unwrap_or_default(),
                            f["message"].as_str().unwrap_or_default()
                        ),
                        reference: None,
                    });
                }
            }
        }
        self.got_netlist = true;
    }

    /// Where a row's Show goes: a removed object to its ghost on the
    /// sheet its layout entry names, anything else to itself.
    pub fn show(row: &Row) -> Show {
        match (row.verb.as_str(), &row.sheet) {
            ("removed", Some(s)) => Show::Removed {
                block: row.block.clone(),
                sheet: s.clone(),
                reference: row.reference.clone(),
            },
            ("removed", None) => Show::Nowhere,
            _ => Show::Working(row.reference.clone()),
        }
    }

    /// A reference's verb and change lines, for Properties.
    pub fn lines_of(&self, reference: &str) -> Vec<String> {
        self.blocks
            .iter()
            .flat_map(|(_, rows)| rows)
            .find(|r| r.reference == reference)
            .map(|r| {
                std::iter::once(r.verb.clone())
                    .chain(r.lines.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layout() -> Value {
        json!({"from": {"rev": "HEAD", "commit": "c0"}, "blocks": [{
            "block": "breakout", "ref": "breakout", "status": "changed",
            "components": [
                {"id": "r1", "ref": "breakout/r1", "status": "changed", "name": "R1",
                 "changes": [{"at": "/value", "from": "4k7", "to": "10k"}]},
                {"id": "r3", "ref": "breakout/r3", "status": "added", "name": "R3",
                 "pins": ["pin 1: none → SDA"]},
                {"id": "c3", "ref": "breakout/c3", "status": "removed", "name": "C3"}
            ],
            "nets": [{"id": "scl", "ref": "breakout/scl", "status": "changed", "name": "I2C_SCL",
                      "changes": [{"at": "/name", "from": "SCL", "to": "I2C_SCL"}]}],
            "instances": [], "ports": [], "buses": [],
            "layout": {"status": "changed", "changes": [
                {"key": "placed/j1", "change": "moved", "sheet": "s1", "ref": "breakout/j1"},
                {"key": "placed/c3", "change": "removed", "sheet": "s1", "ref": "breakout/c3"}
            ]}
        }]})
    }

    fn netlist() -> Value {
        json!({"from": {"rev": "HEAD"}, "netlist": [{
            "design": "breakout", "status": "changed",
            "occurrences": [{"occurrence": "breakout/r3", "status": "added", "refdes": "R3", "value": "4k7"}],
            "nets": [{"net": "breakout/sda", "status": "changed", "name": "SDA", "joined": ["R3.1"]}],
            "erc": {"appeared": [{"id": "e1", "code": "erc_pull", "severity": "warning", "message": "two pull-ups"}], "gone": []}
        }]})
    }

    #[test]
    fn rows_read_from_the_layout_diff_and_show_where_they_are() {
        let mut c = Changes::new("HEAD");
        assert!(!c.loaded());
        c.layout(&layout());
        let rows = &c.blocks[0].1;
        assert_eq!(c.blocks[0].0, "breakout");
        let r1 = rows.iter().find(|r| r.reference == "breakout/r1").unwrap();
        assert_eq!(
            (r1.verb.as_str(), r1.lines.as_slice()),
            ("changed", &["value: 4k7 → 10k".to_owned()][..])
        );
        let r3 = rows.iter().find(|r| r.reference == "breakout/r3").unwrap();
        assert_eq!(r3.lines, ["pin 1: none → SDA"]);
        let j1 = rows.iter().find(|r| r.reference == "breakout/j1").unwrap();
        assert_eq!(
            (j1.verb.as_str(), j1.lines.as_slice()),
            ("moved", &["moved on s1".to_owned()][..])
        );
        let c3 = rows.iter().find(|r| r.reference == "breakout/c3").unwrap();
        assert_eq!(c3.sheet.as_deref(), Some("s1"));
        assert_eq!(Changes::show(&rows[0]), Show::Working("breakout/r1".into()));
        assert_eq!(
            Changes::show(c3),
            Show::Removed {
                block: "breakout".into(),
                sheet: "s1".into(),
                reference: "breakout/c3".into()
            }
        );
        assert_eq!(c.lines_of("breakout/r1"), ["changed", "value: 4k7 → 10k"]);
        assert!(c.lines_of("breakout/r9").is_empty());
        c.netlist(&netlist());
        assert!(c.loaded());
        let e: Vec<&str> = c.electrical.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(
            e,
            [
                "added R3 4k7",
                "changed net SDA: joined R3.1",
                "ERC appeared: erc_pull two pull-ups"
            ]
        );
        assert_eq!(c.electrical[0].reference.as_deref(), Some("breakout/r3"));
        assert_eq!(
            c.electrical[2].reference, None,
            "ERC findings in the diff carry no references"
        );
    }
}
