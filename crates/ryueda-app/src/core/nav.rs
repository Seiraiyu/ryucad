//! The Navigator (design 519, The window): designs, the hierarchy as
//! instances place blocks, each block's sheets and regions, and
//! searchable lists of a design's components and a block's nets, built
//! from answers the daemon already gives (`design.list`, `netlist`,
//! `libcomp.list`, `block.show`, `scene`). Pure: the window asks, this
//! keeps what was answered.

use crate::core::scene::Scene;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// A component row: a designator at one place in the hierarchy.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentRow {
    pub refdes: String,
    pub value: String,
    pub mpn: String,
    /// `<block>/<component>`: what selecting it shows.
    pub reference: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetRow {
    pub name: String,
    pub reference: String,
}

/// A node of the hierarchy: a design's root, or an instance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub label: String,
    pub block: String,
    /// The instance path this node is (`s1`, `s1/adc`), none for a root.
    pub layer: Option<String>,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, Default)]
struct Block {
    instances: Vec<(String, String)>,
    nets: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Navigator {
    designs: Vec<(String, String)>,
    roots: BTreeMap<String, String>,
    occurrences: BTreeMap<String, Vec<(String, String, String, String)>>,
    /// A design's flattened occurrence (`board:s1/u1`, as ERC names
    /// them) to `<block>/<component>`.
    flat: BTreeMap<String, String>,
    mpns: BTreeMap<String, String>,
    blocks: BTreeMap<String, Block>,
    sheets: BTreeMap<String, Vec<(String, String)>>,
    /// What each sheet draws: `(block, sheet)` to its references and its
    /// labels' nets.
    drawn: BTreeMap<(String, String), BTreeSet<String>>,
    regions: BTreeMap<(String, String), Vec<String>>,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

impl Navigator {
    /// `design.list`.
    pub fn designs(&mut self, v: &Value) {
        self.designs = v["designs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                (
                    s(&d["id"]),
                    d["name"]
                        .as_str()
                        .map_or_else(|| s(&d["id"]), str::to_owned),
                )
            })
            .collect();
    }

    pub fn design_ids(&self) -> impl Iterator<Item = &str> {
        self.designs.iter().map(|(id, _)| id.as_str())
    }

    /// `netlist` of `design`: its root and every occurrence.
    pub fn netlist(&mut self, design: &str, v: &Value) {
        self.roots.insert(design.to_owned(), s(&v["root"]));
        let mut rows = Vec::new();
        for o in v["occurrences"].as_array().into_iter().flatten() {
            let local = format!("{}/{}", s(&o["block"]), s(&o["component"]));
            self.flat.insert(s(&o["reference"]), local.clone());
            rows.push((s(&o["refdes"]), s(&o["value"]), s(&o["of"]), local));
        }
        self.occurrences.insert(design.to_owned(), rows);
    }

    /// A finding's reference as the window shows it: a design's
    /// flattened occurrence (`board:s1/u1`), or one of its pins
    /// (`board:s1/u1/3`), as `<block>/<component>` (with the pin); any
    /// other as it is.
    pub fn local(&self, r: &str) -> String {
        if let Some(l) = self.flat.get(r) {
            return l.clone();
        }
        if let Some((occ, pin)) = r.rsplit_once('/')
            && let Some(l) = self.flat.get(occ)
        {
            return format!("{l}/{pin}");
        }
        r.to_owned()
    }

    /// `libcomp.list`: MPNs by library component reference.
    pub fn libcomps(&mut self, v: &Value) {
        for c in v["libcomps"].as_array().into_iter().flatten() {
            if let Some(m) = c["mpn"].as_str() {
                self.mpns.insert(s(&c["reference"]), m.to_owned());
            }
        }
    }

    /// `block.show`: a block's instances and nets.
    pub fn block(&mut self, v: &Value) {
        let pairs = |list: &Value, a: &str, b: &str| -> Vec<(String, String)> {
            list.as_array()
                .into_iter()
                .flatten()
                .map(|x| (s(&x[a]), s(&x[b])))
                .collect()
        };
        self.blocks.insert(
            s(&v["reference"]),
            Block {
                instances: pairs(&v["instances"], "id", "of"),
                nets: pairs(&v["nets"], "id", "name"),
            },
        );
    }

