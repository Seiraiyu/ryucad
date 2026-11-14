//! #810's project, as a build script declares it (design 603, plan
//! 31.C): `parts` leaves of primitives, sketches and revolves, and one
//! assembly tree placing them `instances` times, with materials and
//! properties. [`Script::batch`] is the script's one `batch` of
//! `part.ensure`s; [`Script::singles`] the same project request by
//! request.

use serde_json::{Map, Value, json};

/// Holes in each plate.
pub const HOLES: usize = 14;
/// Modules under `top`.
pub const MODULES: usize = 4;
/// Leaves `top` places itself.
pub const AT_TOP: usize = 5;

/// One part as declared.
#[derive(Clone, Debug)]
pub struct Part {
    pub id: String,
    pub kind: &'static str,
    pub params: Option<Map<String, Value>>,
    /// Each feature as `part.ensure` takes it: a sketch with its `ops`.
    pub features: Vec<Value>,
    pub props: Map<String, Value>,
}

/// The whole project as declared.
#[derive(Clone, Debug)]
pub struct Script {
    pub materials: Vec<(String, String)>,
    pub parts: Vec<Part>,
}

pub fn leaf_name(i: usize) -> String {
    format!("p{i:02}")
}

/// A `w` × `h` rectangle's ops, its left side `l4` on the sketch's axis.
fn rectangle(w: &str, h: &str) -> Value {
    let mut ops = vec![
        json!({"op": "add", "id": "p1", "type": "point", "at": [0, 0]}),
        json!({"op": "add", "id": "p2", "type": "point", "at": [19, 1]}),
        json!({"op": "add", "id": "p3", "type": "point", "at": [21, 9]}),
        json!({"op": "add", "id": "p4", "type": "point", "at": [1, 11]}),
    ];
    for (id, a, b) in [
        ("l1", "p1", "p2"),
        ("l2", "p2", "p3"),
        ("l3", "p3", "p4"),
        ("l4", "p4", "p1"),
    ] {
        ops.push(json!({"op": "add", "id": id, "type": "line", "start": a, "end": b}));
    }
    for (ty, a) in [
        ("horizontal", "l1"),
        ("horizontal", "l3"),
        ("vertical", "l2"),
        ("vertical", "l4"),
    ] {
        ops.push(json!({"op": "add", "type": ty, "a": a}));
    }
    ops.push(json!({"op": "add", "type": "fix", "a": "p1"}));
    ops.push(
        json!({"op": "add", "id": "w", "type": "distance_x", "a": "p1", "b": "p3", "value": w}),
    );
    ops.push(
        json!({"op": "add", "id": "h", "type": "distance_y", "a": "p1", "b": "p3", "value": h}),
    );
    json!(ops)
}

const MATERIALS: [(&str, &str); 3] = [
    ("steel", "7850 kg/m^3"),
    ("aluminium", "2700 kg/m^3"),
    ("brass", "8500 kg/m^3"),
];

/// Leaf `i`: a plate less [`HOLES`] holes (`i % 4` 0 or 1), a sketch
/// extruded (2) or revolved (3), each sized by its parameter `k`.
fn leaf(i: usize) -> Part {
    let mut params = Map::new();
    params.insert("k".into(), json!(format!("{} mm", 20 + i % 7)));
    let features = match i % 4 {
        0 | 1 => {
            let mut f =
                vec![json!({"id": "base", "type": "box", "params": {"x": "k", "y": 30, "z": 6}})];
            let tools: Vec<String> = (0..HOLES).map(|h| format!("h{h:02}")).collect();
            for (h, t) in tools.iter().enumerate() {
                let (x, y) = (2 + (h % 7) * 2, 5 + (h / 7) * 20);
                f.push(
                    json!({"id": t, "type": "cylinder", "params": {"radius": 0.8, "height": 20},
                              "placement": {"origin": [x, y, -5]}}),
                );
            }
            f.push(json!({"id": "cut", "type": "boolean", "op": "subtract", "target": "base", "tools": tools}));
            f
        }
        2 => vec![
            json!({"id": "s", "type": "sketch", "ops": rectangle("k", "10 mm")}),
            json!({"id": "e", "type": "extrude", "sketch": "s", "extent": {"length": "5 mm"}}),
        ],
        _ => vec![
            json!({"id": "s", "type": "sketch", "ops": rectangle("k", "10 mm")}),
            json!({"id": "r", "type": "revolve", "sketch": "s", "axis": "l4", "angle": "full"}),
        ],
    };
    let mut props = Map::new();
    props.insert("name".into(), json!(format!("Part {i:02}")));
    props.insert("material".into(), json!(MATERIALS[i % 3].0));
    Part {
        id: leaf_name(i),
        kind: "part",
        params: Some(params),
        features,
        props,
    }
}

fn instance(id: &str, of: &str, x: i64, y: i64) -> Value {
    json!({"id": id, "type": "instance", "of": of, "placement": {"origin": [x, y, 0]}})
}

