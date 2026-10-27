//! Assemblies: parts placed as instances. A part's *model* is what an
//! instance of it places: its solids as the part shows them, or, for an
//! assembly, every leaf part its instances place, at any depth, each an
//! *occurrence* with its instance path and its pose in the assembly.
//!
//! Instances read other parts, so assemblies are rebuilt across the
//! project: a part before every assembly that places it. An instance
//! whose part places, through instances at any depth, the assembly it is
//! in is `assembly_cycle`; cycles are found from the project's files
//! before anything is built, so what fails doesn't depend on which
//! assembly was asked for first.

use crate::doc::assembly::{Instance, Pose, Rev};
use crate::doc::files::{Feature, PartKind};
use crate::doc::id::Id;
use crate::doc::project::{Part, Project};
use crate::error::{Code, Error, Result};
use crate::rebuild::{Built, Cache, Output, PartResult, rebuild_part_in};
use crate::solid::Solids;
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A leaf part placed in a model: the instances it is reached through,
/// from the model's own (none for a part's own model), the part and the
/// revision it shows, its solids as shown, by founder, and its pose in
/// the model.
#[derive(Clone, Debug)]
pub struct Occurrence {
    pub path: Vec<Id>,
    pub part: Id,
    pub rev: Rev,
    pub solids: Vec<Solid>,
    pub pose: Rigid,
}

/// A solid of an occurrence's part as the part shows it: its founder, the
/// feature whose result shows (which sizes its display mesh), and its
/// body.
#[derive(Clone, Debug)]
pub struct Solid {
    pub founder: Id,
    pub feature: Arc<Feature>,
    pub built: Arc<Built>,
}

/// What an instance of a part places: its occurrences in order, a hash
/// of them (equal hashes, equal models), and for a part read at a release
/// label, how the label resolved.
#[derive(Clone, Debug)]
pub struct Model {
    pub occurrences: Vec<Occurrence>,
    pub hash: String,
    pub pin: Option<Pin>,
}

/// How a release label resolved: at the commit where its record became
/// official, or (`commit: None`) to the working files, which its proposed
/// record describes. `obsolete` when an obsolete record names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub commit: Option<String>,
    pub obsolete: bool,
}

impl Pin {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = match &self.commit {
            Some(c) => serde_json::json!({"official": true, "commit": c}),
            None => serde_json::json!({"official": false, "proposed": true}),
        };
        if self.obsolete {
            v["obsolete"] = serde_json::json!(true);
        }
        v
    }
}

/// A part as a release label has it: its files where the record became
/// official (or the working files, for a proposal that describes them),
/// loaded, and how the label resolved.
#[derive(Clone, Debug)]
pub struct Pinned {
    pub part: Part,
    pub pin: Pin,
}

/// Every release label the project's instances reach, at any depth, by
/// part and label: the part as the label has it, or why it can't be read
/// (`label_not_found`, `label_not_official`).
pub type Pins = BTreeMap<(Id, String), Result<Arc<Pinned>>>;

/// A part at a revision: what an instance names.
pub type Node = (Id, Rev);

impl Model {
    fn new(occurrences: Vec<Occurrence>) -> Model {
        let mut h = blake3::Hasher::new();
        for o in &occurrences {
            let path: Vec<&str> = o.path.iter().map(Id::as_str).collect();
            h.update(format!("{} {} {}\n", path.join("/"), o.part, o.rev.text()).as_bytes());
            for s in &o.solids {
                h.update(format!(" {} {}\n", s.founder, s.built.hash).as_bytes());
            }
            h.update(&pose_bits(&o.pose));
        }
        Model {
            occurrences,
            hash: h.finalize().to_hex()[..32].to_owned(),
            pin: None,
        }
    }

    /// A part's own model: one occurrence of its solids as they show, at
    /// the part's origin.
    pub fn of_part(part: &Part, rev: &Rev, result: &PartResult) -> Model {
        let solids = Solids::of(part);
        let shown = result
            .shown(&solids)
            .into_iter()
            .filter_map(|s| {
                let (f, b) = s.state?;
                Some(Solid {
                    founder: s.solid.clone(),
                    feature: Arc::new(part.features.get(f)?.as_ref().ok()?.clone()),
                    built: Arc::clone(b),
                })
            })
            .collect();
        Model::new(vec![Occurrence {
            path: Vec::new(),
            part: part.id.clone(),
            rev: rev.clone(),
            solids: shown,
            pose: Rigid::IDENTITY,
        }])
    }

    /// An assembly's model: the occurrences of each instance that placed
    /// its part, in list order, under the instance's ID and moved by its
    /// pose.
    pub fn of_assembly(result: &PartResult) -> Model {
        let mut out = Vec::new();
        let mut place = |prefix: Vec<Id>, placed: &Placed| {
            for o in &placed.model.occurrences {
                let mut path = prefix.clone();
                path.extend(o.path.iter().cloned());
                out.push(Occurrence {
                    path,
                    part: o.part.clone(),
                    rev: o.rev.clone(),
                    solids: o.solids.clone(),
                    pose: placed.pose.then_after(&o.pose),
                });
            }
        };
        for (id, r) in &result.features {
            match r {
                Ok(Output::Instance(placed)) => place(vec![id.clone()], placed),
                // A copy is reached as `<pattern>:<i>/<seed>`.
                Ok(Output::Copies(c)) => {
                    for copy in &c.copies {
                        place(
                            vec![Id::copy(id, copy.index), copy.seed.clone()],
                            &copy.placed,
                        );
                    }
                }
                _ => {}
            }
        }
        Model::new(out)
    }
}

/// The bits of a pose's rotation and translation, for hashing.
fn pose_bits(r: &Rigid) -> Vec<u8> {
    let mut out = Vec::with_capacity(96);
    for row in r.rotation_matrix() {
        for x in row {
            out.extend_from_slice(&x.to_bits().to_le_bytes());
        }
    }
    let t = r.translation_part();
    for x in [t.x, t.y, t.z] {
        out.extend_from_slice(&x.to_bits().to_le_bytes());
    }
    out
}

/// What an instance rebuilds to: its part and revision, its pose, and
/// the model it places.
#[derive(Clone, Debug)]
pub struct Placed {
    pub part: Id,
    pub rev: Rev,
    /// Whether mates hold it where it is.
    pub fixed: bool,
    pub pose: Rigid,
    pub model: Arc<Model>,
    /// A hash of the revision, the pose's bits and the model's hash.
    pub hash: String,
}

/// The pose of a rigid placement: where it takes the origin and its z
/// and x axes.
pub fn rigid_pose(r: &Rigid) -> Pose {
    let m = r.rotation_matrix();
    let t = r.translation_part();
    Pose {
        origin_m: [t.x, t.y, t.z],
        z: [m[0][2], m[1][2], m[2][2]],
        x: [m[0][0], m[1][0], m[2][0]],
    }
}

/// An instance path as references write it: its IDs joined by `/`.
pub fn path_text(path: &[Id]) -> String {
    path.iter().map(Id::as_str).collect::<Vec<_>>().join("/")
}

impl Occurrence {
    /// How reports give it: its path, part and revision, its solids by
    /// founder with their hashes, and its pose.
    pub fn to_json(&self) -> serde_json::Value {
        let solids: Vec<serde_json::Value> = self
            .solids
            .iter()
            .map(|s| serde_json::json!({"solid": s.founder, "hash": s.built.hash}))
            .collect();
        serde_json::json!({
            "path": path_text(&self.path),
            "part": self.part,
            "rev": self.rev.to_json(),
            "solids": solids,
            "pose": rigid_pose(&self.pose).to_json(),
        })
    }
}

impl Placed {
    /// How a rebuild report gives an instance: its part, revision (and how
    /// a label resolved), pose and how many occurrences it places.
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "part": self.part,
            "rev": self.rev.to_json(),
            "pose": rigid_pose(&self.pose).to_json(),
            "occurrences": self.model.occurrences.len(),
        });
        if let Some(pin) = &self.model.pin {
            v["pin"] = pin.to_json();
        }
        if self.fixed {
            v["fixed"] = serde_json::json!(true);
        }
        v
    }
}

/// `instance_obsolete`: instance `id` places a release an obsolete record
/// names.
pub fn obsolete_warning(id: &Id, p: &Placed) -> serde_json::Value {
    serde_json::json!({
        "code": "instance_obsolete",
        "feature": id,
        "part": p.part,
        "rev": p.rev.to_json(),
        "message": format!("instance {id} places {} at {}, which is obsolete", p.part, p.rev.text()),
    })
}

/// The rigid placement of a pose: its frame, `x` made perpendicular to
/// `z`. Axes with no direction, or parallel ones, and an origin outside
/// the size box are `invalid_placement`.
pub fn pose_rigid(p: &Pose) -> Result<Rigid> {
    let v = |a: [f64; 3]| Vector3::new(a[0], a[1], a[2]);
    let [ox, oy, oz] = p.origin_m;
    let frame = Frame::new(Point3::new(ox, oy, oz), v(p.z), v(p.x)).map_err(|e| {
        let why = match e {
            ryukei::Error::Degenerate => "z and x must be non-zero and not parallel",
            ryukei::Error::OutsideSizeBox => "the origin is outside the ±500 m size box",
            _ => "the axes must be finite",
        };
        Error::new(Code::InvalidPlacement, format!("bad pose: {why}"))
            .with("kernel", format!("{e:?}"))
    })?;
    Ok(Rigid::from_frame(&frame))
}

/// The models an assembly's instances place, by part and revision: what
/// its rebuild reads, each found before it is rebuilt.
pub type Children = BTreeMap<(Id, Rev), Result<Arc<Model>>>;

/// Places instance `id`: its pose, and the model of its part at its
/// revision from `children`.
pub fn place(id: &Id, inst: &Instance, children: &Children) -> Result<Placed> {
    let pose = pose_rigid(&inst.pose).map_err(|e| e.with("feature", id))?;
    let model = match children.get(&(inst.part.clone(), inst.rev.clone())) {
        Some(Ok(m)) => Arc::clone(m),
        // Why the part couldn't be placed at all is the instance's fault.
        Some(Err(e))
            if matches!(
                e.code,
                Code::AssemblyCycle
                    | Code::NotFound
                    | Code::LabelNotFound
                    | Code::LabelNotOfficial
                    | Code::InstanceNotReleased
            ) =>
        {
            return Err(e.clone().with("feature", id));
        }
        Some(Err(e)) => {
            return Err(Error::new(
                Code::DependencyFailed,
                format!("{id} places {}, which failed to rebuild", inst.part),
            )
            .with("feature", id)
            .with("dependency", &inst.part)
            .with("cause", e.code));
        }
        None => {
            return Err(Error::new(
                Code::Internal,
                format!("{id}: {} wasn't rebuilt before the assembly", inst.part),
            )
            .with("feature", id));
        }
    };
    Ok(Placed {
        part: inst.part.clone(),
        rev: inst.rev.clone(),
        fixed: inst.fixed,
        pose,
        hash: placed_hash(&inst.part, &inst.rev, inst.fixed, &pose, &model),
        model,
    })
}

