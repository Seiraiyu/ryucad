//! Files for tests, here and in the crates above.

use serde_json::{Value, json};

/// A 20 × 10 rectangle with a horizontal bottom, its width a
/// parameter, and a construction diagonal.
pub fn rectangle() -> Value {
    json!({
        "constraints": {
            "c1": {"a": "l1", "type": "horizontal"},
            "c2": {"a": "l1", "b": "l2", "type": "perpendicular"},
            "d1": {"a": "l1", "type": "length", "value": "width"},
            "d2": {"a": "p1", "b": "p3", "type": "distance", "value": 22.360679774997898}
        },
        "entities": {
            "l1": {"end": "p2", "start": "p1", "type": "line"},
            "l2": {"end": "p3", "start": "p2", "type": "line"},
            "l3": {"end": "p4", "start": "p3", "type": "line"},
            "l4": {"end": "p1", "start": "p4", "type": "line"},
            "l5": {"construction": true, "end": "p3", "start": "p1", "type": "line"},
            "p1": {"m": [0, 0], "type": "point"},
            "p2": {"m": [0.02, 0], "type": "point"},
            "p3": {"m": [0.02, 0.01], "type": "point"},
            "p4": {"m": [0, 0.01], "type": "point"}
        },
        "format": 2,
        "kind": "ryucad.feature",
        "plane": "xy",
        "type": "sketch"
    })
}

/// A project of parts and assemblies made from bytes, and what it says
/// in plain numbers, so a BOM read from its files can be checked against
/// a sum worked out apart from them.
pub struct Structure {
    /// The files, by path relative to the project.
    pub files: std::collections::BTreeMap<String, Vec<u8>>,
    /// Each assembly's children: part and how many it places, in list
    /// order, a part placed twice listed twice. Parts are `p0`, `p1`, …;
    /// assemblies `a0`, `a1`, …, each placing only parts and assemblies
    /// before it, so `a<last>` places no other assembly's parent.
    pub assemblies: Vec<(String, Vec<(String, u64)>)>,
    /// Assemblies bought whole.
    pub bought: Vec<String>,
}

impl Structure {
    /// How many of each leaf one `part` holds, by the generator's account:
    /// the products of counts along every path down to a part or an
    /// assembly bought whole (`part` itself when it is one).
    pub fn totals(&self, part: &str) -> std::collections::BTreeMap<String, u64> {
        let mut out = std::collections::BTreeMap::new();
        if self.bought.iter().any(|b| b == part) {
            out.insert(part.to_owned(), 1);
        } else {
            self.sum(part, 1, &mut out);
        }
        out
    }

    fn sum(&self, part: &str, times: u64, out: &mut std::collections::BTreeMap<String, u64>) {
        match self.assemblies.iter().find(|(a, _)| a == part) {
            Some((a, children)) if !self.bought.contains(a) => {
                for (c, n) in children {
                    self.sum(c, times * n, out);
                }
            }
            _ => *out.entry(part.to_owned()).or_default() += times,
        }
    }
}

/// One to five parts (some counted in metres with an amount, some
/// bought), then one to four assemblies, each placing two to five
/// children by instances and instance patterns (copies of an instance,
/// some skipped), from `bytes` (read as zeros past their end).
pub fn structure(bytes: &[u8]) -> Structure {
    use crate::doc::canonical;
    let mut at = 0;
    let mut next = |n: u8| -> u8 {
        let b = bytes.get(at).copied().unwrap_or(0);
        at += 1;
        b % n
    };
    let mut files = std::collections::BTreeMap::new();
    let mut put = |path: String, v: Value| {
        files.insert(path, canonical::to_string(&v).into_bytes());
    };
    put(
        "ryucad.json".into(),
        json!({"default_unit": "mm", "format": 1, "kind": "ryucad.project"}),
    );
    let parts = 1 + next(5) as usize;
    let mut names: Vec<String> = Vec::new();
    for i in 0..parts {
        let p = format!("p{i}");
        put(
            format!("parts/{p}/part.json"),
            json!({"features": ["b"], "format": 1, "kind": "ryucad.part"}),
        );
        put(
            format!("parts/{p}/features/b.json"),
            json!({"format": 1, "kind": "ryucad.feature", "type": "box",
                   "x": "1", "y": "1", "z": "1"}),
        );
        let mut props = json!({"format": 1, "kind": "ryupdm.properties",
                               "number": format!("{}", 100 + i)});
        match next(4) {
            0 => {
                props["uom"] = json!("m");
                props["amount"] = json!(format!("{} mm", 100 * (1 + next(9) as u64)));
                props["format"] = json!(2);
            }
            1 => props["make_buy"] = json!("buy"),
            _ => {}
        }
        put(format!("parts/{p}/properties.json"), props);
        names.push(p);
    }
    let count = 1 + next(4) as usize;
    let mut assemblies = Vec::new();
    let mut bought = Vec::new();
    for i in 0..count {
        let a = format!("a{i}");
        let mut list: Vec<String> = Vec::new();
        let mut children = Vec::new();
        let pose = json!({"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]});
        for k in 0..2 + next(4) {
            let of = names[next(names.len() as u8) as usize].clone();
            let id = format!("i{k}");
            put(
                format!("parts/{a}/features/{id}.json"),
                json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                       "part": of, "rev": "working", "pose": pose}),
            );
            list.push(id.clone());
            children.push((of.clone(), 1));
            if next(3) == 0 {
                let n = 2 + next(3) as u32;
                let skip: Vec<u32> = (1..n).filter(|_| next(4) == 0).collect();
                let pid = format!("r{k}");
                put(
                    format!("parts/{a}/features/{pid}.json"),
                    json!({"format": 4, "kind": "ryucad.feature", "type": "instance_pattern",
                           "form": "linear", "instances": [id], "direction": "x",
                           "spacing": "10 mm", "count": n, "skip": skip}),
                );
                list.push(pid);
                children.push((of, (n - 1) as u64 - skip.len() as u64));
            }
        }
        put(
            format!("parts/{a}/part.json"),
            json!({"features": list, "format": 2, "kind": "ryucad.part",
                   "part_kind": "assembly"}),
        );
        if next(4) == 0 {
            put(
                format!("parts/{a}/properties.json"),
                json!({"format": 1, "kind": "ryupdm.properties", "make_buy": "buy"}),
            );
            bought.push(a.clone());
        }
        assemblies.push((a.clone(), children));
        names.push(a);
    }
    Structure {
        files,
        assemblies,
        bought,
    }
}