fn assembly(id: &str, name: &str, features: Vec<Value>) -> Part {
    let mut props = Map::new();
    props.insert("name".into(), json!(name));
    Part {
        id: id.into(),
        kind: "assembly",
        params: None,
        features,
        props,
    }
}

impl Script {
    /// `parts` leaves (at least [`AT_TOP`] + 1) and `instances` instances
    /// (at least [`MODULES`] + [`AT_TOP`]): `top` places the modules and
    /// the last [`AT_TOP`] leaves; the modules place the rest of the
    /// instances, round robin, of the other leaves.
    pub fn new(parts: usize, instances: usize) -> Script {
        assert!(parts > AT_TOP && instances >= MODULES + AT_TOP);
        let mut out: Vec<Part> = (0..parts).map(leaf).collect();
        let placed = parts - AT_TOP;
        let mut modules = vec![Vec::new(); MODULES];
        for k in 0..instances - MODULES - AT_TOP {
            let (x, y) = ((k % 10) as i64 * 40, (k / 10) as i64 * 40);
            modules[k % MODULES].push(instance(&format!("i{k:03}"), &leaf_name(k % placed), x, y));
        }
        let mut top = Vec::new();
        for (j, m) in modules.into_iter().enumerate() {
            out.push(assembly(&format!("m{j}"), &format!("Module {j}"), m));
            top.push(instance(
                &format!("m{j}"),
                &format!("m{j}"),
                0,
                j as i64 * 500,
            ));
        }
        for i in placed..parts {
            top.push(instance(
                &format!("q{i:02}"),
                &leaf_name(i),
                -100,
                i as i64 * 50,
            ));
        }
        out.push(assembly("top", "Machine", top));
        Script {
            materials: MATERIALS
                .iter()
                .map(|(n, d)| (n.to_string(), d.to_string()))
                .collect(),
            parts: out,
        }
    }

    /// The script's one `batch`: the materials, then each part's ensure
    /// (leaves, modules, top).
    pub fn batch(&self) -> Value {
        let mut ops: Vec<Value> = self
            .materials
            .iter()
            .map(|(n, d)| json!({"method": "materials.set", "params": {"name": n, "density": d}}))
            .collect();
        for p in &self.parts {
            let mut params =
                json!({"part": p.id, "kind": p.kind, "features": p.features, "props": p.props});
            if let Some(x) = &p.params {
                params["params"] = json!(x);
            }
            ops.push(json!({"method": "part.ensure", "params": params}));
        }
        json!({"ops": ops})
    }

    /// The same project as single requests, in order.
    pub fn singles(&self) -> Vec<(String, Value)> {
        let mut out: Vec<(String, Value)> = self
            .materials
            .iter()
            .map(|(n, d)| ("materials.set".to_owned(), json!({"name": n, "density": d})))
            .collect();
        for p in &self.parts {
            out.push(("part.create".into(), json!({"part": p.id, "kind": p.kind})));
            if let Some(x) = p.params.as_ref().filter(|x| !x.is_empty()) {
                out.push(("param.set".into(), json!({"part": p.id, "parameters": x})));
            }
            for f in &p.features {
                let mut add = f.clone();
                let ops = add.as_object_mut().unwrap().remove("ops");
                add["part"] = json!(p.id);
                out.push(("feature.add".into(), add));
                if let Some(ops) = ops {
                    out.push((
                        "sketch.edit".into(),
                        json!({"part": p.id, "sketch": f["id"], "ops": ops}),
                    ));
                }
            }
            out.push((
                "props.set".into(),
                json!({"part": p.id, "properties": p.props}),
            ));
        }
        out
    }

    pub fn part_mut(&mut self, id: &str) -> &mut Part {
        self.parts
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("no part {id}"))
    }

    /// #810's review round: one constant of `p04`, 16 nuts (`p10`) and
    /// washers (`p11`) added to `m0`, the last leaf gone from `top` and
    /// the project (with `prune`), and two materials.
    pub fn review(&mut self) {
        self.part_mut("p04")
            .params
            .as_mut()
            .unwrap()
            .insert("k".into(), json!("33 mm"));
        let m0 = self.part_mut("m0");
        for n in 0..8 {
            m0.features
                .push(instance(&format!("n{n:02}"), "p10", n * 30, 600));
            m0.features
                .push(instance(&format!("w{n:02}"), "p11", n * 30, 650));
        }
        let last = self.parts.iter().filter(|p| p.kind == "part").count() - 1;
        let gone = leaf_name(last);
        self.parts.retain(|p| p.id != gone);
        let top = self.part_mut("top");
        top.features.retain(|f| f["of"] != json!(gone));
        self.materials.push(("bronze".into(), "8800 kg/m^3".into()));
        self.materials
            .push(("titanium".into(), "4500 kg/m^3".into()));
    }
}