    /// The blocks this navigator still needs `block.show` for: each
    /// design's root and whatever its known blocks instance.
    pub fn wanted_blocks(&self) -> Vec<String> {
        let mut want: BTreeSet<String> = self.roots.values().cloned().collect();
        for b in self.blocks.values() {
            want.extend(b.instances.iter().map(|(_, of)| of.clone()));
        }
        want.into_iter()
            .filter(|b| !b.is_empty() && !self.blocks.contains_key(b))
            .collect()
    }

    /// A scene seen: its block's sheets, this sheet's regions and what it
    /// draws.
    pub fn scene(&mut self, sc: &Scene) {
        self.sheets.insert(sc.block.clone(), sc.sheets.clone());
        let key = (sc.block.clone(), sc.sheet.clone());
        let mut drawn = BTreeSet::new();
        let mut regions = Vec::new();
        for o in &sc.owners {
            drawn.insert(o.reference.clone());
            drawn.extend(o.net.clone());
            if o.kind == "region" {
                regions.push(o.reference.clone());
            }
        }
        self.drawn.insert(key.clone(), drawn);
        self.regions.insert(key, regions);
    }

    /// The design's hierarchy from its root.
    pub fn tree(&self, design: &str) -> Node {
        let name = self
            .designs
            .iter()
            .find(|(id, _)| id == design)
            .map_or(design, |(_, n)| n.as_str());
        let root = self.roots.get(design).cloned().unwrap_or_default();
        let mut node = Node {
            label: format!("{name} ({root})"),
            block: root.clone(),
            layer: None,
            children: Vec::new(),
        };
        self.grow(&mut node, "", 0);
        node
    }

    fn grow(&self, node: &mut Node, path: &str, depth: usize) {
        // A cycle is a fault the daemon reports; the tree stops at 32.
        let Some(b) = self.blocks.get(&node.block).filter(|_| depth < 32) else {
            return;
        };
        for (id, of) in &b.instances {
            let layer = if path.is_empty() {
                id.clone()
            } else {
                format!("{path}/{id}")
            };
            let mut child = Node {
                label: format!("{id} ({of})"),
                block: of.clone(),
                layer: Some(layer.clone()),
                children: Vec::new(),
            };
            self.grow(&mut child, &layer, depth + 1);
            node.children.push(child);
        }
    }

    /// A block's sheets in canvas order, once a scene of it was seen.
    pub fn sheets(&self, block: &str) -> &[(String, String)] {
        self.sheets.get(block).map_or(&[], Vec::as_slice)
    }