/// Whether `f` is an assembly's feature: an instance, a mate or an
/// instance pattern.
pub fn assembles(f: &Feature) -> bool {
    matches!(
        f,
        Feature::Instance(_) | Feature::Mate(_) | Feature::InstancePattern(_)
    )
}

/// `p` placed at `pose` instead, its hash made again.
pub fn placed_at(p: &Placed, pose: Rigid) -> Placed {
    Placed {
        pose,
        hash: placed_hash(&p.part, &p.rev, p.fixed, &pose, &p.model),
        ..p.clone()
    }
}

/// An instance's hash: its revision, whether it is fixed, its pose's bits
/// and its model's hash.
fn placed_hash(part: &Id, rev: &Rev, fixed: bool, pose: &Rigid, model: &Model) -> String {
    let mut h = blake3::Hasher::new();
    h.update(format!("{part} {} {fixed}\n", rev.text()).as_bytes());
    h.update(&pose_bits(pose));
    h.update(model.hash.as_bytes());
    h.finalize().to_hex()[..32].to_owned()
}

/// `wrong_part_kind` for a feature its part doesn't hold: an instance, a
/// mate or an instance pattern in a part, or anything else in an
/// assembly.
pub fn kind_error(part: &Part, id: &Id, f: &Feature) -> Option<Error> {
    kind_error_of(part, id, f.type_name())
}

/// [`kind_error`] by a feature's type name, so a write can refuse a
/// feature its part doesn't hold before reading its fields.
pub fn kind_error_of(part: &Part, id: &Id, ty: &str) -> Option<Error> {
    let kind = part.file.as_ref().map_or(PartKind::Part, |p| p.kind);
    let assembles = matches!(ty, "instance" | "mate" | "instance_pattern");
    let why = match (kind, assembles) {
        (PartKind::Part, true) => format!(
            "{id} is {} {ty}, and {} is a part: only assemblies place parts",
            if ty == "instance" { "an" } else { "a" },
            part.id
        ),
        (PartKind::Assembly, false) => format!(
            "{id} is a {ty}, and {} is an assembly: assemblies hold only instances, mates and instance patterns",
            part.id
        ),
        _ => return None,
    };
    Some(
        Error::new(Code::WrongPartKind, why)
            .with("part", &part.id)
            .with("feature", id)
            .with("kind", kind.as_str())
            .with("type", ty),
    )
}

/// What each part at a revision places, by node: a part's listed
/// instances at `working` and in releases the pins hold. An instance at
/// `working` inside a release reads nothing (it is
/// `instance_not_released`). These are the edges cycles are found on.
fn edges(project: &Project, pins: &Pins) -> BTreeMap<Node, BTreeSet<Node>> {
    let instances = |p: &Part, inside_release: bool| -> BTreeSet<Node> {
        p.order()
            .iter()
            .filter_map(|fid| match p.features.get(fid) {
                Some(Ok(Feature::Instance(i))) if !(inside_release && i.rev == Rev::Working) => {
                    Some((i.part.clone(), i.rev.clone()))
                }
                _ => None,
            })
            .collect()
    };
    let mut out = BTreeMap::new();
    for (id, p) in &project.parts {
        let to = instances(p, false);
        if !to.is_empty() {
            out.insert((id.clone(), Rev::Working), to);
        }
    }
    for ((id, label), pinned) in pins {
        if let Ok(pinned) = pinned {
            let to = instances(&pinned.part, true);
            if !to.is_empty() {
                out.insert((id.clone(), Rev::Label(label.clone())), to);
            }
        }
    }
    out
}

/// The parts at revisions that lie on a cycle of instances: Tarjan's
/// strongly connected components over [`edges`]. An instance of `b` in
/// `a` closes a cycle when both are in one component.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cycles {
    edges: BTreeMap<Node, BTreeSet<Node>>,
    component: BTreeMap<Node, usize>,
}

impl Cycles {
    pub fn of(project: &Project, pins: &Pins) -> Cycles {
        let edges = edges(project, pins);
        let mut t = Tarjan {
            edges: &edges,
            index: BTreeMap::new(),
            low: BTreeMap::new(),
            stack: Vec::new(),
            on: BTreeSet::new(),
            next: 0,
            component: BTreeMap::new(),
            components: 0,
        };
        for v in edges.keys() {
            if !t.index.contains_key(v) {
                t.visit(v);
            }
        }
        let mut component = t.component;
        // A component of one part is a cycle only through itself.
        let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
        for c in component.values() {
            *sizes.entry(*c).or_default() += 1;
        }
        component.retain(|v, c| sizes[c] > 1 || edges.get(v).is_some_and(|to| to.contains(v)));
        Cycles { edges, component }
    }

    /// Whether an instance of `child` in `assembly` closes a cycle.
    pub fn closes(&self, assembly: &Node, child: &Node) -> bool {
        matches!((self.component.get(assembly), self.component.get(child)), (Some(a), Some(b)) if a == b)
    }
}

struct Tarjan<'a> {
    edges: &'a BTreeMap<Node, BTreeSet<Node>>,
    index: BTreeMap<Node, usize>,
    low: BTreeMap<Node, usize>,
    stack: Vec<Node>,
    on: BTreeSet<Node>,
    next: usize,
    component: BTreeMap<Node, usize>,
    components: usize,
}

impl Tarjan<'_> {
    fn visit(&mut self, v: &Node) {
        self.index.insert(v.clone(), self.next);
        self.low.insert(v.clone(), self.next);
        self.next += 1;
        self.stack.push(v.clone());
        self.on.insert(v.clone());
        let to: Vec<Node> = self
            .edges
            .get(v)
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default();
        for w in &to {
            if !self.index.contains_key(w) {
                self.visit(w);
                let l = self.low[v].min(self.low[w]);
                self.low.insert(v.clone(), l);
            } else if self.on.contains(w) {
                let l = self.low[v].min(self.index[w]);
                self.low.insert(v.clone(), l);
            }
        }
        if self.low[v] == self.index[v] {
            loop {
                let w = self.stack.pop().expect("v is on the stack");
                self.on.remove(&w);
                self.component.insert(w.clone(), self.components);
                if w == *v {
                    break;
                }
            }
            self.components += 1;
        }
    }
}

/// The path from `from` back to `to` through instances, for the
/// message: the shortest, the first in order at each step.
fn path_back(edges: &BTreeMap<Node, BTreeSet<Node>>, from: &Node, to: &Node) -> Vec<Node> {
    let mut prev: BTreeMap<Node, Node> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([from.clone()]);
    let mut seen = BTreeSet::from([from.clone()]);
    while let Some(v) = queue.pop_front() {
        if v == *to {
            break;
        }
        for w in edges.get(&v).into_iter().flatten() {
            if seen.insert(w.clone()) {
                prev.insert(w.clone(), v.clone());
                queue.push_back(w.clone());
            }
        }
    }
    let mut path = vec![to.clone()];
    let mut at = to.clone();
    while let Some(p) = prev.get(&at) {
        path.push(p.clone());
        at = p.clone();
    }
    path.reverse();
    path
}

/// A node as people read it: `frame`, or `bolt@B` at a label.
fn node_text((part, rev): &Node) -> String {
    match rev {
        Rev::Working => part.to_string(),
        Rev::Label(l) => format!("{part}@{l}"),
    }
}

/// `assembly_cycle` for an instance of `child` in `assembly`.
pub fn cycle_error(cycles: &Cycles, assembly: &Node, child: &Node) -> Error {
    let mut path = vec![assembly.clone()];
    path.extend(path_back(&cycles.edges, child, assembly));
    let text: Vec<String> = path.iter().map(node_text).collect();
    let (a, c) = (node_text(assembly), node_text(child));
    Error::new(
        Code::AssemblyCycle,
        format!(
            "{a} places {c}, which places {a} again: {}",
            text.join(" > ")
        ),
    )
    .with("part", &assembly.0)
    .with("child", &child.0)
    .with("path", &text)
}

/// `instance_not_released` for an instance at `working` inside a part
/// read at a release label.
fn working_in_release(node: &Node, child: &Id) -> Error {
    Error::new(
        Code::InstanceNotReleased,
        format!(
            "{} places {child} as it is now (working), and a release can only place releases",
            node_text(node)
        ),
    )
    .with("part", &node.0)
    .with("child", child)
}

/// What a session keeps between rebuilds: each part's rebuild by a key of
/// everything it read (its revision, and for an assembly its children's
/// models) and its feature cache, the same for parts read at release
/// labels, by part and label, and the disk cache beneath them, if any.
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub results: BTreeMap<Id, (String, PartResult)>,
    pub caches: BTreeMap<Id, Cache>,
    pub pinned: BTreeMap<(Id, String), (String, PartResult)>,
    pub pinned_caches: BTreeMap<(Id, String), Cache>,
    pub disk: Option<Arc<crate::cache::Disk>>,
}

/// Rebuilds parts and the assemblies over them, a part before every
/// assembly that places it, reusing what its [`Store`] holds. `fresh`
/// lists, in order, the parts at `working` it rebuilt.
pub struct Rebuild<'p> {
    project: &'p Project,
    pins: &'p Pins,
    cycles: Cycles,
    pub store: Store,
    pub fresh: Vec<Id>,
}

