//! What the window shows of the model: parts, their features in order with
//! their rebuild status, their solids, and a display mesh per solid. It is
//! built from the daemon's reports and never from the files.
//!
//! An assembly's solids are its occurrences' solids, keyed
//! `<path>/<solid>` (as entity references name them), each mesh moved
//! where its occurrence is when it comes.

use crate::core::sketch::Drawing;
use ryucad::mesh::WireBody;
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

    /// The hash a mesh of one of its bodies, moved where it is, is kept
    /// by: the body's and the pose's together, so a move fetches again.
    fn placed_hash(&self, body: &str) -> String {
        let pose: Vec<String> = self
            .origin
            .iter()
            .chain(&self.z)
            .chain(&self.x)
            .map(|x| format!("{:x}", x.to_bits()))
            .collect();
        format!("{body}@{}", pose.join(","))
    }

    /// `m` moved from its part's frame to where the occurrence is.
    fn place(&self, m: &mut ryucad::mesh::WireMesh) {
        let unit = |a: [f64; 3]| {
            let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            [a[0] / n, a[1] / n, a[2] / n]
        };
        let z = unit(self.z);
        // x made perpendicular to z, as poses are built.
        let d = self.x[0] * z[0] + self.x[1] * z[1] + self.x[2] * z[2];
        let x = unit([
            self.x[0] - d * z[0],
            self.x[1] - d * z[1],
            self.x[2] - d * z[2],
        ]);
        let y = [
            z[1] * x[2] - z[2] * x[1],
            z[2] * x[0] - z[0] * x[2],
            z[0] * x[1] - z[1] * x[0],
        ];
        let turn = |v: [f64; 3]| {
            [
                x[0] * v[0] + y[0] * v[1] + z[0] * v[2],
                x[1] * v[0] + y[1] * v[1] + z[1] * v[2],
                x[2] * v[0] + y[2] * v[1] + z[2] * v[2],
            ]
        };
        let o = turn(m.origin);
        m.origin = [
            o[0] + self.origin[0],
            o[1] + self.origin[1],
            o[2] + self.origin[2],
        ];
        let f = |v: [f32; 3]| {
            let t = turn([f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]);
            [t[0] as f32, t[1] as f32, t[2] as f32]
        };
        for p in &mut m.positions {
            *p = f(*p);
        }
        for c in &mut m.corners {
            c.1 = f(c.1);
        }
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
        // instance that places it, by its placed hash.
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
                        hash: Some(o.placed_hash(h)),
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
    /// Display meshes by part and solid, each for the body hash it says.
    pub meshes: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Meshes asked for and not yet answered, by the body hash they were
    /// asked for at: a burst of changes asks for each body once.
    pub asked: BTreeMap<BodyKey, String>,
    /// How sketches are drawn, by part and feature, each for the part
    /// revision it says.
    pub sketches: BTreeMap<BodyKey, Arc<Drawing>>,
    /// Sketches asked for and not yet answered, by the part revision they
    /// were asked for at.
    pub sketches_asked: BTreeMap<BodyKey, String>,
    /// The part or assembly the window shows, as its participant's focus
    /// says; `None` for the project's model.
    pub focus: Option<String>,
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
        // Meshes of solids that are gone, or changed, go.
        self.meshes.retain(|(p, s), m| {
            *p != id
                || part
                    .solid(s)
                    .is_some_and(|s| s.hash.as_deref() == Some(&m.hash))
        });
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

    /// The bodies of `keys` not already asked for at their current hash,
    /// with that hash, now noted as asked for.
    pub fn ask(&mut self, keys: Vec<BodyKey>) -> Vec<(BodyKey, String)> {
        let mut out = Vec::new();
        for key in keys {
            let Some(hash) = self
                .parts
                .get(&key.0)
                .and_then(|p| p.solid(&key.1))
                .and_then(|s| s.hash.clone())
            else {
                continue;
            };
            if self.asked.get(&key) != Some(&hash) {
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

    /// Keeps a fetched mesh if its body is still the current one; an
    /// assembly's moved where its occurrence is.
    pub fn take_mesh(&mut self, part: &str, body: WireBody) -> bool {
        let mut body = body;
        if let Some(p) = self.parts.get(part).filter(|p| p.assembly) {
            let Some((o, hash)) = p.occurrence(&body.feature) else {
                return false;
            };
            if *hash != body.hash {
                return false;
            }
            if let Ok(m) = &mut body.mesh {
                o.place(m);
            }
            body.hash = o.placed_hash(hash);
        }
        let current = self
            .parts
            .get(part)
            .and_then(|p| p.solid(&body.feature))
            .and_then(|s| s.hash.as_deref());
        if current != Some(&body.hash) {
            return false;
        }
        let key = (part.to_owned(), body.feature.clone());
        self.meshes.insert(key, Arc::new(body));
        self.generation += 1;
        true
    }

    /// Drops every mesh: the display quality changed.
    pub fn clear_meshes(&mut self) -> Vec<BodyKey> {
        self.meshes.clear();
        self.asked.clear();
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

    /// The meshes the window draws: those of the parts it shows.
    pub fn drawn(&self) -> impl Iterator<Item = (&BodyKey, &Arc<WireBody>)> {
        let shown = self.shown();
        self.meshes
            .iter()
            .filter(move |(k, _)| shown.contains(&k.0.as_str()))
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
        assert_eq!(
            m.solid_of(&("frame".into(), "b2".into())),
            Some(("frame".into(), "b2/s".into()))
        );
        // A mesh comes in the part's frame and is moved where b2 is: turned
        // a quarter about z and 1 m along x.
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
        assert!(m.take_mesh("frame", wire.clone()));
        let placed = m.meshes[&("frame".to_owned(), "b2/s".to_owned())]
            .mesh
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(placed.origin, [1.0, 0.5, 0.0]);
        assert_eq!(placed.positions[0], [0.0, 1.0, 0.0]);
        assert_eq!(placed.corners[0].1, [0.0, 1.0, 0.0]);
        // A stale body is refused.
        let mut stale = wire;
        stale.hash = "0".into();
        assert!(!m.take_mesh("frame", stale));
        // The frame is the one root, so it is drawn and the bolt isn't; a
        // focus shows the bolt.
        m.take_mesh("p", body("s", "1"));
        assert_eq!(m.shown(), ["frame"]);
        assert_eq!(m.drawn().count(), 1);
        m.focus = Some("p".into());
        assert_eq!(m.shown(), ["p"]);
        assert_eq!(
            m.drawn().map(|(k, _)| k.1.clone()).collect::<Vec<_>>(),
            ["s"]
        );
    }
}
