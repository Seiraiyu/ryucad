//! What the window shows of the model: parts, their features in order with
//! their rebuild status, their solids, and a display mesh per solid. It is
//! built from the daemon's reports and never from the files.
//!
//! An assembly's solids are its occurrences' solids, keyed
//! `<path>/<solid>` (as entity references name them). A mesh is kept in
//! its part's frame and shared by every occurrence of its body, each
//! drawn at its own [`Pose`]: a thousand bolts are one mesh.

use crate::core::lod;
use crate::core::sketch::Drawing;
use ryucad::mesh::{Quality, WireBody};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// A feature as the last rebuild left it.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub id: String,
    /// `box`, `cylinder` and so on; `None` when the file can't be read.
    pub ty: Option<String>,
    /// Its result's hash, when it built.
    pub hash: Option<String>,
    /// What it does to solids: `join`, `cut` or `intersect`, or a
    /// boolean's `unite`, `subtract` or `intersect`; `None` for a feature
    /// that founds a solid, or makes none.
    pub operation: Option<String>,
    /// The solids it founds or acts on, by founder.
    pub solids: Vec<String>,
    /// The solids a boolean consumes.
    pub consumes: Vec<String>,
    /// A sketch's solve, when it solved.
    pub sketch: Option<SketchState>,
    /// The features it reads.
    pub uses: Vec<String>,
    /// Why it didn't build.
    pub error: Option<Value>,
    /// What an instance places, when it placed it.
    pub instance: Option<Placed>,
    /// A mate's form, connectors and state.
    pub mate: Option<MateInfo>,
    /// The copies an instance pattern places, by path (`ring:1/pin`).
    pub copies: Vec<String>,
    /// What an import reads, whether or not it built.
    pub import: Option<ImportInfo>,
}

/// An import feature's file, product and body, and what healing did when
/// it built.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportInfo {
    /// `imports/<name>.step`.
    pub file: String,
    pub blake3: String,
    /// The `PRODUCT`'s id and name, and its ordinal among definitions
    /// with both equal; none for a body no product reaches.
    pub product: Option<(String, String, u64)>,
    pub body: u64,
    /// Each heal count with a value (`sewn`, `merged`, …) and the largest
    /// tolerance given, in meters, when it built.
    pub heal: Vec<(String, u64)>,
    pub tolerance_m: Option<f64>,
}

impl ImportInfo {
    pub fn from_json(v: &Value) -> ImportInfo {
        let heal = &v["heal"];
        ImportInfo {
            file: text(&v["file"]),
            blake3: text(&v["blake3"]),
            product: v["product"].is_object().then(|| {
                (
                    text(&v["product"]["id"]),
                    text(&v["product"]["name"]),
                    v["product"]["ordinal"].as_u64().unwrap_or_default(),
                )
            }),
            body: v["body"].as_u64().unwrap_or_default(),
            heal: heal
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, n)| Some((k.clone(), n.as_u64()?)))
                .collect(),
            tolerance_m: heal["tolerance_m"].as_f64(),
        }
    }

    /// The file's name: `bracket.step`.
    pub fn file_name(&self) -> &str {
        self.file.rsplit('/').next().unwrap_or(&self.file)
    }
}

/// What an instance places: a part at a revision, whether it is fixed,
/// and what its assembly's mates leave it.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub part: String,
    /// `working`, `label B`, or `label B obsolete` when its record is.
    pub rev: String,
    pub fixed: bool,
    /// Its state in the solve (`well`, `under`, `over`, `failing`) and
    /// its degrees of freedom, when its assembly has mates.
    pub solve: Option<(String, u64)>,
    /// Where it is in its assembly.
    pub pose: Option<Pose>,
}

/// A mate: its form (`revolute` and so on), its connectors as requests
/// give them, its state in the solve when it built (`well`,
/// `redundant`), and both connectors' frames in the assembly as solved.
#[derive(Clone, Debug, PartialEq)]
pub struct MateInfo {
    pub form: String,
    pub a: String,
    pub b: String,
    pub state: Option<String>,
    pub frames: Option<[Pose; 2]>,
}

impl MateInfo {
    /// The instance path a connector is on: `base`, `ring:2/pin`. The
    /// connector is `<assembly>/<path>[/<solid>/<name>]`.
    pub fn path_of(connector: &str) -> String {
        let segments: Vec<&str> = connector.split('/').skip(1).collect();
        let entity = segments
            .last()
            .is_some_and(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()));
        let keep = if entity {
            segments.len().saturating_sub(2)
        } else {
            segments.len()
        };
        segments[..keep].join("/")
    }

    /// The body key and face name a connector on a face names:
    /// `(<path>/<solid>, <name>)`; `None` for an occurrence's own frame.
    pub fn face_of(connector: &str) -> Option<(String, String)> {
        let segments: Vec<&str> = connector.split('/').skip(1).collect();
        let (name, rest) = segments.split_last()?;
        let entity = name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit());
        (entity && rest.len() >= 2).then(|| (rest.join("/"), (*name).to_owned()))
    }
}

/// An assembly's solve, in brief: its state, its freedom, whether it
/// holds and whether its stored poses are stale.
#[derive(Clone, Debug, PartialEq)]
pub struct Solve {
    pub state: String,
    pub dof: u64,
    pub converged: bool,
    pub stale: bool,
}

/// A rigid placement: where a part's origin and axes sit in the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub z: [f64; 3],
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        origin: [0.0; 3],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        z: [0.0, 0.0, 1.0],
    };

    /// The pose with its origin at `origin`, its z along `z` and its x
    /// along `x` made perpendicular to it, as the daemon builds poses.
    pub fn of(origin: [f64; 3], z: [f64; 3], x: [f64; 3]) -> Pose {
        let unit = |a: [f64; 3]| {
            let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            [a[0] / n, a[1] / n, a[2] / n]
        };
        let z = unit(z);
        let d = x[0] * z[0] + x[1] * z[1] + x[2] * z[2];
        let x = unit([x[0] - d * z[0], x[1] - d * z[1], x[2] - d * z[2]]);
        let y = [
            z[1] * x[2] - z[2] * x[1],
            z[2] * x[0] - z[0] * x[2],
            z[0] * x[1] - z[1] * x[0],
        ];
        Pose { origin, x, y, z }
    }

    /// A direction in the part's frame, in the model's.
    pub fn turn(&self, v: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|k| self.x[k] * v[0] + self.y[k] * v[1] + self.z[k] * v[2])
    }

    /// A point in the part's frame, in the model's.
    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let t = self.turn(p);
        [0, 1, 2].map(|k| t[k] + self.origin[k])
    }

    /// The ends of its x, y and z axes drawn `length` long from its
    /// origin.
    pub fn axes(&self, length: f64) -> [[f64; 3]; 3] {
        [self.x, self.y, self.z].map(|a| [0, 1, 2].map(|k| self.origin[k] + a[k] * length))
    }

    /// A direction in the model's frame, in the part's.
    pub fn unturn(&self, v: [f64; 3]) -> [f64; 3] {
        let dot = |a: [f64; 3]| a[0] * v[0] + a[1] * v[1] + a[2] * v[2];
        [dot(self.x), dot(self.y), dot(self.z)]
    }

    /// A point in the model's frame, in the part's.
    pub fn unapply(&self, p: [f64; 3]) -> [f64; 3] {
        self.unturn([0, 1, 2].map(|k| p[k] - self.origin[k]))
    }
}

/// An occurrence of an assembly: its instance path, its part, its solids
/// by founder with their body hashes, and its pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub path: String,
    pub part: String,
    pub solids: Vec<(String, String)>,
    pub origin: [f64; 3],
    pub z: [f64; 3],
    pub x: [f64; 3],
}

/// How a sketch solved: `under`, `well` or `over`, the freedom left, and
/// whether its stored positions are stale.
#[derive(Clone, Debug, PartialEq)]
pub struct SketchState {
    pub state: String,
    pub dof: u64,
    pub stale: bool,
}