impl<'p> Rebuild<'p> {
    pub fn new(project: &'p Project, pins: &'p Pins, store: Store) -> Rebuild<'p> {
        Rebuild {
            project,
            pins,
            cycles: Cycles::of(project, pins),
            store,
            fresh: Vec::new(),
        }
    }

    /// Builds those of `parts` that place nothing and aren't built at
    /// their revisions yet side by side, keeping each as [`Rebuild::result`]
    /// would: the parts an assembly places, an imported file's hundreds
    /// among them, heal in parallel. Each is the same bits on any number
    /// of threads, so the order they finish in changes nothing.
    pub fn build_parts(&mut self, parts: &[Id]) {
        use rayon::prelude::*;
        let todo: BTreeMap<&Id, &Part> = parts
            .iter()
            .filter_map(|id| self.project.parts.get(id))
            .filter(|p| !p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly))
            .filter(|p| {
                !self
                    .store
                    .results
                    .get(&p.id)
                    .is_some_and(|(k, _)| *k == p.revision)
            })
            .map(|p| (&p.id, p))
            .collect();
        let empty = Cache::new();
        let (caches, disk) = (&self.store.caches, self.store.disk.as_deref());
        let built: Vec<(Id, PartResult, Cache)> = todo
            .into_par_iter()
            .map(|(id, p)| {
                let cache = caches.get(id).unwrap_or(&empty);
                let (r, c) = rebuild_part_in(p, cache, &Children::new(), disk);
                (id.clone(), r, c)
            })
            .collect();
        for (id, r, c) in built {
            let key = self.project.parts[&id].revision.clone();
            self.store.caches.insert(id.clone(), c);
            self.store.results.insert(id.clone(), (key, r));
            self.fresh.push(id);
        }
    }

    /// The rebuild of `part`, and of every part it places first.
    pub fn result(&mut self, part: &Id) -> Result<&PartResult> {
        let p = self.project.part(part)?;
        let (key, children) = self.children(p, &Rev::Working);
        let fresh = self.store.results.get(part).is_some_and(|(k, _)| *k == key);
        if !fresh {
            let empty = Cache::new();
            let cache = self.store.caches.get(part).unwrap_or(&empty);
            let (result, cache) = rebuild_part_in(p, cache, &children, self.store.disk.as_deref());
            self.store.caches.insert(part.clone(), cache);
            self.store.results.insert(part.clone(), (key, result));
            self.fresh.push(part.clone());
        }
        Ok(&self.store.results[part].1)
    }

    /// The rebuild of `part` as release `label` has it.
    fn pinned_result(&mut self, part: &Id, label: &str, pinned: &Pinned) -> &PartResult {
        let at = (part.clone(), label.to_owned());
        let (key, children) = self.children(&pinned.part, &Rev::Label(label.to_owned()));
        let fresh = self.store.pinned.get(&at).is_some_and(|(k, _)| *k == key);
        if !fresh {
            let empty = Cache::new();
            let cache = self.store.pinned_caches.get(&at).unwrap_or(&empty);
            let (result, cache) =
                rebuild_part_in(&pinned.part, cache, &children, self.store.disk.as_deref());
            self.store.pinned_caches.insert(at.clone(), cache);
            self.store.pinned.insert(at.clone(), (key, result));
        }
        &self.store.pinned[&at].1
    }

    /// The models what `p` (`part` at `rev`) places, by part and
    /// revision, each found first, and the key of its rebuild: its files'
    /// revision and those models' hashes.
    fn children(&mut self, p: &Part, rev: &Rev) -> (String, Children) {
        let mut key = p.revision.clone();
        let mut children = Children::new();
        if !p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
            return (key, children);
        }
        let node = (p.id.clone(), rev.clone());
        // The parts it places as they are now, built side by side first.
        if *rev == Rev::Working {
            let placed: Vec<Id> = p
                .order()
                .iter()
                .filter_map(|fid| match p.features.get(fid) {
                    Some(Ok(Feature::Instance(i))) if i.rev == Rev::Working => Some(i.part.clone()),
                    _ => None,
                })
                .collect();
            self.build_parts(&placed);
        }
        for fid in p.order() {
            let Some(Ok(Feature::Instance(i))) = p.features.get(fid) else {
                continue;
            };
            let at = (i.part.clone(), i.rev.clone());
            if children.contains_key(&at) {
                continue;
            }
            let m = if *rev != Rev::Working && i.rev == Rev::Working {
                Err(working_in_release(&node, &i.part))
            } else if self.cycles.closes(&node, &at) {
                Err(cycle_error(&self.cycles, &node, &at))
            } else {
                self.model(&i.part, &i.rev)
            };
            children.insert(at, m);
        }
        let mut h = blake3::Hasher::new();
        for ((c, crev), m) in &children {
            let what = match m {
                Ok(m) => format!("{} {:?}", m.hash, m.pin),
                Err(e) => serde_json::to_string(&e.code).expect("codes serialize"),
            };
            h.update(format!("{c} {} {what}\n", crev.text()).as_bytes());
        }
        key = format!("{key}+{}", &h.finalize().to_hex()[..32]);
        (key, children)
    }

    /// The model of `part` at `rev`: what an instance of it places.
    pub fn model(&mut self, part: &Id, rev: &Rev) -> Result<Arc<Model>> {
        match rev {
            Rev::Working => {
                let p = self.project.part(part).map_err(|_| {
                    Error::new(Code::NotFound, format!("no part {part} to place"))
                        .with("part", part)
                })?;
                let result = self.result(part)?;
                Ok(Arc::new(model_of(p, rev, result)))
            }
            Rev::Label(l) => {
                let pinned = match self.pins.get(&(part.clone(), l.clone())) {
                    Some(Ok(p)) => Arc::clone(p),
                    Some(Err(e)) => return Err(e.clone()),
                    None => {
                        return Err(Error::new(
                            Code::Internal,
                            format!("{part}@{l} wasn't looked up before the rebuild"),
                        ));
                    }
                };
                let result = self.pinned_result(part, l, &pinned);
                let mut m = model_of(&pinned.part, rev, result);
                m.pin = Some(pinned.pin.clone());
                Ok(Arc::new(m))
            }
        }
    }
}

impl Rebuild<'_> {
    /// `part` at `rev` as rebuilt, every part it places first: its files,
    /// its rebuild, and for a release label how the label resolved.
    pub fn node(&mut self, part: &Id, rev: &Rev) -> Result<(Part, PartResult, Option<Pin>)> {
        let model = self.model(part, rev)?;
        Ok(match rev {
            Rev::Working => (
                self.project.part(part)?.clone(),
                self.store.results[part].1.clone(),
                None,
            ),
            Rev::Label(l) => {
                let at = (part.clone(), l.clone());
                let Some(Ok(pinned)) = self.pins.get(&at) else {
                    unreachable!("the model was found")
                };
                (
                    pinned.part.clone(),
                    self.store.pinned[&at].1.clone(),
                    model.pin.clone(),
                )
            }
        })
    }
}

/// The model of `p` at `rev` from its rebuild.
fn model_of(p: &Part, rev: &Rev, result: &PartResult) -> Model {
    if p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
        Model::of_assembly(result)
    } else {
        Model::of_part(p, rev, result)
    }
}

/// Every release label `project`'s instances reach, at any depth, each
/// looked up by `look` once: what [`Rebuild`] reads labels from.
pub fn pins(project: &Project, look: impl FnMut(&Id, &str) -> Result<Pinned>) -> Pins {
    pins_from(project, Vec::new(), look)
}

/// [`pins`], with the labels in `todo` too, and every label they reach.
pub fn pins_from(
    project: &Project,
    mut todo: Vec<(Id, String)>,
    mut look: impl FnMut(&Id, &str) -> Result<Pinned>,
) -> Pins {
    let mut out = Pins::new();
    let labels = |p: &Part| -> Vec<(Id, String)> {
        p.order()
            .iter()
            .filter_map(|fid| match p.features.get(fid) {
                Some(Ok(Feature::Instance(Instance {
                    part,
                    rev: Rev::Label(l),
                    ..
                }))) => Some((part.clone(), l.clone())),
                _ => None,
            })
            .collect()
    };
    for p in project.parts.values() {
        todo.extend(labels(p));
    }
    while let Some(at) = todo.pop() {
        if out.contains_key(&at) {
            continue;
        }
        let r = look(&at.0, &at.1).map(Arc::new);
        if let Ok(p) = &r {
            todo.extend(labels(&p.part));
        }
        out.insert(at, r);
    }
    out
}

/// The parts that make up the project's model when nothing narrower is
/// asked for: its one root assembly (one no assembly places) if it has
/// exactly one, else every part that isn't an assembly, as before
/// assemblies.
pub fn shown_parts(project: &Project) -> Vec<Id> {
    let assemblies: Vec<&Id> = project
        .parts
        .iter()
        .filter(|(_, p)| p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly))
        .map(|(id, _)| id)
        .collect();
    let placed: BTreeSet<&Id> = project
        .parts
        .values()
        .flat_map(|p| {
            p.features.values().filter_map(|f| match f {
                Ok(Feature::Instance(i)) => Some(&i.part),
                _ => None,
            })
        })
        .collect();
    let roots: Vec<&Id> = assemblies
        .iter()
        .copied()
        .filter(|a| !placed.contains(a))
        .collect();
    match roots.as_slice() {
        [one] => vec![(*one).clone()],
        _ => project
            .parts
            .keys()
            .filter(|id| !assemblies.contains(id))
            .cloned()
            .collect(),
    }
}

/// How `assembly.move` moves an instance.
pub struct Move {
    pub placement: Option<crate::proto::PlacementArg>,
    pub translate: Option<[serde_json::Value; 3]>,
    pub axis: Option<[f64; 3]>,
    pub angle: Option<serde_json::Value>,
}

