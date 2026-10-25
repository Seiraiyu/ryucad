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
    pub solids: Vec<(Id, Arc<Built>)>,
    pub pose: Rigid,
}

/// What an instance of a part places: its occurrences in order, and a
/// hash of them: equal hashes, equal models.
#[derive(Clone, Debug)]
pub struct Model {
    pub occurrences: Vec<Occurrence>,
    pub hash: String,
}

impl Model {
    fn new(occurrences: Vec<Occurrence>) -> Model {
        let mut h = blake3::Hasher::new();
        for o in &occurrences {
            let path: Vec<&str> = o.path.iter().map(Id::as_str).collect();
            h.update(format!("{} {} {}\n", path.join("/"), o.part, o.rev.text()).as_bytes());
            for (s, b) in &o.solids {
                h.update(format!(" {s} {}\n", b.hash).as_bytes());
            }
            h.update(&pose_bits(&o.pose));
        }
        Model {
            occurrences,
            hash: h.finalize().to_hex()[..32].to_owned(),
        }
    }

    /// A part's own model: one occurrence of its solids as they show, at
    /// the part's origin.
    pub fn of_part(part: &Part, rev: &Rev, result: &PartResult) -> Model {
        let solids = Solids::of(part);
        let shown = result
            .shown(&solids)
            .into_iter()
            .filter_map(|s| Some((s.solid.clone(), Arc::clone(s.state?.1))))
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
        for (id, r) in &result.features {
            let Ok(Output::Instance(placed)) = r else {
                continue;
            };
            for o in &placed.model.occurrences {
                let mut path = vec![id.clone()];
                path.extend(o.path.iter().cloned());
                out.push(Occurrence {
                    path,
                    part: o.part.clone(),
                    rev: o.rev.clone(),
                    solids: o.solids.clone(),
                    pose: placed.pose.then_after(&o.pose),
                });
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
    pub pose: Rigid,
    pub model: Arc<Model>,
    /// A hash of the revision, the pose's bits and the model's hash.
    pub hash: String,
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
        Some(Err(e)) if e.code == Code::AssemblyCycle || e.code == Code::NotFound => {
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
    let mut h = blake3::Hasher::new();
    h.update(format!("{} {}\n", inst.part, inst.rev.text()).as_bytes());
    h.update(&pose_bits(&pose));
    h.update(model.hash.as_bytes());
    Ok(Placed {
        part: inst.part.clone(),
        rev: inst.rev.clone(),
        pose,
        model,
        hash: h.finalize().to_hex()[..32].to_owned(),
    })
}

/// `wrong_part_kind` for a feature its part doesn't hold: an instance in
/// a part, or anything but an instance in an assembly.
pub fn kind_error(part: &Part, id: &Id, f: &Feature) -> Option<Error> {
    let kind = part.file.as_ref().map_or(PartKind::Part, |p| p.kind);
    let instance = f.instance().is_some();
    let why = match (kind, instance) {
        (PartKind::Part, true) => format!(
            "{id} is an instance, and {} is a part: only assemblies place parts",
            part.id
        ),
        (PartKind::Assembly, false) => format!(
            "{id} is a {}, and {} is an assembly: assemblies hold only instances",
            f.type_name(),
            part.id
        ),
        _ => return None,
    };
    Some(
        Error::new(Code::WrongPartKind, why)
            .with("part", &part.id)
            .with("feature", id)
            .with("kind", kind.as_str())
            .with("type", f.type_name()),
    )
}

/// The project's assemblies and what their listed instances place at
/// `working`, by assembly: the edges cycles are found on.
fn edges(project: &Project) -> BTreeMap<Id, BTreeSet<Id>> {
    let mut out = BTreeMap::new();
    for (id, p) in &project.parts {
        let mut to = BTreeSet::new();
        for fid in p.order() {
            if let Some(Ok(Feature::Instance(i))) = p.features.get(fid)
                && i.rev == Rev::Working
            {
                to.insert(i.part.clone());
            }
        }
        if !to.is_empty() {
            out.insert(id.clone(), to);
        }
    }
    out
}

/// The parts on a cycle of instances at `working`, each with the cycle
/// through it as a path of parts, starting and ending with it: Tarjan's
/// strongly connected components over [`edges`]. An instance of part `b`
/// in assembly `a` closes a cycle when both are in one component.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cycles {
    component: BTreeMap<Id, usize>,
}

impl Cycles {
    pub fn of(project: &Project) -> Cycles {
        let edges = edges(project);
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
        Cycles { component }
    }

    /// Whether an instance of `child` in `assembly` closes a cycle.
    pub fn closes(&self, assembly: &Id, child: &Id) -> bool {
        matches!((self.component.get(assembly), self.component.get(child)), (Some(a), Some(b)) if a == b)
    }
}

struct Tarjan<'a> {
    edges: &'a BTreeMap<Id, BTreeSet<Id>>,
    index: BTreeMap<Id, usize>,
    low: BTreeMap<Id, usize>,
    stack: Vec<Id>,
    on: BTreeSet<Id>,
    next: usize,
    component: BTreeMap<Id, usize>,
    components: usize,
}

impl Tarjan<'_> {
    fn visit(&mut self, v: &Id) {
        self.index.insert(v.clone(), self.next);
        self.low.insert(v.clone(), self.next);
        self.next += 1;
        self.stack.push(v.clone());
        self.on.insert(v.clone());
        let to: Vec<Id> = self
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

/// The path of parts from `from` back to `to` through instances at
/// `working`, for the message: the shortest, the first by ID at each step.
fn path_back(project: &Project, from: &Id, to: &Id) -> Vec<Id> {
    let edges = edges(project);
    let mut prev: BTreeMap<Id, Id> = BTreeMap::new();
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

/// `assembly_cycle` for an instance of `child` in `assembly`.
pub fn cycle_error(project: &Project, assembly: &Id, child: &Id) -> Error {
    let mut path = vec![assembly.clone()];
    path.extend(path_back(project, child, assembly));
    let text: Vec<&str> = path.iter().map(Id::as_str).collect();
    Error::new(
        Code::AssemblyCycle,
        format!(
            "{assembly} places {child}, which places {assembly} again: {}",
            text.join(" > ")
        ),
    )
    .with("part", assembly)
    .with("child", child)
    .with("path", &path)
}

/// Rebuilds parts and the assemblies over them, a part before every
/// assembly that places it, reusing what `results` and `caches` hold:
/// each part's rebuild by a key of everything it read (its revision, and
/// for an assembly its children's models), and each part's feature
/// cache. `fresh` lists, in order, the parts it rebuilt.
pub struct Rebuild<'p> {
    project: &'p Project,
    cycles: Cycles,
    pub results: BTreeMap<Id, (String, PartResult)>,
    pub caches: BTreeMap<Id, Cache>,
    pub fresh: Vec<Id>,
}

impl<'p> Rebuild<'p> {
    pub fn new(
        project: &'p Project,
        results: BTreeMap<Id, (String, PartResult)>,
        caches: BTreeMap<Id, Cache>,
    ) -> Rebuild<'p> {
        Rebuild {
            project,
            cycles: Cycles::of(project),
            results,
            caches,
            fresh: Vec::new(),
        }
    }

    /// The rebuild of `part`, and of every part it places first.
    pub fn result(&mut self, part: &Id) -> Result<&PartResult> {
        let p = self.project.part(part)?;
        let mut key = p.revision.clone();
        let mut children = Children::new();
        if p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly) {
            for fid in p.order() {
                let Some(Ok(Feature::Instance(i))) = p.features.get(fid) else {
                    continue;
                };
                let at = (i.part.clone(), i.rev.clone());
                if children.contains_key(&at) {
                    continue;
                }
                let m = self.child(part, &i.part, &i.rev);
                children.insert(at, m);
            }
            let mut h = blake3::Hasher::new();
            for ((c, rev), m) in &children {
                let what = match m {
                    Ok(m) => m.hash.clone(),
                    Err(e) => serde_json::to_string(&e.code).expect("codes serialize"),
                };
                h.update(format!("{c} {} {what}\n", rev.text()).as_bytes());
            }
            key = format!("{key}+{}", &h.finalize().to_hex()[..32]);
        }
        let fresh = self.results.get(part).is_some_and(|(k, _)| *k == key);
        if !fresh {
            let empty = Cache::new();
            let cache = self.caches.get(part).unwrap_or(&empty);
            let (result, cache) = rebuild_part_in(p, cache, &children);
            self.caches.insert(part.clone(), cache);
            self.results.insert(part.clone(), (key, result));
            self.fresh.push(part.clone());
        }
        Ok(&self.results[part].1)
    }

    /// The model an instance in `assembly` places of `part` at `rev`.
    fn child(&mut self, assembly: &Id, part: &Id, rev: &Rev) -> Result<Arc<Model>> {
        if *rev == Rev::Working && self.cycles.closes(assembly, part) {
            return Err(cycle_error(self.project, assembly, part));
        }
        self.model(part, rev)
    }

    /// The model of `part` at `rev`: what an instance of it places.
    pub fn model(&mut self, part: &Id, rev: &Rev) -> Result<Arc<Model>> {
        match rev {
            Rev::Working => {
                let p = self.project.part(part).map_err(|_| {
                    Error::new(Code::NotFound, format!("no part {part} to place"))
                        .with("part", part)
                })?;
                let assembly = p.file.as_ref().is_ok_and(|f| f.kind == PartKind::Assembly);
                let result = self.result(part)?;
                Ok(Arc::new(if assembly {
                    Model::of_assembly(result)
                } else {
                    Model::of_part(p, rev, result)
                }))
            }
            Rev::Label(l) => Err(
                Error::new(Code::NotFound, format!("{part} has no release {l}"))
                    .with("part", part)
                    .with("label", l),
            ),
        }
    }
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
        assert_eq!(m.occurrences[1].solids[0].1.hash, bolt);
        assert!(Arc::ptr_eq(
            &m.occurrences[1].solids[0].1,
            &m.occurrences[2].solids[0].1
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
}