/// A solid as the model shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    /// Its founder.
    pub id: String,
    /// Its chain, founder first.
    pub features: Vec<String>,
    /// The hash of its body as shown, when anything of it built.
    pub hash: Option<String>,
    /// Why its last feature failed: it shows as the last that built.
    pub error: Option<Value>,
}

/// A part: its features in order, and its solids.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub id: String,
    pub revision: String,
    pub features: Vec<Feature>,
    /// The solids of the model, not consumed, in list order.
    pub solids: Vec<Solid>,
    /// Why the part's list can't be read.
    pub error: Option<Value>,
    /// Whether it is an assembly.
    pub assembly: bool,
    /// An assembly's occurrences.
    pub occurrences: Vec<Occurrence>,
    /// An assembly's solve, when it has mates or instance patterns.
    pub solve: Option<Solve>,
    /// Each solid's place in `solids`, by ID: an assembly of 100,000
    /// occurrences has as many solids (empty: looked for in order).
    pub by_id: HashMap<String, usize>,
    /// Each occurrence's place in `occurrences`, by path (empty: looked
    /// for in order).
    pub by_path: HashMap<String, usize>,
}

/// Three numbers from JSON.
fn triple(v: &Value) -> [f64; 3] {
    let x = |i: usize| v[i].as_f64().unwrap_or_default();
    [x(0), x(1), x(2)]
}

impl Occurrence {
    pub fn from_json(v: &Value) -> Occurrence {
        Occurrence {
            path: text(&v["path"]),
            part: text(&v["part"]),
            solids: v["solids"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| (text(&s["solid"]), text(&s["hash"])))
                .collect(),
            origin: triple(&v["pose"]["origin_m"]),
            z: triple(&v["pose"]["z"]),
            x: triple(&v["pose"]["x"]),
        }
    }

    /// Where it is.
    pub fn pose(&self) -> Pose {
        Pose::of(self.origin, self.z, self.x)
    }
}