/// Where `how` puts instance `inst` (in an assembly whose default unit is
/// `unit` and parameters `env`): a placement first (what it leaves out
/// stays), then a turn of `angle` about `axis` through its origin, then
/// `translate`, in the assembly's frame.
pub fn move_pose(
    inst: &Instance,
    how: Move,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Pose> {
    use crate::units::{Field, Quantity};
    let lookup = |n: &str| env.get(n);
    let mut pose = inst.pose;
    if let Some(arg) = how.placement {
        if let Some(origin) = arg.origin {
            for (i, v) in origin.iter().enumerate() {
                let q = Quantity::from_json_as(v, Field::Length(unit), &lookup)
                    .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
                pose.origin_m[i] = q.meters();
            }
        }
        if let Some(z) = arg.z {
            pose.z = z;
        }
        if let Some(x) = arg.x {
            pose.x = x;
        }
    }
    match (how.axis, how.angle) {
        (None, None) => {}
        (Some(axis), Some(angle)) => {
            let a = Quantity::from_json_as(&angle, Field::Angle, &lookup)
                .map_err(|e| e.with("param", "angle"))?;
            let axis = Vector3::new(axis[0], axis[1], axis[2])
                .normalize()
                .ok_or_else(|| {
                    Error::new(Code::InvalidParam, "the axis has no direction")
                        .with("param", "axis")
                })?;
            let turn = Rigid::rotation(axis, a.radians());
            let v = |a: [f64; 3]| Vector3::new(a[0], a[1], a[2]);
            let z = turn.apply_vector(v(pose.z));
            let x = turn.apply_vector(v(pose.x));
            pose.z = [z.x, z.y, z.z];
            pose.x = [x.x, x.y, x.z];
        }
        _ => {
            return Err(
                Error::new(Code::InvalidParam, "a turn needs both an axis and an angle")
                    .with("param", "angle"),
            );
        }
    }
    if let Some(t) = how.translate {
        for (i, v) in t.iter().enumerate() {
            let q = Quantity::from_json_as(v, Field::Length(unit), &lookup)
                .map_err(|e| e.with("pointer", format!("/translate/{i}")))?;
            pose.origin_m[i] += q.meters();
        }
    }
    // A quarter turn leaves 6.1e-17 where 0 is meant (issue #108).
    let pose = pose.snapped();
    pose_rigid(&pose)?;
    Ok(pose)
}

/// Instance `instance` of assembly `p`, which `assembly.move` and
/// `assembly.solve` may move: `wrong_part_kind` outside an assembly,
/// `reference_wrong_kind` for a feature that isn't an instance,
/// `instance_fixed` for one fixed where it is.
pub fn movable(p: &Part, iid: &Id) -> Result<Instance> {
    if crate::server::part_kind(p) != PartKind::Assembly {
        return Err(Error::new(
            Code::WrongPartKind,
            format!("{} is a part; only an assembly's instances move", p.id),
        )
        .with("part", &p.id));
    }
    let Feature::Instance(inst) = crate::server::feature(p, iid)?.clone() else {
        return Err(
            Error::new(Code::ReferenceWrongKind, format!("{iid} isn't an instance"))
                .with("feature", iid),
        );
    };
    if inst.fixed {
        return Err(Error::new(
            Code::InstanceFixed,
            format!("{iid} is fixed where it is; free it with feature.edit (fixed: false)"),
        )
        .with("feature", iid));
    }
    Ok(inst)
}

/// `assembly.move`: moves instance `instance` of assembly `part` where
/// `how` puts it, and writes the poses its mates then solve to, every
/// instance that moves in one change (see [`crate::mate::after_moves`]).
/// A fixed instance is `instance_fixed`.
pub fn moved(
    session: &mut crate::session::Session,
    part: &str,
    instance: &str,
    how: Move,
    expected: Option<&str>,
) -> Result<serde_json::Value> {
    let pid = crate::server::part_id(part)?;
    let iid = crate::server::feature_id(instance)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    crate::server::check_revision(p, expected)?;
    let inst = movable(p, &iid)?;
    let env = p.env.clone();
    let pose = move_pose(&inst, how, unit, &env)?;
    let solved = crate::mate::after_moves(session, &pid, &[(iid.clone(), pose)])?;
    let writes = solved.writes(session.project().part(&pid)?);
    let list = crate::server::order(session.project().part(&pid)?)?;
    let refs: Vec<(&Id, &Feature)> = writes.iter().map(|(id, f)| (id, f)).collect();
    let revision = crate::server::save(session, &pid, &refs, list)?;
    let pose = match writes.iter().find(|(id, _)| *id == iid) {
        Some((_, Feature::Instance(i))) => i.pose,
        _ => pose,
    };
    let moved: Vec<&Id> = writes.iter().map(|(id, _)| id).collect();
    Ok(
        serde_json::json!({"part": pid, "instance": iid, "pose": pose.to_json(),
                          "moved": moved, "held": solved.held, "revision": revision}),
    )
}

/// `assembly.tree`: assembly `part`'s instances, each with its part,
/// revision and how it resolved, whether it is fixed, its pose in the
/// assembly, what its mates leave it, a subassembly's instances under it,
/// and why it failed; and its instance patterns, each with its copies.
pub fn tree(session: &mut crate::session::Session, part: &str) -> Result<serde_json::Value> {
    let pid = crate::server::part_id(part)?;
    let p = session.project().part(&pid)?.clone();
    if crate::server::part_kind(&p) != PartKind::Assembly {
        return Err(Error::new(
            Code::WrongPartKind,
            format!("{pid} is a part, not an assembly"),
        )
        .with("part", &pid));
    }
    let result = session.rebuilt(&pid)?.clone();
    let instances = nodes(session, &p, &result, &Rigid::IDENTITY, &[])?;
    Ok(serde_json::json!({"part": pid, "revision": p.revision, "instances": instances}))
}

/// The tree under `p`, rebuilt as `result`, placed at `at` with its
/// instances' paths starting with `prefix`.
fn nodes(
    session: &mut crate::session::Session,
    p: &Part,
    result: &PartResult,
    at: &Rigid,
    prefix: &[Id],
) -> Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    for (fid, r) in &result.features {
        // A pattern's copies, each under the pattern with its seed.
        if let Some(Ok(Feature::InstancePattern(_))) = p.features.get(fid) {
            let mut v = serde_json::json!({"id": fid, "type": "instance_pattern",
                                           "ok": r.is_ok()});
            match r {
                Err(e) => v["error"] = serde_json::json!(e),
                Ok(o) => {
                    let copies: Vec<serde_json::Value> = o
                        .copies()
                        .map(|c| c.copies.as_slice())
                        .unwrap_or_default()
                        .iter()
                        .map(|c| {
                            let mut path = prefix.to_vec();
                            path.extend([Id::copy(fid, c.index), c.seed.clone()]);
                            serde_json::json!({
                                "path": path_text(&path),
                                "seed": c.seed,
                                "part": c.placed.part,
                                "rev": c.placed.rev.to_json(),
                                "pose": rigid_pose(&at.then_after(&c.placed.pose)).to_json(),
                            })
                        })
                        .collect();
                    v["copies"] = serde_json::json!(copies);
                }
            }
            out.push(v);
            continue;
        }
        let Some(Ok(Feature::Instance(inst))) = p.features.get(fid) else {
            continue;
        };
        let mut path = prefix.to_vec();
        path.push(fid.clone());
        let mut v = serde_json::json!({"id": fid, "path": path_text(&path), "part": inst.part,
                                       "rev": inst.rev.to_json(), "ok": r.is_ok()});
        if inst.fixed {
            v["fixed"] = serde_json::json!(true);
        }
        // What its assembly's mates leave it.
        if let Some(b) = result.solve.as_ref().and_then(|s| s.instance(fid)) {
            v["solve"] = crate::mate::body_json(b);
        }
        match r {
            Err(e) => v["error"] = serde_json::json!(e),
            Ok(o) => {
                let placed = o.instance().expect("an instance places");
                let here = at.then_after(&placed.pose);
                v["pose"] = rigid_pose(&here).to_json();
                if let Some(pin) = &placed.model.pin {
                    v["pin"] = pin.to_json();
                }
                // A subassembly's own instances, as its revision has them.
                let child = match &inst.rev {
                    Rev::Working => session.project().part(&inst.part).ok().cloned(),
                    Rev::Label(l) => crate::release::pinned(session, &inst.part, l)
                        .ok()
                        .map(|x| x.part),
                };
                if let Some(c) =
                    child.filter(|c| c.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly))
                {
                    let r = match &inst.rev {
                        Rev::Working => session.rebuilt(&inst.part)?.clone(),
                        Rev::Label(l) => {
                            match session.store.pinned.get(&(inst.part.clone(), l.clone())) {
                                Some((_, r)) => r.clone(),
                                None => continue,
                            }
                        }
                    };
                    v["kind"] = serde_json::json!("assembly");
                    v["instances"] = serde_json::json!(nodes(session, &c, &r, &here, &path)?);
                }
            }
        }
        out.push(v);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::files::PartFile;
    use crate::doc::project::{feature_path, part_path, write_json};
    use crate::server::tests::{call, fresh};
    use crate::session::Session;
    use serde_json::{Value, json};
    use std::path::Path;

    #[test]
    fn an_assemblys_parts_build_side_by_side_to_the_same_results() {
        let (dir, mut s) = fresh();
        for (p, x) in [("a", 1), ("b", 2), ("c", 3)] {
            call(&mut s, "part.create", json!({"part": p})).unwrap();
            call(
                &mut s,
                "feature.add",
                json!({"part": p, "type": "box", "id": "x", "params": {"x": x, "y": 2, "z": 3}}),
            )
            .unwrap();
        }
        call(
            &mut s,
            "part.create",
            json!({"part": "asm", "kind": "assembly"}),
        )
        .unwrap();
        for p in ["a", "b", "c"] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "asm", "type": "instance", "id": format!("i{p}"), "of": p}),
            )
            .unwrap();
        }
        // From nothing, on one thread and on four: the assembly builds its
        // parts first, each once, to the same results.
        let built = |threads| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let mut t = Session::open(dir.path()).unwrap();
            let asm = Id::parse("asm", "part").unwrap();
            let hash = pool.install(|| t.rebuilt(&asm).unwrap().hash.clone());
            let parts: Vec<String> = ["a", "b", "c"]
                .iter()
                .map(|p| {
                    let r = t.rebuilt(&Id::parse(p, "part").unwrap()).unwrap();
                    assert_eq!(r.built.len(), 1, "{p} built once");
                    r.hash.clone()
                })
                .collect();
            (hash, parts)
        };
        assert_eq!(built(1), built(4));
    }

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// Writes instance `inst` of `part` (at `rev`) into assembly `asm`,
    /// at `origin` in meters, listed last.
    pub(crate) fn place_in(
        root: &Path,
        asm: &str,
        inst: &str,
        part: &str,
        rev: Value,
        origin: [f64; 3],
    ) {
        let v = json!({"format": 4, "kind": "ryucad.feature", "type": "instance",
                       "part": part, "rev": rev,
                       "pose": {"origin_m": origin, "z": [0, 0, 1], "x": [1, 0, 0]}});
        write_json(&feature_path(root, &id(asm), &id(inst)), &v).unwrap();
        let p = part_path(root, &id(asm));
        let mut list = PartFile::from_json(
            &crate::doc::canonical::parse(&std::fs::read_to_string(&p).unwrap()).unwrap(),
        )
        .unwrap();
        list.features.push(id(inst));
        write_json(&p, &list.to_json()).unwrap();
    }

    /// A project with parts `bolt` (a cylinder) and `plate` (a box), and
    /// an empty assembly `frame`.
    pub(crate) fn project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "bolt"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "bolt", "type": "cylinder", "id": "shank",
                   "params": {"radius": 3, "height": 20}}),
        )
        .unwrap();
        call(&mut s, "part.create", json!({"part": "plate"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "plate", "type": "box", "id": "slab",
                   "params": {"x": 100, "y": 50, "z": 5}}),
        )
        .unwrap();
        call(
            &mut s,
            "part.create",
            json!({"part": "frame", "kind": "assembly"}),
        )
        .unwrap();
        (dir, s)
    }

    fn origin(o: &Occurrence) -> [f64; 3] {
        let t = o.pose.translation_part();
        [t.x, t.y, t.z]
    }

    fn paths(m: &Model) -> Vec<String> {
        m.occurrences
            .iter()
            .map(|o| {
                let p: Vec<&str> = o.path.iter().map(Id::as_str).collect();
                format!("{} {}", p.join("/"), o.part)
            })
            .collect()
    }

    #[test]
    fn an_assembly_places_its_parts() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(
            r,
            "frame",
            "base",
            "plate",
            json!("working"),
            [0.0, 0.0, 0.0],
        );
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.02, 0.005],
        );
        place_in(
            r,
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.09, 0.02, 0.005],
        );
        s.refresh().unwrap();
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        assert_eq!(paths(&m), ["base plate", "b1 bolt", "b2 bolt"]);
        assert_eq!(origin(&m.occurrences[1]), [0.01, 0.02, 0.005]);
        assert_eq!(origin(&m.occurrences[2]), [0.09, 0.02, 0.005]);
        // Both bolts place the one body.
        let bolt = s
            .rebuilt(&id("bolt"))
            .unwrap()
            .output(&id("shank"))
            .unwrap()
            .hash()
            .to_owned();
        assert_eq!(m.occurrences[1].solids[0].built.hash, bolt);
        assert!(Arc::ptr_eq(
            &m.occurrences[1].solids[0].built,
            &m.occurrences[2].solids[0].built
        ));
        let result = s.rebuilt(&id("frame")).unwrap();
        assert_eq!(result.failed(), 0);
        let placed = result.output(&id("b1")).unwrap().instance().unwrap();
        assert_eq!(placed.part, id("bolt"));
    }

    #[test]
    fn subassemblies_nest_and_compose_their_poses() {
        let (dir, mut s) = project();
        let r = dir.path();
        call(
            &mut s,
            "part.create",
            json!({"part": "pair", "kind": "assembly"}),
        )
        .unwrap();
        place_in(
            r,
            "pair",
            "left",
            "bolt",
            json!("working"),
            [-0.01, 0.0, 0.0],
        );
        place_in(
            r,
            "pair",
            "right",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.0],
        );
        place_in(
            r,
            "frame",
            "base",
            "plate",
            json!("working"),
            [0.0, 0.0, 0.0],
        );
        place_in(
            r,
            "frame",
            "p1",
            "pair",
            json!("working"),
            [0.05, 0.025, 0.005],
        );
        s.refresh().unwrap();
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        assert_eq!(paths(&m), ["base plate", "p1/left bolt", "p1/right bolt"]);
        assert_eq!(origin(&m.occurrences[1]), [0.04, 0.025, 0.005]);
        assert_eq!(
            origin(&m.occurrences[2]),
            [0.060000000000000005, 0.025, 0.005]
        );
    }

    #[test]
    fn a_cycle_fails_its_instances_whichever_is_asked_first() {
        let build = |first: &str| {
            let (dir, mut s) = project();
            let r = dir.path();
            call(
                &mut s,
                "part.create",
                json!({"part": "sub", "kind": "assembly"}),
            )
            .unwrap();
            place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
            place_in(r, "frame", "s1", "sub", json!("working"), [0.0; 3]);
            place_in(r, "sub", "f1", "frame", json!("working"), [0.0; 3]);
            place_in(r, "sub", "b1", "bolt", json!("working"), [0.0; 3]);
            s.refresh().unwrap();
            s.rebuilt(&id(first)).unwrap();
            let mut out = Vec::new();
            for (asm, inst) in [
                ("frame", "s1"),
                ("sub", "f1"),
                ("frame", "base"),
                ("sub", "b1"),
            ] {
                let result = s.rebuilt(&id(asm)).unwrap();
                let r = &result
                    .features
                    .iter()
                    .find(|(f, _)| *f == id(inst))
                    .unwrap()
                    .1;
                out.push((
                    inst,
                    r.as_ref()
                        .err()
                        .map(|e| (e.code, e.details["path"].clone())),
                ));
            }
            (out, s.rebuilt(&id("frame")).unwrap().hash.clone())
        };
        let (a, ha) = build("frame");
        let (b, hb) = build("sub");
        assert_eq!(a, b);
        assert_eq!(ha, hb);
        assert_eq!(
            a[0],
            (
                "s1",
                Some((Code::AssemblyCycle, json!(["frame", "sub", "frame"])))
            )
        );
        assert_eq!(a[1].1.as_ref().unwrap().0, Code::AssemblyCycle);
        assert_eq!(a[2], ("base", None));
        assert_eq!(a[3], ("b1", None));
    }

    #[test]
    fn parts_and_assemblies_hold_their_own_features() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "bolt", "oops", "plate", json!("working"), [0.0; 3]);
        let boxed = json!({"format": 1, "kind": "ryucad.feature", "type": "box",
                           "params": {"x": "1 mm", "y": "1 mm", "z": "1 mm"},
                           "placement": {"origin": ["0 mm", "0 mm", "0 mm"],
                                         "x": [1, 0, 0], "z": [0, 0, 1]}});
        write_json(&feature_path(r, &id("frame"), &id("cube")), &boxed).unwrap();
        write_json(
            &part_path(r, &id("frame")),
            &PartFile {
                kind: PartKind::Assembly,
                features: vec![id("cube")],
            }
            .to_json(),
        )
        .unwrap();
        s.refresh().unwrap();
        let e = s.rebuilt(&id("bolt")).unwrap().features[1]
            .1
            .clone()
            .unwrap_err();
        assert_eq!(
            (e.code, e.details["kind"].clone()),
            (Code::WrongPartKind, json!("part"))
        );
        let e = s.rebuilt(&id("frame")).unwrap().features[0]
            .1
            .clone()
            .unwrap_err();
        assert_eq!(
            (e.code, e.details["type"].clone()),
            (Code::WrongPartKind, json!("box"))
        );
    }

    #[test]
    fn a_missing_part_fails_its_instance() {
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "ghost",
            "nothing",
            json!("working"),
            [0.0; 3],
        );
        s.refresh().unwrap();
        let e = s.rebuilt(&id("frame")).unwrap().features[0]
            .1
            .clone()
            .unwrap_err();
        assert_eq!(
            (e.code, e.details["feature"].clone()),
            (Code::NotFound, json!("ghost"))
        );
    }

    #[test]
    fn an_edited_part_rebuilds_the_assemblies_over_it() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(r, "frame", "b1", "bolt", json!("working"), [0.01, 0.0, 0.0]);
        s.refresh().unwrap();
        let before = s.rebuilt(&id("frame")).unwrap().clone();
        assert_eq!(before.built, [id("base"), id("b1")]);
        // Nothing changed: the session answers from what it built.
        assert_eq!(s.rebuilt(&id("frame")).unwrap().hash, before.hash);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "bolt", "id": "shank", "params": {"radius": 4}}),
        )
        .unwrap();
        let after = s.rebuilt(&id("frame")).unwrap().clone();
        assert_ne!(after.hash, before.hash);
        // Only the instance of the bolt is placed again.
        assert_eq!(after.built, [id("b1")]);
    }

    /// [`project`] in a repository on `main`, the bolt numbered, released
    /// as A and committed, then made thicker: A and the working bolt
    /// differ.
    pub(crate) fn released() -> (tempfile::TempDir, Session) {
        use crate::git::tests::git;
        let (dir, mut s) = project();
        let r = dir.path();
        git(r, &["init", "-q", "-b", "main"]);
        call(
            &mut s,
            "props.set",
            json!({"part": "bolt", "properties": {"number": "100"}}),
        )
        .unwrap();
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "bolt A"]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "bolt", "id": "shank", "params": {"radius": 4}}),
        )
        .unwrap();
        (dir, s)
    }

    /// Writes release `label` of `part`'s record as its working files
    /// are, past the release checks.
    fn write_record(s: &mut Session, part: &str, label: &str) {
        let content = ryupdm::content::hash(&*s, &id(part)).unwrap();
        let project = s.project().manifest.project.clone().unwrap();
        let record = ryupdm::record::Record::Release(ryupdm::record::Release {
            project,
            part: id(part),
            label: label.to_owned(),
            content,
            properties: Default::default(),
            results: Default::default(),
            built_with: ryupdm::record::BuiltWith {
                ryucad: "0.0.0".into(),
                ryukei: "0.0.0".into(),
                commit: "unknown".into(),
            },
            released_by: ryupdm::record::ReleasedBy {
                name: None,
                email: None,
                participant: "test".into(),
            },
            supersedes: None,
            note: None,
        });
        let at = s.root().join(record.path());
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        write_json(&at, &record.to_json()).unwrap();
        s.refresh().unwrap();
    }

    fn feature_error(s: &mut Session, asm: &str, inst: &str) -> Error {
        let result = s.rebuilt(&id(asm)).unwrap();
        let (_, r) = result
            .features
            .iter()
            .find(|(f, _)| *f == id(inst))
            .unwrap();
        r.clone().unwrap_err()
    }

    #[test]
    fn a_label_places_the_release_as_it_became_official() {
        let (dir, mut s) = released();
        let r = dir.path();
        place_in(r, "frame", "old", "bolt", json!({"label": "A"}), [0.0; 3]);
        place_in(
            r,
            "frame",
            "new",
            "bolt",
            json!("working"),
            [0.02, 0.0, 0.0],
        );
        s.refresh().unwrap();
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        let (old, new) = (&m.occurrences[0], &m.occurrences[1]);
        assert_eq!(old.rev, Rev::Label("A".into()));
        assert_ne!(old.solids[0].built.hash, new.solids[0].built.hash);
        let result = s.rebuilt(&id("frame")).unwrap();
        let placed = result.output(&id("old")).unwrap().instance().unwrap();
        let commit = crate::git::repo(r).unwrap().locate("HEAD").unwrap().0;
        assert_eq!(
            placed.model.pin,
            Some(Pin {
                commit: Some(commit),
                obsolete: false
            })
        );
        assert!(result.warnings(&id("frame")).is_empty());
        // A's body is the bolt's as it was released.
        let a = crate::release::pinned(&mut s, &id("bolt"), "A").unwrap();
        assert_eq!(a.part.revision.len(), 32);
        assert_eq!(
            crate::rebuild::rebuild_part(&a.part)
                .output(&id("shank"))
                .unwrap()
                .hash(),
            old.solids[0].built.hash
        );
    }

    #[test]
    fn a_proposal_places_the_working_files_while_it_describes_them() {
        let (dir, mut s) = released();
        let r = dir.path();
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        place_in(r, "frame", "b", "bolt", json!({"label": "B"}), [0.0; 3]);
        s.refresh().unwrap();
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        let working = s
            .rebuilt(&id("bolt"))
            .unwrap()
            .output(&id("shank"))
            .unwrap()
            .hash()
            .to_owned();
        assert_eq!(m.occurrences[0].solids[0].built.hash, working);
        let placed = s
            .rebuilt(&id("frame"))
            .unwrap()
            .output(&id("b"))
            .unwrap()
            .instance()
            .unwrap()
            .clone();
        assert_eq!(placed.model.pin.as_ref().unwrap().commit, None);
        // Changed since: B no longer says what the bolt is.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "bolt", "id": "shank", "params": {"radius": 5}}),
        )
        .unwrap();
        let e = feature_error(&mut s, "frame", "b");
        assert_eq!(
            (e.code, e.details["label"].clone()),
            (Code::LabelNotOfficial, json!("B"))
        );
    }

    #[test]
    fn a_label_follows_the_release_branch_as_it_moves() {
        use crate::git::tests::git;
        let (dir, mut s) = released();
        let r = dir.path();
        // B proposed and placed, in one session, as a daemon keeps it.
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        place_in(r, "frame", "b", "bolt", json!({"label": "B"}), [0.0; 3]);
        s.refresh().unwrap();
        let pin = |s: &mut Session| -> (Option<Pin>, Vec<Value>) {
            let result = s.rebuilt(&id("frame")).unwrap().clone();
            let placed = result.output(&id("b")).unwrap().instance().unwrap();
            (placed.model.pin.clone(), result.warnings(&id("frame")))
        };
        assert_eq!(
            pin(&mut s).0,
            Some(Pin {
                commit: None,
                obsolete: false
            })
        );
        // Committed on the release branch: official from that commit,
        // though no file the session reads changed (issue #105).
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "bolt B"]);
        let commit = crate::git::repo(r).unwrap().locate("HEAD").unwrap().0;
        s.refresh().unwrap();
        assert_eq!(
            pin(&mut s).0,
            Some(Pin {
                commit: Some(commit.clone()),
                obsolete: false
            })
        );
        // Made obsolete and committed: the warning comes.
        call(
            &mut s,
            "release.obsolete",
            json!({"part": "bolt", "label": "B"}),
        )
        .unwrap();
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "B is obsolete"]);
        s.refresh().unwrap();
        let (p, w) = pin(&mut s);
        assert_eq!(
            p,
            Some(Pin {
                commit: Some(commit),
                obsolete: true
            })
        );
        assert_eq!(w[0]["code"], "instance_obsolete", "{w:?}");
    }

    #[test]
    fn a_label_without_a_record_fails_its_instance() {
        let (dir, mut s) = released();
        place_in(
            dir.path(),
            "frame",
            "z",
            "bolt",
            json!({"label": "Z"}),
            [0.0; 3],
        );
        s.refresh().unwrap();
        let e = feature_error(&mut s, "frame", "z");
        assert_eq!(
            (e.code, e.details["feature"].clone()),
            (Code::LabelNotFound, json!("z"))
        );
    }

    #[test]
    fn an_obsolete_label_still_places_with_a_warning() {
        use crate::git::tests::git;
        let (dir, mut s) = released();
        let r = dir.path();
        call(
            &mut s,
            "release.obsolete",
            json!({"part": "bolt", "label": "A"}),
        )
        .unwrap();
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "A is obsolete"]);
        place_in(r, "frame", "old", "bolt", json!({"label": "A"}), [0.0; 3]);
        s.refresh().unwrap();
        let result = s.rebuilt(&id("frame")).unwrap();
        assert_eq!(result.failed(), 0);
        let w = result.warnings(&id("frame"));
        assert_eq!(w.len(), 1);
        // check says so, as a warning: it still passes.
        let v = call(&mut s, "check", json!({"part": "frame"})).unwrap();
        let f: Vec<&Value> = v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["code"] == "instance_obsolete")
            .collect();
        assert_eq!((f.len(), &f[0]["severity"]), (1, &json!("warning")));
        let result = s.rebuilt(&id("frame")).unwrap();
        let w = result.warnings(&id("frame"));
        assert_eq!(
            (&w[0]["code"], &w[0]["feature"]),
            (&json!("instance_obsolete"), &json!("old"))
        );
    }

    #[test]
    fn a_release_places_only_releases() {
        use crate::git::tests::git;
        let (dir, mut s) = released();
        let r = dir.path();
        // A pair that places the bolt as it is, released anyway.
        call(
            &mut s,
            "part.create",
            json!({"part": "pair", "kind": "assembly"}),
        )
        .unwrap();
        place_in(r, "pair", "one", "bolt", json!("working"), [0.0; 3]);
        place_in(
            r,
            "pair",
            "two",
            "bolt",
            json!({"label": "A"}),
            [0.01, 0.0, 0.0],
        );
        s.refresh().unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "pair", "properties": {"number": "200"}}),
        )
        .unwrap();
        // The release rule refuses it now; a record written anyway (an
        // older build, or by hand) still reads as releases are read.
        let e = call(&mut s, "release.propose", json!({"part": "pair"})).unwrap_err();
        assert_eq!(e.code, Code::ReleaseNotReady);
        let problems = e.details["problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            (&problems[0]["code"], &problems[0]["details"]["instance"]),
            (&json!("instance_not_released"), &json!("one"))
        );
        write_record(&mut s, "pair", "A");
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "pair A"]);
        place_in(r, "frame", "p", "pair", json!({"label": "A"}), [0.0; 3]);
        s.refresh().unwrap();
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        // Only the released bolt is placed; the working one fails in the
        // pair as released.
        assert_eq!(paths(&m), ["p/two bolt"]);
        let pinned = s
            .store
            .pinned
            .get(&(id("pair"), "A".to_owned()))
            .unwrap()
            .1
            .clone();
        let e = pinned.features[0].1.clone().unwrap_err();
        assert_eq!(e.code, Code::InstanceNotReleased);
    }

    #[test]
    fn a_rebuild_reports_an_assemblys_instances_and_occurrences() {
        let (dir, mut s) = released();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!({"label": "A"}),
            [0.01, 0.02, 0.005],
        );
        s.refresh().unwrap();
        let v = call(&mut s, "rebuild", json!({"part": "frame"})).unwrap();
        let part = &v["parts"][0];
        assert_eq!(part["kind"], "assembly");
        assert_eq!(part["solids"], json!([]));
        let b1 = &part["features"][1];
        assert_eq!(
            (&b1["id"], &b1["type"], &b1["ok"]),
            (&json!("b1"), &json!("instance"), &json!(true))
        );
        assert_eq!(b1["instance"]["part"], "bolt");
        assert_eq!(b1["instance"]["rev"], json!({"label": "A"}));
        assert_eq!(b1["instance"]["pin"]["official"], true);
        assert_eq!(
            b1["instance"]["pose"]["origin_m"],
            json!([0.01, 0.02, 0.005])
        );
        assert_eq!(b1["instance"]["occurrences"], 1);
        let occ = part["occurrences"].as_array().unwrap();
        assert_eq!(occ.len(), 2);
        assert_eq!(
            (&occ[1]["path"], &occ[1]["part"]),
            (&json!("b1"), &json!("bolt"))
        );
        assert_eq!(occ[1]["solids"][0]["solid"], "shank");
        assert_eq!(
            occ[1]["pose"],
            json!({"origin_m": [0.01, 0.02, 0.005], "z": [0, 0, 1], "x": [1, 0, 0]})
        );
        // A part's report is as it was.
        let v = call(&mut s, "rebuild", json!({"part": "bolt"})).unwrap();
        assert!(v["parts"][0].get("kind").is_none());
        assert!(v["parts"][0].get("occurrences").is_none());
        // A failing instance fails the rebuild, with every status.
        place_in(r, "frame", "z", "bolt", json!({"label": "Z"}), [0.0; 3]);
        s.refresh().unwrap();
        let e = call(&mut s, "rebuild", json!({"part": "frame"})).unwrap_err();
        assert_eq!(e.code, Code::RebuildFailed);
        assert_eq!(
            e.details["parts"][0]["features"][2]["error"]["code"],
            "label_not_found"
        );
    }

    #[test]
    fn subscribers_hear_an_assembly_change_with_its_parts() {
        use crate::live::Live;
        use crate::notify::Topic;
        use crate::server::tests::{call_live, drain};
        let (dir, s) = project();
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.0; 3],
        );
        let mut live = Live::new();
        let mut slot = Some(s);
        crate::server::settle(&mut slot, &mut live, dir.path());
        let sub = live.hub.subscribe("p-x", &[Topic::Model]);
        let mut s = slot.take().unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.edit",
            json!({"part": "bolt", "id": "shank", "params": {"radius": 4}}),
        )
        .unwrap();
        live.hub.send(Topic::Model, "marker", &json!({}));
        let got = drain(&sub);
        assert_eq!(got.len(), 1);
        let parts: Vec<&Value> = got[0].1["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| &p["part"])
            .collect();
        assert_eq!(parts, [&json!("bolt"), &json!("frame")]);
    }

    #[test]
    fn instances_are_added_and_edited_through_the_protocol() {
        let (dir, mut s) = project();
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "b1", "of": "bolt",
                   "placement": {"origin": ["10 mm", "2 cm", 0], "z": [0, 0, 1]}}),
        )
        .unwrap();
        assert_eq!(v["feature"]["part"], "bolt");
        let text =
            std::fs::read_to_string(dir.path().join("parts/frame/features/b1.json")).unwrap();
        let file = crate::doc::canonical::parse(&text).unwrap();
        assert_eq!(file["rev"], "working");
        assert_eq!(file["pose"]["origin_m"], json!([0.01, 0.02, 0]));
        assert_eq!(file["format"], 4);
        // An ID is generated from the type.
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "of": "plate", "fixed": true}),
        )
        .unwrap();
        let generated = v["feature"].clone();
        assert_eq!(generated["fixed"], true);
        let list = call(&mut s, "feature.list", json!({"part": "frame"})).unwrap();
        assert_eq!(list["features"][1]["id"], "instance-1");
        // An edit lays what is given over what is there.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "b1", "placement": {"origin": [0, 0, "5 mm"]},
                   "fixed": true}),
        )
        .unwrap();
        let shown = call(&mut s, "feature.show", json!({"part": "frame", "id": "b1"})).unwrap();
        assert_eq!(shown["pose"]["origin_m"], json!([0, 0, 0.005]));
        assert_eq!(shown["fixed"], true);
        assert_eq!(shown["part"], "bolt");
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        assert_eq!(origin(&m.occurrences[0]), [0.0, 0.0, 0.005]);
    }

    #[test]
    fn instance_writes_refuse_what_cant_build() {
        let (_dir, mut s) = project();
        let add = |s: &mut Session, part: &str, fields: Value| {
            let mut v = json!({"part": part, "type": "instance"});
            for (k, x) in fields.as_object().unwrap() {
                v[k] = x.clone();
            }
            call(s, "feature.add", v)
        };
        let code = |r: Result<Value>| r.unwrap_err().code;
        assert_eq!(
            code(add(&mut s, "bolt", json!({"of": "plate"}))),
            Code::WrongPartKind
        );
        assert_eq!(
            code(call(
                &mut s,
                "feature.add",
                json!({"part": "frame", "type": "box", "params": {"x": 1, "y": 1, "z": 1}}),
            )),
            Code::WrongPartKind
        );
        // The part's kind is judged before the feature's own fields (#94).
        for (part, ty) in [("frame", "box"), ("frame", "sketch"), ("bolt", "mate")] {
            let e = call(&mut s, "feature.add", json!({"part": part, "type": ty})).unwrap_err();
            assert_eq!(e.code, Code::WrongPartKind, "{ty} in {part}: {e:?}");
        }
        assert_eq!(code(add(&mut s, "frame", json!({}))), Code::InvalidParam);
        assert_eq!(
            code(add(&mut s, "frame", json!({"of": "nothing"}))),
            Code::NotFound
        );
        assert_eq!(
            code(add(&mut s, "frame", json!({"of": "frame"}))),
            Code::AssemblyCycle
        );
        assert_eq!(
            code(add(&mut s, "frame", json!({"of": "bolt", "rev": "Z"}))),
            Code::LabelNotFound
        );
        assert_eq!(
            code(add(&mut s, "frame", json!({"of": "bolt", "rev": "b"}))),
            Code::InvalidDocument
        );
        assert_eq!(
            code(add(
                &mut s,
                "frame",
                json!({"of": "bolt", "placement": {"z": [0, 0, 0]}})
            )),
            Code::InvalidPlacement
        );
        assert_eq!(
            code(add(
                &mut s,
                "frame",
                json!({"of": "bolt", "placement": {"origin": ["1 deg", 0, 0]}})
            )),
            Code::UnitMismatch
        );
        assert_eq!(
            code(add(&mut s, "frame", json!({"of": "bolt", "radius": 2}))),
            Code::InvalidParam
        );
        // A pair placed in the frame can't place the frame.
        call(
            &mut s,
            "part.create",
            json!({"part": "pair", "kind": "assembly"}),
        )
        .unwrap();
        add(&mut s, "frame", json!({"of": "pair", "id": "p1"})).unwrap();
        let e = add(&mut s, "pair", json!({"of": "frame"})).unwrap_err();
        assert_eq!(e.code, Code::AssemblyCycle);
        assert_eq!(e.details["path"], json!(["pair", "frame", "pair"]));
        // Nothing was written by the refusals.
        let list = call(&mut s, "feature.list", json!({"part": "frame"})).unwrap();
        assert_eq!(list["features"].as_array().unwrap().len(), 1);
        // A placed part can't be deleted; an unplaced one can.
        let e = call(&mut s, "part.delete", json!({"part": "pair"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["instances"].clone()),
            (Code::HasDependents, json!(["frame/p1"]))
        );
        call(&mut s, "part.delete", json!({"part": "plate"})).unwrap();
    }

    #[test]
    fn instances_move_and_moves_undo() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (_dir, mut s) = project();
        call(
            &mut s,
            "feature.add",
            json!({"part": "frame", "type": "instance", "id": "b1", "of": "bolt"}),
        )
        .unwrap();
        let mut live = Live::new();
        let v = call_live(
            &mut s,
            &mut live,
            None,
            "assembly.move",
            json!({"part": "frame", "instance": "b1", "placement": {"origin": ["10 mm", 0, 0]},
                   "translate": [0, "5 mm", 0]}),
        )
        .unwrap();
        assert_eq!(v["pose"]["origin_m"], json!([0.01, 0.005, 0]));
        // A turn about its own origin, in the assembly's frame.
        let v = call_live(
            &mut s,
            &mut live,
            None,
            "assembly.move",
            json!({"part": "frame", "instance": "b1", "axis": [0, 0, 1], "angle": "90 deg"}),
        )
        .unwrap();
        assert_eq!(v["pose"]["origin_m"], json!([0.01, 0.005, 0]));
        // Exactly, not 6.1e-17 off (issue #108).
        assert_eq!(v["pose"]["x"], json!([0, 1, 0]));
        let shown = call(&mut s, "feature.show", json!({"part": "frame", "id": "b1"})).unwrap();
        assert_eq!(shown["pose"]["x"], json!([0, 1, 0]));
        let m = s.model(&id("frame"), &Rev::Working).unwrap();
        assert_eq!(origin(&m.occurrences[0]), [0.01, 0.005, 0.0]);
        // Undone, it is back where the translation left it.
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        let shown = call(&mut s, "feature.show", json!({"part": "frame", "id": "b1"})).unwrap();
        assert_eq!(shown["pose"]["x"], json!([1, 0, 0]));
        // Refusals.
        let mv = |s: &mut Session, v: Value| call(s, "assembly.move", v).unwrap_err().code;
        assert_eq!(
            mv(
                &mut s,
                json!({"part": "frame", "instance": "b1", "axis": [0, 0, 1]})
            ),
            Code::InvalidParam
        );
        assert_eq!(
            mv(
                &mut s,
                json!({"part": "frame", "instance": "b1", "expected_revision": "x"})
            ),
            Code::Conflict
        );
        assert_eq!(
            mv(&mut s, json!({"part": "bolt", "instance": "shank"})),
            Code::WrongPartKind
        );
        assert_eq!(
            mv(&mut s, json!({"part": "frame", "instance": "nope"})),
            Code::NotFound
        );
        call(
            &mut s,
            "feature.edit",
            json!({"part": "frame", "id": "b1", "fixed": true}),
        )
        .unwrap();
        assert_eq!(
            mv(
                &mut s,
                json!({"part": "frame", "instance": "b1", "translate": [1, 0, 0]})
            ),
            Code::InstanceFixed
        );
    }

    #[test]
    fn the_tree_nests_subassemblies_and_says_what_failed() {
        let (dir, mut s) = released();
        let r = dir.path();
        call(
            &mut s,
            "part.create",
            json!({"part": "pair", "kind": "assembly"}),
        )
        .unwrap();
        place_in(
            r,
            "pair",
            "left",
            "bolt",
            json!({"label": "A"}),
            [-0.01, 0.0, 0.0],
        );
        place_in(
            r,
            "pair",
            "right",
            "bolt",
            json!("working"),
            [0.01, 0.0, 0.0],
        );
        place_in(r, "frame", "p1", "pair", json!("working"), [0.05, 0.0, 0.0]);
        place_in(r, "frame", "z", "bolt", json!({"label": "Z"}), [0.0; 3]);
        s.refresh().unwrap();
        let t = call(&mut s, "assembly.tree", json!({"part": "frame"})).unwrap();
        let p1 = &t["instances"][0];
        assert_eq!(
            (&p1["kind"], &p1["path"]),
            (&json!("assembly"), &json!("p1"))
        );
        let left = &p1["instances"][0];
        assert_eq!(left["path"], "p1/left");
        assert_eq!(left["pin"]["official"], true);
        assert_eq!(left["pose"]["origin_m"], json!([0.04, 0, 0]));
        assert!(left.get("instances").is_none());
        let z = &t["instances"][1];
        assert_eq!(
            (&z["ok"], &z["error"]["code"]),
            (&json!(false), &json!("label_not_found"))
        );
        let e = call(&mut s, "assembly.tree", json!({"part": "bolt"})).unwrap_err();
        assert_eq!(e.code, Code::WrongPartKind);
    }

    #[test]
    fn meshes_renders_and_inspect_show_occurrences() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.01, 0.02, 0.005],
        );
        place_in(
            r,
            "frame",
            "b2",
            "bolt",
            json!("working"),
            [0.09, 0.02, 0.005],
        );
        s.refresh().unwrap();
        // Each body once, and where each occurrence puts it.
        let m = call(
            &mut s,
            "mesh.get",
            json!({"part": "frame", "quality": "coarse"}),
        )
        .unwrap();
        let labels: Vec<&Value> = m["bodies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| &b["feature"])
            .collect();
        assert_eq!(labels, [&json!("base/slab"), &json!("b1/shank")]);
        assert_eq!(m["occurrences"].as_array().unwrap().len(), 3);
        assert_eq!(
            m["occurrences"][2]["pose"]["origin_m"],
            json!([0.09, 0.02, 0.005])
        );
        let one = call(
            &mut s,
            "mesh.get",
            json!({"part": "frame", "feature": "b2"}),
        )
        .unwrap();
        assert_eq!(one["bodies"][0]["feature"], "b2/shank");
        assert_eq!(one["occurrences"].as_array().unwrap().len(), 1);
        let e = call(
            &mut s,
            "mesh.get",
            json!({"part": "frame", "feature": "b9"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // The same as bytes.
        let req = crate::proto::Request::from_rpc(
            "mesh.get",
            Some(json!({"part": "frame", "transfer": "binary", "quality": "coarse"})),
        )
        .unwrap();
        let a = crate::server::handle_safely(&mut Some(s), None, None, r, req).unwrap();
        let wire = crate::mesh::decode(&a.binary.unwrap()).unwrap();
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[1].feature, "b1/shank");
        assert_eq!(a.result["occurrences"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn an_assembly_renders_and_is_the_model_when_it_is_the_one_root() {
        use crate::server::tests::render_pixels;
        let (dir, s) = project();
        let r = dir.path();
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(
            r,
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.05, 0.025, 0.005],
        );
        drop(s);
        let open = || Session::open(r).unwrap();
        let s = open();
        assert_eq!(shown_parts(s.project()), [id("frame")]);
        let small = json!({"width": 64, "height": 48});
        let mut asked = small.clone();
        asked["part"] = json!("frame");
        let frame = render_pixels(r, s, asked);
        let model = render_pixels(r, open(), small.clone());
        assert_eq!(frame, model);
        let mut plate = small;
        plate["part"] = json!("plate");
        assert_ne!(frame, render_pixels(r, open(), plate));
        // Framing the whole model frames the assembly.
        let mut s = open();
        let pts = crate::server::model_points(&mut s, &[]).unwrap();
        assert!(
            pts.iter().any(|p| p[2] > 0.02),
            "the bolt stands on the plate"
        );
        // With two roots, the model is every part, as before assemblies.
        call(
            &mut s,
            "part.create",
            json!({"part": "other", "kind": "assembly"}),
        )
        .unwrap();
        assert_eq!(shown_parts(s.project()), [id("bolt"), id("plate")]);
    }

    #[test]
    fn inspect_lists_occurrences_and_finds_their_entities() {
        let (dir, mut s) = project();
        let r = dir.path();
        place_in(r, "frame", "b1", "bolt", json!("working"), [0.01, 0.0, 0.0]);
        s.refresh().unwrap();
        let v = call(&mut s, "inspect", json!({"part": "frame"})).unwrap();
        let occ = &v["occurrences"][0];
        assert_eq!((&occ["path"], &occ["part"]), (&json!("b1"), &json!("bolt")));
        let face = occ["bodies"][0]["faces"][0]["name"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(occ["bodies"][0]["solid"], "shank");
        let v = call(
            &mut s,
            "inspect",
            json!({"part": "frame", "entity": format!("frame/b1/shank/{face}")}),
        )
        .unwrap();
        assert_eq!(
            (&v["kind"], &v["occurrence"]),
            (&json!("face"), &json!("b1"))
        );
        // In an assembly an entity is always named in full.
        let e = call(&mut s, "inspect", json!({"part": "frame", "entity": face})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn a_focus_says_what_a_window_shows() {
        use crate::live::{Kind, Live};
        use crate::server::tests::call_live;
        let (dir, mut s) = project();
        place_in(
            dir.path(),
            "frame",
            "b1",
            "bolt",
            json!("working"),
            [0.0; 3],
        );
        s.refresh().unwrap();
        let mut live = Live::new();
        let focus =
            |s: &mut Session, live: &mut Live, v: Value| call_live(s, live, None, "view.focus", v);
        // Nobody focused: the project's model, its one root.
        let v = focus(&mut s, &mut live, json!({})).unwrap();
        assert_eq!(
            (&v["focus"], &v["shows"]),
            (&Value::Null, &json!(["frame"]))
        );
        // An agent shows the human a part.
        let human = live.hello(Kind::Human, Some("ann"), None).unwrap();
        let v = focus(&mut s, &mut live, json!({"part": "bolt"})).unwrap();
        assert_eq!(
            (&v["participant"], &v["focus"]),
            (&json!(human), &json!("bolt"))
        );
        assert_eq!(live.get(&human).unwrap().to_json()["focus"], "bolt");
        let v = focus(&mut s, &mut live, json!({"project": true})).unwrap();
        assert_eq!(v["focus"], Value::Null);
        let code = |r: Result<Value>| r.unwrap_err().code;
        assert_eq!(
            code(focus(&mut s, &mut live, json!({"part": "nothing"}))),
            Code::NotFound
        );
        assert_eq!(
            code(focus(
                &mut s,
                &mut live,
                json!({"part": "bolt", "project": true})
            )),
            Code::Usage
        );
        assert_eq!(
            code(call(&mut s, "view.focus", json!({}))),
            Code::NoLiveSession
        );
    }

    #[test]
    fn an_instance_reads_as_text_with_its_pose_last() {
        let v = json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "bolt",
                       "rev": {"label": "B"}, "fixed": true,
                       "pose": {"origin_m": [0.01, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}});
        let text = crate::diff::textconv(crate::doc::canonical::to_string(&v).as_bytes());
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "ryucad.feature instance (format 4)\n\
             fixed = true\n\
             part = bolt\n\
             rev = label B\n\
             \n\
             pose.origin_m = [0.01, 0, 0]\n\
             pose.x = [1, 0, 0]\n\
             pose.z = [0, 0, 1]\n"
        );
    }

    #[test]
    fn a_diff_summarises_poses_and_says_what_occurrences_did() {
        use crate::git::tests::git;
        let (dir, mut s) = project();
        let r = dir.path();
        git(r, &["init", "-q", "-b", "main"]);
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(r, "frame", "b1", "bolt", json!("working"), [0.01, 0.0, 0.0]);
        place_in(r, "frame", "b2", "bolt", json!("working"), [0.02, 0.0, 0.0]);
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "frame"]);
        s.refresh().unwrap();
        call(
            &mut s,
            "assembly.move",
            json!({"part": "frame", "instance": "b1", "translate": ["1 mm", 0, 0]}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.delete",
            json!({"part": "frame", "id": "b2"}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": 6}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({"part": "frame"})).unwrap();
        let b1 = &v["parts"]["frame"]["features"]["b1"];
        assert_eq!(
            (&b1["status"], &b1["pose"], &b1["changes"]),
            (&json!("changed"), &json!("changed"), &json!([]))
        );
        let v = call(&mut s, "diff", json!({"part": "frame", "positions": true})).unwrap();
        let pose = &v["parts"]["frame"]["features"]["b1"]["pose"];
        assert_eq!(pose[0]["field"], "origin_m");
        assert_eq!(pose[0]["to"], json!([0.011, 0, 0]));
        let v = call(&mut s, "diff", json!({"model": true})).unwrap();
        let occ = &v["parts"]["frame"]["occurrences"];
        let lists = |k: &str| occ[k].clone();
        assert_eq!(
            [
                lists("added"),
                lists("removed"),
                lists("moved"),
                lists("changed")
            ],
            [json!([]), json!(["b2"]), json!(["b1"]), json!(["base"])]
        );
        // Each with its sides, as a compare paints and ghosts them.
        assert_eq!(occ["sides"]["b2"]["to"], Value::Null);
        assert_eq!(occ["sides"]["b2"]["from"]["part"], "bolt");
        assert_eq!(
            occ["sides"]["b1"]["to"]["pose"]["origin_m"],
            json!([0.011, 0, 0])
        );
        assert_ne!(
            occ["sides"]["base"]["from"]["solids"][0]["hash"],
            occ["sides"]["base"]["to"]["solids"][0]["hash"]
        );
        // The plate thicker: its top moved, its sides grew.
        let body = &occ["bodies"]["base/slab"];
        assert!(
            !body["faces"]["changed"].as_array().unwrap().is_empty(),
            "{body}"
        );
        assert_eq!(occ["bodies"].as_object().unwrap().len(), 1);
        // A part has no occurrences to report.
        assert!(v["parts"]["plate"].get("occurrences").is_none());
    }

    #[test]
    fn a_compare_render_paints_and_ghosts_an_assemblys_occurrences() {
        use crate::git::tests::git;
        use crate::server::tests::render_pixels;
        let (dir, mut s) = project();
        let r = dir.path();
        git(r, &["init", "-q", "-b", "main"]);
        place_in(r, "frame", "base", "plate", json!("working"), [0.0; 3]);
        place_in(r, "frame", "b1", "bolt", json!("working"), [0.01, 0.0, 0.0]);
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "frame"]);
        s.refresh().unwrap();
        let small = json!({"part": "frame", "width": 96, "height": 72});
        let mut compared = small.clone();
        compared["compare"] = json!("HEAD");
        let open = || Session::open(r).unwrap();
        // Nothing changed: a compare draws what a plain render does.
        assert_eq!(
            render_pixels(r, open(), compared.clone()),
            render_pixels(r, open(), small.clone())
        );
        // #135: a placed part's body changed, so its occurrence is painted.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "plate", "id": "slab", "params": {"z": 6}}),
        )
        .unwrap();
        drop(s);
        assert_ne!(
            render_pixels(r, open(), compared.clone()),
            render_pixels(r, open(), small.clone())
        );
        // Only moved: its ghost is drawn where it was.
        git(r, &["commit", "-qam", "thicker"]);
        let mut s = open();
        call(
            &mut s,
            "assembly.move",
            json!({"part": "frame", "instance": "b1", "translate": ["5 mm", 0, 0]}),
        )
        .unwrap();
        drop(s);
        assert_ne!(
            render_pixels(r, open(), compared),
            render_pixels(r, open(), small)
        );
    }

    #[test]
    fn an_assembly_releases_when_every_instance_is_an_official_release() {
        use crate::git::tests::git;
        let (dir, mut s) = released();
        let r = dir.path();
        call(
            &mut s,
            "props.set",
            json!({"part": "frame", "properties": {"number": "300"}}),
        )
        .unwrap();
        let problems = |s: &mut Session| -> Vec<Value> {
            let v = call(
                s,
                "release.propose",
                json!({"part": "frame", "dry_run": true}),
            )
            .unwrap();
            v["problems"].as_array().unwrap().clone()
        };
        place_in(r, "frame", "b1", "bolt", json!({"label": "A"}), [0.0; 3]);
        s.refresh().unwrap();
        assert_eq!(problems(&mut s), Vec::<Value>::new());
        // A proposal of the bolt's next revision isn't official yet.
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        place_in(
            r,
            "frame",
            "b2",
            "bolt",
            json!({"label": "B"}),
            [0.02, 0.0, 0.0],
        );
        s.refresh().unwrap();
        let p = problems(&mut s);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["details"]["instance"], "b2");
        assert!(p[0]["message"].as_str().unwrap().contains("isn't official"));
        // Committed, B is official. Made obsolete, it is refused again.
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "bolt B"]);
        s.refresh().unwrap();
        assert_eq!(problems(&mut s), Vec::<Value>::new());
        call(
            &mut s,
            "release.obsolete",
            json!({"part": "bolt", "label": "B"}),
        )
        .unwrap();
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "B obsolete"]);
        s.refresh().unwrap();
        let p = problems(&mut s);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["details"]["instance"], "b2");
        assert!(p[0]["message"].as_str().unwrap().contains("obsolete"));
    }
}
