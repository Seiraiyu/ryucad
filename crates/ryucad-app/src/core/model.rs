//! What the window shows of the model: parts, their features in order with
//! their rebuild status, their solids, and a display mesh per solid. It is
//! built from the daemon's reports and never from the files.
//!
//! An assembly's solids are its occurrences' solids, keyed
//! `<path>/<solid>` (as entity references name them). A mesh is kept in
//! its part's frame and shared by every occurrence of its body, each
//! drawn at its own [`Pose`]: a thousand bolts are one mesh.

use crate::core::sketch::Drawing;
use ryucad::mesh::{Quality, WireBody};
use serde_json::Value;
use std::collections::BTreeMap;
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
}

/// What an instance places: a part at a revision, whether it is fixed,
/// and what its assembly's mates leave it.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub part: String,
    /// `working`, or `label B`.
    pub rev: String,
    pub fixed: bool,
    /// Its state in the solve (`well`, `under`, `over`, `failing`) and
    /// its degrees of freedom, when its assembly has mates.
    pub solve: Option<(String, u64)>,
    /// Where it is in its assembly.
    pub pose: Option<Pose>,
}

/// A mate: its form (`revolute` and so on), its connectors as requests
/// give them, and its state in the solve when it built (`well`,
/// `redundant`).
#[derive(Clone, Debug, PartialEq)]
pub struct MateInfo {
    pub form: String,
    pub a: String,
    pub b: String,
    pub state: Option<String>,
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
}

/// Three numbers from JSON.
fn triple(v: &Value) -> [f64; 3] {
    let x = |i: usize| v[i].as_f64().unwrap_or_default();
    [x(0), x(1), x(2)]
}

impl Occurrence {
    fn from_json(v: &Value) -> Occurrence {
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
                        }),
                        copies: f["copies"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|c| format!("{}/{}", text(&c["copy"]), text(&c["seed"])))
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let solids = v["solids"]
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
        Part {
            id: text(&v["part"]),
            revision: text(&v["revision"]),
            features,
            solids,
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
        let o = self.occurrences.iter().find(|o| o.path == path)?;
        let (_, hash) = o.solids.iter().find(|(s, _)| s == solid)?;
        Some((o, hash))
    }

    pub fn feature(&self, id: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    /// The solid founded by `id`, if the model shows it.
    pub fn solid(&self, id: &str) -> Option<&Solid> {
        self.solids.iter().find(|s| s.id == id)
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

/// The model as the window knows it.
#[derive(Debug, Default)]
pub struct Model {
    pub default_unit: String,
    pub parts: BTreeMap<String, Part>,
    /// Display meshes by part and solid, each for the body hash it says,
    /// in its part's frame: occurrences of one body share one.
    pub meshes: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Meshes asked for and not yet answered, by the body hash they were
    /// asked for at: a burst of changes asks for each body once.
    pub asked: BTreeMap<BodyKey, String>,
    /// The user's display quality: the level `meshes` are at.
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
    /// Bumped whenever anything here changes, so views know to redraw.
    pub generation: u64,
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
        let mut out = Vec::new();
        for key in keys {
            let Some(hash) = self.hash_of(&key) else {
                continue;
            };
            if let Some(m) = self.meshes.values().find(|m| m.hash == hash).cloned() {
                self.meshes.insert(key, m);
                self.generation += 1;
                continue;
            }
            if !self.asked.values().any(|h| *h == hash) {
                self.asked.insert(key.clone(), hash.clone());
                out.push((key, hash));
            }
        }
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
        self.parts.remove(id);
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
            .parts
            .values()
            .flat_map(|p| {
                p.solids
                    .iter()
                    .filter(|s| s.hash.as_deref() == Some(&body.hash))
                    .map(|s| (p.id.clone(), s.id.clone()))
            })
            .collect();
        for key in showing {
            self.meshes.insert(key, Arc::clone(&body));
        }
        self.generation += 1;
        true
    }

    /// `key`'s mesh at `level`, if the window has it.
    pub fn mesh_at(&self, key: &BodyKey, level: Quality) -> Option<Arc<WireBody>> {
        let base = self.meshes.get(key)?;
        if level == self.base {
            return Some(Arc::clone(base));
        }
        self.levels.get(&(base.hash.clone(), level)).cloned()
    }

    /// The levels of `wanted` to fetch, each body's once, now noted as
    /// asked for, in the order given.
    pub fn ask_levels(
        &mut self,
        wanted: Vec<(BodyKey, Quality)>,
    ) -> Vec<(BodyKey, String, Quality)> {
        let mut out = Vec::new();
        for (key, level) in wanted {
            let Some(hash) = self.meshes.get(&key).map(|m| m.hash.clone()) else {
                continue;
            };
            let at = (hash.clone(), level);
            if level == self.base
                || self.levels.contains_key(&at)
                || self.levels_asked.contains(&at)
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
        self.levels.insert(at, Arc::new(body));
        self.generation += 1;
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
        match (dragged, was) {
            // Where the occurrence is relative to its instance stays.
            (Some(now), Some(was)) => Pose {
                origin: now.apply(was.unapply(at.origin)),
                x: now.turn(was.unturn(at.x)),
                y: now.turn(was.unturn(at.y)),
                z: now.turn(was.unturn(at.z)),
            },
            _ => at,
        }
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
            .map(|(k, m)| (k, m, self.pose(k)))
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
                          "a": format!("frame/base/slab/{n}"), "b": "frame/ring:2/pin"}},
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