impl Part {
    /// A part from a rebuild report (`rebuild` or `model.changed`).
    pub fn from_report(v: &Value) -> Part {
        let features = v["features"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .map(|f| Feature {
                        id: text(&f["id"]),
                        ty: f["type"].as_str().map(str::to_owned),
                        hash: f["hash"].as_str().map(str::to_owned),
                        sketch: f.get("sketch").map(|s| SketchState {
                            state: text(&s["state"]),
                            dof: s["dof"].as_u64().unwrap_or_default(),
                            stale: s["stale"] == true,
                        }),
                        uses: texts(&f["uses"]),
                        operation: f["operation"].as_str().map(str::to_owned),
                        solids: texts(&f["solids"]),
                        consumes: texts(&f["consumes"]),
                        error: (f["ok"] == false).then(|| f["error"].clone()),
                        instance: f.get("instance").map(|i| Placed {
                            part: text(&i["part"]),
                            rev: match i["rev"]["label"].as_str() {
                                Some(l) if i["pin"]["obsolete"] == true => {
                                    format!("label {l} obsolete")
                                }
                                Some(l) => format!("label {l}"),
                                None => text(&i["rev"]),
                            },
                            fixed: i["fixed"] == true,
                            solve: i.get("solve").map(|s| {
                                (text(&s["state"]), s["dof"].as_u64().unwrap_or_default())
                            }),
                            pose: i.get("pose").map(|p| {
                                Pose::of(triple(&p["origin_m"]), triple(&p["z"]), triple(&p["x"]))
                            }),
                        }),
                        mate: f.get("mate").map(|m| MateInfo {
                            form: text(&m["form"]),
                            a: text(&m["a"]),
                            b: text(&m["b"]),
                            state: m["state"].as_str().map(str::to_owned),
                            frames: m.get("frames").map(|f| {
                                let pose = |p: &Value| {
                                    Pose::of(
                                        triple(&p["origin_m"]),
                                        triple(&p["z"]),
                                        triple(&p["x"]),
                                    )
                                };
                                [pose(&f["a"]), pose(&f["b"])]
                            }),
                        }),
                        copies: f["copies"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|c| format!("{}/{}", text(&c["copy"]), text(&c["seed"])))
                            .collect(),
                        import: f.get("import").map(ImportInfo::from_json),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let solids: Vec<Solid> = v["solids"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| Solid {
                id: text(&s["solid"]),
                features: texts(&s["features"]),
                hash: s["hash"].as_str().map(str::to_owned),
                error: (s["ok"] == false).then(|| s["error"].clone()),
            })
            .collect();
        let assembly = v["kind"] == "assembly";
        let occurrences: Vec<Occurrence> = v["occurrences"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Occurrence::from_json)
            .collect();
        // An assembly's solids are its occurrences': each under the
        // instance that places it, by its body's hash.
        let solids = match assembly {
            false => solids,
            true => occurrences
                .iter()
                .flat_map(|o| {
                    // A copy is under the pattern that places it.
                    let top = o
                        .path
                        .split(['/', ':'])
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    o.solids.iter().map(move |(s, h)| Solid {
                        id: format!("{}/{s}", o.path),
                        features: vec![top.clone()],
                        hash: Some(h.clone()),
                        error: None,
                    })
                })
                .collect(),
        };
        let by_id = solids
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();
        let by_path = occurrences
            .iter()
            .enumerate()
            .map(|(i, o)| (o.path.clone(), i))
            .collect();
        Part {
            id: text(&v["part"]),
            revision: text(&v["revision"]),
            features,
            solids,
            by_id,
            by_path,
            error: v.get("error").cloned(),
            assembly,
            occurrences,
            solve: v.get("solve").map(|s| Solve {
                state: text(&s["state"]),
                dof: s["dof"].as_u64().unwrap_or_default(),
                converged: s["converged"] == true,
                stale: s["stale"] == true,
            }),
        }
    }

    /// The occurrence a solid key (`<path>/<solid>`) of an assembly names,
    /// with the solid.
    pub fn occurrence(&self, key: &str) -> Option<(&Occurrence, &str)> {
        let (path, solid) = key.rsplit_once('/')?;
        let o = match self.by_path.is_empty() {
            true => self.occurrences.iter().find(|o| o.path == path)?,
            false => &self.occurrences[*self.by_path.get(path)?],
        };
        let (_, hash) = o.solids.iter().find(|(s, _)| s == solid)?;
        Some((o, hash))
    }

    pub fn feature(&self, id: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    /// The solid founded by `id`, if the model shows it.
    pub fn solid(&self, id: &str) -> Option<&Solid> {
        match self.by_id.is_empty() {
            true => self.solids.iter().find(|s| s.id == id),
            false => self.by_id.get(id).map(|i| &self.solids[*i]),
        }
    }

    /// The features that found solids, in list order: what a feature can
    /// act on, consumed ones included (the daemon says which can't).
    pub fn founders(&self) -> Vec<String> {
        self.features
            .iter()
            .filter(|f| f.operation.is_none() && f.solids == [f.id.clone()])
            .map(|f| f.id.clone())
            .collect()
    }

    /// The solids the model shows that `feature` founds or acts on.
    pub fn solids_of(&self, feature: &str) -> Vec<&Solid> {
        self.solids
            .iter()
            .filter(|s| s.features.iter().any(|f| f == feature))
            .collect()
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().map(text).collect()
}

/// A body's place: its part and the feature that names it: a solid's
/// founder, or a sketch.
pub type BodyKey = (String, String);

/// A part as `part.list` lists it: what the tree says of a part whose
/// report the window hasn't loaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    pub assembly: bool,
    pub features: usize,
    pub revision: String,
    /// How many of its features failed, when the daemon has built it.
    pub failed: Option<u64>,
    /// Why its list can't be read.
    pub error: Option<Value>,
}

impl Listed {
    pub fn from_json(v: &Value) -> Listed {
        Listed {
            assembly: v["kind"] == "assembly",
            features: v["features"].as_u64().unwrap_or(0) as usize,
            revision: v["revision"].as_str().unwrap_or_default().to_owned(),
            failed: v["failed"].as_u64(),
            error: v.get("error").cloned(),
        }
    }
}

/// The model as the window knows it.
#[derive(Debug, Default)]
pub struct Model {
    pub default_unit: String,
    /// The parts whose reports the window has loaded.
    pub parts: BTreeMap<String, Part>,
    /// Every part the daemon lists, loaded or not.
    pub listed: BTreeMap<String, Listed>,
    /// The keys that show each body, by its hash: where a mesh fetched
    /// once goes, for every occurrence of it.
    showing: HashMap<String, BTreeSet<BodyKey>>,
    /// What the window shows when it isn't told, as the daemon has it:
    /// the project's one root assembly, else every part that isn't one.
    pub model_parts: Vec<String>,
    /// Display meshes by part and solid, each for the body hash it says,
    /// in its part's frame: occurrences of one body share one. Each is at
    /// the level it was fetched at ([`Model::fetch_level`]).
    pub meshes: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Meshes asked for and not yet answered, by the body hash they were
    /// asked for at: a burst of changes asks for each body once.
    pub asked: BTreeMap<BodyKey, String>,
    /// The user's display quality: the level `meshes` are fetched at,
    /// but in a large view.
    pub base: Quality,
    /// Bodies' meshes at the other levels of detail, by body hash and
    /// level (see [`crate::core::lod`]).
    pub levels: BTreeMap<(String, Quality), Arc<WireBody>>,
    /// Levels asked for and not yet answered.
    pub levels_asked: std::collections::BTreeSet<(String, Quality)>,
    /// How sketches are drawn, by part and feature, each for the part
    /// revision it says.
    pub sketches: BTreeMap<BodyKey, Arc<Drawing>>,
    /// Sketches asked for and not yet answered, by the part revision they
    /// were asked for at.
    pub sketches_asked: BTreeMap<BodyKey, String>,
    /// The part or assembly the window shows, as its participant's focus
    /// says; `None` for the project's model.
    pub focus: Option<String>,
    /// Instances being dragged, by assembly and instance: where the last
    /// `assembly.solve` put them, drawn there until the move is written
    /// and the assembly's next report says where they are.
    pub preview: BTreeMap<(String, String), Pose>,
    /// What the window hides, as its participant's hidden set says:
    /// occurrences (`<asm>/<path>`) and solids (`<part>/<solid>`).
    pub hidden: std::collections::BTreeSet<String>,
    /// Where the explode drawn moves an assembly's occurrences: its
    /// assembly, and each moved occurrence's offset by path, applied
    /// after its pose ([`Model::pose`]).
    pub exploded: Option<(String, BTreeMap<String, ryucad::explode::Offset>)>,
    /// How each drawn body's faces look, by key, as the daemon says with
    /// what the Appearance window drags laid over it; a body with none is
    /// drawn plain.
    pub looks: Arc<BTreeMap<BodyKey, Arc<crate::core::appearance::Look>>>,
    /// Bumped whenever anything here changes, so views know to redraw.
    pub generation: u64,
    /// [`Model::held_bytes`], and the generation and number of levels it
    /// was worked out at.
    held: std::sync::Mutex<Option<((u64, usize), u64)>>,
}

impl Model {
    /// Takes a part's report. Returns the bodies whose meshes are now
    /// missing or stale: they need fetching ([`Model::sketches_wanted`]
    /// says the same of sketches).
    pub fn apply_part(&mut self, report: &Value) -> Vec<BodyKey> {
        let part = Part::from_report(report);
        let id = part.id.clone();
        // The report says where everything is now.
        self.preview.retain(|(a, _), _| *a != id);
        // Meshes of solids that are gone, or changed, go, and other levels
        // of bodies nothing shows any more.
        self.meshes.retain(|(p, s), m| {
            *p != id
                || part
                    .solid(s)
                    .is_some_and(|s| s.hash.as_deref() == Some(&m.hash))
        });
        let kept: std::collections::BTreeSet<&String> =
            self.meshes.values().map(|m| &m.hash).collect();
        self.levels.retain(|(h, _), _| kept.contains(h));
        // Drawings of sketches that are gone, or failed, go; the others
        // stay drawn until their new drawing comes.
        self.sketches
            .retain(|(p, f), _| *p != id || part.feature(f).is_some_and(|f| f.sketch.is_some()));
        let wanted = part
            .solids
            .iter()
            .filter(|s| s.hash.is_some())
            .map(|s| (id.clone(), s.id.clone()))
            .filter(|k| !self.meshes.contains_key(k))
            .collect();
        self.unshow(&id);
        for s in &part.solids {
            if let Some(h) = &s.hash {
                let key = (id.clone(), s.id.clone());
                self.showing.entry(h.clone()).or_default().insert(key);
            }
        }
        self.parts.insert(id, part);
        self.generation += 1;
        wanted
    }

    /// Where `feature` of `part` comes from, for people: its type
    /// (`box`); for an assembly's solid (`<path>/<solid>`), the part its
    /// occurrence places and its founder's type there (`plate, box`).
    /// Empty when the window doesn't know.
    pub fn source(&self, part: &str, feature: &str) -> String {
        let Some(p) = self.parts.get(part) else {
            return String::new();
        };
        let ty = |p: &Part, f: &str| p.feature(f).and_then(|f| f.ty.clone());
        if !p.assembly {
            return ty(p, feature).unwrap_or_default();
        }
        let Some((o, _)) = p.occurrence(feature) else {
            return String::new();
        };
        let solid = feature.rsplit_once('/').map_or(feature, |(_, s)| s);
        match self.parts.get(&o.part).and_then(|q| ty(q, solid)) {
            Some(t) => format!("{}, {t}", o.part),
            None => o.part.clone(),
        }
    }

    /// Forgets which bodies `part`'s keys showed.
    fn unshow(&mut self, part: &str) {
        let Some(p) = self.parts.get(part) else {
            return;
        };
        for s in &p.solids {
            if let Some(h) = &s.hash
                && let Some(keys) = self.showing.get_mut(h)
            {
                keys.remove(&(p.id.clone(), s.id.clone()));
                if keys.is_empty() {
                    self.showing.remove(h);
                }
            }
        }
    }

    /// The body hash `key` shows now.
    fn hash_of(&self, key: &BodyKey) -> Option<String> {
        self.parts
            .get(&key.0)
            .and_then(|p| p.solid(&key.1))
            .and_then(|s| s.hash.clone())
    }

    /// The bodies of `keys` to fetch, with their hashes, now noted as
    /// asked for: each body once, however many keys show it. A key whose
    /// body is already here (another occurrence of it) takes that mesh,
    /// and one whose body is asked for under another key waits for it.
    pub fn ask(&mut self, keys: Vec<BodyKey>) -> Vec<(BodyKey, String)> {
        // Indexed once per call: an assembly asks for 100,000 keys at once.
        let mut here: HashMap<String, Arc<WireBody>> = self
            .meshes
            .values()
            .map(|m| (m.hash.clone(), Arc::clone(m)))
            .collect();
        let mut asked: std::collections::HashSet<String> = self.asked.values().cloned().collect();
        let mut out = Vec::new();
        for key in keys {
            let Some(hash) = self.hash_of(&key) else {
                continue;
            };
            if let Some(m) = here.get(&hash).cloned() {
                self.meshes.insert(key, m);
                self.generation += 1;
                continue;
            }
            if asked.insert(hash.clone()) {
                self.asked.insert(key.clone(), hash.clone());
                out.push((key, hash));
            }
        }
        here.clear();
        out
    }

    /// An answer to asking for `key` at `hash` came, or will never come.
    /// Whatever it brought, later reports ask again if they need to.
    pub fn answered(&mut self, key: &BodyKey, hash: &str) {
        if self.asked.get(key).is_some_and(|h| h == hash) {
            self.asked.remove(key);
        }
    }

    /// The sketches of `part` whose drawings are missing or older than
    /// the part, not already asked for at its revision, with that
    /// revision, now noted as asked for.
    pub fn ask_sketches(&mut self, part: &str) -> Vec<(BodyKey, String)> {
        let Some(p) = self.parts.get(part) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for f in p.features.iter().filter(|f| f.sketch.is_some()) {
            let key = (p.id.clone(), f.id.clone());
            let drawn = self
                .sketches
                .get(&key)
                .is_some_and(|d| d.revision == p.revision);
            if !drawn && self.sketches_asked.get(&key) != Some(&p.revision) {
                self.sketches_asked.insert(key.clone(), p.revision.clone());
                out.push((key, p.revision.clone()));
            }
        }
        out
    }

    /// An answer to asking for sketch `key` at `revision` came, or never
    /// will.
    pub fn sketch_answered(&mut self, key: &BodyKey, revision: &str) {
        if self.sketches_asked.get(key).is_some_and(|r| r == revision) {
            self.sketches_asked.remove(key);
        }
    }

    /// Keeps a sketch's drawing if it is still a sketch that solved, and
    /// the drawing is of the part as it is now, or the only one there is.
    pub fn take_sketch(&mut self, key: &BodyKey, d: Drawing) -> bool {
        let Some(p) = self.parts.get(&key.0) else {
            return false;
        };
        let solved = p.feature(&key.1).is_some_and(|f| f.sketch.is_some());
        let current = d.revision == p.revision || !self.sketches.contains_key(key);
        if !solved || !current {
            return false;
        }
        self.sketches.insert(key.clone(), Arc::new(d));
        self.generation += 1;
        true
    }

    /// Drops every drawing: the display quality or the default unit
    /// changed. Returns the parts to ask again.
    pub fn clear_sketches(&mut self) -> Vec<String> {
        self.sketches.clear();
        self.sketches_asked.clear();
        self.generation += 1;
        self.parts.keys().cloned().collect()
    }

    /// Forgets a part, its meshes and its sketches.
    pub fn remove_part(&mut self, id: &str) {
        self.unshow(id);
        self.parts.remove(id);
        self.listed.remove(id);
        self.meshes.retain(|(p, _), _| p != id);
        self.asked.retain(|(p, _), _| p != id);
        self.sketches.retain(|(p, _), _| p != id);
        self.sketches_asked.retain(|(p, _), _| p != id);
        self.generation += 1;
    }

    /// Keeps a fetched mesh if its body is still the current one, for
    /// every key that shows that body: the occurrences of it.
    pub fn take_mesh(&mut self, part: &str, body: WireBody) -> bool {
        let current = self
            .parts
            .get(part)
            .and_then(|p| p.solid(&body.feature))
            .and_then(|s| s.hash.as_deref());
        if current != Some(&body.hash) {
            return false;
        }
        let body = Arc::new(body);
        let showing: Vec<BodyKey> = self
            .showing
            .get(&body.hash)
            .map(|k| k.iter().cloned().collect())
            .unwrap_or_default();
        for key in showing {
            self.meshes.insert(key, Arc::clone(&body));
        }
        self.generation += 1;
        true
    }

    /// `key`'s mesh at `level`, if the window has it.
    pub fn mesh_at(&self, key: &BodyKey, level: Quality) -> Option<Arc<WireBody>> {
        let first = self.meshes.get(key)?;
        if lod::level_of(first, self.base) == level {
            return Some(Arc::clone(first));
        }
        self.levels.get(&(first.hash.clone(), level)).cloned()
    }

    /// What the GPU would hold of every mesh the window has, each body
    /// and level once ([`lod::gpu_bytes`]): what is drawn holds no more.
    /// Worked out once per generation and level taken.
    pub fn held_bytes(&self) -> u64 {
        let at = (self.generation, self.levels.len());
        let mut held = self.held.lock().expect("never poisoned");
        if let Some((g, b)) = *held
            && g == at
        {
            return b;
        }
        let mut seen = std::collections::HashSet::new();
        let b = (self.meshes.values().chain(self.levels.values()))
            .filter(|m| seen.insert(Arc::as_ptr(m)))
            .map(|m| lod::gpu_bytes(m))
            .sum();
        *held = Some((at, b));
        b
    }

    /// The level bodies' first meshes are fetched at: coarse when the
    /// parts shown draw more than [`lod::COARSE_FIRST`] occurrences of
    /// solids, else the user's display quality.
    pub fn fetch_level(&self) -> Quality {
        let drawn: usize = self
            .shown()
            .iter()
            .filter_map(|p| self.parts.get(*p))
            .map(|p| p.solids.len())
            .sum();
        if drawn > lod::COARSE_FIRST {
            Quality::Coarse
        } else {
            self.base
        }
    }

    /// The mesh picking and painting index faces in for a body whose
    /// first mesh is `first`: at the user's display quality once the
    /// window has that, else `first`.
    fn indexed<'a>(&'a self, first: &'a Arc<WireBody>) -> &'a Arc<WireBody> {
        if lod::level_of(first, self.base) == self.base {
            return first;
        }
        self.levels
            .get(&(first.hash.clone(), self.base))
            .unwrap_or(first)
    }

    /// The levels of `wanted` to fetch, each body's once, now noted as
    /// asked for, in the order given.
    pub fn ask_levels(
        &mut self,
        wanted: Vec<(BodyKey, Quality)>,
    ) -> Vec<(BodyKey, String, Quality)> {
        let mut out = Vec::new();
        for (key, level) in wanted {
            let Some(first) = self.meshes.get(&key) else {
                continue;
            };
            let (hash, fetched) = (first.hash.clone(), lod::level_of(first, self.base));
            let at = (hash.clone(), level);
            if level == fetched || self.levels.contains_key(&at) || self.levels_asked.contains(&at)
            {
                continue;
            }
            self.levels_asked.insert(at);
            out.push((key, hash, level));
        }
        out
    }

    /// A fetched level came, or never will: keeps it if its body is one
    /// the window still has.
    pub fn take_level(&mut self, body: WireBody, level: Quality) -> bool {
        let at = (body.hash.clone(), level);
        self.levels_asked.remove(&at);
        if !self.meshes.values().any(|m| m.hash == body.hash) || body.mesh.is_err() {
            return false;
        }
        // Only the user's display quality changes what picking reads
        // ([`Model::drawn`]): another level needs no new picker, which at
        // 100,000 occurrences takes a tenth of a second.
        if level == self.base {
            self.generation += 1;
        }
        self.levels.insert(at, Arc::new(body));
        true
    }

    /// Where `key`'s body is drawn: its occurrence's pose in an assembly
    /// (moved with its instance while it is dragged), else where its part
    /// puts it.
    pub fn pose(&self, key: &BodyKey) -> Pose {
        let Some(p) = self.parts.get(&key.0).filter(|p| p.assembly) else {
            return Pose::IDENTITY;
        };
        let Some((o, _)) = p.occurrence(&key.1) else {
            return Pose::IDENTITY;
        };
        let at = o.pose();
        let instance = o.path.split('/').next().unwrap_or_default();
        let dragged = self.preview.get(&(p.id.clone(), instance.to_owned()));
        let was = p
            .feature(instance)
            .and_then(|f| f.instance.as_ref())
            .and_then(|i| i.pose);
        let at = match (dragged, was) {
            // Where the occurrence is relative to its instance stays.
            (Some(now), Some(was)) => Pose {
                origin: now.apply(was.unapply(at.origin)),
                x: now.turn(was.unturn(at.x)),
                y: now.turn(was.unturn(at.y)),
                z: now.turn(was.unturn(at.z)),
            },
            _ => at,
        };
        // An explode moves it after its pose, for display only.
        match self
            .exploded
            .as_ref()
            .filter(|(asm, _)| *asm == p.id)
            .and_then(|(_, offsets)| offsets.get(&o.path))
        {
            Some(off) => Pose {
                origin: off.apply(at.origin),
                x: off.turn(at.x),
                y: off.turn(at.y),
                z: off.turn(at.z),
            },
            None => at,
        }
    }

    /// The assembly and occurrence path an entity reference of an
    /// assembly's occurrence names (`<asm>/<path>/<solid>/<name>`, or
    /// `<asm>/<path>` itself): the longest of its occurrences' paths it
    /// starts with.
    pub fn occurrence_path(&self, reference: &str) -> Option<(String, String)> {
        let (asm, rest) = reference.split_once('/')?;
        let p = self.parts.get(asm).filter(|p| p.assembly)?;
        p.occurrences
            .iter()
            .map(|o| o.path.as_str())
            .filter(|path| rest == *path || rest.starts_with(&format!("{path}/")))
            .max_by_key(|path| path.len())
            .map(|path| (asm.to_owned(), path.to_owned()))
    }

    /// Sets where the explode drawn moves `part`'s occurrences (`None`
    /// for no explode); returns whether anything moved.
    pub fn explode(
        &mut self,
        moved: Option<(String, BTreeMap<String, ryucad::explode::Offset>)>,
    ) -> bool {
        let moved = moved.filter(|(_, m)| !m.is_empty());
        if self.exploded == moved {
            return false;
        }
        self.exploded = moved;
        self.generation += 1;
        true
    }

    /// The middle of each occurrence's box of `part` (an assembly), where
    /// its assembly puts it, by path: where explode trails start.
    pub fn middles(&self, part: &str) -> BTreeMap<String, [f64; 3]> {
        let mut boxes: BTreeMap<String, ([f64; 3], [f64; 3])> = BTreeMap::new();
        let Some(p) = self.parts.get(part).filter(|p| p.assembly) else {
            return BTreeMap::new();
        };
        for (key, mesh) in self.meshes.iter().filter(|(k, _)| k.0 == part) {
            let Some((path, _)) = key.1.rsplit_once('/') else {
                continue;
            };
            let Some((o, _)) = p.occurrence(&key.1) else {
                continue;
            };
            let Ok(m) = &mesh.mesh else { continue };
            let at = o.pose();
            for q in &m.positions {
                let w = at.apply([0, 1, 2].map(|k| m.origin[k] + f64::from(q[k])));
                let b = boxes.entry(path.to_owned()).or_insert((w, w));
                for (k, x) in w.iter().enumerate() {
                    b.0[k] = b.0[k].min(*x);
                    b.1[k] = b.1[k].max(*x);
                }
            }
        }
        boxes
            .into_iter()
            .map(|(p, (lo, hi))| (p, [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0)))
            .collect()
    }

    /// The assembly and instance a drag on `key`'s body moves, or why it
    /// can't: a part isn't placed, a pattern's copy moves with its seed,
    /// a fixed instance and one its mates hold all round don't move.
    pub fn movable(&self, key: &BodyKey) -> Result<(String, String), String> {
        let p = self
            .parts
            .get(&key.0)
            .filter(|p| p.assembly)
            .ok_or_else(|| format!("{} is a part: only an assembly's instances move", key.0))?;
        let instance = key.1.split('/').next().unwrap_or_default();
        if instance.contains(':') {
            return Err(format!(
                "{instance} is a pattern's copy: it moves with its seed"
            ));
        }
        let placed = p
            .feature(instance)
            .and_then(|f| f.instance.as_ref())
            .ok_or_else(|| format!("{instance} isn't an instance of {}", p.id))?;
        if placed.fixed {
            return Err(format!("{instance} is fixed"));
        }
        if placed.solve.as_ref().is_some_and(|(_, dof)| *dof == 0) {
            return Err(format!("{instance} has no freedom left: its mates hold it"));
        }
        Ok((p.id.clone(), instance.to_owned()))
    }

    /// Drops every mesh: the display quality changed.
    pub fn clear_meshes(&mut self) -> Vec<BodyKey> {
        self.meshes.clear();
        self.asked.clear();
        self.levels.clear();
        self.levels_asked.clear();
        self.generation += 1;
        self.parts
            .values()
            .flat_map(|p| {
                p.solids
                    .iter()
                    .filter(|s| s.hash.is_some())
                    .map(|s| (p.id.clone(), s.id.clone()))
            })
            .collect()
    }

    /// The parts the window shows: its focus, else the project's one root
    /// assembly (one nothing places), else every part that isn't an
    /// assembly, as the daemon has it.
    pub fn shown(&self) -> Vec<&str> {
        if let Some(f) = &self.focus
            && self.parts.contains_key(f)
        {
            return vec![f.as_str()];
        }
        if !self.model_parts.is_empty() {
            return self.model_parts.iter().map(String::as_str).collect();
        }
        let placed: std::collections::BTreeSet<&str> = self
            .parts
            .values()
            .flat_map(|p| {
                p.features
                    .iter()
                    .filter_map(|f| Some(f.instance.as_ref()?.part.as_str()))
            })
            .collect();
        let roots: Vec<&str> = self
            .parts
            .values()
            .filter(|p| p.assembly && !placed.contains(p.id.as_str()))
            .map(|p| p.id.as_str())
            .collect();
        match roots.as_slice() {
            [one] => vec![one],
            _ => self
                .parts
                .values()
                .filter(|p| !p.assembly)
                .map(|p| p.id.as_str())
                .collect(),
        }
    }

    /// The drawings of the sketches the window shows: those of the parts
    /// it shows. An assembly holds none, so its view shows none, as a
    /// render of it doesn't (issue #95).
    pub fn drawn_sketches(&self) -> impl Iterator<Item = (&BodyKey, &Arc<Drawing>)> {
        let shown = self.shown();
        self.sketches
            .iter()
            .filter(move |(k, _)| shown.contains(&k.0.as_str()))
    }

    /// What hiding `key`'s body hides, as `visibility.set` names it: its
    /// occurrence in an assembly (`<asm>/<path>`), else its solid
    /// (`<part>/<solid>`).
    pub fn leaf(&self, key: &BodyKey) -> String {
        let assembly = self.parts.get(&key.0).is_some_and(|p| p.assembly);
        match key.1.rsplit_once('/') {
            Some((path, _)) if assembly => format!("{}/{path}", key.0),
            _ => format!("{}/{}", key.0, key.1),
        }
    }

    /// Whether everything `reference` reaches is hidden: an assembly's
    /// occurrences under an instance path (`<asm>/<path>`), or a part's
    /// solid (`<part>/<solid>`). `false` when it reaches nothing.
    pub fn all_hidden(&self, reference: &str) -> bool {
        let Some((part, rest)) = reference.split_once('/') else {
            return false;
        };
        let Some(p) = self.parts.get(part) else {
            return false;
        };
        if !p.assembly {
            return self.hidden.contains(reference);
        }
        let mut leaves = p
            .occurrences
            .iter()
            .filter(|o| o.path == rest || o.path.starts_with(&format!("{rest}/")))
            .map(|o| format!("{part}/{}", o.path))
            .peekable();
        leaves.peek().is_some() && leaves.all(|l| self.hidden.contains(&l))
    }

    /// Whether `key`'s body is hidden.
    pub fn is_hidden(&self, key: &BodyKey) -> bool {
        !self.hidden.is_empty() && self.hidden.contains(&self.leaf(key))
    }

    /// The meshes the window draws, each where it is: those of the parts
    /// it shows, less what it hides.
    pub fn drawn(&self) -> impl Iterator<Item = (&BodyKey, &Arc<WireBody>, Pose)> {
        self.shown_meshes(false)
    }

    /// The meshes of the parts the window shows that it hides: what
    /// Shift+Tab may show again.
    pub fn hidden_drawn(&self) -> impl Iterator<Item = (&BodyKey, &Arc<WireBody>, Pose)> {
        self.shown_meshes(true)
    }

    fn shown_meshes(&self, hidden: bool) -> impl Iterator<Item = (&BodyKey, &Arc<WireBody>, Pose)> {
        let shown = self.shown();
        self.meshes
            .iter()
            .filter(move |(k, _)| shown.contains(&k.0.as_str()) && self.is_hidden(k) == hidden)
            .map(|(k, m)| (k, self.indexed(m), self.pose(k)))
    }

    /// The bodies choosing `key` frames: in an assembly, every solid of
    /// every occurrence under the instance (or pattern) it names, at any
    /// depth (issue #107); else the solid it shows in, or itself (a
    /// sketch).
    pub fn framed(&self, key: &BodyKey) -> Vec<BodyKey> {
        if let Some(p) = self.parts.get(&key.0).filter(|p| p.assembly) {
            let (path, copy) = (format!("{}/", key.1), format!("{}:", key.1));
            let under: Vec<BodyKey> = p
                .solids
                .iter()
                .filter(|s| s.id.starts_with(&path) || s.id.starts_with(&copy))
                .map(|s| (key.0.clone(), s.id.clone()))
                .collect();
            if !under.is_empty() {
                return under;
            }
        }
        vec![self.solid_of(key).unwrap_or_else(|| key.clone())]
    }

    /// The bodies of the occurrences an occurrence reference names
    /// (`<asm>/<path>`): every solid of every occurrence at that path or
    /// under it. Nothing for any other reference, so a selection holding
    /// one paints and frames what it places (a BOM line's occurrences).
    pub fn occurrence_bodies(&self, reference: &str) -> Vec<BodyKey> {
        let Some((asm, path)) = reference.split_once('/') else {
            return Vec::new();
        };
        let Some(p) = self.parts.get(asm).filter(|p| p.assembly) else {
            return Vec::new();
        };
        let reaches = |o: &Occurrence| o.path == path || o.path.starts_with(&format!("{path}/"));
        p.solids
            .iter()
            .filter(|s| {
                s.id.rsplit_once('/')
                    .is_some_and(|(at, _)| p.occurrences.iter().any(|o| o.path == at && reaches(o)))
            })
            .map(|s| (asm.to_owned(), s.id.clone()))
            .collect()
    }

    /// The solid a feature shows in: its own, or the first the model
    /// shows of those it acts on.
    pub fn solid_of(&self, key: &BodyKey) -> Option<BodyKey> {
        let p = self.parts.get(&key.0)?;
        let s = p.solids_of(&key.1).into_iter().next()?;
        Some((key.0.clone(), s.id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A part of solids, each founded by one feature with its hash, or
    /// failed.
    fn report(hashes: &[(&str, Option<&str>)]) -> Value {
        let features: Vec<Value> = hashes
            .iter()
            .map(|(id, h)| match h {
                Some(h) => json!({"id": id, "type": "box", "ok": true, "hash": h, "solids": [id]}),
                None => json!({"id": id, "type": "torus", "ok": false, "solids": [id],
                               "error": {"code": "rebuild_failed"}}),
            })
            .collect();
        let solids: Vec<Value> = hashes
            .iter()
            .map(|(id, h)| match h {
                Some(h) => json!({"solid": id, "ok": true, "features": [id], "hash": h}),
                None => json!({"solid": id, "ok": false, "features": [id],
                               "error": {"code": "rebuild_failed"}}),
            })
            .collect();
        json!({"part": "p", "revision": "r", "features": features, "solids": solids})
    }

    fn body(feature: &str, hash: &str) -> WireBody {
        WireBody {
            feature: feature.into(),
            hash: hash.into(),
            mesh: Err(json!(null)),
        }
    }

    #[test]
    fn meshes_follow_the_bodies() {
        let mut m = Model::default();
        let want = m.apply_part(&report(&[("a", Some("1")), ("b", Some("2"))]));
        assert_eq!(want.len(), 2);
        assert!(m.take_mesh("p", body("a", "1")));
        assert!(m.take_mesh("p", body("b", "2")));
        // A stale mesh is refused.
        assert!(!m.take_mesh("p", body("a", "0")));
        // Only the changed body is wanted again; the failed one loses its mesh.
        let want = m.apply_part(&report(&[("a", Some("1")), ("b", None), ("c", Some("3"))]));
        assert_eq!(want, [("p".to_owned(), "c".to_owned())]);
        assert_eq!(m.meshes.len(), 1);
        assert_eq!(
            m.parts["p"].features[1].error,
            Some(json!({"code": "rebuild_failed"}))
        );
        assert_eq!(m.parts["p"].features[0].ty.as_deref(), Some("box"));
        assert_eq!(m.parts["p"].features[1].ty.as_deref(), Some("torus"));
        let g = m.generation;
        m.remove_part("p");
        assert!(m.meshes.is_empty() && m.parts.is_empty() && m.generation > g);
    }

    fn sketched(revision: &str, ok: bool) -> Value {
        let s = if ok {
            json!({"id": "s", "type": "sketch", "ok": true,
                   "sketch": {"state": "under", "dof": 3, "stale": false}})
        } else {
            json!({"id": "s", "type": "sketch", "ok": false, "error": {"code": "sketch_unsolved"}})
        };
        json!({"part": "p", "revision": revision, "features": [s,
               {"id": "e", "type": "extrude", "ok": true, "hash": "1", "uses": ["s"]}]})
    }

    fn drawing(revision: &str) -> Drawing {
        Drawing {
            revision: revision.into(),
            ..Drawing::default()
        }
    }

    #[test]
    fn sketches_follow_the_part_revision() {
        let mut m = Model::default();
        m.apply_part(&sketched("r1", true));
        let p = &m.parts["p"];
        assert_eq!(p.features[1].uses, ["s"]);
        assert_eq!(p.features[0].sketch.as_ref().unwrap().dof, 3);
        let key = ("p".to_owned(), "s".to_owned());
        assert_eq!(m.ask_sketches("p"), [(key.clone(), "r1".to_owned())]);
        // Asked once per revision.
        assert!(m.ask_sketches("p").is_empty());
        assert!(m.take_sketch(&key, drawing("r1")));
        m.sketch_answered(&key, "r1");
        assert!(m.ask_sketches("p").is_empty());
        // A new revision asks again, and the old drawing stays until the
        // new one comes; an answer from before the drawing kept is refused.
        m.apply_part(&sketched("r2", true));
        assert_eq!(m.ask_sketches("p").len(), 1);
        assert!(m.sketches.contains_key(&key));
        assert!(m.take_sketch(&key, drawing("r2")));
        assert!(!m.take_sketch(&key, drawing("r1")));
        // A sketch that fails is no longer drawn.
        m.apply_part(&sketched("r3", false));
        assert!(m.sketches.is_empty());
        assert!(m.ask_sketches("p").is_empty());
        assert!(!m.take_sketch(&key, drawing("r3")));
        m.apply_part(&sketched("r4", true));
        assert_eq!(m.clear_sketches(), ["p"]);
        assert_eq!(m.ask_sketches("p").len(), 1);
    }

    #[test]
    fn choosing_an_instance_frames_everything_it_places() {
        let occurrence = |path: &str| {
            json!({"path": path, "part": "bolt", "solids": [{"solid": "shank", "hash": "h"}],
                   "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
        };
        let mut m = Model::default();
        m.apply_part(&json!({"part": "top", "revision": "r", "kind": "assembly",
            "features": [{"id": "s1", "type": "instance", "ok": true, "hash": "x",
                          "instance": {"part": "sub", "rev": "working"}}],
            "occurrences": [occurrence("s1/b1"), occurrence("s1/b2"), occurrence("s10/b1"),
                            occurrence("row:1/s1/b1")]}));
        let keys = |m: &Model, f: &str| -> Vec<String> {
            m.framed(&("top".to_owned(), f.to_owned()))
                .into_iter()
                .map(|k| k.1)
                .collect()
        };
        assert_eq!(keys(&m, "s1"), ["s1/b1/shank", "s1/b2/shank"]);
        assert_eq!(keys(&m, "s1/b2"), ["s1/b2/shank"]);
        assert_eq!(keys(&m, "row"), ["row:1/s1/b1/shank"]);
        // An occurrence reference reaches the same bodies, by whole steps
        // only, and a face or a part reaches none.
        let reached =
            |r: &str| -> Vec<String> { m.occurrence_bodies(r).into_iter().map(|k| k.1).collect() };
        assert_eq!(reached("top/s1"), ["s1/b1/shank", "s1/b2/shank"]);
        assert_eq!(reached("top/row:1/s1/b1"), ["row:1/s1/b1/shank"]);
        assert!(reached("top/row").is_empty());
        assert!(reached("top/s1/b1/shank/0123").is_empty());
        // A part's feature frames the solid it shows in.
        m.apply_part(&json!({"part": "bolt", "revision": "r", "features": [
            {"id": "shank", "type": "cylinder", "ok": true, "hash": "1"}],
            "solids": [{"solid": "shank", "features": ["shank"], "hash": "1"}]}));
        assert_eq!(
            m.framed(&("bolt".to_owned(), "shank".to_owned())),
            [("bolt".to_owned(), "shank".to_owned())]
        );
    }

    #[test]
    fn only_the_shown_parts_sketches_are_drawn() {
        let part = |id: &str| {
            let mut v = sketched("r1", true);
            v["part"] = json!(id);
            v
        };
        let mut m = Model::default();
        for id in ["p", "q"] {
            m.apply_part(&part(id));
            let key = (id.to_owned(), "s".to_owned());
            assert!(m.take_sketch(&key, drawing("r1")));
        }
        let drawn =
            |m: &Model| -> Vec<String> { m.drawn_sketches().map(|(k, _)| k.0.clone()).collect() };
        assert_eq!(drawn(&m), ["p", "q"]);
        m.focus = Some("q".to_owned());
        assert_eq!(drawn(&m), ["q"]);
        // The project's model is its one root assembly, which holds no
        // sketch: none is drawn, wherever its parts are placed.
        m.focus = None;
        m.apply_part(
            &json!({"part": "asm", "revision": "r", "kind": "assembly", "features": [
            {"id": "a", "type": "instance", "ok": true, "hash": "x",
             "instance": {"part": "p", "rev": "working"}},
            {"id": "b", "type": "instance", "ok": true, "hash": "y",
             "instance": {"part": "q", "rev": "working"}}]}),
        );
        assert_eq!(m.shown(), ["asm"]);
        assert_eq!(drawn(&m), Vec::<String>::new());
    }

    #[test]
    fn an_assembly_reports_its_mates_freedom_and_copies() {
        let n = "0123456789abcdef0123456789abcdef";
        let v = serde_json::json!({
            "part": "frame", "revision": "r1", "kind": "assembly",
            "solve": {"state": "under", "dof": 1, "converged": true, "stale": false},
            "features": [
                {"id": "pin", "type": "instance", "ok": true, "hash": "x",
                 "instance": {"part": "bolt", "rev": "working",
                              "solve": {"state": "under", "dof": 1}}},
                {"id": "m1", "type": "mate", "ok": true, "hash": "y",
                 "mate": {"state": "redundant", "form": "revolute",
                          "a": format!("frame/base/slab/{n}"), "b": "frame/ring:2/pin",
                          "frames": {"a": {"origin_m": [0.02, 0.01, 0.005], "z": [0, 0, 1], "x": [1, 0, 0]},
                                     "b": {"origin_m": [0.02, 0.01, 0.005], "z": [0, 0, -1], "x": [0, 1, 0]}}}},
                {"id": "ring", "type": "instance_pattern", "ok": true, "hash": "z",
                 "copies": [{"copy": "ring:1", "seed": "pin"}, {"copy": "ring:2", "seed": "pin"}]},
            ],
            "occurrences": [{"path": "ring:1/pin", "part": "bolt",
                             "solids": [{"solid": "shank", "hash": "h"}],
                             "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}],
        });
        let p = Part::from_report(&v);
        assert_eq!(
            p.features[0].instance.as_ref().unwrap().solve,
            Some(("under".into(), 1))
        );
        let m = p.features[1].mate.as_ref().unwrap();
        assert_eq!(
            (m.form.as_str(), m.state.as_deref()),
            ("revolute", Some("redundant"))
        );
        assert_eq!(MateInfo::path_of(&m.a), "base");
        assert_eq!(MateInfo::path_of(&m.b), "ring:2/pin");
        assert_eq!(
            MateInfo::face_of(&m.a),
            Some(("base/slab".into(), n.into()))
        );
        assert_eq!(MateInfo::face_of(&m.b), None);
        // Both connectors' frames as solved; b turned over.
        let [fa, fb] = m.frames.unwrap();
        assert_eq!(fa.origin, [0.02, 0.01, 0.005]);
        assert_eq!(fb.z, [0.0, 0.0, -1.0]);
        assert_eq!(fb.y, [1.0, 0.0, 0.0], "z × x");
        let axes = fa.axes(0.01);
        assert_eq!(axes[0], [0.03, 0.01, 0.005]);
        assert_eq!(axes[2], [0.02, 0.01, 0.015]);
        assert_eq!(p.features[2].copies, ["ring:1/pin", "ring:2/pin"]);
        assert_eq!(p.solve.as_ref().unwrap().dof, 1);
        // A copy's solids are under the pattern that places it.
        assert_eq!(p.solids[0].features, ["ring"]);
    }

    #[test]
    fn a_body_is_asked_for_once_per_hash() {
        let mut m = Model::default();
        let want = m.apply_part(&report(&[("a", Some("1"))]));
        assert_eq!(m.ask(want).len(), 1);
        // More reports before the answer: nothing new is asked for.
        let want = m.apply_part(&report(&[("a", Some("1")), ("b", Some("2"))]));
        let asked = m.ask(want);
        assert_eq!(asked, [(("p".to_owned(), "b".to_owned()), "2".to_owned())]);
        // A body that changed while asked for is asked for again.
        let want = m.apply_part(&report(&[("a", Some("3")), ("b", Some("2"))]));
        let asked = m.ask(want);
        assert_eq!(asked, [(("p".to_owned(), "a".to_owned()), "3".to_owned())]);
        // The stale answer leaves the newer question open.
        let a = ("p".to_owned(), "a".to_owned());
        assert!(!m.take_mesh("p", body("a", "1")));
        m.answered(&a, "1");
        assert!(m.ask(vec![a.clone()]).is_empty());
        // An answer that brought nothing lets the next report ask again.
        m.answered(&a, "3");
        assert_eq!(m.ask(vec![a]).len(), 1);
    }

    fn assembly() -> Value {
        json!({"part": "frame", "revision": "r", "kind": "assembly",
               "features": [
                   {"id": "b1", "type": "instance", "ok": true, "hash": "x",
                    "instance": {"part": "bolt", "rev": "working", "fixed": true}},
                   {"id": "b2", "type": "instance", "ok": true, "hash": "y",
                    "instance": {"part": "bolt", "rev": {"label": "B"}}}],
               "solids": [],
               "occurrences": [
                   {"path": "b1", "part": "bolt", "solids": [{"solid": "s", "hash": "1"}],
                    "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}},
                   {"path": "b2", "part": "bolt", "solids": [{"solid": "s", "hash": "1"}],
                    "pose": {"origin_m": [1, 0, 0], "z": [0, 0, 1], "x": [0, 1, 0]}}]})
    }

    #[test]
    fn an_instance_pinned_to_an_obsolete_label_says_so() {
        // #140: the rebuild report's pin says the label is obsolete.
        let mut v = assembly();
        v["features"][1]["instance"]["pin"] = json!({"official": true, "obsolete": true});
        let mut m = Model::default();
        m.apply_part(&report(&[("s", Some("1"))]));
        m.apply_part(&v);
        let b2 = &m.parts["frame"].features[1];
        assert_eq!(b2.instance.as_ref().unwrap().rev, "label B obsolete");
        let row = crate::ui::tiles::tree::row_text(b2, 0);
        assert!(row.contains("bolt @ label B obsolete"), "{row}");
    }

    #[test]
    fn an_assemblys_solids_are_its_occurrences_placed() {
        let mut m = Model::default();
        m.apply_part(&report(&[("s", Some("1"))]));
        let want = m.apply_part(&assembly());
        let frame = &m.parts["frame"];
        assert!(frame.assembly);
        assert_eq!(frame.features[1].instance.as_ref().unwrap().rev, "label B");
        assert!(frame.features[0].instance.as_ref().unwrap().fixed);
        assert_eq!(want.len(), 2);
        assert_eq!(frame.solids[1].id, "b2/s");
        assert_eq!(frame.solids[1].features, ["b2"]);
        // #96: an occurrence's solid says the part placed and its type.
        let mut bolt = report(&[("s", Some("1"))]);
        bolt["part"] = json!("bolt");
        m.apply_part(&bolt);
        assert_eq!(m.source("frame", "b2/s"), "bolt, box");
        assert_eq!(m.source("p", "s"), "box");
        assert_eq!(m.source("frame", "b9/s"), "");
        assert_eq!(
            m.solid_of(&("frame".into(), "b2".into())),
            Some(("frame".into(), "b2/s".into()))
        );
        // A mesh comes in the part's frame, and every key showing its
        // body shares it; b2 draws it turned a quarter about z and 1 m
        // along x.
        let mesh = ryucad::mesh::WireMesh {
            origin: [0.5, 0.0, 0.0],
            chord: 0.0,
            angle: 0.0,
            positions: vec![[1.0, 0.0, 0.0]],
            corners: vec![(0, [1.0, 0.0, 0.0])],
            triangles: Vec::new(),
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
            faults: Vec::new(),
        };
        let wire = WireBody {
            feature: "b2/s".into(),
            hash: "1".into(),
            mesh: Ok(mesh),
        };
        // Asked for once, for all three keys showing body 1.
        assert_eq!(m.ask(want).len(), 1);
        assert!(m.take_mesh("frame", wire.clone()));
        let key = |p: &str, s: &str| (p.to_owned(), s.to_owned());
        let shared = &m.meshes[&key("frame", "b2/s")];
        assert!(Arc::ptr_eq(shared, &m.meshes[&key("frame", "b1/s")]));
        assert!(Arc::ptr_eq(shared, &m.meshes[&key("p", "s")]));
        assert_eq!(shared.mesh.as_ref().unwrap().origin, [0.5, 0.0, 0.0]);
        let pose = m.pose(&key("frame", "b2/s"));
        assert_eq!(pose.apply([1.5, 0.0, 0.0]), [1.0, 1.5, 0.0]);
        assert_eq!(pose.turn([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        assert_eq!(pose.unapply([1.0, 1.5, 0.0]), [1.5, 0.0, 0.0]);
        assert_eq!(m.pose(&key("p", "s")), Pose::IDENTITY);
        // A stale body is refused.
        let mut stale = wire;
        stale.hash = "0".into();
        assert!(!m.take_mesh("frame", stale));
        // A move keeps the mesh: only the pose changes.
        let mut moved = assembly();
        moved["occurrences"][1]["pose"]["origin_m"] = json!([2, 0, 0]);
        assert!(m.apply_part(&moved).is_empty());
        assert_eq!(m.pose(&key("frame", "b2/s")).origin, [2.0, 0.0, 0.0]);
        // The frame is the one root, so it is drawn and the bolt isn't; a
        // focus shows the bolt.
        assert_eq!(m.shown(), ["frame"]);
        assert_eq!(m.drawn().count(), 2);
        m.focus = Some("p".into());
        assert_eq!(m.shown(), ["p"]);
        assert_eq!(
            m.drawn().map(|(k, ..)| k.1.clone()).collect::<Vec<_>>(),
            ["s"]
        );
    }

    #[test]
    fn an_explode_moves_occurrences_where_they_are_drawn() {
        let mut m = Model::default();
        m.apply_part(&report(&[("s", Some("1"))]));
        let want = m.apply_part(&assembly());
        m.ask(want);
        let mesh = ryucad::mesh::WireMesh {
            origin: [0.0; 3],
            chord: 0.0,
            angle: 0.0,
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            corners: vec![(0, [1.0, 0.0, 0.0])],
            triangles: Vec::new(),
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
            faults: Vec::new(),
        };
        m.take_mesh(
            "frame",
            WireBody {
                feature: "b2/s".into(),
                hash: "1".into(),
                mesh: Ok(mesh),
            },
        );
        let key = ("frame".to_owned(), "b2/s".to_owned());
        let before = m.pose(&key);
        let lift = ryucad::explode::offsets(
            &[ryucad::explode::Step {
                occurrences: vec!["b2".into()],
                translate_m: [0.0, 0.0, 0.5],
                rotate: None,
                centre_m: [0.0; 3],
            }],
            1.0,
        );
        let g = m.generation;
        assert!(m.explode(Some(("frame".into(), lift.clone()))));
        assert!(m.generation > g);
        assert!(!m.explode(Some(("frame".into(), lift))), "no change");
        let after = m.pose(&key);
        assert_eq!(
            after.origin,
            [before.origin[0], before.origin[1], before.origin[2] + 0.5]
        );
        assert_eq!(after.x, before.x);
        // Its trail starts at its box's middle where its assembly puts it.
        let middles = m.middles("frame");
        let mid = before.apply([0.5, 0.0, 0.0]);
        assert_eq!(middles["b2"], mid);
        assert!(m.explode(None));
        assert_eq!(m.pose(&key), before);
    }

    #[test]
    fn a_drag_moves_what_its_instance_places_and_says_what_cant_move() {
        let mut m = Model::default();
        let mut a = assembly();
        // b2 free, with a pose of its own; a copy of it under a pattern.
        a["features"][1]["instance"]["pose"] =
            json!({"origin_m": [1, 0, 0], "z": [0, 0, 1], "x": [0, 1, 0]});
        a["occurrences"].as_array_mut().unwrap().push(
            json!({"path": "ring:1/b2", "part": "bolt", "solids": [{"solid": "s", "hash": "1"}],
                         "pose": {"origin_m": [3, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
        );
        m.apply_part(&a);
        let key = |s: &str| ("frame".to_owned(), s.to_owned());
        assert_eq!(m.movable(&key("b2/s")), Ok(("frame".into(), "b2".into())));
        assert_eq!(m.movable(&key("b1/s")).unwrap_err(), "b1 is fixed");
        assert!(m.movable(&key("ring:1/b2/s")).unwrap_err().contains("copy"));
        assert!(m.movable(&("p".into(), "s".into())).is_err());
        // Dragged 2 m along y: its occurrence goes with it, turned as it was.
        m.preview.insert(
            ("frame".into(), "b2".into()),
            Pose::of([1.0, 2.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        );
        let p = m.pose(&key("b2/s"));
        assert_eq!(p.origin, [1.0, 2.0, 0.0]);
        assert_eq!(p.x, [0.0, 1.0, 0.0]);
        assert_eq!(m.pose(&key("b1/s")).origin, [0.0; 3], "others stay");
        // Its assembly's next report ends the preview.
        m.apply_part(&a);
        assert!(m.preview.is_empty());
        assert_eq!(m.pose(&key("b2/s")).origin, [1.0, 0.0, 0.0]);
    }
}