    pub fn regions(&self, block: &str, sheet: &str) -> &[String] {
        self.regions
            .get(&(block.to_owned(), sheet.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    /// The sheets of `block` that draw `reference` (a net through its
    /// wires or labels), in canvas order.
    pub fn reach(&self, block: &str, reference: &str) -> Vec<String> {
        self.sheets(block)
            .iter()
            .filter(|(sid, _)| {
                self.drawn
                    .get(&(block.to_owned(), sid.clone()))
                    .is_some_and(|d| d.contains(reference))
            })
            .map(|(sid, _)| sid.clone())
            .collect()
    }

    /// The design's components matching `query` in any column, ignoring
    /// case, in designator order.
    pub fn components(&self, design: &str, query: &str) -> Vec<ComponentRow> {
        let q = query.to_lowercase();
        let mut rows: Vec<ComponentRow> = self
            .occurrences
            .get(design)
            .into_iter()
            .flatten()
            .map(|(refdes, value, of, reference)| ComponentRow {
                refdes: refdes.clone(),
                value: value.clone(),
                mpn: self.mpns.get(of).cloned().unwrap_or_default(),
                reference: reference.clone(),
            })
            .filter(|r| {
                q.is_empty()
                    || [&r.refdes, &r.value, &r.mpn]
                        .iter()
                        .any(|c| c.to_lowercase().contains(&q))
            })
            .collect();
        rows.sort_by(|a, b| a.refdes.cmp(&b.refdes).then(a.reference.cmp(&b.reference)));
        rows
    }

    /// The block's nets whose names match `query`, ignoring case.
    pub fn nets(&self, block: &str, query: &str) -> Vec<NetRow> {
        let q = query.to_lowercase();
        self.blocks
            .get(block)
            .into_iter()
            .flat_map(|b| b.nets.iter())
            .filter(|(_, name)| q.is_empty() || name.to_lowercase().contains(&q))
            .map(|(id, name)| NetRow {
                name: name.clone(),
                reference: format!("{block}/{id}"),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::{Scene, tests::small};
    use serde_json::json;

    fn nav() -> Navigator {
        let mut n = Navigator::default();
        n.designs(&json!({"designs": [{"id": "d", "name": "Breakout"}], "revision": "x"}));
        n.netlist("d", &json!({"design": "d", "root": "top", "occurrences": [
            {"reference": "d:r1", "block": "top", "component": "r1", "refdes": "R1", "value": "100k", "of": "libcomp/r_100k"},
            {"reference": "d:s1/u1", "block": "sensor", "component": "u1", "refdes": "U1", "value": null, "of": "libcomp/bme280"},
        ], "nets": []}));
        n.libcomps(&json!({"library": "self", "libcomps": [
            {"id": "bme280", "reference": "libcomp/bme280", "mpn": "BME280"},
            {"id": "r_100k", "reference": "libcomp/r_100k", "mpn": null},
        ]}));
        n.block(&json!({"reference": "top", "instances": [{"id": "s1", "of": "sensor"}],
            "nets": [{"id": "sda", "name": "SDA"}, {"id": "gnd", "name": "GND"}], "components": []}));
        n.block(&json!({"reference": "sensor", "instances": [], "nets": [], "components": []}));
        n
    }

    #[test]
    fn the_hierarchy_as_instances_place_it() {
        let n = nav();
        let tree = n.tree("d");
        assert_eq!(tree.label, "Breakout (top)");
        assert_eq!(tree.block, "top");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].label, "s1 (sensor)");
        assert_eq!(tree.children[0].layer.as_deref(), Some("s1"));
    }

    #[test]
    fn components_and_nets_filter_by_any_column() {
        let n = nav();
        let all = n.components("d", "");
        assert_eq!(
            all.iter().map(|r| r.refdes.as_str()).collect::<Vec<_>>(),
            ["R1", "U1"]
        );
        assert_eq!(all[1].mpn, "BME280");
        assert_eq!(all[1].reference, "sensor/u1");
        assert_eq!(n.components("d", "bme").len(), 1);
        assert_eq!(n.components("d", "100K").len(), 1);
        // ERC's flattened names, as the window shows them.
        assert_eq!(n.local("d:s1/u1"), "sensor/u1");
        assert_eq!(n.local("d:s1/u1/3"), "sensor/u1/3");
        assert_eq!(n.local("waiver/w1"), "waiver/w1");
        let nets = n.nets("top", "s");
        assert_eq!(
            nets,
            [NetRow {
                name: "SDA".into(),
                reference: "top/sda".into()
            }]
        );
    }

    #[test]
    fn sheets_their_regions_and_where_a_net_goes() {
        let mut n = nav();
        let s1 = Scene::from_json(&small()).unwrap();
        let mut v = small();
        v["sheet"] = json!("s2");
        v["name"] = json!("IO");
        let s2 = Scene::from_json(&v).unwrap();
        n.scene(&s1);
        n.scene(&s2);
        assert_eq!(
            n.sheets("top"),
            [
                ("s1".to_owned(), "Power".to_owned()),
                ("s2".to_owned(), "IO".to_owned())
            ]
        );
        assert_eq!(n.regions("top", "s1"), ["top/region:power"]);
        // The net on s1 and, through its label, on s2.
        assert_eq!(n.reach("top", "top/sda"), ["s1", "s2"]);
        assert!(n.reach("top", "top/gnd").is_empty());
    }
}
